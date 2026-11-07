//! `duck-ear` — voice control for the duck.
//!
//! Microphone → Silero VAD → one utterance at a time into offline SenseVoice → a small
//! command vocabulary → the same JSON-RPC intents the gamepad sends. Everything is on the
//! board: the audio never leaves it.
//!
//! ## What this is not
//!
//! It does not enable the robot. Starting the policy stays the gamepad's job (see
//! `intent`'s module docs for why), and it does not play replies — robotd already owns the
//! speaker and the prompt bank, so acknowledgements go through `robot.sound` like any other
//! client's.
//!
//! ## The echo problem, stated
//!
//! The speaker and the microphone share a board and there is no echo cancellation. The duck
//! hears its own prompts and its own quack. Two things keep that from turning into motion:
//! the vocabulary contains none of the prompt sentences (a test asserts it), and a sound
//! this daemon asks for is followed by a short deaf window. A prompt `robotd` decides to
//! play on its own — a battery warning, a fault — is not covered by that window; that is a
//! known gap, not an oversight, and the vocabulary is the thing doing the real work.
//!
//! ## Why segmentation
//!
//! The recognizer is offline, so the daemon pays for ASR only when somebody speaks: the VAD
//! runs continuously at a few percent of a core and a command costs about half a second of
//! two cores. The price is latency — the detector waits 0.5 s of silence before it releases
//! an utterance, and the decode adds 0.4–0.8 s — so a command lands about a second after
//! the speaker stops.
//!
//! ```text
//! duck-ear --dry-run            # print transcripts, send nothing
//! duck-ear                      # listen and act
//! ```

mod asr;
mod audio;
mod client;
mod intent;
mod vad;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use duck_ipc_proto::{self as proto, Call, SoundTag};
use tracing_subscriber::EnvFilter;

use crate::asr::Recognizer;
use crate::audio::{Downsample3, Microphone};
use crate::client::Robot;
use crate::intent::Command;
use crate::vad::Segmenter;

/// How long to stay deaf after asking robotd to play a sound.
///
/// The quack is under a second; this covers it and the microphone's own buffering with
/// margin. Silence is not fed to the segmenter during the window rather than fed as
/// zeroes — either ends the current utterance, and dropping the audio keeps the block the
/// quack arrives in out of the next utterance's features.
const DEAF_AFTER_SOUND: Duration = Duration::from_millis(900);

/// How often a `forward` command is refreshed. The deadman is 500 ms; 100 ms leaves room
/// for a decode to run long without the robot stuttering to a halt mid-step.
const MOVE_REFRESH: Duration = Duration::from_millis(100);

#[derive(Parser)]
#[command(about = "Hear Chinese commands and send the matching robot intents")]
struct Args {
    /// robotd's socket.
    #[arg(long, default_value = "/run/robotd.sock")]
    socket: PathBuf,

    /// Directory holding the SenseVoice model (model.int8.onnx and tokens.txt).
    #[arg(long, default_value = "/var/lib/robot/asr/sensevoice")]
    model: PathBuf,

    /// Silero VAD model. Ships next to the recognizer.
    #[arg(long, default_value = "/var/lib/robot/asr/silero_vad.onnx")]
    vad: PathBuf,

    /// ALSA capture device. `plughw` does the format conversion; the codec is stereo with
    /// signal on the right channel only, which `audio` keeps.
    #[arg(long, default_value = voice::CAPTURE_DEVICE)]
    device: String,

    /// ONNX Runtime intra-op threads. Two is `asr::Recognizer::new`'s measurement.
    #[arg(long, default_value_t = 2)]
    threads: i32,

    /// Transcribe and classify, but never open the robotd socket.
    #[arg(long)]
    dry_run: bool,
}

/// Exit status for "this board is not configured for the ear": no model, no microphone.
///
/// `duck-ear.service` has `RestartPreventExitStatus=78`, so a missing 240 MB artifact does
/// not become a restart every five seconds in the journal. A microphone that disappears
/// *later* is a different failure and returns 1, which does restart.
const EXIT_CONFIG: u8 = 78;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let mut ear = match Ear::new(&args) {
        Ok(ear) => ear,
        Err(error) => {
            tracing::error!("{error:#}");
            return ExitCode::from(EXIT_CONFIG);
        }
    };
    tracing::info!("listening — say 停下, 坐下, 前进 or 叫一声");
    match ear.listen(args.dry_run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

struct Ear {
    mic: Microphone,
    segmenter: Segmenter,
    recognizer: Recognizer,
    down: Downsample3,
    pcm: Vec<f32>,
    /// Utterances returned by the segmenter this block, reused across blocks.
    segments: Vec<Vec<f32>>,
    robot: Option<Robot>,
    /// Set while a bounded forward command is running.
    forward: Option<Instant>,
    /// When the last movement refresh went out, so the loop sends at [`MOVE_REFRESH`] rather
    /// than once per 30 ms audio block.
    last_move: Instant,
    /// No audio is fed to the segmenter before this instant — see [`DEAF_AFTER_SOUND`].
    deaf_until: Instant,
}

impl Ear {
    fn new(args: &Args) -> Result<Self> {
        tracing::info!(
            model = %args.model.display(),
            vad = %args.vad.display(),
            device = %args.device,
            "loading the ASR model and the VAD"
        );
        // Models before microphone: a missing 240 MB artifact is a configuration error worth
        // the exit status 78 that stops systemd restarting, and it should be reported as
        // such even while something else has the capture device open.
        let segmenter = Segmenter::new(&args.vad)?;
        let recognizer = Recognizer::new(&args.model, args.threads)?;
        let mic = Microphone::open(&args.device)?;
        let robot = (!args.dry_run).then(|| {
            // Not connected here: the first command connects, and a robotd that is down at
            // startup must not keep the listener from starting — it would come up deaf
            // after every update.
            tracing::info!(socket = %args.socket.display(), "commands go to robotd at");
            Robot::new(args.socket.clone())
        });
        Ok(Self {
            mic,
            segmenter,
            recognizer,
            down: Downsample3::new(),
            pcm: Vec::with_capacity(audio::BLOCK_SAMPLES / 3 + 1),
            segments: Vec::new(),
            robot,
            forward: None,
            last_move: Instant::now(),
            deaf_until: Instant::now(),
        })
    }

    fn listen(&mut self, dry_run: bool) -> Result<()> {
        if dry_run {
            tracing::warn!("dry run: transcripts are logged, nothing is sent to robotd");
        }
        loop {
            let block = self.mic.read_block()?;

            if Instant::now() >= self.deaf_until {
                self.pcm.clear();
                self.down.push(&block, &mut self.pcm);
                self.segmenter.push(&self.pcm, &mut self.segments);
                for segment in std::mem::take(&mut self.segments) {
                    self.utterance(&segment, dry_run);
                }
            }

            self.refresh_forward(dry_run);
        }
    }

    /// One finished utterance: transcribe it, then act on what it said.
    fn utterance(&mut self, samples: &[f32], dry_run: bool) {
        let seconds = samples.len() as f32 / audio::MODEL_RATE as f32;
        let started = Instant::now();
        let text = self.recognizer.transcribe(samples);
        let decode = started.elapsed();
        tracing::debug!(
            seconds = format!("{seconds:.1}"),
            decode_ms = decode.as_millis(),
            "segment decoded"
        );
        if text.trim().is_empty() {
            return;
        }
        self.handle(&text, dry_run);
    }

    fn handle(&mut self, text: &str, dry_run: bool) {
        let Some(command) = intent::parse(text) else {
            tracing::info!(heard = %text, "not a command");
            return;
        };
        tracing::info!(heard = %text, ?command, "command");
        if dry_run {
            return;
        }
        if let Err(error) = self.dispatch(command) {
            tracing::warn!(?command, %error, "command was not carried out");
            self.forward = None;
        }
    }

    fn dispatch(&mut self, command: Command) -> Result<()> {
        let robot = self.robot.as_mut().context("dry run")?;
        match command {
            Command::Stop => {
                self.forward = None;
                let response = robot.call(&Call::RobotStop)?;
                client::accepted(&response)?;
                self.chirp();
            }
            Command::SitToggle => {
                let response = robot.call(&Call::RobotDo(proto::DoParams {
                    skill: "sit_toggle".to_owned(),
                }))?;
                client::accepted(&response)?;
                self.chirp();
            }
            Command::Quack => {
                self.chirp();
            }
            Command::Forward => {
                self.forward = Some(Instant::now() + Duration::from_secs_f64(intent::FORWARD_SECONDS));
                self.send_forward();
            }
        }
        Ok(())
    }

    fn send_forward(&mut self) {
        if let Some(robot) = self.robot.as_mut() {
            robot.notify(&Call::RobotMove(proto::MoveParams {
                vx: intent::FORWARD_SPEED,
                vy: 0.0,
                vyaw: 0.0,
            }));
        }
    }

    /// Keep a forward command alive until its deadline, then stop it ourselves. The deadman
    /// would stop it anyway half a second later; stopping deliberately means the duck does
    /// not coast for that half second, and means a lost loop ends in a stop, not a walk.
    fn refresh_forward(&mut self, dry_run: bool) {
        let Some(until) = self.forward else {
            return;
        };
        if Instant::now() >= until {
            self.forward = None;
            if !dry_run && let Some(robot) = self.robot.as_mut() {
                let _ = robot.call(&Call::RobotStop);
            }
            return;
        }
        if self.last_move.elapsed() >= MOVE_REFRESH {
            self.last_move = Instant::now();
            self.send_forward();
        }
    }

    /// The acknowledgement quack, and the deaf window that goes with it.
    fn chirp(&mut self) {
        self.deaf_until = Instant::now() + DEAF_AFTER_SOUND;
        // Whatever the detector has buffered is the tail of the command, or the start of the
        // quack — either way not an utterance to decode.
        self.segmenter.reset();
        if let Some(robot) = self.robot.as_mut() {
            robot.notify(&Call::RobotSound(proto::SoundParams {
                tag: SoundTag::Chirp,
                hold: None,
            }));
        }
    }
}
