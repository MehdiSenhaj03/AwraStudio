//! Loudness (ITU-R BS.1770 / EBU R128): K-weighting, gated integrated
//! loudness, short-term loudness, true peak estimate.

use crate::filters::{Biquad, Coefs};

fn k_filters(sr: f64) -> [Biquad; 2] {
    // the BS.1770 pre-filter (high shelf +4 dB) and RLB high-pass, at any rate
    let shelf = {
        let f0 = 1681.974450955533;
        let g = 3.999843853973347;
        let q = 0.7071752369554196;
        let k = (std::f64::consts::PI * f0 / sr).tan();
        let vh = 10f64.powf(g / 20.0);
        let vb = vh.powf(0.4996667741545416);
        let a0 = 1.0 + k / q + k * k;
        Coefs {
            b0: (vh + vb * k / q + k * k) / a0,
            b1: 2.0 * (k * k - vh) / a0,
            b2: (vh - vb * k / q + k * k) / a0,
            a1: 2.0 * (k * k - 1.0) / a0,
            a2: (1.0 - k / q + k * k) / a0,
        }
    };
    let hp = {
        let f0 = 38.13547087602444;
        let q = 0.5003270373238773;
        let k = (std::f64::consts::PI * f0 / sr).tan();
        let a0 = 1.0 + k / q + k * k;
        Coefs { b0: 1.0, b1: -2.0, b2: 1.0, a1: 2.0 * (k * k - 1.0) / a0, a2: (1.0 - k / q + k * k) / a0 }
    };
    [Biquad::new(shelf), Biquad::new(hp)]
}

/// Mean square of the K-weighted signal per 100 ms step.
fn block_powers(channels: &[&[f32]], sr: u32) -> Vec<f64> {
    let step = (sr / 10) as usize;
    let n = channels.iter().map(|c| c.len()).min().unwrap_or(0);
    let mut per_step = vec![0.0f64; n.div_ceil(step.max(1))];
    for ch in channels {
        let mut f = k_filters(sr as f64);
        for (i, &s) in ch.iter().take(n).enumerate() {
            let a = f[0].tick(s as f64);
            let y = f[1].tick(a);
            per_step[i / step] += y * y;
        }
    }
    per_step.iter().map(|p| p / step as f64).collect()
}

fn lufs(power: f64) -> f64 {
    -0.691 + 10.0 * power.max(1e-12).log10()
}

/// Integrated loudness (LUFS) with the two gates; -70 for silence.
pub fn integrated(channels: &[&[f32]], sr: u32) -> f64 {
    let steps = block_powers(channels, sr);
    if steps.len() < 4 {
        let p: f64 = steps.iter().sum::<f64>() / steps.len().max(1) as f64;
        return lufs(p).max(-70.0);
    }
    // 400 ms blocks, 75 % overlap
    let blocks: Vec<f64> = steps.windows(4).map(|w| w.iter().sum::<f64>() / 4.0).collect();
    let abs: Vec<f64> = blocks.iter().copied().filter(|&p| lufs(p) > -70.0).collect();
    if abs.is_empty() {
        return -70.0;
    }
    let mean = abs.iter().sum::<f64>() / abs.len() as f64;
    let rel_gate = lufs(mean) - 10.0;
    let rel: Vec<f64> = abs.iter().copied().filter(|&p| lufs(p) > rel_gate).collect();
    if rel.is_empty() {
        return lufs(mean);
    }
    lufs(rel.iter().sum::<f64>() / rel.len() as f64)
}

/// Short-term loudness (3 s windows), one value per second.
pub fn short_term(channels: &[&[f32]], sr: u32) -> Vec<f64> {
    let steps = block_powers(channels, sr);
    steps.windows(30.min(steps.len().max(1))).step_by(10).map(|w| lufs(w.iter().sum::<f64>() / w.len() as f64)).collect()
}

/// True peak (dBTP), estimated with 4x linear-phase interpolation.
pub fn true_peak_db(channels: &[&[f32]]) -> f64 {
    let mut pk = 0.0f32;
    for ch in channels {
        for w in ch.windows(4) {
            pk = pk.max(w[1].abs());
            // cubic (Catmull-Rom) interpolation between w[1] and w[2]
            for k in 1..4 {
                let t = k as f32 / 4.0;
                let (p0, p1, p2, p3) = (w[0], w[1], w[2], w[3]);
                let v = 0.5
                    * ((2.0 * p1)
                        + (-p0 + p2) * t
                        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
                        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t);
                pk = pk.max(v.abs());
            }
        }
    }
    20.0 * (pk.max(1e-9) as f64).log10()
}

/// Loudness range (LRA, LU): spread of short-term loudness.
pub fn range(channels: &[&[f32]], sr: u32) -> f64 {
    let mut st: Vec<f64> = short_term(channels, sr).into_iter().filter(|&l| l > -70.0).collect();
    if st.len() < 3 {
        return 0.0;
    }
    st.sort_by(|a, b| a.total_cmp(b));
    let lo = st[(st.len() as f64 * 0.10) as usize];
    let hi = st[((st.len() as f64 * 0.95) as usize).min(st.len() - 1)];
    hi - lo
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_1k_at_minus_20_dbfs() {
        // EBU: a 1 kHz sine at -20 dBFS in both channels reads about -20 LUFS
        // (K-weighting is ~+0.7 dB at 1 kHz, -3 dB for a sine's RMS, +3 dB for two channels)
        let sr = 48000;
        let a = 10f32.powf(-20.0 / 20.0);
        let x: Vec<f32> = (0..sr * 5).map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr as f32).sin() * a).collect();
        let l = integrated(&[&x, &x], sr as u32);
        assert!((l + 20.0).abs() < 0.5, "{l}");
    }
}
