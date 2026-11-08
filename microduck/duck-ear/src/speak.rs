//! The speaking half: fetch a WAV from the host's TTS service and play it here.
//!
//! The synthesizer does not run on the board. VITS on the A55 measured **RTF 8.8–13** for the
//! fanchen-C Chinese voice — a two-second sentence took nineteen seconds to make — while the
//! same model on the host is **RTF 0.4**. So the host synthesizes and this crate plays: one
//! `GET /say?text=…`, a WAV back, `aplay` through the codec. The audio is synthesized text,
//! not recorded speech; nothing the microphone heard is sent to the host.
//!
//! Playback is the daemon's business rather than `robotd`'s because `robot.sound` plays from
//! the prompt bank and cannot play a file. The caller must treat the microphone as deaf for
//! the duration — see the deaf window in `main`.

use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::net::Service;

/// Synthesis takes 1–3 s for a short sentence on the host; the network is one gigabit hop.
const TIMEOUT: Duration = Duration::from_secs(15);

/// A spoken answer is a sentence or two — a few hundred kilobytes. Anything in the megabytes
/// is the wrong endpoint answering, and is refused before it reaches the disk.
const MAX_BYTES: usize = 8 << 20;

pub struct Voice {
    service: Service,
    /// ALSA playback device, `plughw` so the 16 kHz the model produces is converted by the
    /// stack rather than here.
    device: String,
}

impl Voice {
    pub fn new(addr: &str, device: String) -> Result<Self> {
        Ok(Self {
            service: Service::parse(addr).context("the TTS service's address")?,
            device,
        })
    }

    /// Speak `text` and return when the sound has finished.
    ///
    /// Blocking on purpose: the caller turns the microphone off around this call, because the
    /// duck hearing its own reply is the one way a conversation would feed itself.
    pub fn say(&self, text: &str) -> Result<()> {
        let path = format!("/say?text={}", percent_encode(text));
        let wav = self.service.get(&path, TIMEOUT).context("fetching the reply")?;
        self.play(&wav, "reply")
    }

    /// Play a WAV that ships with the binary, with no network involved.
    ///
    /// One sentence needs this: the one that reports the host is gone. Its whole point is
    /// that the host is what is missing, so it cannot be synthesized when it is needed — it
    /// is baked in (`HOST_OFFLINE` in `main`) and played through the same `aplay` path.
    pub fn play(&self, wav: &[u8], name: &str) -> Result<()> {
        if wav.len() < 44 || &wav[..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
            bail!("the audio is not a WAV ({} bytes)", wav.len());
        }
        if wav.len() > MAX_BYTES {
            bail!("the audio is {} bytes, too long to be a sentence", wav.len());
        }

        // A file, not a pipe: `aplay` reads the header and its format from the file, and a
        // failed playback leaves the artifact on disk to listen to.
        let path = std::env::temp_dir().join(format!("duck-ear-{name}.wav"));
        std::fs::write(&path, wav)
            .with_context(|| format!("cannot write {}", path.display()))?;

        let out = Command::new("aplay")
            .args(["-D", &self.device, "-q"])
            .arg(&path)
            .output()
            .with_context(|| format!("cannot run aplay on {}", self.device))?;
        if !out.status.success() {
            bail!(
                "aplay failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }
}

/// Percent-encode for a query string. Chinese is three UTF-8 bytes per character and the
/// request line is ASCII, so everything outside the unreserved set is escaped.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_is_escaped_per_byte() {
        assert_eq!(percent_encode("你好"), "%E4%BD%A0%E5%A5%BD");
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("1.5"), "1.5");
    }
}
