//! Planar audio buffers, waveform peaks, resampling.

use std::f64::consts::PI;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioBuffer {
    pub sample_rate: u32,
    /// One Vec per channel, all the same length.
    pub channels: Vec<Vec<f32>>,
}

impl AudioBuffer {
    pub fn new(sample_rate: u32, channels: usize, frames: usize) -> AudioBuffer {
        AudioBuffer { sample_rate, channels: vec![vec![0.0; frames]; channels.max(1)] }
    }
    pub fn from_mono(sample_rate: u32, data: Vec<f32>) -> AudioBuffer {
        AudioBuffer { sample_rate, channels: vec![data] }
    }
    pub fn from_stereo(sample_rate: u32, l: Vec<f32>, r: Vec<f32>) -> AudioBuffer {
        AudioBuffer { sample_rate, channels: vec![l, r] }
    }
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }
    pub fn duration(&self) -> f64 {
        self.frames() as f64 / self.sample_rate.max(1) as f64
    }
    /// Left and right (a mono buffer gives its channel twice).
    pub fn lr(&self) -> (&[f32], &[f32]) {
        let l = &self.channels[0];
        let r = self.channels.get(1).unwrap_or(l);
        (l, r)
    }
    /// The channels averaged.
    pub fn mono(&self) -> Vec<f32> {
        let n = self.channels.len() as f32;
        (0..self.frames()).map(|i| self.channels.iter().map(|c| c[i]).sum::<f32>() / n).collect()
    }
    pub fn peak(&self) -> f32 {
        self.channels.iter().flat_map(|c| c.iter()).fold(0.0f32, |m, &s| m.max(s.abs()))
    }
    /// A slice of time (s) as a new buffer.
    pub fn slice(&self, start: f64, length: f64) -> AudioBuffer {
        let a = ((start * self.sample_rate as f64) as usize).min(self.frames());
        let b = (((start + length) * self.sample_rate as f64) as usize).min(self.frames());
        AudioBuffer { sample_rate: self.sample_rate, channels: self.channels.iter().map(|c| c[a..b].to_vec()).collect() }
    }
    pub fn resampled(&self, rate: u32) -> AudioBuffer {
        if rate == self.sample_rate {
            return self.clone();
        }
        AudioBuffer {
            sample_rate: rate,
            channels: self.channels.iter().map(|c| resample(c, self.sample_rate, rate)).collect(),
        }
    }
    /// Waveform overview: (min, max) of each block of `block` frames, mono.
    pub fn peaks(&self, block: usize) -> Vec<(f32, f32)> {
        let block = block.max(1);
        let n = self.frames().div_ceil(block);
        let mut out = Vec::with_capacity(n);
        for b in 0..n {
            let a = b * block;
            let e = (a + block).min(self.frames());
            let mut mn = 0.0f32;
            let mut mx = 0.0f32;
            for c in &self.channels {
                for &s in &c[a..e] {
                    mn = mn.min(s);
                    mx = mx.max(s);
                }
            }
            out.push((mn, mx));
        }
        out
    }
}

/// A multi-resolution waveform overview for drawing at any zoom.
#[derive(Clone, Debug, Default)]
pub struct Peaks {
    pub sample_rate: u32,
    /// Level i holds (min, max) per 2^(i+6) frames.
    pub levels: Vec<Vec<(f32, f32)>>,
}

impl Peaks {
    pub fn of(buf: &AudioBuffer) -> Peaks {
        let mut levels = vec![buf.peaks(64)];
        while levels.last().unwrap().len() > 64 {
            let prev = levels.last().unwrap();
            let next: Vec<(f32, f32)> = prev
                .chunks(2)
                .map(|c| c.iter().fold((0.0f32, 0.0f32), |(a, b), &(mn, mx)| (a.min(mn), b.max(mx))))
                .collect();
            levels.push(next);
        }
        Peaks { sample_rate: buf.sample_rate, levels }
    }
    /// (min, max) over a time range (s).
    pub fn range(&self, t0: f64, t1: f64) -> (f32, f32) {
        if self.levels.is_empty() || t1 <= t0 {
            return (0.0, 0.0);
        }
        let frames = (t1 - t0) * self.sample_rate as f64;
        // the coarsest level with at least one block per range
        let mut lvl = 0;
        while lvl + 1 < self.levels.len() && (64u64 << (lvl + 1)) as f64 <= frames {
            lvl += 1;
        }
        let per = (64u64 << lvl) as f64;
        let data = &self.levels[lvl];
        let a = ((t0 * self.sample_rate as f64) / per).floor().max(0.0) as usize;
        let b = (((t1 * self.sample_rate as f64) / per).ceil() as usize).min(data.len());
        if a >= b {
            return (0.0, 0.0);
        }
        data[a..b].iter().fold((0.0f32, 0.0f32), |(x, y), &(mn, mx)| (x.min(mn), y.max(mx)))
    }
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

/// Windowed-sinc resampling (Blackman window, 32 zero crossings).
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to as f64 / from as f64;
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let cutoff = ratio.min(1.0) * 0.95;
    let half = 32.0 / cutoff;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let center = i as f64 / ratio;
        let a = (center - half).ceil().max(0.0) as usize;
        let b = ((center + half).floor() as usize).min(input.len() - 1);
        let mut acc = 0.0;
        let mut norm = 0.0;
        for (j, &s) in input.iter().enumerate().take(b + 1).skip(a) {
            let x = j as f64 - center;
            let w = 0.42 + 0.5 * (PI * x / half).cos() + 0.08 * (2.0 * PI * x / half).cos();
            let k = cutoff * sinc(cutoff * x) * w;
            acc += s as f64 * k;
            norm += k;
        }
        out.push(if norm.abs() > 1e-9 { (acc / norm) as f32 } else { 0.0 });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_keeps_tone() {
        let sr = 44100;
        let f = 1000.0;
        let x: Vec<f32> = (0..sr).map(|i| (2.0 * PI * f * i as f64 / sr as f64).sin() as f32 * 0.5).collect();
        let y = resample(&x, sr, 48000);
        assert!((y.len() as i64 - 48000).abs() <= 1);
        // amplitude preserved in the middle
        let mid = &y[10000..40000];
        let pk = mid.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((pk - 0.5).abs() < 0.03, "peak {pk}");
    }

    #[test]
    fn peaks_ranges() {
        let mut b = AudioBuffer::new(48000, 1, 48000);
        b.channels[0][24000] = 0.9;
        let p = Peaks::of(&b);
        assert_eq!(p.range(0.0, 1.0).1, 0.9);
        assert_eq!(p.range(0.0, 0.4).1, 0.0);
    }
}
