//! The microphone as a stream, and the arithmetic between it and the recognizer.
//!
//! The codec is stereo but only the **right** channel carries the microphone — the same
//! fact `voice` states and its capture path obeys. The recognizer is fixed at 16 kHz; the
//! codec runs at 48 kHz, so every three input samples become one output sample. Dropping
//! two of every three would alias everything above 8 kHz into speech band; a short FIR
//! before the decimation costs nothing at this rate and removes that.

use std::io::{BufReader, Read};
use std::process::{Child, ChildStdout, Command, Stdio};

use anyhow::{Context, Result};
use voice::RATE;

/// How many mono samples one [`Microphone::read_block`] returns at 48 kHz.
///
/// 30 ms. The VAD works on 32 ms windows and buffers anything shorter, so this is the
/// granularity it can keep up with without being handed a chunk whose internal alignment
/// it has to re-derive; and one `read_exact` per 30 ms keeps the motion-refresh loop and
/// the syscall rate trivial.
pub const BLOCK_SAMPLES: usize = (RATE as usize * 30) / 1000;

/// The sample rate the model was trained at; `sherpa-onnx` will not resample for us.
pub const MODEL_RATE: u32 = 16_000;

/// A running `arecord`, read block by block.
///
/// Spawned rather than opened through ALSA, for the reason `voice::record` gives: the same
/// choice `pet-detect` makes, and it keeps this crate out of the audio stack. The
/// difference from `voice::record` is only that this one never exits on its own — `-d` is
/// absent on purpose, and the child lives until [`Drop`].
pub struct Microphone {
    child: Child,
    stdout: BufReader<ChildStdout>,
}

impl Microphone {
    pub fn open(device: &str) -> Result<Self> {
        let mut child = Command::new("arecord")
            .args([
                "-D",
                device,
                "-f",
                "S16_LE",
                "-r",
                &RATE.to_string(),
                "-c",
                "2", // both channels, right kept below; see voice::stereo_to_mono_right
                "-t",
                "raw",
                "-q",
            ])
            .stdout(Stdio::piped())
            // Inherited, not piped: this daemon stops reading while it decodes an utterance,
            // which makes ALSA report overruns, and a pipe nobody drains would eventually
            // block `arecord` on its own diagnostics. The journal is the right place for
            // them, and one overrun line per command is useful evidence of the stall.
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("cannot run arecord on {device}"))?;
        let stdout = child.stdout.take().context("arecord has no stdout")?;
        Ok(Self {
            child,
            stdout: BufReader::new(stdout),
        })
    }

    /// Read one block of mono samples at 48 kHz.
    ///
    /// Blocks are exactly [`BLOCK_SAMPLES`] long, so the caller sees the same amount of
    /// audio however the driver chooses to deliver it. A short read at end of stream is an
    /// error: `arecord` without `-d` should never end.
    pub fn read_block(&mut self) -> Result<Vec<f32>> {
        let mut raw = vec![0u8; BLOCK_SAMPLES * 4];
        self.stdout
            .read_exact(&mut raw)
            .context("arecord stopped producing audio")?;
        Ok(stereo_to_mono_right(&raw))
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        // SIGKILL, not a polite signal: `arecord` flushes and closes cleanly on SIGTERM,
        // which is a discard of the block we are about to stop needing anyway.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Keep the right channel, as `voice` does.
fn stereo_to_mono_right(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| i16::from_le_bytes([b[2], b[3]]) as f32 / 32768.0)
        .collect()
}

/// 48 kHz → 16 kHz, a windowed-sinc low-pass and then every third sample.
///
/// The filter length and cutoff are chosen for this exact ratio: 24 taps at 48 kHz is 0.5 ms
/// of history, and 7.2 kHz is comfortably below the 8 kHz the output can carry while
/// leaving the speech band untouched. The kernel is built once in [`Downsample3::new`].
pub struct Downsample3 {
    taps: Vec<f32>,
    history: Vec<f32>,
    phase: usize,
}

const TAPS: usize = 24;
const CUTOFF_HZ: f32 = 7_200.0;

impl Downsample3 {
    pub fn new() -> Self {
        let mut taps = vec![0.0f32; TAPS];
        let mid = (TAPS - 1) as f32 / 2.0;
        let fc = CUTOFF_HZ / RATE as f32; // cycles per input sample
        let mut sum = 0.0;
        for (n, tap) in taps.iter_mut().enumerate() {
            let x = n as f32 - mid;
            let sinc = if x == 0.0 {
                1.0
            } else {
                (std::f32::consts::PI * 2.0 * fc * x).sin() / (std::f32::consts::PI * 2.0 * fc * x)
            };
            // Hamming, which trades a wider transition for a much deeper stop band than a
            // rectangular window at this length.
            let window =
                0.54 - 0.46 * (2.0 * std::f32::consts::PI * n as f32 / (TAPS - 1) as f32).cos();
            *tap = 2.0 * fc * sinc * window;
            sum += *tap;
        }
        // Unity gain at DC, so the filter cannot change how loud the duck sounds to the model.
        for tap in &mut taps {
            *tap /= sum;
        }
        Self {
            taps,
            history: vec![0.0; TAPS],
            phase: 0,
        }
    }

    /// Downsample one input block, appending to `out`.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &sample in input {
            self.history.copy_within(0..TAPS - 1, 1);
            self.history[0] = sample;
            if self.phase == 0 {
                let mut acc = 0.0;
                for (tap, h) in self.taps.iter().zip(&self.history) {
                    acc += tap * h;
                }
                out.push(acc);
            }
            self.phase = (self.phase + 1) % 3;
        }
    }
}

impl Default for Downsample3 {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / RATE as f32).sin())
            .collect()
    }

    fn peak_after_downsample(input: &[f32]) -> f32 {
        let mut down = Downsample3::new();
        let mut out = Vec::new();
        down.push(input, &mut out);
        // Skip the filter's start-up transient.
        out.iter().skip(50).fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn speech_band_passes_at_the_right_rate() {
        let out = {
            let mut down = Downsample3::new();
            let mut out = Vec::new();
            down.push(&tone(1_000.0, 4_800), &mut out);
            out
        };
        // Three input samples in, one out.
        assert_eq!(out.len(), 1_600);
        let peak = out.iter().skip(50).fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 1.0).abs() < 0.05, "1 kHz peak {peak}");
    }

    #[test]
    fn above_nyquist_is_removed_not_folded() {
        // 10 kHz is above the 8 kHz the output can represent. Decimating without the low
        // pass would fold it to 6 kHz at full amplitude; with it, it is gone.
        let peak = peak_after_downsample(&tone(10_000.0, 4_800));
        assert!(peak < 0.05, "10 kHz leaked through at {peak}");
    }

    #[test]
    fn the_right_channel_is_the_microphone() {
        let mut raw = Vec::new();
        for _ in 0..3 {
            raw.extend_from_slice(&0i16.to_le_bytes()); // left: silent
            raw.extend_from_slice(&8192i16.to_le_bytes()); // right: 0.25
        }
        let mono = stereo_to_mono_right(&raw);
        assert_eq!(mono.len(), 3);
        assert!((mono[0] - 0.25).abs() < 1e-3);
    }
}
