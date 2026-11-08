# duck-tts

The duck's voice: a small HTTP service on the **host** that turns text into a WAV.
The board's `duck-ear` fetches `/say?text=…` and plays it through the codec's speaker.

Synthesis does not run on the board. VITS (fanchen-C) measured RTF 8.8–13 on the board's
A55 — a two-second sentence took nineteen seconds to make — against a fraction of realtime
here.

## Voices

Two services, one HTTP contract. `serve_f5.py` is what runs now (port 8100); `serve.py` is
the sherpa-onnx fallback if the GPU or the torch stack is unavailable.

| voice | service | model | rate | RTF on the host | character |
|---|---|---|---|---|---|
| **clone** (default) | `serve_f5.py` | F5-TTS Base + vocos, CUDA | 24 kHz | ~0.4 (2.3 s for a 4.6 s line) | **clones the duck's own prompt bank** — a real human recording, lowered 2 semitones |
| melo | `serve.py` | MeloTTS zh+en, fp32, 8 threads | 44.1 kHz | 0.19–0.29 | natural female, Chinese and English |
| fanchen | `serve.py` | VITS fanchen-C, 2 threads | 16 kHz | ~0.4 | cute anime voice |

The clone is the answer to "make it sound like the boot prompt": F5-TTS takes a 4-second
reference clip plus its transcript and speaks new text in that voice. The reference is
`fault_reset.wav` from the board's prompt bank ("伺服故障已复位，请按开始键继续"), 24 kHz mono
under `models/f5/`. Zero-shot — no training, the weights are not touched.

Three settings were decided by ear and by measurement:

- **`F5_PITCH=0.891`** (2 semitones down). The clone comes out higher than the reference; lowered
  it sits where the prompt bank does. ffmpeg `rubberband` keeps the duration; it costs tens of
  milliseconds and drops the level ~10 %, so the peak normalisation runs *after* the shift.
  `F5_PITCH=1` disables it.
- **`F5_NFE=32`** (the reference setting). `16` measured 1.1 s vs 1.6 s for the same line and
  was audibly rougher.
- **VRAM 1.2 GB** on top of Ollama's 6.5 GB. Both fit the 12 GB card.

MeloTTS int8 is **10× slower** on this machine (RTF 2.1) than fp32: the Xeon E5-2680 v4 is
Broadwell, which has no VNNI, so quantized kernels take a slow path. Keep the fp32 model.

Every voice's output is peak-normalised to 0.85 — the models emit quiet audio (peak 0.1–0.2)
that is hard to hear on the duck's small speaker. This cannot make the duck hear itself: the
microphone is off while a reply plays.

## Run

```bash
cd tools/duck-tts
nohup .venv-f5/bin/python serve_f5.py > f5.log 2>&1 &    # the current voice
nohup .venv/bin/python serve.py > serve.log 2>&1 &       # fallback, needs port 8100 free
# or once, in the foreground, to watch it: .venv-f5/bin/python serve_f5.py
```

```
GET /say?text=你好&speed=1.0   → audio/wav, mono s16le
GET /health                    → ok
```

`serve_f5.py --check` loads the model, synthesises 你好，我是小鸭。 and exits — the fastest
way to tell whether CUDA and the weights are in place. Env: `F5_REF`, `F5_REF_TEXT`,
`F5_DEVICE` (`cuda`/`cpu`), `F5_NFE`, `F5_PITCH`, `DUCK_TTS_PORT`.

## Install from scratch

### The clone (`serve_f5.py`) — needs an NVIDIA GPU

```bash
cd tools/duck-tts
uv venv .venv-f5 --python 3.12
uv pip install --python .venv-f5/bin/python \
  --index-url https://pypi.tuna.tsinghua.edu.cn/simple \
  -r requirements-f5.txt

# Reference clip: the duck's own prompt, resampled to what F5 wants (24 kHz mono).
# The bank is on the board, not in the repo.
mkdir -p models/f5
scp duck@192.168.3.10:/var/lib/robot/prompts/fault_reset.wav /tmp/
ffmpeg -y -i /tmp/fault_reset.wav -ac 1 -ar 24000 models/f5/ref_fault_reset.wav
# same for a second reference if wanted: relax.wav, transcript 我累了，休息一下

.venv-f5/bin/python serve_f5.py --check     # first run downloads ~1.4 GB via hf-mirror
```

`HF_ENDPOINT=https://hf-mirror.com` is set inside `serve_f5.py` before `huggingface_hub` is
imported; weights land in `~/.cache/huggingface/hub`, not in `models/`. The reference text
must match the clip exactly — it is the prompt the model conditions on.

### The fallback (`serve.py`)

```bash
cd tools/duck-tts
mkdir -p models/melo models/fanchen

# MeloTTS, from the HuggingFace mirror (the GitHub release is slow from here)
B=https://hf-mirror.com/csukuangfj/vits-melo-tts-zh_en/resolve/main
for f in model.onnx tokens.txt lexicon.txt date.fst number.fst phone.fst \
         new_heteronym.fst dict/hmm_model.utf8 dict/idf.utf8 dict/jieba.dict.utf8 \
         dict/stop_words.utf8 dict/user.dict.utf8 dict/pos_dict/char_state_tab.utf8 \
         dict/pos_dict/prob_emit.utf8 dict/pos_dict/prob_start.utf8 dict/pos_dict/prob_trans.utf8; do
  mkdir -p "models/melo/$(dirname "$f")"
  curl -sL -o "models/melo/$f" "$B/$f"
done

# fanchen-C, from the sherpa-onnx release
curl -LO https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-zh-hf-fanchen-C.tar.bz2
tar xjf vits-zh-hf-fanchen-C.tar.bz2
mv vits-zh-hf-fanchen-C/* models/fanchen/ && rmdir vits-zh-hf-fanchen-C

uv venv .venv
uv pip install --python .venv/bin/python \
  --index-url https://pypi.tuna.tsinghua.edu.cn/simple \
  -r requirements.txt
```

`models/`, `.venv/` and `.venv-f5/` are not in the repository (`.gitignore`): the model files
and the interpreters are local artifacts, the service is not.

## Autostart

A user unit; steps and the unit file are in
[`docs/语音对话操作.md`](../../docs/语音对话操作.md).
