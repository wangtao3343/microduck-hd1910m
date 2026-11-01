//! `duck-voice`: record a voice memo on the onboard microphone, clean it, play it back.
//!
//! ```text
//! duck-voice say                      # beep, record 6 s, clean, play
//! duck-voice record --seconds 10 -o memo.wav
//! duck-voice play memo.wav
//! ```
//!
//! The beep is not decoration: a recording window that starts without a cue is a recording
//! of whatever the person was doing while reading the instructions.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "duck-voice",
    about = "Record a voice memo on the onboard microphone and play it back",
    version
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Record, clean, and save to a WAV file.
    Record {
        /// How long to record.
        #[arg(short, long, default_value_t = 8)]
        seconds: u32,
        /// Where to write the (cleaned) recording.
        #[arg(short, long, default_value = "memo.wav")]
        out: PathBuf,
        /// Capture device, e.g. `plughw:aic3104,0`.
        #[arg(long, default_value = voice::CAPTURE_DEVICE)]
        device: String,
        /// Save the raw capture, without the cleanup chain.
        #[arg(long)]
        raw: bool,
        /// Skip the beep that says "recording starts now".
        #[arg(long)]
        no_beep: bool,
    },
    /// Play a WAV file through the speaker.
    Play {
        file: PathBuf,
        #[arg(long, default_value = voice::PLAYBACK_DEVICE)]
        device: String,
    },
    /// Record and immediately play the result back — the "did I sound like that?" loop.
    Say {
        #[arg(short, long, default_value_t = 6)]
        seconds: u32,
        #[arg(long, default_value = voice::CAPTURE_DEVICE)]
        capture: String,
        #[arg(long, default_value = voice::PLAYBACK_DEVICE)]
        playback: String,
        /// Keep the recording here instead of a temporary file.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Play the raw capture, without the cleanup chain.
        #[arg(long)]
        raw: bool,
    },
}

fn main() -> Result<()> {
    match Args::parse().command {
        Command::Record {
            seconds,
            out,
            device,
            raw,
            no_beep,
        } => {
            if !no_beep {
                beep(&voice::PLAYBACK_DEVICE)?;
            }
            let samples = capture(&device, seconds, raw)?;
            voice::write_wav(&out, &samples)?;
            // The path on stdout, everything else on stderr: `duck-voice record` should be
            // usable as `wav=$(duck-voice record -o memo.wav)`.
            eprintln!("recorded {seconds} s to {}", out.display());
            println!("{}", out.display());
        }
        Command::Play { file, device } => {
            let (samples, _rate) = voice::read_wav(&file)?;
            voice::play(&device, &samples)?;
        }
        Command::Say {
            seconds,
            capture: capture_device,
            playback,
            out,
            raw,
        } => {
            beep(&playback)?;
            let samples = capture(&capture_device, seconds, raw)?;
            if let Some(path) = out {
                voice::write_wav(&path, &samples)?;
                eprintln!("kept the recording at {}", path.display());
            }
            voice::play(&playback, &samples)?;
        }
    }
    Ok(())
}

fn capture(device: &str, seconds: u32, raw: bool) -> Result<Vec<f32>> {
    let samples = voice::record(device, seconds)?;
    Ok(if raw { samples } else { voice::dsp::clean(&samples, voice::RATE) })
}

/// 150 ms of 1200 Hz — the same cue the bring-up used, small enough not to startle.
fn beep(device: &str) -> Result<()> {
    let n = (voice::RATE as f32 * 0.15) as usize;
    let tone: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / voice::RATE as f32;
            0.25 * (2.0 * std::f32::consts::PI * 1200.0 * t).sin()
        })
        .collect();
    voice::play(device, &tone)
}
