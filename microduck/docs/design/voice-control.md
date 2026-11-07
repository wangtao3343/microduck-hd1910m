# Voice control

Status: implemented 2026-11-07 · crate `duck-ear` · planning doc
[`docs/语音交互方案.md`](../../../docs/语音交互方案.md)

Speaking a Chinese command to the duck moves it. Everything on the instruction path is
local: the microphone, the recognizer, the vocabulary, the intent. Nothing is uploaded.

```
arecord (48 kHz stereo, right channel) → 16 kHz mono → sherpa-onnx streaming zipformer
   → transcript → vocabulary → robot.move / robot.do / robot.sound / robot.stop
```

## Why full streaming ASR and no wake word

The planning doc left one architecture question open — a small keyword spotter in front of
the recognizer, or the recognizer itself — to be decided by measuring this board. Measured
on 2026-11-07 with `sherpa-onnx-streaming-zipformer-zh-14M-2023-02-23` (int8, 29 MB):

| | result | criterion |
|---|---|---|
| RTF, one thread, real-time pacing | 0.26 | < 0.5 |
| CPU frequency, 2 minutes continuous | 1800 MHz throughout | ≥ 1.4 GHz |
| SoC temperature | 67–73 °C (idle 63 °C) | ≤ 85 °C |
| RSS | 117 MB, flat | no growth |

Two threads bought 15% of wall clock for 2.5× the CPU time — the model is small enough that
ONNX Runtime's intra-op parallelism is mostly spin — so the daemon runs one thread and
leaves three of the SoC's four cores to `robotd`.

The consequence is that continuous listening is affordable, so there is no keyword spotter
and no wake word: the recognizer runs all the time and decides for itself when an utterance
ended (2.4 s of silence with nothing decoded, 1.2 s after the last word). If the enclosure
ever runs hot in summer, the fallback the planning doc describes — a keyword model in front,
recognizer on demand — is still available and does not change anything above the vocabulary.

The daemon itself, measured on the board on 2026-11-07 (`--dry-run`, 70 s, one thread):
RSS 101 MB flat, 65 °C, 1800 MHz throughout, about a quarter of a core. It loads the four
model files in **15 s** — the recognizer is ready 15 s after start, which is why the unit is
`Type=exec` and nothing waits on it.

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
  never fed to the recognizer.

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
| model (`encoder/decoder/joiner/tokens`) | `/var/lib/robot/asr/zh-14m` | 29 MB, changes yearly |
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
# journal: "heard so far" partials, then `heard = "坐下"` / "not a command"`
```

To make it hear a known sentence, play it through the codec, not the card's default device
(the default is HDMI and the microphone will hear nothing):

```bash
aplay -D plughw:aic3104,0 /var/lib/robot/prompts/relax.wav   # → 我累了休息一下
```

Then with the robot enabled on the pad — say 叫一声 before anything that walks.
