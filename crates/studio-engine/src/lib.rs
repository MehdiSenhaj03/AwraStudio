//! The real-time engine: plays the mix graph on the sound card, monitors
//! the live input through an armed track's effects (live autotune), and
//! records takes.
//!
//! The audio callback owns the graph. The UI thread builds new graphs and
//! sends them over a channel; the callback adopts the processors of the
//! old graph (no allocation) and sends the old one back to be dropped off
//! the audio thread. Without a sound card a timer thread drives the same
//! callback, so the transport works everywhere.
//!
//! SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use studio_core::model::Id;
use studio_core::ops::Op;
use studio_dsp::graph::{Meters, MixGraph, MAX_BLOCK};
use studio_dsp::AudioBuffer;

pub enum Cmd {
    Graph(Box<MixGraph>),
    Light(Op),
    Play,
    Stop,
    Seek(i64),
    Loop(Option<(i64, i64)>),
    Monitor(Option<Id>),
    Metronome(bool),
    Bpm(f64),
    /// How the live input is heard (see MixGraph::monitor_mode).
    MonitorMode(u8),
}

/// State shared with the UI (lock-free reads).
pub struct Shared {
    pub pos: AtomicI64,
    pub playing: AtomicBool,
    pub recording: AtomicBool,
    pub meters: Mutex<Meters>,
    input_peak: AtomicU32,
    pub device_rate: AtomicU32,
    pub callbacks: AtomicI64,
    /// Frames per callback of the input and the output (latency).
    pub in_period: AtomicU32,
    pub out_period: AtomicU32,
    pub input_rate: AtomicU32,
}

impl Shared {
    pub fn input_peak(&self) -> f32 {
        f32::from_bits(self.input_peak.load(Ordering::Relaxed))
    }
}

/// A recorded take, at the project rate.
pub struct Take {
    pub buffer: AudioBuffer,
    /// Where it starts on the timeline (s), latency compensated.
    pub start: f64,
    pub track: Id,
}

struct Rt {
    graph: Box<MixGraph>,
    cmds: Receiver<Cmd>,
    trash: Sender<Box<MixGraph>>,
    shared: Arc<Shared>,
    project_rate: f64,
    device_rate: f64,
    playing: bool,
    pos: i64,
    loop_region: Option<(i64, i64)>,
    l: Vec<f32>,
    r: Vec<f32>,
    input: Vec<f32>,
    monitor: Option<rtrb::Consumer<f32>>,
    monitoring: bool,
    monitor_mode: u8,
    metronome: bool,
    bpm: f64,
    // output resampling when the device's rate differs (linear)
    frac: f64,
    hold: Vec<(f32, f32)>,
    hold_pos: usize,
}

impl Rt {
    fn handle_commands(&mut self) {
        while let Ok(cmd) = self.cmds.try_recv() {
            match cmd {
                Cmd::Graph(mut g) => {
                    g.adopt(&mut self.graph);
                    let old = std::mem::replace(&mut self.graph, g);
                    let _ = self.trash.try_send(old);
                    self.graph.set_monitor_mode(self.monitor_mode);
                }
                Cmd::Light(op) => {
                    self.graph.apply_light(&op);
                }
                Cmd::Play => self.playing = true,
                Cmd::Stop => {
                    self.playing = false;
                    self.graph.reset();
                }
                Cmd::Seek(p) => {
                    self.pos = p.max(0);
                    self.graph.reset();
                }
                Cmd::Loop(l) => self.loop_region = l,
                Cmd::Monitor(t) => {
                    self.monitoring = t.is_some();
                    self.graph.set_monitor(t);
                }
                Cmd::Metronome(on) => self.metronome = on,
                Cmd::Bpm(b) => self.bpm = b,
                Cmd::MonitorMode(m) => {
                    self.monitor_mode = m;
                    self.graph.set_monitor_mode(m);
                }
            }
        }
        self.shared.playing.store(self.playing, Ordering::Relaxed);
    }

    /// Renders `n` frames at the project rate into self.l / self.r.
    fn render_block(&mut self, n: usize) {
        let n = n.min(MAX_BLOCK);
        let (l, r) = (&mut self.l[..n], &mut self.r[..n]);
        l.fill(0.0);
        r.fill(0.0);
        // live input for monitoring
        let mut have_input = false;
        if let Some(c) = self.monitor.as_mut() {
            let avail = c.slots();
            // the shortest possible monitoring: never more waiting in the
            // ring than one input period beyond this block (what piles up
            // from start-up or clock drift is dropped)
            let period = self.shared.in_period.load(Ordering::Relaxed) as usize;
            let keep = n + period.max(32);
            if avail > keep + 32 {
                for _ in 0..avail - keep {
                    let _ = c.pop();
                }
            }
            for i in 0..n {
                self.input[i] = c.pop().unwrap_or(0.0);
            }
            have_input = self.monitoring;
        }
        let input = if have_input { Some(&self.input[..n]) } else { None };
        if self.playing {
            // a loop wraps inside the block
            let mut done = 0;
            while done < n {
                let mut len = n - done;
                if let Some((a, b)) = self.loop_region {
                    if b > a && self.pos >= b {
                        self.pos = a;
                    }
                    if b > a && self.pos < b {
                        len = len.min((b - self.pos) as usize);
                    }
                }
                let inp = input.map(|x| &x[done..done + len]);
                self.graph.play_clips = true;
                self.graph.render(self.pos, &mut l[done..done + len], &mut r[done..done + len], inp);
                if self.metronome {
                    click(self.pos, &mut l[done..done + len], &mut r[done..done + len], self.project_rate, self.bpm);
                }
                self.pos += len as i64;
                done += len;
            }
        } else {
            self.graph.play_clips = false;
            self.graph.render(self.pos, l, r, input);
        }
        self.shared.pos.store(self.pos, Ordering::Relaxed);
        if let Some(mut m) = self.shared.meters.try_lock() {
            m.clone_from(&self.graph.meters);
        }
    }

    /// Fills an interleaved device buffer.
    fn fill<T: cpal::SizedSample + cpal::FromSample<f32>>(&mut self, data: &mut [T], channels: usize) {
        self.handle_commands();
        self.shared.callbacks.fetch_add(1, Ordering::Relaxed);
        let frames = data.len() / channels.max(1);
        self.shared.out_period.store(frames as u32, Ordering::Relaxed);
        let ratio = self.project_rate / self.device_rate;
        let mut f = 0;
        while f < frames {
            if (ratio - 1.0).abs() < 1e-9 {
                let n = (frames - f).min(MAX_BLOCK);
                self.render_block(n);
                for i in 0..n {
                    write_frame(&mut data[(f + i) * channels..(f + i + 1) * channels], self.l[i], self.r[i]);
                }
                f += n;
            } else {
                // linear interpolation from the project rate to the device's
                if self.hold_pos + 1 >= self.hold.len() {
                    let keep = self.hold.last().copied().unwrap_or((0.0, 0.0));
                    // only what this callback still needs: no extra delay
                    let n = (((frames - f) as f64 * ratio).ceil() as usize + 1).clamp(16, MAX_BLOCK);
                    self.render_block(n);
                    self.hold.clear();
                    self.hold.push(keep);
                    for i in 0..n {
                        self.hold.push((self.l[i], self.r[i]));
                    }
                    self.hold_pos = 0;
                }
                let a = self.hold[self.hold_pos];
                let b = self.hold[self.hold_pos + 1];
                let t = self.frac as f32;
                write_frame(
                    &mut data[f * channels..(f + 1) * channels],
                    a.0 + (b.0 - a.0) * t,
                    a.1 + (b.1 - a.1) * t,
                );
                self.frac += ratio;
                while self.frac >= 1.0 {
                    self.frac -= 1.0;
                    self.hold_pos += 1;
                    if self.hold_pos + 1 >= self.hold.len() {
                        break;
                    }
                }
                f += 1;
            }
        }
    }
}

fn write_frame<T: cpal::SizedSample + cpal::FromSample<f32>>(out: &mut [T], l: f32, r: f32) {
    match out.len() {
        0 => {}
        1 => out[0] = T::from_sample((l + r) * 0.5),
        _ => {
            out[0] = T::from_sample(l.clamp(-1.0, 1.0));
            out[1] = T::from_sample(r.clamp(-1.0, 1.0));
            for o in out.iter_mut().skip(2) {
                *o = T::from_sample(0.0f32);
            }
        }
    }
}

/// A short click on every beat (accented on the bar).
fn click(pos: i64, l: &mut [f32], r: &mut [f32], sr: f64, bpm: f64) {
    let beat = (60.0 / bpm.max(20.0) * sr) as i64;
    if beat <= 0 {
        return;
    }
    let len = (0.03 * sr) as i64;
    for i in 0..l.len() {
        let t = pos + i as i64;
        let k = t % beat;
        if k < len {
            let accent = (t / beat) % 4 == 0;
            let f = if accent { 1600.0 } else { 1000.0 };
            let env = 1.0 - k as f32 / len as f32;
            let s = (2.0 * std::f64::consts::PI * f * k as f64 / sr).sin() as f32 * env * env * 0.25;
            l[i] += s;
            r[i] += s;
        }
    }
}

struct Recorder {
    samples: Vec<f32>,
    start: i64,
    track: Id,
}

/// Which sound devices to use, by name; None: the system's default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceChoice {
    pub input: Option<String>,
    pub output: Option<String>,
    /// No microphone at all (nothing is opened for input).
    pub no_input: bool,
    /// Frames per callback asked of the card (0: its default). Small is
    /// fast (monitoring), too small crackles on a busy machine.
    pub buffer_frames: u32,
}

/// The buffer size a device accepts nearest to the wish (None: default).
fn buffer_size(range: &cpal::SupportedBufferSize, want: u32) -> cpal::BufferSize {
    if want == 0 {
        return cpal::BufferSize::Default;
    }
    match range {
        cpal::SupportedBufferSize::Range { min, max } => cpal::BufferSize::Fixed(want.clamp(*min, *max)),
        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Fixed(want),
    }
}

/// The sound devices of the machine.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioDevices {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub default_input: Option<String>,
    pub default_output: Option<String>,
}

/// Fake devices for tests and screenshots without a sound card:
/// AWRA_STUDIO_FAKE_DEVICES="Micro USB;Micro intégré|Casque;Haut-parleurs".
fn fake_devices() -> Option<AudioDevices> {
    let v = std::env::var("AWRA_STUDIO_FAKE_DEVICES").ok()?;
    let (i, o) = v.split_once('|').unwrap_or((&v, ""));
    let list = |s: &str| s.split(';').map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect::<Vec<_>>();
    let (inputs, outputs) = (list(i), list(o));
    Some(AudioDevices { default_input: inputs.first().cloned(), default_output: outputs.first().cloned(), inputs, outputs })
}

/// Lists the microphones and outputs (takes a moment on some systems).
pub fn list_devices() -> AudioDevices {
    if let Some(f) = fake_devices() {
        return f;
    }
    if std::env::var("AWRA_STUDIO_NO_AUDIO").is_ok() {
        return AudioDevices::default();
    }
    let host = cpal::default_host();
    let names = |it: Option<Vec<cpal::Device>>| {
        let mut v: Vec<String> = it.unwrap_or_default().iter().filter_map(|d| d.name().ok()).filter(|n| n != "null").collect();
        v.dedup();
        v
    };
    AudioDevices {
        inputs: names(host.input_devices().ok().map(|d| d.collect())),
        outputs: names(host.output_devices().ok().map(|d| d.collect())),
        default_input: host.default_input_device().and_then(|d| d.name().ok()),
        default_output: host.default_output_device().and_then(|d| d.name().ok()),
    }
}

fn find_device(host: &cpal::Host, input: bool, name: Option<&str>) -> (Option<cpal::Device>, Option<String>) {
    let default = || if input { host.default_input_device() } else { host.default_output_device() };
    let Some(name) = name else { return (default(), None) };
    let found = if input { host.input_devices().ok().and_then(|mut d| d.find(|d| d.name().ok().as_deref() == Some(name))) } else { host.output_devices().ok().and_then(|mut d| d.find(|d| d.name().ok().as_deref() == Some(name))) };
    match found {
        Some(d) => (Some(d), None),
        None => (default(), Some(format!("« {name} » est introuvable : celui du système est utilisé"))),
    }
}

pub struct Engine {
    cmds: Sender<Cmd>,
    trash: Receiver<Box<MixGraph>>,
    pub shared: Arc<Shared>,
    pub project_rate: u32,
    output: Option<cpal::Stream>,
    input: Option<cpal::Stream>,
    input_rate: u32,
    rec_rx: Option<rtrb::Consumer<f32>>,
    recorder: Option<Recorder>,
    null_thread: Option<(Arc<AtomicBool>, std::thread::JoinHandle<()>)>,
    pub output_name: String,
    pub input_name: Option<String>,
    /// Added to the take's start to align it (ms, negative = earlier).
    pub latency_ms: f64,
    pub last_error: Option<String>,
}

impl Engine {
    /// Starts the engine at the project's rate with an empty graph.
    pub fn new(project_rate: u32, graph: MixGraph) -> Engine {
        Engine::with_devices(project_rate, graph, &DeviceChoice::default())
    }

    /// Starts the engine on the chosen devices.
    pub fn with_devices(project_rate: u32, graph: MixGraph, choice: &DeviceChoice) -> Engine {
        let (cmd_tx, cmd_rx) = bounded::<Cmd>(1024);
        let (trash_tx, trash_rx) = bounded::<Box<MixGraph>>(64);
        let shared = Arc::new(Shared {
            pos: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            meters: Mutex::new(Meters::default()),
            input_peak: AtomicU32::new(0),
            device_rate: AtomicU32::new(project_rate),
            callbacks: AtomicI64::new(0),
            in_period: AtomicU32::new(0),
            out_period: AtomicU32::new(0),
            input_rate: AtomicU32::new(project_rate),
        });

        // input first, so that monitoring has its ring
        let host = cpal::default_host();
        let (mon_tx, mon_rx) = rtrb::RingBuffer::<f32>::new(project_rate as usize);
        let (rec_tx, rec_rx) = rtrb::RingBuffer::<f32>::new(project_rate as usize * 4);
        let mut input_rate = project_rate;
        let mut input_name = None;
        let mut warnings = Vec::new();
        let no_audio = std::env::var("AWRA_STUDIO_NO_AUDIO").is_ok();
        let fake = fake_devices();
        let in_dev = if no_audio || choice.no_input {
            None
        } else {
            let (d, w) = find_device(&host, true, choice.input.as_deref());
            warnings.extend(w);
            d
        };
        if let (Some(f), false) = (&fake, choice.no_input) {
            input_name = choice.input.clone().filter(|n| f.inputs.contains(n)).or(f.default_input.clone());
        }
        let input = match in_dev {
            Some(dev) => {
                input_name = dev.name().ok();
                match open_input(&dev, project_rate, choice.buffer_frames, mon_tx, rec_tx, shared.clone()) {
                    Ok((s, rate)) => {
                        input_rate = rate;
                        Some(s)
                    }
                    Err(e) => {
                        eprintln!("awra-studio: entrée audio indisponible : {e}");
                        None
                    }
                }
            }
            None => None,
        };

        let mut rt = Rt {
            graph: Box::new(graph),
            cmds: cmd_rx,
            trash: trash_tx,
            shared: shared.clone(),
            project_rate: project_rate as f64,
            device_rate: project_rate as f64,
            playing: false,
            pos: 0,
            loop_region: None,
            l: vec![0.0; MAX_BLOCK],
            r: vec![0.0; MAX_BLOCK],
            input: vec![0.0; MAX_BLOCK],
            monitor: if input.is_some() { Some(mon_rx) } else { None },
            monitoring: false,
            monitor_mode: 1,
            metronome: false,
            bpm: 120.0,
            frac: 0.0,
            hold: Vec::with_capacity(1024),
            hold_pos: 0,
        };

        let mut output = None;
        let mut output_name = "silencieuse (aucune carte son)".to_string();
        let mut last_error = None;
        let mut null_thread = None;
        let disable = std::env::var("AWRA_STUDIO_NO_AUDIO").is_ok();
        let dev = if disable {
            None
        } else {
            let (d, w) = find_device(&host, false, choice.output.as_deref());
            warnings.extend(w);
            d
        };
        if let Some(f) = &fake {
            output_name = choice.output.clone().filter(|n| f.outputs.contains(n)).or(f.default_output.clone()).unwrap_or(output_name);
        }
        match dev.map(|d| (d.name().unwrap_or_default(), open_output(&d, project_rate, choice.buffer_frames), d)) {
            Some((name, Ok((mut cfg, fmt)), dev)) => {
                rt.device_rate = cfg.sample_rate.0 as f64;
                shared.device_rate.store(cfg.sample_rate.0, Ordering::Relaxed);
                let mut res = build_output(&dev, &cfg, fmt, rt);
                // a card that refuses the small buffer: its own size then
                if let Err((_, Some(_))) = &res {
                    if cfg.buffer_size != cpal::BufferSize::Default {
                        if let Err((_, Some(back))) = res {
                            cfg.buffer_size = cpal::BufferSize::Default;
                            res = build_output(&dev, &cfg, fmt, back);
                        }
                    }
                }
                match res {
                    Ok(s) => {
                        output_name = name;
                        output = Some(s);
                    }
                    Err((e, back)) => {
                        last_error = Some(e.to_string());
                        null_thread = back.map(|rt| spawn_null(rt, project_rate));
                    }
                }
            }
            Some((_, Err(e), _)) => {
                last_error = Some(e.to_string());
                null_thread = Some(spawn_null(rt, project_rate));
            }
            None => null_thread = Some(spawn_null(rt, project_rate)),
        }
        let latency_ms = 20.0;
        if last_error.is_none() && !warnings.is_empty() {
            last_error = Some(warnings.join(" ; "));
        }
        Engine {
            cmds: cmd_tx,
            trash: trash_rx,
            shared,
            project_rate,
            output,
            input,
            input_rate,
            rec_rx: Some(rec_rx),
            recorder: None,
            null_thread,
            output_name,
            input_name,
            latency_ms,
            last_error,
        }
    }

    /// Closes the sound devices (before opening others).
    pub fn close(&mut self) {
        self.output = None;
        self.input = None;
        if let Some((stop, h)) = self.null_thread.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = h.join();
        }
    }

    pub fn has_device(&self) -> bool {
        self.output.is_some()
    }
    pub fn has_input(&self) -> bool {
        self.input.is_some()
    }
    /// The microphone's level now (peak, 0..1, decaying).
    pub fn input_level(&self) -> f32 {
        f32::from_bits(self.shared.input_peak.load(Ordering::Relaxed))
    }
    /// The rate the sound card runs at.
    pub fn device_rate(&self) -> u32 {
        self.shared.device_rate.load(Ordering::Relaxed)
    }
    /// How long the voice takes from the microphone to the ears (ms),
    /// as the cards run now: an input period, an output period, the ring
    /// margin, and the effects' own delay in the monitoring mode.
    pub fn monitor_latency_ms(&self, mode: u8) -> Option<f64> {
        self.input.as_ref()?;
        let rate = self.device_rate().max(1) as f64;
        let inp = self.shared.in_period.load(Ordering::Relaxed) as f64;
        let out = self.shared.out_period.load(Ordering::Relaxed) as f64;
        let dsp = match mode {
            0 => 0.0,
            1 => 5.0,
            _ => 36.0,
        };
        Some((inp + out + 32.0) / rate * 1000.0 + dsp)
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.cmds.try_send(cmd);
    }

    /// Drops graphs the audio thread gave back (call regularly).
    pub fn collect_garbage(&self) {
        while self.trash.try_recv().is_ok() {}
    }

    pub fn set_graph(&self, g: MixGraph) {
        self.send(Cmd::Graph(Box::new(g)));
        self.collect_garbage();
    }

    pub fn play(&self) {
        self.send(Cmd::Play)
    }
    pub fn stop(&self) {
        self.send(Cmd::Stop)
    }
    pub fn seek(&self, seconds: f64) {
        self.send(Cmd::Seek((seconds.max(0.0) * self.project_rate as f64) as i64));
        self.shared.pos.store((seconds.max(0.0) * self.project_rate as f64) as i64, Ordering::Relaxed);
    }
    pub fn position(&self) -> f64 {
        self.shared.pos.load(Ordering::Relaxed) as f64 / self.project_rate as f64
    }
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }
    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// Starts recording on a track from the current position (and plays).
    pub fn start_recording(&mut self, track: Id) -> Result<()> {
        if self.input.is_none() {
            return Err(anyhow!("aucun micro détecté"));
        }
        if let Some(rx) = self.rec_rx.as_mut() {
            while rx.pop().is_ok() {}
        }
        self.recorder = Some(Recorder { samples: Vec::new(), start: self.shared.pos.load(Ordering::Relaxed), track });
        self.shared.recording.store(true, Ordering::Relaxed);
        self.send(Cmd::Monitor(Some(track)));
        self.play();
        Ok(())
    }

    /// Moves captured samples into the take (call from the UI loop).
    pub fn pump_recording(&mut self) {
        if let (Some(rec), Some(rx)) = (self.recorder.as_mut(), self.rec_rx.as_mut()) {
            while let Ok(s) = rx.pop() {
                rec.samples.push(s);
            }
        }
    }

    /// Stops recording and playback; returns the take.
    pub fn stop_recording(&mut self) -> Option<Take> {
        self.pump_recording();
        self.shared.recording.store(false, Ordering::Relaxed);
        self.stop();
        let rec = self.recorder.take()?;
        if rec.samples.is_empty() {
            return None;
        }
        let buf = AudioBuffer::from_mono(self.input_rate, rec.samples).resampled(self.project_rate);
        let start = rec.start as f64 / self.project_rate as f64 - self.latency_ms / 1000.0;
        // what was recorded before 0 is cut
        let (buffer, start) = if start < 0.0 { (buf.slice(-start, buf.duration()), 0.0) } else { (buf, start) };
        Some(Take { buffer, start, track: rec.track })
    }

    pub fn monitor(&self, track: Option<Id>) {
        self.send(Cmd::Monitor(track));
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some((stop, h)) = self.null_thread.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = h.join();
        }
    }
}

fn spawn_null(mut rt: Rt, rate: u32) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    rt.device_rate = rate as f64;
    let h = std::thread::Builder::new()
        .name("awra-studio-null-audio".into())
        .spawn(move || {
            let block = 512usize;
            let mut buf = vec![0.0f32; block * 2];
            let period = Duration::from_secs_f64(block as f64 / rate as f64);
            let start = std::time::Instant::now();
            let mut n: u32 = 0;
            while !s2.load(Ordering::Relaxed) {
                rt.fill(&mut buf, 2);
                n += 1;
                let due = start + period * n;
                let now = std::time::Instant::now();
                if due > now {
                    std::thread::sleep(due - now);
                }
            }
        })
        .expect("audio thread");
    (stop, h)
}

fn open_output(dev: &cpal::Device, rate: u32, buffer: u32) -> Result<(cpal::StreamConfig, cpal::SampleFormat)> {
    let def = dev.default_output_config()?;
    let range = def.buffer_size().clone();
    let fmt = def.sample_format();
    let mut cfg: cpal::StreamConfig = def.config();
    // the project's rate when the device offers it
    if let Ok(mut ranges) = dev.supported_output_configs() {
        if ranges.any(|r| {
            r.min_sample_rate().0 <= rate && r.max_sample_rate().0 >= rate && r.sample_format() == fmt
        }) {
            cfg.sample_rate = cpal::SampleRate(rate);
        }
    }
    cfg.buffer_size = buffer_size(&range, buffer);
    Ok((cfg, fmt))
}

#[allow(clippy::result_large_err)]
fn build_output(
    dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    fmt: cpal::SampleFormat,
    rt: Rt,
) -> Result<cpal::Stream, (anyhow::Error, Option<Rt>)> {
    let ch = cfg.channels as usize;
    let err = |e| eprintln!("awra-studio: erreur de flux audio : {e}");
    // the callback takes the state on its first call; when building or
    // starting fails before that, the state comes back for a timer thread
    let cell = std::sync::Arc::new(Mutex::new(Some(rt)));
    let c2 = cell.clone();
    macro_rules! build {
        ($t:ty) => {{
            let mut st: Option<Rt> = None;
            dev.build_output_stream(
                cfg,
                move |data: &mut [$t], _| {
                    if st.is_none() {
                        st = c2.lock().take();
                    }
                    if let Some(s) = st.as_mut() {
                        s.fill(data, ch);
                    }
                },
                err,
                None,
            )
        }};
    }
    let res = match fmt {
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::U16 => build!(u16),
        cpal::SampleFormat::I32 => build!(i32),
        other => {
            return Err((anyhow!("format de sortie non géré : {other:?}"), cell.lock().take()));
        }
    };
    match res.map_err(anyhow::Error::from).and_then(|s| s.play().map(|_| s).map_err(anyhow::Error::from)) {
        Ok(s) => Ok(s),
        Err(e) => Err((e, cell.lock().take())),
    }
}

/// What the input callback carries (taken on its first call, given back
/// when the stream can't be built so that it can be tried again).
struct InState {
    mon: rtrb::Producer<f32>,
    rec: rtrb::Producer<f32>,
    // monitoring at the project's rate when the card runs at another one
    prev: f32,
    t: f64,
}

fn open_input(
    dev: &cpal::Device,
    rate: u32,
    buffer: u32,
    mon: rtrb::Producer<f32>,
    rec: rtrb::Producer<f32>,
    shared: Arc<Shared>,
) -> Result<(cpal::Stream, u32)> {
    let def = dev.default_input_config()?;
    let fmt = def.sample_format();
    let range = def.buffer_size().clone();
    let mut cfg: cpal::StreamConfig = def.config();
    if let Ok(mut ranges) = dev.supported_input_configs() {
        if ranges.any(|r| r.min_sample_rate().0 <= rate && r.max_sample_rate().0 >= rate && r.sample_format() == fmt) {
            cfg.sample_rate = cpal::SampleRate(rate);
        }
    }
    cfg.buffer_size = buffer_size(&range, buffer);
    let got_rate = cfg.sample_rate.0;
    shared.input_rate.store(got_rate, Ordering::Relaxed);
    let cell = Arc::new(Mutex::new(Some(InState { mon, rec, prev: 0.0, t: 0.0 })));
    let build = |cfg: &cpal::StreamConfig| -> Result<cpal::Stream> {
        let ch = cfg.channels as usize;
        // input samples per monitored sample
        let step = got_rate as f64 / rate as f64;
        let same_rate = got_rate == rate;
        let err = |e| eprintln!("awra-studio: erreur d'entrée audio : {e}");
        macro_rules! build {
            ($t:ty) => {{
                let (c2, shared) = (cell.clone(), shared.clone());
                let mut st: Option<InState> = None;
                dev.build_input_stream(
                    cfg,
                    move |data: &[$t], _| {
                        if st.is_none() {
                            st = c2.lock().take();
                        }
                        let Some(s) = st.as_mut() else { return };
                        let frames = data.len() / ch.max(1);
                        shared.in_period.store((frames as f64 / step).ceil() as u32, Ordering::Relaxed);
                        let rec_on = shared.recording.load(Ordering::Relaxed);
                        let mut pk = 0.0f32;
                        for frame in data.chunks(ch.max(1)) {
                            // the first channel: a microphone is on input 1
                            let x: f32 = <f32 as cpal::FromSample<$t>>::from_sample_(frame[0]);
                            pk = pk.max(x.abs());
                            if same_rate {
                                let _ = s.mon.push(x);
                            } else {
                                // linear interpolation to the project's rate
                                while s.t <= 1.0 {
                                    let _ = s.mon.push(s.prev + (x - s.prev) * s.t as f32);
                                    s.t += step;
                                }
                                s.t -= 1.0;
                                s.prev = x;
                            }
                            if rec_on {
                                let _ = s.rec.push(x);
                            }
                        }
                        let old = f32::from_bits(shared.input_peak.load(Ordering::Relaxed));
                        let v = pk.max(old * 0.9);
                        shared.input_peak.store(v.to_bits(), Ordering::Relaxed);
                    },
                    err,
                    None,
                )?
            }};
        }
        let stream = match fmt {
            cpal::SampleFormat::F32 => build!(f32),
            cpal::SampleFormat::I16 => build!(i16),
            cpal::SampleFormat::U16 => build!(u16),
            cpal::SampleFormat::I32 => build!(i32),
            other => return Err(anyhow!("format d'entrée non géré : {other:?}")),
        };
        stream.play()?;
        Ok(stream)
    };
    let stream = match build(&cfg) {
        Ok(s) => s,
        // a card that refuses the small buffer: its own size then
        Err(_) if cfg.buffer_size != cpal::BufferSize::Default => {
            cfg.buffer_size = cpal::BufferSize::Default;
            build(&cfg)?
        }
        Err(e) => return Err(e),
    };
    Ok((stream, got_rate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc as A;
    use studio_core::model::*;
    use studio_dsp::graph::AssetStore;

    #[test]
    fn transport_runs_without_a_device() {
        std::env::set_var("AWRA_STUDIO_NO_AUDIO", "1");
        let mut p = Project::new("t");
        let mut assets = AssetStore::new();
        assets.insert("a".into(), A::new(AudioBuffer::from_mono(48000, vec![0.5; 48000])));
        let mut t = Track::new("v", TrackKind::Vocal);
        t.clips.push(Clip {
            id: new_id(),
            asset: "a".into(),
            name: "c".into(),
            start: 0.0,
            offset: 0.0,
            length: 1.0,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        });
        p.tracks.push(t);
        let g = MixGraph::build(&p, &assets, None, true);
        let e = Engine::new(48000, g);
        e.play();
        std::thread::sleep(Duration::from_millis(200));
        let pos = e.position();
        assert!(pos > 0.1 && pos < 0.5, "{pos}");
        let m = e.shared.meters.lock().clone();
        assert!(m.master.0 > 0.4, "{:?}", m.master);
        e.stop();
        e.seek(3.0);
        std::thread::sleep(Duration::from_millis(50));
        assert!((e.position() - 3.0).abs() < 0.01);
    }
}
