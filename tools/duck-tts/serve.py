#!/usr/bin/env python3
"""把中文文本合成成 WAV 的小服务，给板子上的 duck-ear 用。

GET /say?text=你好&speed=1.0   -> audio/wav（单声道 s16le，采样率随模型）
GET /health                    -> ok

主机 CPU 上合成，板子的 A55 上 RTF 是 8.8–13，放这里才有实时性。
换声音：`DUCK_TTS_VOICE=fanchen .venv/bin/python serve.py`（默认 melo）。
"""
import io
import os
import sys
import time
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

import numpy as np
import sherpa_onnx

# Relative to this file, so the service works wherever the repo is checked out.
HERE = os.path.dirname(os.path.abspath(__file__))
PORT = 8100
MAX_CHARS = 300

# Peak the output is scaled to. The models emit quiet audio (peak 0.1–0.2) that is hard to
# hear on the duck's small speaker; the prompt bank plays at full scale, so the reply does too.
# This cannot make the duck hear itself — the microphone is off while a reply plays.
TARGET_PEAK = 0.85

# One entry per directory under models/. All of them share tokens/lexicon/dict and the four
# rule FSTs; only the network and the thread count differ.
#
# Threads matter more than the model: MeloTTS int8 measured RTF 2.1 here and fp32 0.13 at
# 16 threads, because this Xeon (E5-2680 v4, Broadwell) has no VNNI and quantized kernels fall
# back to a slow path. Keep fp32.
VOICES = {
    "melo": {
        "model": "model.onnx",
        "threads": 8,
        "note": "MeloTTS 中文+英文，44.1 kHz，自然女声（默认）",
    },
    "fanchen": {
        "model": "vits-zh-hf-fanchen-C.onnx",
        "threads": 2,
        "note": "VITS 中文，16 kHz，偏可爱的动漫音",
    },
}
VOICE = os.environ.get("DUCK_TTS_VOICE", "melo")
if VOICE not in VOICES:
    sys.exit(f"unknown DUCK_TTS_VOICE {VOICE!r}; choose from {', '.join(VOICES)}")


def load():
    t0 = time.time()
    voice = VOICES[VOICE]
    model_dir = os.path.join(HERE, "models", VOICE)
    config = sherpa_onnx.OfflineTtsConfig(
        model=sherpa_onnx.OfflineTtsModelConfig(
            vits=sherpa_onnx.OfflineTtsVitsModelConfig(
                model=os.path.join(model_dir, voice["model"]),
                lexicon=os.path.join(model_dir, "lexicon.txt"),
                tokens=os.path.join(model_dir, "tokens.txt"),
                dict_dir=os.path.join(model_dir, "dict"),
            ),
            num_threads=voice["threads"],
        ),
        rule_fsts=",".join(
            os.path.join(model_dir, f)
            for f in ("date.fst", "number.fst", "phone.fst", "new_heteronym.fst")
        ),
    )
    tts = sherpa_onnx.OfflineTts(config)
    print(
        f"tts ready in {time.time() - t0:.1f}s voice={VOICE} rate={tts.sample_rate} "
        f"threads={voice['threads']} port={PORT}",
        flush=True,
    )
    return tts


TTS = load()


def synth(text, speed):
    audio = TTS.generate(text, sid=0, speed=speed)
    samples = np.asarray(audio.samples, dtype=np.float32)
    peak = float(np.abs(samples).max()) if samples.size else 0.0
    if peak > 0.01:
        samples = samples * (TARGET_PEAK / peak)
    pcm = np.clip(samples * 32767, -32768, 32767).astype("<i2")
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(audio.sample_rate)
        w.writeframes(pcm.tobytes())
    return buf.getvalue(), len(pcm) / audio.sample_rate


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/health":
            body = b"ok"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if url.path != "/say":
            self.send_error(404)
            return
        q = parse_qs(url.query)
        text = (q.get("text") or [""])[0].strip()
        if not text:
            self.send_error(400, "missing text")
            return
        if len(text) > MAX_CHARS:
            self.send_error(413, f"text longer than {MAX_CHARS} characters")
            return
        try:
            speed = float((q.get("speed") or ["1.0"])[0])
        except ValueError:
            speed = 1.0
        t0 = time.time()
        data, seconds = synth(text, speed)
        print(
            f'{time.strftime("%H:%M:%S")} say {seconds:4.1f}s audio '
            f'{time.time() - t0:4.2f}s synth  {text}',
            flush=True,
        )
        self.send_response(200)
        self.send_header("Content-Type", "audio/wav")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):
        pass  # the line above is the log


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--check":
        data, seconds = synth("你好，我是小鸭。", 1.0)
        print(f"ok: {len(data)} bytes, {seconds:.2f}s audio")
        sys.exit(0)
    ThreadingHTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
