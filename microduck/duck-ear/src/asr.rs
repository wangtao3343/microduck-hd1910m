//! SenseVoice-small, through sherpa-onnx's offline C API.
//!
//! Offline, not streaming, and that is the second architecture decision this crate has made.
//! The streaming zipformer (29 MB, `zh-14m`) was cheap — RTF 0.21 continuous — but it cost a
//! third of a core all the time and still transcribed 叫一声 as 叫医生. Measured against the
//! board's own prompt bank:
//!
//! | model | CER | decode RTF (2 threads) | RSS |
//! |---|---|---|---|
//! | streaming zipformer 14M | 12.5 % | 0.21 while always on | 108 MB |
//! | SenseVoice-small int8 | 7.8 % | 0.42 per utterance | ~366 MB |
//! | paraformer-zh int8 | 7.8 % | 0.46 per utterance | ~400 MB |
//!
//! SenseVoice wins on latency per utterance and carries punctuation; paraformer was a
//! tie on accuracy. Neither streams, so the segmenter in `vad` decides when to run them —
//! which also means the SoC is idle between commands instead of continuously decoding.
//!
//! Two things this model cannot do that the streaming one could: give a partial transcript
//! while the person is still talking, and take hotwords (`intent::hotwords` biases a beam
//! search; SenseVoice is a CTC-style model with greedy decoding). The accuracy is the
//! trade for both.

use std::ffi::{CStr, CString};
use std::path::Path;

use anyhow::{Context, Result, bail};
use sherpa_rs::sherpa_rs_sys as sys;

use crate::audio::MODEL_RATE;

/// The files a SenseVoice release artifact is made of. Fixed names for the same reason the
/// streaming layout used fixed names: a renamed file should be a startup error naming it,
/// not a recognizer that loads something else.
const MODEL: &str = "model.int8.onnx";
const TOKENS: &str = "tokens.txt";

pub struct Recognizer {
    recognizer: *const sys::SherpaOnnxOfflineRecognizer,
}

impl Recognizer {
    /// Load the model in `dir`.
    ///
    /// Two intra-op threads by default: measured at 0.42 RTF against 0.63 for one, and the
    /// decode is a burst of half a second rather than the streaming model's steady tax, so
    /// it only competes with `robotd` while somebody is talking.
    pub fn new(dir: &Path, threads: i32) -> Result<Self> {
        let path = |name: &str| -> Result<CString> {
            let full = dir.join(name);
            if !full.exists() {
                bail!(
                    "ASR model file {} is missing — the model directory should contain \
                     {MODEL} and {TOKENS}",
                    full.display()
                );
            }
            cstring(&full.to_string_lossy())
        };

        let model = path(MODEL)?;
        let tokens = path(TOKENS)?;
        // SenseVoice is bilingual per the model name, but the duck hears Chinese; stating
        // the language keeps the tag block out of the way and is what the accuracy numbers
        // above were measured with.
        let language = CString::new("zh")?;
        // Greedy search: the model has no language model to re-rank through, and beam
        // search on it is not supported by this build.
        let decoding = CString::new("greedy_search")?;

        // Zeroed is the intended construction: every field the C API does not see set falls
        // back to its default, and there is no `Default` for a C struct.
        let mut config: sys::SherpaOnnxOfflineRecognizerConfig = unsafe { std::mem::zeroed() };
        config.feat_config.sample_rate = MODEL_RATE as i32;
        config.feat_config.feature_dim = 80;
        config.model_config.tokens = tokens.as_ptr();
        config.model_config.num_threads = threads;
        config.model_config.sense_voice.model = model.as_ptr();
        config.model_config.sense_voice.language = language.as_ptr();
        // Inverse text normalisation: numbers and punctuation come out as words and marks.
        config.model_config.sense_voice.use_itn = 1;
        config.decoding_method = decoding.as_ptr();

        let recognizer = unsafe { sys::SherpaOnnxCreateOfflineRecognizer(&config) };
        if recognizer.is_null() {
            bail!("sherpa-onnx could not create the recognizer from {}", dir.display());
        }
        Ok(Self { recognizer })
    }

    /// Transcribe one utterance of 16 kHz mono samples.
    ///
    /// A fresh stream per call because the C API allows exactly one waveform per offline
    /// stream; the stream is cheap next to the decode it drives.
    pub fn transcribe(&self, samples: &[f32]) -> String {
        unsafe {
            let stream = sys::SherpaOnnxCreateOfflineStream(self.recognizer);
            if stream.is_null() {
                return String::new();
            }
            sys::SherpaOnnxAcceptWaveformOffline(
                stream,
                MODEL_RATE as i32,
                samples.as_ptr(),
                samples.len() as i32,
            );
            sys::SherpaOnnxDecodeOfflineStream(self.recognizer, stream);
            let result = sys::SherpaOnnxGetOfflineStreamResult(stream);
            let text = if result.is_null() || (*result).text.is_null() {
                String::new()
            } else {
                CStr::from_ptr((*result).text)
                    .to_string_lossy()
                    .into_owned()
            };
            if !result.is_null() {
                sys::SherpaOnnxDestroyOfflineRecognizerResult(result);
            }
            sys::SherpaOnnxDestroyOfflineStream(stream);
            text
        }
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) {
        unsafe { sys::SherpaOnnxDestroyOfflineRecognizer(self.recognizer) };
    }
}

fn cstring(value: &str) -> Result<CString> {
    CString::new(value).with_context(|| format!("path {value:?} contains a NUL byte"))
}
