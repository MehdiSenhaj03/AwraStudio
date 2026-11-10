//! The built-in effects as [`Processor`]s.

use std::collections::BTreeMap;

use studio_core::effects::EffectKind;

use crate::filters::*;

/// Stereo, in-place audio processing. Implementations must not allocate
/// in `process`.
pub trait Processor: Send {
    fn set_param(&mut self, id: &str, value: f64);
    fn process(&mut self, l: &mut [f32], r: &mut [f32]);
    fn reset(&mut self) {}
    /// Gain reduction (dB, >= 0) for meters, when it applies.
    fn reduction_db(&self) -> f32 {
        0.0
    }
    /// The tempo changed (synced delays).
    fn set_bpm(&mut self, _bpm: f64) {}
}

/// Creates a built-in effect with its parameters (None for plugins).
pub fn create(kind: &EffectKind, sr: f64, params: &BTreeMap<String, f64>, bpm: f64) -> Option<Box<dyn Processor>> {
    let mut p: Box<dyn Processor> = match kind {
        EffectKind::Gain => Box::new(Gain::new()),
        EffectKind::Eq => Box::new(Eq::new(sr)),
        EffectKind::Compressor => Box::new(Compressor::new(sr)),
        EffectKind::Gate => Box::new(Gate::new(sr)),
        EffectKind::DeEsser => Box::new(DeEsser::new(sr)),
        EffectKind::Denoise => Box::new(crate::denoise::Denoise::new(sr)),
        EffectKind::Reverb => Box::new(Reverb::new(sr)),
        EffectKind::Delay => Box::new(Delay::new(sr)),
        EffectKind::Saturation => Box::new(Saturation::new(sr)),
        EffectKind::Autotune => Box::new(crate::autotune::Autotune::new(sr)),
        EffectKind::Limiter => Box::new(Limiter::new(sr)),
        EffectKind::Doubler => Box::new(Doubler::new(sr)),
        EffectKind::StereoWidth => Box::new(StereoWidth::new(sr)),
        EffectKind::Plugin { .. } => return None,
    };
    p.set_bpm(bpm);
    // defaults first, then the slot's values
    for spec in studio_core::effects::spec(kind).params {
        p.set_param(spec.id, *params.get(spec.id).unwrap_or(&spec.default));
    }
    Some(p)
}

fn db(v: f64) -> f64 {
    10f64.powf(v / 20.0)
}

// ---------------------------------------------------------------------------

pub struct Gain {
    g: Smoothed,
}
impl Gain {
    pub fn new() -> Gain {
        Gain { g: Smoothed::new(1.0, 512) }
    }
}
impl Processor for Gain {
    fn set_param(&mut self, id: &str, v: f64) {
        if id == "gain_db" {
            self.g.set(db(v));
        }
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for i in 0..l.len() {
            let g = self.g.next() as f32;
            l[i] *= g;
            r[i] *= g;
        }
    }
}

// ---------------------------------------------------------------------------

/// Parametric EQ: high-pass (24 dB/oct), low shelf, two bells, presence,
/// air shelf, low-pass.
pub struct Eq {
    sr: f64,
    p: BTreeMap<&'static str, f64>,
    bands: [[Biquad; 2]; 8],
    active: [bool; 8],
    out: Smoothed,
}

const EQ_IDS: [&str; 15] = [
    "hp_freq",
    "low_gain",
    "low_freq",
    "lowmid_gain",
    "lowmid_freq",
    "lowmid_q",
    "mid_gain",
    "mid_freq",
    "mid_q",
    "presence_gain",
    "presence_freq",
    "air_gain",
    "air_freq",
    "lp_freq",
    "output_db",
];

impl Eq {
    pub fn new(sr: f64) -> Eq {
        let mut p = BTreeMap::new();
        for id in EQ_IDS {
            let spec = studio_core::effects::param_spec(&EffectKind::Eq, id).unwrap();
            p.insert(id, spec.default);
        }
        let mut e = Eq { sr, p, bands: [[Biquad::default(); 2]; 8], active: [false; 8], out: Smoothed::new(1.0, 512) };
        e.update();
        e
    }
    /// The coefficients of each active band (for drawing the curve).
    pub fn coefs(&self) -> Vec<Coefs> {
        (0..8).filter(|&i| self.active[i]).map(|i| self.bands[i][0].c).collect()
    }
    fn update(&mut self) {
        let sr = self.sr;
        let g = |k: &str| self.p[k];
        let mut set = |i: usize, active: bool, c: Coefs| {
            self.active[i] = active;
            for b in &mut self.bands[i] {
                b.c = c;
            }
        };
        let hp = g("hp_freq");
        set(0, hp > 21.0, Coefs::new(FilterType::HighPass, sr, hp, 0.54, 0.0));
        set(1, hp > 21.0, Coefs::new(FilterType::HighPass, sr, hp, 1.31, 0.0));
        set(2, g("low_gain").abs() > 0.01, Coefs::new(FilterType::LowShelf, sr, g("low_freq"), 0.7, g("low_gain")));
        set(
            3,
            g("lowmid_gain").abs() > 0.01,
            Coefs::new(FilterType::Peak, sr, g("lowmid_freq"), g("lowmid_q"), g("lowmid_gain")),
        );
        set(4, g("mid_gain").abs() > 0.01, Coefs::new(FilterType::Peak, sr, g("mid_freq"), g("mid_q"), g("mid_gain")));
        set(
            5,
            g("presence_gain").abs() > 0.01,
            Coefs::new(FilterType::Peak, sr, g("presence_freq"), 0.8, g("presence_gain")),
        );
        set(6, g("air_gain").abs() > 0.01, Coefs::new(FilterType::HighShelf, sr, g("air_freq"), 0.7, g("air_gain")));
        let lp = g("lp_freq");
        set(7, lp < 19_900.0, Coefs::new(FilterType::LowPass, sr, lp, 0.707, 0.0));
        self.out.set(db(g("output_db")));
    }
}

impl Processor for Eq {
    fn set_param(&mut self, id: &str, v: f64) {
        if let Some(k) = EQ_IDS.iter().find(|k| **k == id) {
            self.p.insert(k, v);
            self.update();
        }
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for i in 0..l.len() {
            let mut a = l[i] as f64;
            let mut b = r[i] as f64;
            for (band, &on) in self.bands.iter_mut().zip(self.active.iter()) {
                if on {
                    a = band[0].tick(a);
                    b = band[1].tick(b);
                }
            }
            let g = self.out.next();
            l[i] = (a * g) as f32;
            r[i] = (b * g) as f32;
        }
    }
    fn reset(&mut self) {
        for b in self.bands.iter_mut().flatten() {
            b.reset();
        }
    }
}

// ---------------------------------------------------------------------------

/// Feed-forward compressor with soft knee, linked stereo.
pub struct Compressor {
    sr: f64,
    threshold: f64,
    ratio: f64,
    knee: f64,
    att: f64,
    rel: f64,
    makeup: f64,
    mix: f64,
    env_db: f64,
    gr: f32,
}

impl Compressor {
    pub fn new(sr: f64) -> Compressor {
        Compressor {
            sr,
            threshold: -18.0,
            ratio: 3.0,
            knee: 6.0,
            att: time_coef(sr, 8.0),
            rel: time_coef(sr, 120.0),
            makeup: 1.0,
            mix: 1.0,
            env_db: 0.0,
            gr: 0.0,
        }
    }
    fn gain_computer(&self, x_db: f64) -> f64 {
        let t = self.threshold;
        let k = self.knee;
        let r = self.ratio.max(1.0);
        let over = x_db - t;
        if 2.0 * over < -k {
            x_db
        } else if 2.0 * over.abs() <= k && k > 0.0 {
            x_db + (1.0 / r - 1.0) * (over + k / 2.0).powi(2) / (2.0 * k)
        } else {
            t + over / r
        }
    }
}

impl Processor for Compressor {
    fn set_param(&mut self, id: &str, v: f64) {
        match id {
            "threshold_db" => self.threshold = v,
            "ratio" => self.ratio = v,
            "knee_db" => self.knee = v,
            "attack_ms" => self.att = time_coef(self.sr, v),
            "release_ms" => self.rel = time_coef(self.sr, v),
            "makeup_db" => self.makeup = db(v),
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let mut max_gr = 0.0f64;
        for i in 0..l.len() {
            let x = (l[i].abs().max(r[i].abs()) as f64).max(1e-9);
            let x_db = 20.0 * x.log10();
            let reduction = x_db - self.gain_computer(x_db); // >= 0
            let c = if reduction > self.env_db { self.att } else { self.rel };
            self.env_db = reduction + c * (self.env_db - reduction);
            max_gr = max_gr.max(self.env_db);
            let g = db(-self.env_db) * self.makeup;
            let wet = g * self.mix + (1.0 - self.mix);
            l[i] = (l[i] as f64 * wet) as f32;
            r[i] = (r[i] as f64 * wet) as f32;
        }
        self.gr = max_gr as f32;
    }
    fn reset(&mut self) {
        self.env_db = 0.0;
    }
    fn reduction_db(&self) -> f32 {
        self.gr
    }
}

// ---------------------------------------------------------------------------

pub struct Gate {
    sr: f64,
    threshold: f64,
    floor: f64,
    att: f64,
    rel: f64,
    hold: usize,
    held: usize,
    env: f64,
    gain: f64,
}

impl Gate {
    pub fn new(sr: f64) -> Gate {
        Gate {
            sr,
            threshold: db(-55.0),
            floor: db(-30.0),
            att: time_coef(sr, 1.0),
            rel: time_coef(sr, 120.0),
            hold: (0.04 * sr) as usize,
            held: 0,
            env: 0.0,
            gain: 1.0,
        }
    }
}

impl Processor for Gate {
    fn set_param(&mut self, id: &str, v: f64) {
        match id {
            "threshold_db" => self.threshold = db(v),
            "range_db" => self.floor = db(-v),
            "attack_ms" => self.att = time_coef(self.sr, v),
            "hold_ms" => self.hold = (v * 0.001 * self.sr) as usize,
            "release_ms" => self.rel = time_coef(self.sr, v),
            _ => {}
        }
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let env_rel = time_coef(self.sr, 15.0);
        for i in 0..l.len() {
            let x = l[i].abs().max(r[i].abs()) as f64;
            self.env = if x > self.env { x } else { x + env_rel * (self.env - x) };
            let open = if self.env > self.threshold {
                self.held = self.hold;
                true
            } else if self.held > 0 {
                self.held -= 1;
                true
            } else {
                false
            };
            let target = if open { 1.0 } else { self.floor };
            let c = if target > self.gain { self.att } else { self.rel };
            self.gain = target + c * (self.gain - target);
            l[i] = (l[i] as f64 * self.gain) as f32;
            r[i] = (r[i] as f64 * self.gain) as f32;
        }
    }
    fn reset(&mut self) {
        self.env = 0.0;
        self.gain = 1.0;
    }
    fn reduction_db(&self) -> f32 {
        (-20.0 * self.gain.max(1e-6).log10()) as f32
    }
}

// ---------------------------------------------------------------------------

/// Split-band de-esser: the band above `freq` is turned down when its
/// level passes the threshold.
pub struct DeEsser {
    sr: f64,
    split: [OnePole; 2],
    detect: Biquad,
    threshold: f64,
    max_red: f64,
    env: f64,
    gr: f64,
}

impl DeEsser {
    pub fn new(sr: f64) -> DeEsser {
        let mut d = DeEsser {
            sr,
            split: [OnePole::default(); 2],
            detect: Biquad::default(),
            threshold: -30.0,
            max_red: 8.0,
            env: 0.0,
            gr: 0.0,
        };
        d.set_freq(6500.0);
        d
    }
    fn set_freq(&mut self, f: f64) {
        for s in &mut self.split {
            s.set_freq(self.sr, f * 0.7);
        }
        self.detect.c = Coefs::new(FilterType::BandPass, self.sr, f, 1.2, 0.0);
    }
}

impl Processor for DeEsser {
    fn set_param(&mut self, id: &str, v: f64) {
        match id {
            "freq" => self.set_freq(v),
            "threshold_db" => self.threshold = v,
            "amount_db" => self.max_red = v,
            _ => {}
        }
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let att = time_coef(self.sr, 0.5);
        let rel = time_coef(self.sr, 60.0);
        let mut max_gr = 0.0f64;
        for i in 0..l.len() {
            let m = (l[i] as f64 + r[i] as f64) * 0.5;
            let s = self.detect.tick(m).abs();
            let c = if s > self.env { att } else { rel };
            self.env = s + c * (self.env - s);
            let lvl = 20.0 * self.env.max(1e-9).log10();
            let red = ((lvl - self.threshold) * 0.6).clamp(0.0, self.max_red);
            self.gr = red;
            max_gr = max_gr.max(red);
            let g = db(-red);
            let hl = self.split[0].hp(l[i] as f64);
            let hr = self.split[1].hp(r[i] as f64);
            l[i] = (l[i] as f64 - hl + hl * g) as f32;
            r[i] = (r[i] as f64 - hr + hr * g) as f32;
        }
        self.gr = max_gr;
    }
    fn reduction_db(&self) -> f32 {
        self.gr as f32
    }
}

// ---------------------------------------------------------------------------

struct Comb {
    buf: Vec<f32>,
    i: usize,
    store: f32,
}
impl Comb {
    fn new(n: usize) -> Comb {
        Comb { buf: vec![0.0; n.max(1)], i: 0, store: 0.0 }
    }
    #[inline]
    fn tick(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let y = self.buf[self.i];
        self.store = y * (1.0 - damp) + self.store * damp;
        self.buf[self.i] = x + self.store * feedback;
        self.i += 1;
        if self.i >= self.buf.len() {
            self.i = 0;
        }
        y
    }
}

struct Allpass {
    buf: Vec<f32>,
    i: usize,
}
impl Allpass {
    fn new(n: usize) -> Allpass {
        Allpass { buf: vec![0.0; n.max(1)], i: 0 }
    }
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let b = self.buf[self.i];
        let y = -x + b;
        self.buf[self.i] = x + b * 0.5;
        self.i += 1;
        if self.i >= self.buf.len() {
            self.i = 0;
        }
        y
    }
}

/// Freeverb-style stereo reverb with pre-delay and wet filtering.
pub struct Reverb {
    sr: f64,
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
    pre: Vec<f32>,
    pre_i: usize,
    pre_len: usize,
    feedback: f32,
    damp: f32,
    mix: f32,
    width: f32,
    lowcut: [OnePole; 2],
    highcut: [OnePole; 2],
}

const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];

impl Reverb {
    pub fn new(sr: f64) -> Reverb {
        let scale = sr / 44100.0;
        let mk = |spread: usize| -> (Vec<Comb>, Vec<Allpass>) {
            (
                COMB_TUNING.iter().map(|&n| Comb::new(((n + spread) as f64 * scale) as usize)).collect(),
                ALLPASS_TUNING.iter().map(|&n| Allpass::new(((n + spread) as f64 * scale) as usize)).collect(),
            )
        };
        let (cl, al) = mk(0);
        let (cr, ar) = mk(23);
        Reverb {
            sr,
            combs: [cl, cr],
            allpasses: [al, ar],
            pre: vec![0.0; (0.25 * sr) as usize + 1],
            pre_i: 0,
            pre_len: (0.02 * sr) as usize,
            feedback: 0.84,
            damp: 0.2,
            mix: 0.18,
            width: 1.0,
            lowcut: [OnePole::lowpass(sr, 250.0); 2],
            highcut: [OnePole::lowpass(sr, 9000.0); 2],
        }
    }
}

impl Processor for Reverb {
    fn set_param(&mut self, id: &str, v: f64) {
        match id {
            "size" => self.feedback = (0.70 + 0.28 * v) as f32,
            "damping" => self.damp = (v * 0.4) as f32,
            "mix" => self.mix = v as f32,
            "width" => self.width = v as f32,
            "predelay_ms" => self.pre_len = ((v * 0.001 * self.sr) as usize).min(self.pre.len() - 1),
            "low_cut" => {
                for f in &mut self.lowcut {
                    f.set_freq(self.sr, v)
                }
            }
            "high_cut" => {
                for f in &mut self.highcut {
                    f.set_freq(self.sr, v)
                }
            }
            _ => {}
        }
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = self.pre.len();
        for i in 0..l.len() {
            let input = (l[i] + r[i]) * 0.5 * 0.03;
            self.pre[self.pre_i] = input;
            let read = (self.pre_i + n - self.pre_len) % n;
            let x = self.pre[read];
            self.pre_i = (self.pre_i + 1) % n;
            let mut out = [0.0f32; 2];
            for ch in 0..2 {
                let mut acc = 0.0;
                for c in &mut self.combs[ch] {
                    acc += c.tick(x, self.feedback, self.damp);
                }
                for a in &mut self.allpasses[ch] {
                    acc = a.tick(acc);
                }
                let hp = self.lowcut[ch].hp(acc as f64);
                out[ch] = self.highcut[ch].lp(hp) as f32;
            }
            let w1 = self.width / 2.0 + 0.5;
            let w2 = (1.0 - self.width) / 2.0;
            let wl = out[0] * w1 + out[1] * w2;
            let wr = out[1] * w1 + out[0] * w2;
            let dry = 1.0 - self.mix * 0.5;
            l[i] = l[i] * dry + wl * self.mix * 3.0;
            r[i] = r[i] * dry + wr * self.mix * 3.0;
        }
    }
    fn reset(&mut self) {
        for c in self.combs.iter_mut().flatten() {
            c.buf.fill(0.0);
            c.store = 0.0;
        }
        for a in self.allpasses.iter_mut().flatten() {
            a.buf.fill(0.0);
        }
        self.pre.fill(0.0);
    }
}

// ---------------------------------------------------------------------------

/// Tempo-synced stereo delay (ping-pong optional).
pub struct Delay {
    sr: f64,
    buf: [Vec<f32>; 2],
    w: usize,
    time_ms: f64,
    sync: usize,
    bpm: f64,
    len: Smoothed,
    feedback: f32,
    mix: f32,
    ping_pong: bool,
    tone: [OnePole; 2],
}

impl Delay {
    pub fn new(sr: f64) -> Delay {
        let n = (2.5 * sr) as usize;
        Delay {
            sr,
            buf: [vec![0.0; n], vec![0.0; n]],
            w: 0,
            time_ms: 375.0,
            sync: 4,
            bpm: 120.0,
            len: Smoothed::new(0.375 * sr, 2048),
            feedback: 0.3,
            mix: 0.15,
            ping_pong: true,
            tone: [OnePole::lowpass(sr, 6000.0); 2],
        }
    }
    fn update(&mut self) {
        let beat = 60.0 / self.bpm.max(20.0);
        let secs = match self.sync {
            1 => beat / 4.0,
            2 => beat / 2.0,
            3 => beat * 0.75,
            4 => beat,
            5 => beat * 2.0,
            6 => beat * 4.0,
            _ => self.time_ms / 1000.0,
        };
        let max = (self.buf[0].len() - 2) as f64;
        self.len.set((secs * self.sr).clamp(1.0, max));
    }
}

impl Processor for Delay {
    fn set_param(&mut self, id: &str, v: f64) {
        match id {
            "time_ms" => self.time_ms = v,
            "sync" => self.sync = v.round() as usize,
            "feedback" => self.feedback = v as f32,
            "mix" => self.mix = v as f32,
            "ping_pong" => self.ping_pong = v >= 0.5,
            "high_cut" => {
                for t in &mut self.tone {
                    t.set_freq(self.sr, v)
                }
            }
            _ => {}
        }
        self.update();
    }
    fn set_bpm(&mut self, bpm: f64) {
        self.bpm = bpm;
        self.update();
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = self.buf[0].len();
        for i in 0..l.len() {
            let d = self.len.next();
            let pos = self.w as f64 + n as f64 - d;
            let a = pos.floor() as usize % n;
            let b = (a + 1) % n;
            let frac = (pos - pos.floor()) as f32;
            let yl = self.buf[0][a] * (1.0 - frac) + self.buf[0][b] * frac;
            let yr = self.buf[1][a] * (1.0 - frac) + self.buf[1][b] * frac;
            let fl = self.tone[0].lp(yl as f64) as f32 * self.feedback;
            let fr = self.tone[1].lp(yr as f64) as f32 * self.feedback;
            if self.ping_pong {
                let m = (l[i] + r[i]) * 0.5;
                self.buf[0][self.w] = m + fr;
                self.buf[1][self.w] = fl;
            } else {
                self.buf[0][self.w] = l[i] + fl;
                self.buf[1][self.w] = r[i] + fr;
            }
            self.w = (self.w + 1) % n;
            l[i] += yl * self.mix;
            r[i] += yr * self.mix;
        }
    }
    fn reset(&mut self) {
        self.buf[0].fill(0.0);
        self.buf[1].fill(0.0);
    }
}

// ---------------------------------------------------------------------------

pub struct Saturation {
    sr: f64,
    drive: f64,
    tone: f64,
    mix: f64,
    tilt: [OnePole; 2],
}

impl Saturation {
    pub fn new(sr: f64) -> Saturation {
        Saturation { sr, drive: db(6.0), tone: 0.0, mix: 0.5, tilt: [OnePole::lowpass(sr, 1200.0); 2] }
    }
}

impl Processor for Saturation {
    fn set_param(&mut self, id: &str, v: f64) {
        match id {
            "drive_db" => self.drive = db(v),
            "tone" => self.tone = v,
            "mix" => self.mix = v,
            _ => {}
        }
        let _ = self.sr;
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let comp = 1.0 / self.drive.tanh().max(0.1);
        for (ch, buf) in [l, r].into_iter().enumerate() {
            for s in buf.iter_mut() {
                let x = *s as f64;
                let low = self.tilt[ch].lp(x);
                let high = x - low;
                // tone tilts toward lows (<0) or highs (>0) before the shaper
                let tilted = low * (1.0 - 0.5 * self.tone) + high * (1.0 + 0.5 * self.tone);
                let y = (tilted * self.drive).tanh() * comp * 0.9;
                *s = (x * (1.0 - self.mix) + y * self.mix) as f32;
            }
        }
    }
}

// ---------------------------------------------------------------------------

/// Look-ahead peak limiter.
pub struct Limiter {
    sr: f64,
    input: f64,
    ceiling: f64,
    rel: f64,
    look: usize,
    dl: [Vec<f32>; 2],
    gains: Vec<f64>,
    w: usize,
    env: f64,
    smooth: f64,
    gr: f32,
}

impl Limiter {
    pub fn new(sr: f64) -> Limiter {
        let look = (0.005 * sr) as usize + 1;
        Limiter {
            sr,
            input: 1.0,
            ceiling: db(-1.0),
            rel: time_coef(sr, 60.0),
            look,
            dl: [vec![0.0; look], vec![0.0; look]],
            gains: vec![1.0; look],
            w: 0,
            env: 1.0,
            smooth: 1.0,
            gr: 0.0,
        }
    }
}

impl Processor for Limiter {
    fn set_param(&mut self, id: &str, v: f64) {
        match id {
            "gain_db" => self.input = db(v),
            "ceiling_db" => self.ceiling = db(v),
            "release_ms" => self.rel = time_coef(self.sr, v),
            _ => {}
        }
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = self.look;
        // reaches the window's gain (to 0.7 %) before the peak is output
        let att = (-5.0 / n as f64).exp();
        let mut max_gr = 0.0f32;
        for i in 0..l.len() {
            let a = l[i] as f64 * self.input;
            let b = r[i] as f64 * self.input;
            let pk = a.abs().max(b.abs());
            let need = if pk > self.ceiling { self.ceiling / pk } else { 1.0 };
            self.gains[self.w] = need;
            // the lowest gain needed within the look-ahead window
            let mut g = 1.0f64;
            for &x in &self.gains {
                g = g.min(x);
            }
            self.env = if g < self.env { g } else { g + self.rel * (self.env - g) };
            self.smooth = if self.env < self.smooth { self.env + att * (self.smooth - self.env) } else { self.env };
            let gain = self.smooth.min(1.0);
            let ol = self.dl[0][self.w] as f64 * gain;
            let or = self.dl[1][self.w] as f64 * gain;
            self.dl[0][self.w] = a as f32;
            self.dl[1][self.w] = b as f32;
            self.w = (self.w + 1) % n;
            // safety clip for what the smoothing let through
            l[i] = ol.clamp(-self.ceiling, self.ceiling) as f32;
            r[i] = or.clamp(-self.ceiling, self.ceiling) as f32;
            max_gr = max_gr.max((-20.0 * gain.max(1e-6).log10()) as f32);
        }
        self.gr = max_gr;
    }
    fn reset(&mut self) {
        self.dl[0].fill(0.0);
        self.dl[1].fill(0.0);
        self.gains.fill(1.0);
        self.env = 1.0;
        self.smooth = 1.0;
    }
    fn reduction_db(&self) -> f32 {
        self.gr
    }
}

// ---------------------------------------------------------------------------

/// Two detuned, delayed copies panned apart (vocal doubling / chorus).
pub struct Doubler {
    sr: f64,
    buf: Vec<f32>,
    w: usize,
    phase: [f64; 2],
    delay_ms: f64,
    depth_ms: f64,
    width: f32,
    mix: f32,
}

impl Doubler {
    pub fn new(sr: f64) -> Doubler {
        Doubler {
            sr,
            buf: vec![0.0; (0.12 * sr) as usize],
            w: 0,
            phase: [0.0, 0.37],
            delay_ms: 18.0,
            depth_ms: 1.0,
            width: 0.85,
            mix: 0.35,
        }
    }
    fn read(&self, delay: f64) -> f32 {
        let n = self.buf.len();
        let pos = self.w as f64 + n as f64 - delay;
        let a = pos.floor() as usize % n;
        let b = (a + 1) % n;
        let f = (pos - pos.floor()) as f32;
        self.buf[a] * (1.0 - f) + self.buf[b] * f
    }
}

impl Processor for Doubler {
    fn set_param(&mut self, id: &str, v: f64) {
        match id {
            // pitch deviation of a sine-modulated delay: depth * 2π f; at
            // 0.6 Hz, 1 ms of depth is about 6.5 cents
            "detune_cents" => self.depth_ms = v / 6.5,
            "delay_ms" => self.delay_ms = v,
            "width" => self.width = v as f32,
            "mix" => self.mix = v as f32,
            _ => {}
        }
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let rates = [0.6, 0.47];
        let n = self.buf.len();
        for i in 0..l.len() {
            self.buf[self.w] = (l[i] + r[i]) * 0.5;
            let mut v = [0.0f32; 2];
            for k in 0..2 {
                self.phase[k] = (self.phase[k] + rates[k] / self.sr).fract();
                let base = self.delay_ms * if k == 0 { 1.0 } else { 1.37 };
                let d = (base + self.depth_ms * (2.0 * std::f64::consts::PI * self.phase[k]).sin()) * 0.001 * self.sr;
                v[k] = self.read(d.clamp(1.0, (n - 2) as f64));
            }
            self.w = (self.w + 1) % n;
            let a = 0.5 + 0.5 * self.width;
            let b = 1.0 - a;
            l[i] += (v[0] * a + v[1] * b) * self.mix;
            r[i] += (v[1] * a + v[0] * b) * self.mix;
        }
    }
    fn reset(&mut self) {
        self.buf.fill(0.0);
    }
}

// ---------------------------------------------------------------------------

pub struct StereoWidth {
    sr: f64,
    width: Smoothed,
    hp: OnePole,
}

impl StereoWidth {
    pub fn new(sr: f64) -> StereoWidth {
        StereoWidth { sr, width: Smoothed::new(1.0, 512), hp: OnePole::lowpass(sr, 120.0) }
    }
}

impl Processor for StereoWidth {
    fn set_param(&mut self, id: &str, v: f64) {
        match id {
            "width" => self.width.set(v),
            "bass_mono_freq" => self.hp.set_freq(self.sr, v),
            _ => {}
        }
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for i in 0..l.len() {
            let m = (l[i] as f64 + r[i] as f64) * 0.5;
            let s = (l[i] as f64 - r[i] as f64) * 0.5;
            let s = self.hp.hp(s) * self.width.next();
            l[i] = (m + s) as f32;
            r[i] = (m - s) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(f: f64, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / 48000.0).sin() as f32 * amp).collect()
    }

    #[test]
    fn every_effect_is_stable() {
        for k in studio_core::effects::BUILTIN.iter() {
            let mut p = create(k, 48000.0, &Default::default(), 120.0).unwrap();
            let mut l = sine(440.0, 0.5, 48000);
            let mut r = sine(660.0, 0.5, 48000);
            for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
                p.process(a, b);
            }
            let pk = l.iter().chain(r.iter()).fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(pk.is_finite() && pk < 4.0, "{:?} peak {pk}", k);
        }
    }

    #[test]
    fn limiter_holds_ceiling() {
        let mut lim = Limiter::new(48000.0);
        lim.set_param("gain_db", 12.0);
        lim.set_param("ceiling_db", -1.0);
        let mut l = sine(100.0, 0.9, 48000);
        let mut r = l.clone();
        lim.process(&mut l, &mut r);
        let pk = l.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(pk <= db(-1.0) as f32 + 1e-4, "{pk}");
        assert!(pk > 0.8);
    }

    #[test]
    fn compressor_reduces_loud_parts() {
        let mut c = Compressor::new(48000.0);
        c.set_param("threshold_db", -20.0);
        c.set_param("ratio", 4.0);
        c.set_param("makeup_db", 0.0);
        let mut l = sine(200.0, 1.0, 48000);
        let mut r = l.clone();
        c.process(&mut l, &mut r);
        let pk = l[24000..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        // 0 dB in, -20 threshold, 4:1 → about -15 dB
        assert!(pk < 0.3 && pk > 0.1, "{pk}");
    }
}
