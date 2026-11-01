//! Record from the onboard microphone, clean it, play it back.
//!
//! The microphone is mono but the codec only wires it to the **right** channel: capturing
//! stereo and keeping the odd samples is not an accident, it is the only channel that carries
//! signal (the left one is exactly zero). Anything downstream that reads "the microphone" has
//! to start from that fact.

pub mod dsp;

use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Sample rate for both directions. The codec is happy at 48 kHz and the cleanup filters are
/// tuned for it.
pub const RATE: u32 = 48_000;

/// Capture device: the AIC3104 as seen by `plughw`, which does the rate/format conversion.
pub const CAPTURE_DEVICE: &str = "plughw:aic3104,0";
/// Playback device: same codec, line output into the speaker amp.
pub const PLAYBACK_DEVICE: &str = "plughw:aic3104,0";

/// Record `seconds` of mono audio from `device`, as f32 in −1..=1.
///
/// Spawns `arecord` rather than talking to ALSA directly: the same choice `pet-detect` makes,
/// and it keeps the crate dependency-free.
pub fn record(device: &str, seconds: u32) -> Result<Vec<f32>> {
    let out = Command::new("arecord")
        .args([
            "-D",
            device,
            "-f",
            "S16_LE",
            "-r",
            &RATE.to_string(),
            "-c",
            "2", // right channel only; see the module docs
            "-t",
            "raw",
            "-q",
            "-d",
            &seconds.to_string(),
        ])
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("cannot run arecord on {device}"))?;

    if !out.status.success() {
        bail!(
            "arecord failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let pcm = stereo_to_mono_right(&out.stdout);
    if pcm.is_empty() {
        bail!("arecord returned no audio");
    }
    Ok(pcm)
}

/// Play mono f32 samples through `device`.
pub fn play(device: &str, samples: &[f32]) -> Result<()> {
    let mut child = Command::new("aplay")
        .args([
            "-D",
            device,
            "-f",
            "S16_LE",
            "-r",
            &RATE.to_string(),
            "-c",
            "1",
            "-t",
            "raw",
            "-q",
        ])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot run aplay on {device}"))?;
    {
        let stdin = child.stdin.as_mut().context("aplay has no stdin")?;
        let mut buf = Vec::with_capacity(samples.len() * 2);
        for s in samples {
            buf.extend_from_slice(&to_i16(*s).to_le_bytes());
        }
        // A write into a dead aplay must not take the caller down with it.
        if let Err(e) = stdin.write_all(&buf) {
            tracing::debug!(error = %e, "writing to aplay failed");
        }
    }
    let out = child.wait_with_output().context("waiting for aplay")?;
    if !out.status.success() {
        bail!(
            "aplay failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Write mono f32 samples as a 16-bit WAV that `aplay` can also read directly.
pub fn write_wav(path: &std::path::Path, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("cannot create {}", path.display()))?;
    for s in samples {
        writer.write_sample(to_i16(*s))?;
    }
    writer.finalize()?;
    Ok(())
}

/// Read a 16-bit WAV (mono, stereo downmixes by taking the right channel) into f32.
pub fn read_wav(path: &std::path::Path) -> Result<(Vec<f32>, u32)> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let spec = reader.spec();
    if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        bail!("{}: only 16-bit PCM WAV is supported", path.display());
    }
    let channels = spec.channels.max(1) as usize;
    let raw: Vec<i16> = reader
        .samples::<i16>()
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("cannot read samples from {}", path.display()))?;
    // Same right-channel rule as the capture path; a mono file keeps everything.
    let mono = if channels == 1 {
        raw.into_iter().map(|v| v as f32 / 32768.0).collect()
    } else {
        raw.chunks(channels)
            .map(|c| c[channels - 1] as f32 / 32768.0)
            .collect()
    };
    Ok((mono, spec.sample_rate))
}

/// Read S16_LE stereo interleaved PCM and keep the right channel.
fn stereo_to_mono_right(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| {
            let right = i16::from_le_bytes([b[2], b[3]]);
            right as f32 / 32768.0
        })
        .collect()
}

fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_channel_is_the_microphone() {
        // Left 0.5, right -0.25 → the microphone sample is -0.25.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&16384i16.to_le_bytes()); // left
        bytes.extend_from_slice(&(-8192i16).to_le_bytes()); // right
        let mono = stereo_to_mono_right(&bytes);
        assert_eq!(mono.len(), 1);
        assert!((mono[0] + 0.25).abs() < 1e-3);
    }

    #[test]
    fn wav_round_trips() {
        let dir = std::env::temp_dir();
        let path = dir.join("duck-voice-roundtrip.wav");
        let samples: Vec<f32> = (0..1000).map(|i| (i as f32 / 1000.0) - 0.5).collect();
        write_wav(&path, &samples).unwrap();
        let (back, rate) = read_wav(&path).unwrap();
        assert_eq!(rate, RATE);
        assert_eq!(back.len(), samples.len());
        assert!((back[500] - samples[500]).abs() < 1e-3);
        let _ = std::fs::remove_file(path);
    }
}
