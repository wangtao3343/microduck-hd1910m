# Voice control

Status: implemented 2026-11-07 · crate `duck-ear` · planning doc
[`docs/语音交互方案.md`](../../../docs/语音交互方案.md)

Speaking a Chinese command to the duck moves it. Everything on the instruction path is
local: the microphone, the recognizer, the vocabulary, the intent. Nothing is uploaded.

```
arecord (48 kHz stereo, right channel) → 16 kHz mono → Silero VAD (utterance boundaries)
   → sherpa-onnx offline SenseVoice → transcript → vocabulary
   → robot.move / robot.do / robot.sound / robot.stop
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

## Deployment

Board artifacts, none of which ship in the release:

| what | where | why not in the release |
|---|---|---|
| model (`model.int8.onnx`, `tokens.txt`) | `/var/lib/robot/asr/sensevoice` | 240 MB, changes yearly |
| VAD (`silero_vad.onnx`) | `/var/lib/robot/asr/silero_vad.onnx` | 1.8 MB, from the same sherpa-onnx release |
| `libsherpa-onnx-c-api.so`, `libonnxruntime.so` | next to `duck-ear` | see below |

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
