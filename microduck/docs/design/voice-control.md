# Voice control

Status: implemented 2026-11-07, conversation 2026-11-08 · crate `duck-ear` · planning doc
[`docs/语音交互方案.md`](../../../docs/语音交互方案.md) · operations
[`docs/语音对话操作.md`](../../../docs/语音对话操作.md)

Speaking a Chinese command to the duck moves it, and addressing it by name starts a
conversation. Everything on the instruction path is local: the microphone, the recognizer,
the vocabulary, the intent. Nothing is uploaded. A remark — text, not audio — goes to a model
on the user's workstation over the LAN and comes back as speech.

```
arecord (48 kHz stereo, right channel) → 16 kHz mono → Silero VAD (utterance boundaries)
   → sherpa-onnx offline SenseVoice → transcript
        ├─ command vocabulary            → robot.move / robot.do / robot.sound / robot.stop
        └─ contains 小鸭 ("remark")      → POST 192.168.3.11:11434/api/chat
                                         ← reply text
                                         → GET 192.168.3.11:8100/say → WAV → aplay
```

## What it listens with

There is no wake word and no keyword spotter; the VAD is the front end and the recognizer
runs on whatever it hands over. That was decided by measuring, twice.

The first measurement (2026-11-07) chose a *streaming* zipformer — 14M parameters, 29 MB,
RTF 0.26 at one thread, 1800 MHz and 67–73 °C under continuous decoding — because at a
quarter of a core, always-on recognition is affordable. It also could not hear 叫一声: the
model returned 叫医生 and 下一声, the right pinyin and the wrong characters, so the vocabulary
never matched. Hotword biasing (modified beam search, `cjkchar`, score 3.0) fixed some of it
and raised the idle cost to a third of a core — a real tax for a patch.

The second measurement (same evening) compared every Chinese model this board can run, on
the board's own prompt bank (`/var/lib/robot/prompts`, 11 sentences), through the same
24-tap anti-alias decimator and the same CER:

| model | CER | decode RTF (2 threads) | RSS | load |
|---|---|---|---|---|
| streaming zipformer 14M-2023 | 12.5 % | 0.21, always on | 108 MB | 15 s |
| streaming zipformer zh-int8-2025-06-30 | 10.9 % | 0.70, always on | ~200 MB | — |
| **SenseVoice-small int8-2024-07-17** | **7.8 %** | 0.42 per utterance | 366 MB | 11.5 s |
| paraformer-zh int8-2023-09-14 | 7.8 % | 0.46 per utterance | ~400 MB | 10.5 s |

The two offline models tie; SenseVoice is faster per utterance and emits punctuation, so
that is what ships. (The RSS column is the Python bench's figure; the daemon keeps about
600 MB resident, measured below.) The five remaining prompt errors are 伺服 → 四伏/四幅 twice and 噶 → 嘎 —
none of them vocabulary words. The NPU is not a route: `/usr/lib/librknnrt.so` and
`CONFIG_ROCKCHIP_RKNPU=y` are present but there is no `/dev/rknpu`, and the sherpa-onnx
libraries this crate ships are the CPU build.

Offline models need an utterance, so `vad.rs` supplies one: Silero VAD (1.8 MB) classifies
32 ms frames as speech or not, and releases a segment after `min_silence_duration` (0.5 s) of
silence — `max_speech_duration` (10 s) forces a cut so a television cannot hold a segment
open. The detector runs all the time at about a tenth of a core; the recognizer runs only on
segments. Measured on the same prompts: a 1.2 s command decodes in 0.5 s, so the duck moves
about a second after the speaker stops (0.5 s of endpoint silence plus the decode).

The daemon measured on the board (2026-11-07 evening, `--dry-run`, prompts played through
the codec): idle **~9.5 % of a core** (VAD only, against 33 % for the streaming build),
RSS **607 MB** flat across decodes, decode RTF **0.39** with two threads (3.8 s utterance →
1.5 s), and **~25 s** to load the model from a cold start.

The trade for that accuracy is what streaming used to give: no partial transcript while the
person is still talking, and no hotwords (SenseVoice decodes greedily and the C API's hotword
path needs a transducer beam search). The intent layer did not change at all.

The daemon is not ready for **~25 s** after start (model load, measured on the board); that
is why the unit is `Type=exec` and nothing waits on it.

## The vocabulary

`intent.rs`. Containment after normalisation (whitespace and punctuation dropped), checked
in order, stop first.

| intent | phrases | becomes |
|---|---|---|
| `Stop` | 停，停下，停止，别动，站住，… | `robot.stop`, ends a `forward` |
| `SitToggle` | 坐下，坐下来，站起来，起立，… | `robot.do { skill: "sit_toggle" }` |
| `Forward` | 前进，向前，往前走，走过来，… | `robot.move vx = 0.3` refreshed at 10 Hz for 1.5 s |
| `Quack` | 叫一声，叫一下，叫唤，… | `robot.sound { tag: "chirp" }` |

Two deliberate choices:

- **Nothing enables the robot.** Starting the policy stays the gamepad's Start. A
  microphone that can energise the joints can do so because a television said something,
  and the person holding the duck has no way to know why it stood up. Everything here
  assumes an already-enabled, standing duck; when it is not, `robotd` refuses the intent
  and the refusal is logged.
- **`坐下` and `站起来` are the same toggle**, because `sit_toggle` is the only sit/stand
  skill there is. Saying 坐下 while seated stands the duck up.

`forward` is bounded rather than continuous. Continuous motion is guarded by a 500 ms
deadman, so the client must keep refreshing for as long as the robot walks; a bounded step
means a wedged listener ends in a stop rather than in a duck walking into a wall. Say 前进
again for another step.

## Hearing itself

The speaker and the microphone share a board and there is no echo cancellation. Two
mechanisms, both tested:

- No prompt in the voice bank parses as a command — `intent.rs` has a test over the whole
  prompt list, including 我累了，休息一下.
- A sound this daemon asks for is followed by a 900 ms deaf window, so the quack's echo is
  never fed to the segmenter — and the segmenter is reset when the window opens, so a
  half-captured command cannot be completed by the quack.

How loud that echo is, measured: playing `relax.wav` (「我累了，休息一下」) through the codec
at the bank's own level saturates the capture — right channel peak 1.000, RMS 0.38 — and the
recognizer returns 谢谢 / 她的怀孕. The same file at 50%, 20% and 8% of that level transcribes
exactly, all three. Two consequences: the deaf window is not politeness, it is what keeps a
clipped prompt out of the features; and a person speaking at a normal distance is well below
the clipping point, so the vocabulary does not need a level gate in front of it.

**Known gap:** a prompt `robotd` decides to play on its own — a battery warning, a fault —
is not covered by the deaf window. Those sentences are not commands either, so the worst
case is a spurious transcript in the journal, not motion.

## Conversation

A sentence that is not a command is answered if it contains one of the duck's names
(`小鸭`, `小鸭子`, `鸭鸭`, `鸭子` — `intent::remark` strips it and keeps the rest), or if it
arrives within 45 s of the last answer. Without that window a conversation would need the name
before every sentence; without the name every sentence in the room would be answered. Commands
are checked first and always win — the fixed vocabulary is the only speaker in this process
allowed to move the robot, and 小鸭坐下 is a command, not a question about sitting. Saying only
the name is answered with 我在 without asking a model anything.

The model does not run on the board. `brain.rs` is a blocking `POST /api/chat` to Ollama on
the workstation (`qwen3.5:9b`, measured 2 s warm, 8 s cold; the 27B model measured 15 s and is
unusable for speech). Three request settings: `stream: false` because the answer is spoken as
a whole sentence; `think: false` because Qwen3.5's hidden reasoning costs seconds nobody hears;
`keep_alive: 30m` because Ollama unloads after five idle minutes and a conversation has longer
pauses. History is six messages — enough for pronouns to resolve, bounded so a long session
does not grow the prompt. The user message is rolled back when a request fails, so an
unanswered question is not left in the history. At startup a one-token request warms the model
on a thread of its own: a host that is down logs a warning and does not keep the ear from
starting.

Speech is synthesized on the host too. VITS (fanchen-C) measured **RTF 8.8–13** on the board's
A55 — a two-second sentence took nineteen seconds — against **0.4** on the host, so
`speak.rs` fetches `GET /say?text=…`, writes the WAV to `$TMPDIR/duck-ear-reply.wav`, and
plays it with `aplay` through the codec's `plughw`. The reply is a file, which `robot.sound`
cannot play, so this daemon owns the speaker for the length of the answer.

The voice went through three: fanchen-C (too cartoonish), MeloTTS zh+en (natural, but still a
stranger), and now **F5-TTS cloning the duck's own prompt bank** — a 4-second clip of
`fault_reset.wav` from `/var/lib/robot/prompts` plus its transcript, zero-shot, on the host's
3060. That is what "sound like the boot prompt" means: not a better synthetic voice but *that*
voice. Three settings are load-bearing: `nfe_step 32` (16 measured 1.1 s against 1.6 s and was
audibly rougher), 24 kHz (the model's own rate), and a **2-semitone lowering** through ffmpeg
`rubberband` (`F5_PITCH=0.891`), because the clone comes out higher than the reference; the
shift keeps duration, costs tens of milliseconds, and drops the level about 10 %, so peak
normalisation runs after it. Warm synthesis is RTF ≈ 0.4 — 2.3 s for a 4.6 s line, about a
second slower than MeloTTS and 1.2 GB of VRAM beside Ollama's 6.5. `serve.py` still carries
melo and fanchen behind the same HTTP contract if the GPU or the torch stack is unavailable.

Two measurements worth keeping: MeloTTS **int8** ran at RTF 2.1 on the host while fp32 ran at
0.13 — this Xeon is Broadwell, without VNNI, and quantized kernels take a slow path; and every
voice's output is peak-normalized to 0.85, because the models emit quiet audio (peak 0.1–0.2)
that the duck's small speaker cannot make audible. That cannot cause self-hearing because the
microphone is off while a reply plays.

`net.rs` is the HTTP/1.1 client: `Connection: close`, read to end of stream, chunked framing
decoded when present, and a single deadline for the whole exchange rather than a per-read
timeout. It exists instead of `reqwest` because this daemon's loop is blocking audio reads and
an async runtime would buy it nothing.

The microphone is not read while the exchange runs — the duck cannot hear its own reply, and
there is nothing useful to do with the audio during two seconds of inference. When the answer
ends, a 600 ms deaf window discards the pipe's tail (well under half a second) and the
segmenter is reset. The price is that a person talking over the reply is not heard at all; the
alternative was the duck answering its own voice.

A host that cannot be reached is not allowed to look like a deaf duck. When the brain request
fails, the daemon answers with a fixed sentence (我连不上电脑了，请检查一下。); when the voice
request fails, it plays the same sentence **from inside the binary** — synthesized once with
the same cloned voice, 24 kHz mono, 2.9 s, 140 kB (`assets/host_offline.wav`, `include_bytes!`
in `main.rs`), through the same `aplay` path. That is the crux: the TTS is the thing that is
missing, so a sentence about missing it cannot be synthesized at that moment. Only if neither
plays does the duck honk — `robot.sound { tag: "alarm" }`, which needs nothing but robotd and
is deliberately not the `chirp` that acknowledges a command, so a person can tell "done" from
"I could not reach the computer". Silence was the one outcome worth engineering away: with no
reply and no sound, a person cannot tell a broken network from a dead microphone.

The conversation window is extended only by an answer the person actually heard. With a
working voice every answer re-arms the 45 s window; with a broken one, re-arming would answer
everything in the room for as long as the network stays down — a duck holding a conversation
with a television.

Both halves are optional at the command line: `--brain ""` disables conversation entirely,
`--tts ""` logs answers instead of speaking them, and a host that is offline costs remarks,
never commands.

## Deployment

Board artifacts, none of which ship in the release:

| what | where | why not in the release |
|---|---|---|
| model (`model.int8.onnx`, `tokens.txt`) | `/var/lib/robot/asr/sensevoice` | 240 MB, changes yearly |
| VAD (`silero_vad.onnx`) | `/var/lib/robot/asr/silero_vad.onnx` | 1.8 MB, from the same sherpa-onnx release |
| `libsherpa-onnx-c-api.so`, `libonnxruntime.so` | next to `duck-ear` | see below |

The host side — Ollama and the TTS service in `tools/duck-tts` — ships with nothing and is
installed by hand; [`docs/语音对话操作.md`](../../../docs/语音对话操作.md) has the steps and the
default addresses (`192.168.3.11:11434` and `:8100`) are the binary's flag defaults.

The two libraries are the sherpa-onnx release's `linux-aarch64-shared-cpu` build. They
carry **their own ONNX Runtime**, which is a different version from the 1.28 `robotd`
loads (by path, from `ORT_DYLIB_PATH`) — they must not come to share one. Sitting in the
same `bin/` directory is safe because the two `duck-ear → c-api → onnxruntime` hops are
resolved through `duck-ear`'s `$ORIGIN` rpath alone: `robotd` has no rpath, so its own
directory is never on its search path. `build.rs` writes that rpath with
`--disable-new-dtags` (the legacy tag is inherited across the two-hop chain; the modern
one is not), so the pair next to the binary is found wherever the release is staged.

The service units live in `duck-ear/systemd/`: `duck-ear.service` for the `/opt/robot`
release, `duck-ear-runtime.service` for this board, where robotd runs as `duck` out of the
workspace and the socket is `duck:duck` under `/home/duck`. `scripts/build.sh` builds the
binary and copies both `.so` files into `out/<mode>/bin` beside it, and
`scripts/package.sh` tars `out/` whole — so the pair travels with the release instead of
being a separate installation step.

## Trying it without moving the robot

```bash
duck-ear --dry-run
# journal: `heard = "坐下"` / `heard = "我累了，休息一下"` / `heard = "…" not a command`
# one line per utterance; `RUST_LOG=duck_ear=debug` adds the segment length and decode time
```

To make it hear a known sentence, play it through the codec, not the card's default device
(the default is HDMI and the microphone will hear nothing):

```bash
aplay -D plughw:aic3104,0 /var/lib/robot/prompts/relax.wav   # → 我累了休息一下
```

Then with the robot enabled on the pad — say 叫一声 before anything that walks.
