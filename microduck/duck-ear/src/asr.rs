//! The streaming recognizer, through sherpa-onnx's C API.
//!
//! `sherpa-rs` wraps the offline recognizer and the keyword spotter but not the *online*
//! (streaming) one, which is the one this daemon is built on: it accepts audio in chunks and
//! emits a partial transcript that grows while the speaker is still talking, then flags an
//! endpoint after trailing silence. The sys crate exposes the whole C API, so the wrapper
//! here is thin by design: create, feed, decode, read, reset, free.
//!
//! Endpointing is deliberately the recognizer's rather than a separate VAD model. The
//! model's rules (2.4 s of silence with nothing decoded; 1.2 s after the last word; 30 s
//! maximum utterance) are exactly the three cases a push-to-talk loop must end on, and they
//! cost nothing beyond the config fields — a Silero VAD would be a second model, a second
//! `ort` session and a second load at startup for the same decision.

use std::ffi::{CStr, CString};
use std::path::Path;

use anyhow::{Context, Result, bail};
use sherpa_rs::sherpa_rs_sys as sys;

use crate::audio::MODEL_RATE;
use crate::intent;

/// Per-token bonus for a vocabulary phrase, in log-probability units.
///
/// Three is deliberately strong: at one the beam still preferred 叫医生 for 叫一声 because
/// its language model likes a real word. The cost of being wrong in this direction is a
/// command heard in speech that did not mean it — acceptable for a vocabulary this small
/// and this unambiguous, and bounded by the deaf window for anything the duck says itself.
const HOTWORD_SCORE: f32 = 3.0;

/// The files a streaming zipformer is made of. Fixed names because the model directory is
/// a release artifact with one layout, not a place to guess from: a renamed encoder should
/// be a startup error that says which file is missing, not a recognizer that silently
/// loads something else.
const ENCODER: &str = "encoder-epoch-99-avg-1.int8.onnx";
const DECODER: &str = "decoder-epoch-99-avg-1.onnx";
const JOINER: &str = "joiner-epoch-99-avg-1.int8.onnx";
const TOKENS: &str = "tokens.txt";

pub struct Recognizer {
    recognizer: *const sys::SherpaOnnxOnlineRecognizer,
    stream: *const sys::SherpaOnnxOnlineStream,
}

impl Recognizer {
    /// Load the model in `dir` and open one stream.
    ///
    /// One thread, not two, and that is a measured choice: at two threads the wall clock
    /// improved by about 15% while CPU time rose 2.5× — the model is small enough that
    /// ONNX Runtime's intra-op parallelism is mostly spin. The rest of the daemon (and the
    /// 50 Hz control loop on the same SoC) is worth more than the milliseconds.
    pub fn new(dir: &Path, threads: i32) -> Result<Self> {
        let path = |name: &str| -> Result<CString> {
            let full = dir.join(name);
            if !full.exists() {
                bail!(
                    "ASR model file {} is missing — the model directory should contain \
                     {ENCODER}, {DECODER}, {JOINER} and {TOKENS}",
                    full.display()
                );
            }
            cstring(&full.to_string_lossy())
        };

        let encoder = path(ENCODER)?;
        let decoder = path(DECODER)?;
        let joiner = path(JOINER)?;
        let tokens = path(TOKENS)?;
        // Beam search, not greedy: hotword scores only exist in the beam search, and the
        // hotwords are what keep 叫一声 from arriving as 叫医生 (measured — see
        // `intent::hotwords`). The model is small, so a beam of four is cheap.
        let decoding = CString::new("modified_beam_search")?;
        // Required whenever hotwords are given: sherpa-onnx splits a hotword into tokens by
        // this name and exits the process for an unknown one. This model's table is one
        // Chinese character per token.
        let modeling_unit = CString::new("cjkchar")?;
        let hotwords = CString::new(intent::hotwords())?;

        // Every pointer field starts null, and the C API's `SHERPA_ONNX_OR` treats null as
        // "use the default" — this is the one place where a zeroed C struct is the intended
        // construction rather than a shortcut.
        let mut config: sys::SherpaOnnxOnlineRecognizerConfig = unsafe { std::mem::zeroed() };
        config.feat_config.sample_rate = MODEL_RATE as i32;
        config.feat_config.feature_dim = 80;
        config.model_config.transducer.encoder = encoder.as_ptr();
        config.model_config.transducer.decoder = decoder.as_ptr();
        config.model_config.transducer.joiner = joiner.as_ptr();
        config.model_config.tokens = tokens.as_ptr();
        config.model_config.num_threads = threads;
        config.model_config.modeling_unit = modeling_unit.as_ptr();
        config.decoding_method = decoding.as_ptr();
        config.max_active_paths = 4;
        config.hotwords_buf = hotwords.as_ptr();
        config.hotwords_buf_size = hotwords.as_bytes().len() as i32;
        config.hotwords_score = HOTWORD_SCORE;
        config.enable_endpoint = 1;
        config.rule1_min_trailing_silence = 2.4;
        config.rule2_min_trailing_silence = 1.2;
        config.rule3_min_utterance_length = 30.0;

        let recognizer = unsafe { sys::SherpaOnnxCreateOnlineRecognizer(&config) };
        if recognizer.is_null() {
            bail!("sherpa-onnx could not create the recognizer from {}", dir.display());
        }
        let stream = unsafe { sys::SherpaOnnxCreateOnlineStream(recognizer) };
        if stream.is_null() {
            unsafe { sys::SherpaOnnxDestroyOnlineRecognizer(recognizer) };
            bail!("sherpa-onnx could not open a stream");
        }
        Ok(Self { recognizer, stream })
    }

    /// Feed one block of 16 kHz mono samples.
    pub fn accept(&mut self, samples: &[f32]) {
        unsafe {
            sys::SherpaOnnxOnlineStreamAcceptWaveform(
                self.stream,
                MODEL_RATE as i32,
                samples.as_ptr(),
                samples.len() as i32,
            );
        }
    }

    /// Run whatever the stream has ready. Returns how many chunks were decoded, which is
    /// what tells the loop whether it is keeping up.
    pub fn run(&mut self) -> usize {
        let mut decoded = 0;
        unsafe {
            while sys::SherpaOnnxIsOnlineStreamReady(self.recognizer, self.stream) == 1 {
                sys::SherpaOnnxDecodeOnlineStream(self.recognizer, self.stream);
                decoded += 1;
            }
        }
        decoded
    }

    /// The current hypothesis. Valid mid-utterance, which is what makes the partial lines in
    /// the journal useful for tuning the vocabulary.
    pub fn text(&self) -> String {
        unsafe {
            let result = sys::SherpaOnnxGetOnlineStreamResult(self.recognizer, self.stream);
            if result.is_null() {
                return String::new();
            }
            let text = if (*result).text.is_null() {
                String::new()
            } else {
                CStr::from_ptr((*result).text)
                    .to_string_lossy()
                    .into_owned()
            };
            sys::SherpaOnnxDestroyOnlineRecognizerResult(result);
            text
        }
    }

    /// Has the recognizer decided the speaker stopped?
    pub fn is_endpoint(&self) -> bool {
        unsafe { sys::SherpaOnnxOnlineStreamIsEndpoint(self.recognizer, self.stream) == 1 }
    }

    /// Start a fresh utterance on the same stream.
    pub fn reset(&mut self) {
        unsafe { sys::SherpaOnnxOnlineStreamReset(self.recognizer, self.stream) };
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) {
        unsafe {
            sys::SherpaOnnxDestroyOnlineStream(self.stream);
            sys::SherpaOnnxDestroyOnlineRecognizer(self.recognizer);
        }
    }
}

fn cstring(value: &str) -> Result<CString> {
    CString::new(value).with_context(|| format!("path {value:?} contains a NUL byte"))
}
