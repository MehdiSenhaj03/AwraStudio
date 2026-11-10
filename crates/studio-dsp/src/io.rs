//! Reading audio files (WAV, MP3, FLAC, OGG, AAC/M4A, ALAC) and writing WAV.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::buffer::AudioBuffer;

pub const AUDIO_EXTENSIONS: [&str; 9] = ["wav", "mp3", "flac", "ogg", "oga", "m4a", "aac", "aif", "aiff"];

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Decodes a whole file into a planar buffer at its own rate.
pub fn decode_file(path: &Path) -> Result<AudioBuffer> {
    let file = std::fs::File::open(path).with_context(|| format!("ouverture de {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| anyhow!("format audio non reconnu : {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow!("aucune piste audio"))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| anyhow!("codec non pris en charge : {e}"))?;
    let mut rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut channels: Vec<Vec<f32>> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => {
                if channels.is_empty() {
                    return Err(anyhow!("lecture : {e}"));
                }
                break;
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                rate = spec.rate;
                let nch = spec.channels.count().max(1);
                if channels.is_empty() {
                    channels = vec![Vec::new(); nch];
                }
                let mut sb = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                sb.copy_interleaved_ref(decoded);
                for frame in sb.samples().chunks(nch) {
                    for (c, &s) in frame.iter().enumerate() {
                        if let Some(ch) = channels.get_mut(c) {
                            ch.push(s);
                        }
                    }
                }
            }
            Err(SymError::DecodeError(_)) => continue,
            Err(SymError::IoError(_)) => break,
            Err(e) => return Err(anyhow!("décodage : {e}")),
        }
    }
    if channels.is_empty() || rate == 0 {
        return Err(anyhow!("fichier audio vide"));
    }
    // more than two channels: keep the front pair
    channels.truncate(2);
    Ok(AudioBuffer { sample_rate: rate, channels })
}

/// Decodes and converts to the project's rate.
pub fn load(path: &Path, rate: u32) -> Result<AudioBuffer> {
    Ok(decode_file(path)?.resampled(rate))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavFormat {
    Pcm16,
    Pcm24,
    Float32,
}

/// Writes a WAV file (dithered when 16-bit).
pub fn write_wav(path: &Path, buf: &AudioBuffer, fmt: WavFormat) -> Result<()> {
    let bytes = wav_bytes(buf, fmt)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

pub fn wav_bytes(buf: &AudioBuffer, fmt: WavFormat) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: buf.channel_count() as u16,
        sample_rate: buf.sample_rate,
        bits_per_sample: match fmt {
            WavFormat::Pcm16 => 16,
            WavFormat::Pcm24 => 24,
            WavFormat::Float32 => 32,
        },
        sample_format: if fmt == WavFormat::Float32 { hound::SampleFormat::Float } else { hound::SampleFormat::Int },
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cursor, spec)?;
        let mut rng = 0x1234_5678u32;
        let mut tpdf = || {
            // triangular dither from two uniform values
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let a = (rng as f32 / u32::MAX as f32) - 0.5;
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let b = (rng as f32 / u32::MAX as f32) - 0.5;
            a + b
        };
        for i in 0..buf.frames() {
            for c in &buf.channels {
                let s = c[i].clamp(-1.0, 1.0);
                match fmt {
                    WavFormat::Pcm16 => {
                        let v = (s * 32767.0 + tpdf()).round().clamp(-32768.0, 32767.0) as i16;
                        w.write_sample(v)?;
                    }
                    WavFormat::Pcm24 => w.write_sample((s * 8_388_607.0).round() as i32)?,
                    WavFormat::Float32 => w.write_sample(s)?,
                }
            }
        }
        w.finalize()?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_roundtrip() {
        let sr = 48000;
        let l: Vec<f32> = (0..sr).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        let r: Vec<f32> = l.iter().map(|s| -s).collect();
        let b = AudioBuffer::from_stereo(sr, l.clone(), r);
        let p = std::env::temp_dir().join(format!("dsp-io-{}.wav", std::process::id()));
        write_wav(&p, &b, WavFormat::Pcm24).unwrap();
        let back = decode_file(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(back.sample_rate, sr);
        assert_eq!(back.channel_count(), 2);
        assert_eq!(back.frames(), sr as usize);
        assert!((back.channels[0][1000] - l[1000]).abs() < 1e-4);
        assert!((back.channels[1][1000] + l[1000]).abs() < 1e-4);
    }
}
