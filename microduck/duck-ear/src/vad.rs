//! Silero VAD, through sherpa-onnx's C API.
//!
//! The recognizer is offline now — it takes one whole utterance, not a stream — so
//! something has to decide where an utterance begins and ends. An energy gate is not enough
//! on this robot: the loudest thing in the room is often a motor, and motor noise is
//! broadband, so it looks like speech to any threshold that also hears a person a metre
//! away. Silero is a small ONNX model that classifies 32 ms frames as speech or not, and it
//! costs about nothing while the room is quiet.
//!
//! A segment is handed over only after the speaker has stopped: the detector waits
//! [`MIN_SILENCE`] of silence after the last speech frame before it releases the segment.
//! That wait is the delay between finishing a word and the duck moving — measured end to
//! end on this board at about 1 s for a one-second command (0.5 s of silence plus 0.5 s of
//! decoding on two threads).

use std::path::Path;

use anyhow::{Context, Result, bail};
use sherpa_rs::sherpa_rs_sys as sys;

use crate::audio::MODEL_RATE;

/// Probability above which a frame counts as speech. The model's own default, and the value
/// that segmented the prompt bank correctly on this board.
const THRESHOLD: f32 = 0.5;

/// Silence after the last speech frame before the segment is released. Half a second is
/// long enough not to cut a sentence at a comma and short enough not to feel like lag.
const MIN_SILENCE: f32 = 0.5;

/// Anything shorter than this is a door slam, not a command.
const MIN_SPEECH: f32 = 0.25;

/// After this much continuous speech the detector raises its own threshold and forces a
/// cut, so a television in the room cannot hold one segment open forever.
const MAX_SPEECH: f32 = 10.0;

/// Frames are 512 samples of 16 kHz audio for Silero v4 and v5 alike; sherpa-onnx exits the
/// process if the value is wrong, so it is stated here rather than left to a zeroed struct.
const WINDOW: i32 = 512;

/// Longest stretch of audio the detector keeps for segmentation.
const BUFFER_SECONDS: f32 = 30.0;

/// One running detector, fed 30 ms blocks and drained for finished utterances.
pub struct Segmenter {
    vad: *const sys::SherpaOnnxVoiceActivityDetector,
}

impl Segmenter {
    pub fn new(model: &Path) -> Result<Self> {
        if !model.exists() {
            bail!(
                "VAD model {} is missing — silero_vad.onnx ships with the model directory",
                model.display()
            );
        }
        let path = std::ffi::CString::new(model.to_string_lossy().as_bytes())
            .with_context(|| format!("path {:?} contains a NUL byte", model.display()))?;

        // Zeroed then filled: the C API reads 0 as "use the default" for every field we
        // leave alone (provider, debug), which is what we want here.
        let mut config: sys::SherpaOnnxVadModelConfig = unsafe { std::mem::zeroed() };
        config.silero_vad.model = path.as_ptr();
        config.silero_vad.threshold = THRESHOLD;
        config.silero_vad.min_silence_duration = MIN_SILENCE;
        config.silero_vad.min_speech_duration = MIN_SPEECH;
        config.silero_vad.max_speech_duration = MAX_SPEECH;
        config.silero_vad.window_size = WINDOW;
        config.sample_rate = MODEL_RATE as i32;
        // The detector is one small model on 32 ms frames; a second thread would only
        // contend with the recognizer that runs when the segment is done.
        config.num_threads = 1;

        let vad = unsafe { sys::SherpaOnnxCreateVoiceActivityDetector(&config, BUFFER_SECONDS) };
        if vad.is_null() {
            bail!("sherpa-onnx could not create the VAD from {}", model.display());
        }
        Ok(Self { vad })
    }

    /// Feed one block and append every utterance that finished to `out`.
    ///
    /// Blocks, not whole recordings: this build silently truncates an oversized
    /// `AcceptWaveform` to a few hundred milliseconds (measured with the Python binding),
    /// so the caller must keep feeding the daemon's 30 ms pieces.
    pub fn push(&mut self, samples: &[f32], out: &mut Vec<Vec<f32>>) {
        unsafe {
            sys::SherpaOnnxVoiceActivityDetectorAcceptWaveform(
                self.vad,
                samples.as_ptr(),
                samples.len() as i32,
            );
            while sys::SherpaOnnxVoiceActivityDetectorEmpty(self.vad) == 0 {
                let front = sys::SherpaOnnxVoiceActivityDetectorFront(self.vad);
                if front.is_null() {
                    break;
                }
                // `Front` hands over an owned copy; the detector's own buffer is popped
                // separately and must not be read after `Pop`.
                let segment =
                    std::slice::from_raw_parts((*front).samples, (*front).n as usize).to_vec();
                sys::SherpaOnnxDestroySpeechSegment(front);
                sys::SherpaOnnxVoiceActivityDetectorPop(self.vad);
                out.push(segment);
            }
        }
    }

    /// Drop whatever is buffered — used when the duck has just made a noise of its own and
    /// what the microphone is hearing is not the person.
    pub fn reset(&mut self) {
        unsafe { sys::SherpaOnnxVoiceActivityDetectorReset(self.vad) };
    }
}

impl Drop for Segmenter {
    fn drop(&mut self) {
        unsafe { sys::SherpaOnnxDestroyVoiceActivityDetector(self.vad) };
    }
}
