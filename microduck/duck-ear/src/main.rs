//! `duck-ear` — voice control for the duck.
//!
//! Microphone → Silero VAD → one utterance at a time into offline SenseVoice → a small
//! command vocabulary → the same JSON-RPC intents the gamepad sends. Everything is on the
//! board: the audio never leaves it.
//!
//! ## What this is not
//!
//! It does not enable the robot. Starting the policy stays the gamepad's job (see
//! `intent`'s module docs for why). Acknowledgements go through `robot.sound` like any other
//! client's; a conversational answer is different — it is a file, and `robot.sound` plays
//! the prompt bank — so `speak` plays that one itself.
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
//! ## Two kinds of sentence
//!
//! A sentence is either a **command** — matched locally, answered in under a second by the
//! robot — or a **remark**: it contains the duck's name (小鸭), and the duck answers it. The
//! remark goes to a model on the user's workstation over the LAN (`brain`), the answer comes
//! back as a WAV from the host's TTS service (`speak`), and the board plays it. That split is
//! deliberate: motion is decided by a fixed vocabulary that cannot hallucinate, and
//! conversation is allowed to be wrong. For [`CONVERSATION_WINDOW`] after an answer, more
//! remarks do not need the name again — that is what makes it a conversation rather than a
//! series of summonses.
//!
//! ```text
//! duck-ear --dry-run            # print transcripts, send nothing to robotd
//! duck-ear                      # listen and act
//! duck-ear --brain "" --tts ""  # commands only, no conversation
//! ```

mod asr;
mod audio;
mod brain;
mod client;
mod intent;
mod net;
mod speak;
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
use crate::brain::Brain;
use crate::client::Robot;
use crate::intent::Command;
use crate::speak::Voice;
use crate::vad::Segmenter;

/// How long after an answer a remark still counts as part of the conversation.
///
/// The duck is still standing there and the person is still talking to it; requiring the
/// name again after every sentence would make it a vending machine. The cost of the window
/// is that other speech in the room during it is answered too.
const CONVERSATION_WINDOW: Duration = Duration::from_secs(45);

/// What to say when the person only says the duck's name.
const GREETING: &str = "我在。";

/// What to say when the host does not answer a remark.
///
/// The brain and the voice are both on the host, so this sentence mostly cannot be spoken:
/// it is handed to the same unreachable service that just failed. When it can be — Ollama
/// down, host fine — the person hears why the duck went quiet; when it cannot, the duck
/// honks instead (see `alarm`). Silence is the one outcome that must not happen.
const UNREACHABLE: &str = "我连不上电脑了，请检查一下。";

/// The same sentence, performed by the duck's own voice and compiled into the binary.
///
/// The voice service cannot speak it when it is needed — the host is exactly what is missing
/// — so it is synthesized once (F5-TTS cloning the prompt bank, 24 kHz mono s16le, 2.9 s,
/// 140 kB) and played through `speak::Voice::play` with no network involved. Only if even
/// that fails does the duck honk.
const HOST_OFFLINE: &[u8] = include_bytes!("../assets/host_offline.wav");

/// How long to stay deaf after the duck finishes speaking.
///
/// Shorter than the quack's window: the microphone is not read at all while the reply plays,
/// so only the pipe's drained tail (well under half a second) has to be discarded.
const DEAF_AFTER_REPLY: Duration = Duration::from_millis(600);

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

    /// `host:port` of the Ollama on the workstation. Empty means commands only.
    ///
    /// The board cannot run a model worth talking to; see `brain`.
    #[arg(long, default_value = "192.168.3.11:11434")]
    brain: String,

    /// The model to ask. `qwen3.5:9b` answers in about two seconds on the host; the 27B one
    /// measured fifteen seconds and is unusable for speech.
    #[arg(long, default_value = "qwen3.5:9b")]
    brain_model: String,

    /// `host:port` of the TTS service on the workstation. Empty means answers are only
    /// logged.
    #[arg(long, default_value = "192.168.3.11:8100")]
    tts: String,

    /// ALSA playback device for spoken answers. `plughw` converts the model's 16 kHz.
    #[arg(long, default_value = voice::PLAYBACK_DEVICE)]
    speaker: String,

    /// Transcribe, and converse, but never open the robotd socket — no motion, no quack.
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
    tracing::info!("listening — say 停下, 坐下, 前进 or 叫一声; say 小鸭 to talk");
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
    /// No audio is fed to the segmenter before this instant — see [`DEAF_AFTER_SOUND`] and
    /// [`DEAF_AFTER_REPLY`].
    deaf_until: Instant,
    /// Remarks are answered until this instant without the name being said again.
    chat_until: Instant,
    /// `None` when `--brain ""`: remarks are logged and not answered.
    brain: Option<Brain>,
    /// `None` when `--tts ""`: answers are logged instead of spoken.
    voice: Option<Voice>,
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
        // Conversation is optional in both halves: an address of "" disables it, so a board
        // with no workstation on the network is still an ear for commands. A malformed
        // address is a configuration error and stops startup.
        let brain = (!args.brain.is_empty())
            .then(|| Brain::new(&args.brain, args.brain_model.clone()))
            .transpose()?;
        let voice = (!args.tts.is_empty())
            .then(|| Voice::new(&args.tts, args.speaker.clone()))
            .transpose()?;
        if let Some(brain) = &brain {
            tracing::info!(at = %args.brain, model = %args.brain_model, "remarks go to");
            brain.warm();
        }
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
            chat_until: Instant::now(),
            brain,
            voice,
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
        if let Some(command) = intent::parse(text) {
            tracing::info!(heard = %text, ?command, "command");
            if dry_run {
                return;
            }
            if let Err(error) = self.dispatch(command) {
                tracing::warn!(?command, %error, "command was not carried out");
                self.forward = None;
            }
            return;
        }

        // Not a command. It is a remark if the duck was addressed by name, or if the person
        // is still inside a conversation an answer made. Commands were checked first on
        // purpose: a fixed vocabulary is the only speaker of this process allowed to move
        // the robot, and "小鸭坐下" is a command, not a question about sitting.
        match intent::remark(text) {
            Some(remark) => self.converse(text, &remark),
            None if Instant::now() < self.chat_until => self.converse(text, text),
            None => tracing::info!(heard = %text, "not a command"),
        }
    }

    /// Answer a remark: the model on the host, then the host's voice, then the speaker.
    ///
    /// Blocks for the length of the exchange (about two seconds of model, one of synthesis,
    /// then the answer's own duration) with the microphone unread — the duck must not hear
    /// its own reply, and there is nothing to do with the audio anyway.
    fn converse(&mut self, heard: &str, remark: &str) {
        let Some(brain) = self.brain.as_mut() else {
            tracing::info!(heard = %heard, "a remark, and no brain configured");
            return;
        };

        let (reply, answered) = if remark.is_empty() {
            tracing::info!(heard = %heard, "called by name");
            (GREETING.to_owned(), true)
        } else {
            let started = Instant::now();
            match brain.reply(remark) {
                Ok(reply) => {
                    tracing::info!(
                        heard = %heard,
                        remark = %remark,
                        took_ms = started.elapsed().as_millis(),
                        reply = %reply,
                        "answered"
                    );
                    (reply, true)
                }
                Err(error) => {
                    // Say so rather than going quiet: a remark that gets no response at all
                    // is indistinguishable from a deaf duck. The voice is on the same host,
                    // so this often fails too and `alarm` takes over.
                    tracing::warn!(heard = %heard, %error, "the brain did not answer");
                    (UNREACHABLE.to_owned(), false)
                }
            }
        };

        let spoken = match self.voice.as_ref() {
            Some(voice) => match voice.say(&reply) {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%error, "the reply was not spoken");
                    false
                }
            },
            None => {
                tracing::info!(reply = %reply, "no TTS service; the reply is only logged");
                true
            }
        };
        if !spoken {
            self.notice();
        }
        // Only an answer the person actually heard keeps the conversation open. With a working
        // voice, every answer re-arms the window and a follow-up needs no name; with a broken
        // one, re-arming would answer everything in the room for as long as the network stays
        // down — a duck that never stops replying to a television.
        if answered && spoken {
            self.chat_until = Instant::now() + CONVERSATION_WINDOW;
        }
        // The pipe filled while the microphone was unread; drop what it holds of our own
        // voice, and start the detector clean.
        self.deaf_until = Instant::now() + DEAF_AFTER_REPLY;
        self.segmenter.reset();
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

    /// Tell the person the host is gone, in the duck's own voice, out of the binary.
    ///
    /// The voice service is on that host, so this is the only way to say a sentence when it
    /// is unreachable; `alarm` is the fallback if even a local file will not play.
    fn notice(&mut self) {
        let played = match self.voice.as_ref() {
            Some(voice) => match voice.play(HOST_OFFLINE, "offline") {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%error, "the offline notice was not played");
                    false
                }
            },
            None => false,
        };
        if !played {
            self.alarm();
        }
    }

    /// The last resort when neither the brain nor the voice on the host answers, and even the
    /// reply's own recording will not play: a sharp honk, which needs nothing but robotd.
    ///
    /// It is deliberately not a quack — `chirp` is the acknowledgement for a command, and a
    /// person must be able to tell "done" from "I could not reach the computer".
    fn alarm(&mut self) {
        self.deaf_until = Instant::now() + DEAF_AFTER_SOUND;
        self.segmenter.reset();
        if let Some(robot) = self.robot.as_mut() {
            robot.notify(&Call::RobotSound(proto::SoundParams {
                tag: SoundTag::Alarm,
                hold: None,
            }));
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
