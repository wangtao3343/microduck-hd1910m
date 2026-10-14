#!/usr/bin/env bash
# 舵机调试台一键启动。
#
#   ./start.sh                          模拟模式：假总线 15 颗舵机 + 假 IMU 小板，不接硬件
#   ./start.sh --port /dev/ttyUSB0      接真机（URT-2 插 USB）
#   ./start.sh --port /dev/ttyUSB0 --imu-bus   真机 + 从总线读 ID 200 小板
#
# 环境变量：
#   PYTHON=python3.12   指定用哪个解释器建 venv
#   NO_OPEN=1           别自动开浏览器
#   HTTP_PORT=8080      换端口（等价于传 --http-port）
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$HERE"

VENV="$HERE/.venv"
PY="${PYTHON:-python3}"

# ---- 1. 虚拟环境：没有就建，缺依赖就装 ----
if [ ! -x "$VENV/bin/python" ]; then
    echo "→ 建虚拟环境 $VENV（用 $("$PY" -V 2>&1)）"
    "$PY" -m venv "$VENV"
fi

# 依赖齐不齐，缺了才装 —— 每次启动都 pip install 会很慢
if ! "$VENV/bin/python" -c "import starlette, uvicorn, serial" >/dev/null 2>&1; then
    echo "→ 装依赖（第一次要几十秒）"
    "$VENV/bin/pip" install -q --upgrade pip
    "$VENV/bin/pip" install -q -r requirements.txt
fi

# ---- 2. 不带参数 = 模拟模式 ----
MODE="模拟（假总线 + 假小板）"
if [ $# -eq 0 ]; then
    set -- --fake --imu-bus
fi

# ---- 3. HTTP_PORT 环境变量：真把它变成 --http-port 传下去（参数里给了就以参数为准）----
has_port=0
for a in "$@"; do case "$a" in --http-port|--http-port=*) has_port=1 ;; esac; done
if [ "$has_port" = 0 ] && [ -n "${HTTP_PORT:-}" ]; then
    set -- "$@" --http-port "$HTTP_PORT"
fi

# ---- 4. 从参数里抠出端口，好在开服前就把网址打出来 ----
PORT=8080
prev=""
for a in "$@"; do
    [ "$prev" = "--http-port" ] && PORT="$a"
    case "$a" in --http-port=*) PORT="${a#*=}" ;; esac
    prev="$a"
done

URL="http://127.0.0.1:$PORT"
echo "──────────────────────────────────────────────"
echo " 舵机调试台 · $MODE"
echo " $URL"
echo " 局域网：http://$(hostname -I 2>/dev/null | awk '{print $1}'):$PORT"
echo " 停：Ctrl-C"
echo "──────────────────────────────────────────────"

# ---- 4. 有图形界面就顺手开一下浏览器 ----
if [ "${NO_OPEN:-0}" != "1" ] && command -v xdg-open >/dev/null 2>&1 \
        && [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    ( sleep 2; xdg-open "$URL" >/dev/null 2>&1 || true ) &
fi

exec "$VENV/bin/python" server.py "$@"
