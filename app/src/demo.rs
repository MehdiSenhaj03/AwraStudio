//! A small demo song (a synthesized beat and two "voices") so that the
//! studio can be tried without recording anything.

use std::f32::consts::PI;

use studio_core::model::*;
use studio_core::ops::{Op, Transaction};
use studio_core::Document;
use studio_dsp::io::{wav_bytes, WavFormat};
use studio_dsp::AudioBuffer;

const SR: u32 = 48000;

fn noise(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    (*seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
}

/// 8 bars at 92 BPM in A minor: Am F C G.
pub fn beat(bars: usize) -> AudioBuffer {
    let bpm = 92.0f32;
    let beat = 60.0 / bpm;
    let n = (bars as f32 * 4.0 * beat * SR as f32) as usize;
    let mut l = vec![0.0f32; n];
    let mut r = vec![0.0f32; n];
    let mut seed = 11u32;
    let chords: [[f32; 3]; 4] = [[57.0, 60.0, 64.0], [53.0, 57.0, 60.0], [48.0, 52.0, 55.0], [55.0, 59.0, 62.0]];
    let hz = |m: f32| 440.0 * 2f32.powf((m - 69.0) / 12.0);
    for i in 0..n {
        let t = i as f32 / SR as f32;
        let b = t / beat;
        let in_beat = b.fract() * beat;
        let bar = (b / 4.0) as usize;
        // kick on 1 and 3 (and the "and" of 4), snare on 2 and 4, hats on eighths
        let step = (b * 2.0).floor() as i32 % 8;
        let mut s = 0.0;
        let kb = b % 4.0;
        let kick = if kb < 0.35 {
            Some(kb * beat)
        } else if (2.0..2.35).contains(&kb) {
            Some((kb - 2.0) * beat)
        } else if (3.5..3.75).contains(&kb) {
            Some((kb - 3.5) * beat)
        } else {
            None
        };
        if let Some(kt) = kick {
            let f = 45.0 + 90.0 * (-kt * 25.0).exp();
            s += (2.0 * PI * f * kt).sin() * (-kt * 7.0).exp() * 0.9;
        }
        if (kb >= 1.0 && kb < 1.4) || (kb >= 3.0 && kb < 3.4) {
            let st = (kb.fract()) * beat;
            s += (noise(&mut seed) * 0.7 + (2.0 * PI * 190.0 * st).sin() * 0.3) * (-st * 18.0).exp() * 0.55;
        }
        let ht = ((b * 2.0).fract()) * beat / 2.0;
        let hat = noise(&mut seed) * (-ht * 60.0).exp() * if step % 2 == 1 { 0.12 } else { 0.07 };
        // a soft pad
        let chord = chords[bar % 4];
        let mut pad = 0.0;
        for (k, m) in chord.iter().enumerate() {
            let f = hz(*m);
            pad += ((2.0 * PI * f * t).sin() + 0.3 * (2.0 * PI * f * 2.003 * t).sin()) * (0.06 - k as f32 * 0.008);
        }
        let swell = 0.6 + 0.4 * (in_beat / beat * PI).sin();
        // bass on the root
        let bass = (2.0 * PI * hz(chord[0] - 12.0) * t).sin() * 0.22 * (0.4 + 0.6 * (-((b % 1.0) * beat) * 3.0).exp());
        let mono = s + bass + pad * swell;
        l[i] = (mono + hat * 0.8) * 0.7;
        r[i] = (mono + hat * 1.2) * 0.7;
    }
    AudioBuffer::from_stereo(SR, l, r)
}

/// A sung melody: notes (midi, beats), slightly off pitch, with breath
/// noise and room hiss, like a home recording.
pub fn voice(notes: &[(f32, f32)], offset_cents: f32, seed0: u32, bpm: f32) -> AudioBuffer {
    let beat = 60.0 / bpm;
    let total: f32 = notes.iter().map(|n| n.1).sum();
    let n = (total * beat * SR as f32) as usize + SR as usize;
    let mut out = vec![0.0f32; n];
    let mut seed = seed0;
    let mut pos = 0usize;
    let mut phase = 0.0f32;
    for (midi, len) in notes {
        let samples = (len * beat * SR as f32) as usize;
        for k in 0..samples {
            let i = pos + k;
            if i >= n {
                break;
            }
            let t = k as f32 / SR as f32;
            out[i] += noise(&mut seed) * 0.004;
            if *midi <= 0.0 {
                continue;
            }
            let wobble = (t * 5.5 * 2.0 * PI).sin() * 0.25 + offset_cents / 100.0 * (1.0 + 0.3 * (t * 1.3).sin());
            let f = 440.0 * 2f32.powf((midi + wobble - 69.0) / 12.0);
            phase += 2.0 * PI * f / SR as f32;
            let env = (t * 30.0).min(1.0) * ((samples - k) as f32 / (0.08 * SR as f32)).min(1.0);
            // a few harmonics shaped like an "a" vowel
            let s = phase.sin() * 0.5 + (2.0 * phase).sin() * 0.35 + (3.0 * phase).sin() * 0.28 + (4.0 * phase).sin() * 0.12 + (6.0 * phase).sin() * 0.05;
            out[i] += s * env * 0.35;
        }
        pos += samples;
    }
    AudioBuffer::from_mono(SR, out)
}

fn add_asset(doc: &mut Document, buf: &AudioBuffer, name: &str, kind: AssetKind) -> AssetId {
    let bytes = wav_bytes(buf, WavFormat::Pcm24).expect("wav");
    let (id, file) = doc.store_asset_bytes(&bytes, "wav").expect("asset");
    let info = AssetInfo {
        id: id.clone(),
        name: name.into(),
        file,
        kind,
        duration: buf.duration(),
        channels: buf.channel_count() as u16,
        sample_rate: buf.sample_rate,
        author: None,
    };
    doc.commit(Transaction::new("demo", Actor::User, vec![Op::AddAsset { asset: info }])).ok();
    id
}

/// A project with a beat, a lead vocal, a second vocal, adlibs and lyrics.
pub fn demo_document() -> (Document, Vec<(AssetId, AudioBuffer)>) {
    let mut doc = Document::new(Project::new("Démo — Minuit"));
    doc.project.bpm = 92.0;
    let b = beat(8);
    let lead = voice(
        &[(0.0, 4.0), (69.0, 1.0), (72.0, 1.0), (71.0, 0.5), (69.0, 1.5), (0.0, 1.0), (67.0, 1.0), (69.0, 2.0), (64.0, 2.0), (0.0, 2.0),
          (69.0, 1.0), (72.0, 1.0), (74.0, 1.0), (72.0, 1.0), (71.0, 2.0), (69.0, 2.0)],
        28.0, 5, 92.0,
    );
    let second = voice(&[(0.0, 16.0), (64.0, 2.0), (65.0, 2.0), (67.0, 2.0), (64.0, 2.0)], -22.0, 9, 92.0);
    let adlib = voice(&[(0.0, 7.0), (76.0, 0.5), (0.0, 7.5), (74.0, 0.5), (0.0, 3.5), (76.0, 0.5)], 15.0, 13, 92.0);
    let ids = [
        add_asset(&mut doc, &b, "Instru Minuit.wav", AssetKind::Import),
        add_asset(&mut doc, &lead, "Prise Sam 1.wav", AssetKind::Take),
        add_asset(&mut doc, &second, "Prise Léa 1.wav", AssetKind::Take),
        add_asset(&mut doc, &adlib, "Adlibs Sam.wav", AssetKind::Take),
    ];
    let bufs = [b, lead, second, adlib];
    let specs = [
        ("Instru", TrackKind::Instrumental, "#e8b25c", None),
        ("Voix Sam", TrackKind::Vocal, "#5c9ce8", Some("Sam")),
        ("Voix Léa", TrackKind::Vocal, "#e87bb7", Some("Léa")),
        ("Adlibs", TrackKind::Adlib, "#7bd389", Some("Sam")),
    ];
    let mut ops = Vec::new();
    for (i, (name, kind, color, owner)) in specs.iter().enumerate() {
        let mut t = Track::new(name, *kind);
        t.color = color.to_string();
        t.owner = owner.map(String::from);
        t.clips.push(Clip {
            id: new_id(),
            asset: ids[i].clone(),
            name: name.to_string(),
            start: 0.0,
            offset: 0.0,
            length: bufs[i].duration(),
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.05,
            muted: false,
        });
        ops.push(Op::AddTrack { track: t, index: i });
    }
    ops.push(Op::AddSection {
        section: LyricSection {
            id: new_id(),
            title: "Couplet 1".into(),
            text: "Minuit sur la ville, j'écris sous les néons\nLes rues sont vides mais j'entends encore ton nom".into(),
            start: Some(2.6),
        },
        index: 0,
    });
    ops.push(Op::AddSection {
        section: LyricSection { id: new_id(), title: "Refrain".into(), text: "Et on danse, on danse jusqu'au matin\nOn oublie tout, on tient le refrain".into(), start: Some(10.4) },
        index: 1,
    });
    doc.commit(Transaction::new("Projet de démonstration", Actor::User, ops)).ok();
    doc.history = studio_core::history::History::new();
    doc.dirty = false;
    let out = ids.into_iter().zip(bufs).collect();
    (doc, out)
}
