//! `duck-ear` — voice control for the duck.
//!
//! Microphone → streaming Chinese ASR → a small command vocabulary → the same JSON-RPC
//! intents the gamepad sends. Everything is on the board: the audio never leaves it.
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
//! ```text
//! duck-ear --dry-run            # print transcripts, send nothing
//! duck-ear                      # listen and act
//! ```

mod asr;
mod audio;
mod client;
mod intent;

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

/// How long to stay deaf after asking robotd to play a sound.
///
/// The quack is under a second; this covers it and the microphone's own buffering with
/// margin. Silence is not fed to the recognizer during the window rather than fed as
/// zeroes — either ends the current utterance, and dropping the audio keeps the block the
/// quack arrives in out of the next utterance's features.
const DEAF_AFTER_SOUND: Duration = Duration::from_millis(900);

/// How often a `forward` command is refreshed. The deadman is 500 ms; 100 ms leaves room
/// for a decode chunk to run long without the robot stuttering to a halt mid-step.
const MOVE_REFRESH: Duration = Duration::from_millis(100);

/// How often the partial transcript is written to the journal, and only when it changed.
const PARTIAL_LOG: Duration = Duration::from_millis(750);

#[derive(Parser)]
#[command(about = "Hear Chinese commands and send the matching robot intents")]
struct Args {
    /// robotd's socket.
    #[arg(long, default_value = "/run/robotd.sock")]
    socket: PathBuf,

    /// Directory holding the streaming zipformer model (encoder/decoder/joiner/tokens).
    #[arg(long, default_value = "/var/lib/robot/asr/zh-14m")]
    model: PathBuf,

    /// ALSA capture device. `plughw` does the format conversion; the codec is stereo with
    /// signal on the right channel only, which `audio` keeps.
    #[arg(long, default_value = voice::CAPTURE_DEVICE)]
    device: String,

    /// ONNX Runtime intra-op threads. One, for the reason `asr::Recognizer::new` measures.
    #[arg(long, default_value_t = 1)]
    threads: i32,

    /// Transcribe and classify, but never open the robotd socket.
    #[arg(long)]
    dry_run: bool,
}

/// Exit status for "this board is not configured for the ear": no model, no microphone.
///
/// `duck-ear.service` has `RestartPreventExitStatus=78`, so a missing 29 MB artifact does
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
    recognizer: Recognizer,
    down: Downsample3,
    pcm: Vec<f32>,
    robot: Option<Robot>,
    /// Set while a bounded forward command is running.
    forward: Option<Instant>,
    /// When the last movement refresh went out, so the loop sends at [`MOVE_REFRESH`] rather
    /// than once per 30 ms audio block.
    last_move: Instant,
    /// No audio is fed to the recognizer before this instant — see [`DEAF_AFTER_SOUND`].
    deaf_until: Instant,
    last_partial: Instant,
    last_partial_text: String,
}

impl Ear {
    fn new(args: &Args) -> Result<Self> {
        tracing::info!(
            model = %args.model.display(),
            device = %args.device,
            "loading the streaming ASR model"
        );
        let mic = Microphone::open(&args.device)?;
        let recognizer = Recognizer::new(&args.model, args.threads)?;
        let robot = (!args.dry_run).then(|| {
            // Not connected here: the first command connects, and a robotd that is down at
            // startup must not keep the listener from starting — it would come up deaf
            // after every update.
            tracing::info!(socket = %args.socket.display(), "commands go to robotd at");
            Robot::new(args.socket.clone())
        });
        Ok(Self {
            mic,
            recognizer,
            down: Downsample3::new(),
            pcm: Vec::with_capacity(audio::BLOCK_SAMPLES / 3 + 1),
            robot,
            forward: None,
            last_move: Instant::now(),
            deaf_until: Instant::now(),
            last_partial: Instant::now(),
            last_partial_text: String::new(),
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
                self.recognizer.accept(&self.pcm);
                self.recognizer.run();
                self.log_partial();

                if self.recognizer.is_endpoint() {
                    let text = self.recognizer.text();
                    self.recognizer.reset();
                    self.last_partial_text.clear();
                    if !text.trim().is_empty() {
                        self.handle(&text, dry_run);
                    }
                }
            }

            self.refresh_forward(dry_run);
        }
    }

    /// Log the growing hypothesis, but only when it changed and not on every block.
    fn log_partial(&mut self) {
        if self.last_partial.elapsed() < PARTIAL_LOG {
            return;
        }
        self.last_partial = Instant::now();
        let text = self.recognizer.text();
        if text != self.last_partial_text && !text.is_empty() {
            tracing::debug!(partial = %text, "heard so far");
            self.last_partial_text = text;
        }
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
        if let Some(robot) = self.robot.as_mut() {
            robot.notify(&Call::RobotSound(proto::SoundParams {
                tag: SoundTag::Chirp,
                hold: None,
            }));
        }
    }
}
