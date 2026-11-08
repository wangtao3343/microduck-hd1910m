#!/usr/bin/env python3
"""Same HTTP contract as serve.py, but the voice is F5-TTS cloning the duck's own prompt bank.

GET /say?text=你好&speed=1.0   -> audio/wav（24 kHz 单声道 s16le）
GET /health                    -> ok

为什么另起一个文件：serve.py 跑在 sherpa-onnx 的 venv（.venv）里，F5 要 torch + CUDA
（.venv-f5），两套依赖放一起会互相拉扯。HTTP 契约一样，板子那边无感。

参考音是 /var/lib/robot/prompts 里机器人自带的提示音（48 kHz 人声录音），先用 ffmpeg
降到 24 kHz。默认用 fault_reset.wav（「伺服故障已复位请按开始键继续」，4 秒，字句清楚）。

模型权重从 HuggingFace 镜像下（HF_ENDPOINT 已在下面设好），约 1.3 GB，第一次启动会下载。
GPU：RTX 3060 12 GB，Ollama 常驻占约 6.8 GB，F5 Base 再占约 1.2 GB。

输出再降 2 个半音（F5_PITCH=0.891）：克隆出来的声音本身比提示音高，降调后更接近
「开机提示音那种」。降调用 ffmpeg 的 rubberband，时长不变，一句话多花几十毫秒；
降调会把电平压掉约 10%，所以归一化放在降调之后（F5_PITCH=1 可关掉降调）。
"""
import io
import os
import subprocess
import sys
import time
import wave

# Must be set before huggingface_hub is imported anywhere.
os.environ.setdefault("HF_ENDPOINT", "https://hf-mirror.com")

import numpy as np
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

HERE = os.path.dirname(os.path.abspath(__file__))
PORT = int(os.environ.get("DUCK_TTS_PORT", "8100"))
MAX_CHARS = 300
TARGET_PEAK = 0.85

REF = os.environ.get("F5_REF", os.path.join(HERE, "models", "f5", "ref_fault_reset.wav"))
REF_TEXT = os.environ.get(
    "F5_REF_TEXT", "伺服故障已复位，请按开始键继续"
)
DEVICE = os.environ.get("F5_DEVICE", "cuda")
MODEL = os.environ.get("F5_MODEL", "F5TTS_v1_Base")
# 32 is the reference setting; 16 halves the time and is audibly rougher.
NFE = int(os.environ.get("F5_NFE", "32"))
# Pitch factor: 2 semitones down. 1.0 disables the shift.
PITCH = float(os.environ.get("F5_PITCH", "0.891"))


def load():
    from f5_tts.api import F5TTS

    t0 = time.time()
    tts = F5TTS(model=MODEL, device=DEVICE)
    print(
        f"f5 ready in {time.time() - t0:.1f}s model={MODEL} device={DEVICE} "
        f"ref={os.path.basename(REF)} nfe={NFE} pitch={PITCH:.3f} port={PORT}",
        flush=True,
    )
    return tts


TTS = load()


def normalize(samples):
    peak = float(np.abs(samples).max()) if samples.size else 0.0
    if peak > 0.01:
        samples = samples * (TARGET_PEAK / peak)
    return samples


def to_wav(samples, rate):
    pcm = np.clip(samples * 32767, -32768, 32767).astype("<i2")
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(pcm.tobytes())
    return buf.getvalue(), len(pcm)


def from_wav(data):
    with wave.open(io.BytesIO(data)) as w:
        rate = w.getframerate()
        pcm = np.frombuffer(w.readframes(w.getnframes()), dtype="<i2")
    return pcm.astype(np.float32) / 32768.0, rate


def pitch_shift(data):
    if PITCH >= 0.999:
        return data
    run = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", "pipe:0",
         "-af", f"rubberband=pitch={PITCH:.4f}", "-f", "wav", "pipe:1"],
        input=data, capture_output=True,
    )
    if run.returncode != 0 or not run.stdout:
        raise RuntimeError("ffmpeg pitch shift failed: " + run.stderr.decode()[:200])
    return run.stdout


def synth(text, speed):
    t0 = time.time()
    wav, rate, _ = TTS.infer(
        ref_file=REF,
        ref_text=REF_TEXT,
        gen_text=text,
        nfe_step=NFE,
        speed=speed,
    )
    samples = np.asarray(wav, dtype=np.float32).reshape(-1)
    data, n = to_wav(normalize(samples), rate)
    if PITCH < 0.999:
        # rubberband drops the level by about 10 %, so normalise again after it.
        samples, rate = from_wav(pitch_shift(data))
        data, n = to_wav(normalize(samples), rate)
    return data, n / rate, time.time() - t0


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
        data, seconds, dt = synth(text, speed)
        print(
            f'{time.strftime("%H:%M:%S")} say {seconds:4.1f}s audio {dt:4.2f}s synth  {text}',
            flush=True,
        )
        self.send_response(200)
        self.send_header("Content-Type", "audio/wav")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):
        pass


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--check":
        data, seconds, dt = synth("你好，我是小鸭。", 1.0)
        print(f"ok: {len(data)} bytes, {seconds:.2f}s audio, {dt:.2f}s synth")
        sys.exit(0)
    ThreadingHTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
