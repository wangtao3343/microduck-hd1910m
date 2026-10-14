#!/usr/bin/env bash
# 舵机调试台一键启动。默认接真机。
#
#   ./start.sh                          真机：/dev/ttyUSB0 + 从总线读 IMU 小板（ID 200）
#   ./start.sh --port /dev/ttyACM0      真机，换串口
#   ./start.sh --fake                   不接硬件，只看界面（假总线 15 颗舵机）
#
# 接真机前：URT-2 先插 USB 再给舵机上电；别让 robotd 同时占着这个串口。
# 串口打不开（USB 没插、口号变了、被占着）server.py 会把原因写进日志和 stderr 然后直接退出，
# 不会自己换成假总线 —— 那样页面会显示 15 颗并不存在的「在线」舵机。插好线重新跑本脚本即可。
# 只想看界面不动舵机：明写 --fake。
# 小板没接或不应答也不影响舵机调试。
#
# 网页端口默认 9010，改第 3 节那个 PORT= 就行（会一并传给 server.py）。
# 想临时换：./start.sh --http-port 9020，或 HTTP_PORT=9020 ./start.sh。
# 直接跑 python server.py（不走本脚本）仍是它自己的默认 9090。
#
# 环境变量：
#   SERVO_PORT=/dev/ttyACM0   默认串口（只在不带参数时生效，等价于 --port）
#   PYTHON=python3.12         指定用哪个解释器建 venv
#   NO_OPEN=1                 别自动开浏览器
#   HTTP_PORT=9020            换网页端口（等价于传 --http-port）
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
if ! "$VENV/bin/python" -c "import starlette, uvicorn, serial, pytest, httpx" >/dev/null 2>&1; then
    echo "→ 装依赖（第一次要几十秒）"
    "$VENV/bin/pip" install -q --upgrade pip
    "$VENV/bin/pip" install -q -r requirements.txt
fi

# ---- 2. 默认接真机：缺 --port 就补默认串口，缺 --imu-* 就补 --imu-bus ----
#     是「缺什么补什么」，不是「零参数才补」—— 否则 ./start.sh --http-port 9022
#     这种会把串口弄丢，server.py 收到空 --port 就直接退回假总线了。
#     明写 --fake（不想接硬件）时什么都不补；其余情况没给 --port 就一律补默认串口。
#     --imu-swd / --imu-jlink 只换 IMU 的读法，跟舵机总线不冲突，所以照样补串口；
#     只调 IMU 不想碰总线，那才用得上 --fake。
want_fake=0
have_serial=0
have_imu=0
for a in "$@"; do
    case "$a" in
        --fake)          want_fake=1 ;;
        --port|--port=*) have_serial=1 ;;
        --imu-*)         have_imu=1 ;;
    esac
done
if [ "$want_fake" = 0 ]; then
    [ "$have_serial" = 0 ] && set -- "$@" --port "${SERVO_PORT:-/dev/ttyUSB0}"
    [ "$have_imu" = 0 ] && set -- "$@" --imu-bus
fi

# ---- 3. 网页端口：--http-port 参数 > HTTP_PORT 环境变量 > 这里的默认值 ----
#     注意 server.py 自己的 --http-port 默认是 9090。只改这个变量而不把它传下去，
#     横幅会打新端口、服务却还绑 9090 —— 所以下面定完就显式补一个 --http-port。
PORT=9090
have_http_arg=0
prev=""
for a in "$@"; do
    [ "$prev" = "--http-port" ] && { PORT="$a"; have_http_arg=1; }
    case "$a" in --http-port=*) PORT="${a#*=}"; have_http_arg=1 ;; esac
    prev="$a"
done
if [ "$have_http_arg" = 0 ]; then
    [ -n "${HTTP_PORT:-}" ] && PORT="$HTTP_PORT"
    set -- "$@" --http-port "$PORT"
fi

# ---- 4. 认出串口和运行模式，好在开服前就把横幅打对 ----
SERIAL=""
MODE="真机"
prev=""
for a in "$@"; do
    [ "$prev" = "--port" ] && SERIAL="$a"
    case "$a" in
        --fake)   MODE="模拟（假总线，不接硬件）" ;;
        --port=*) SERIAL="${a#*=}" ;;
    esac
    prev="$a"
done
[ -n "$SERIAL" ] && MODE="真机 · $SERIAL"

URL="http://127.0.0.1:$PORT"
# 横幅只留两行：一行说是什么模式配哪个地址，一行说局域网地址和怎么停
echo "舵机调试台 · $MODE"
echo "$URL"
LAN="$(hostname -I 2>/dev/null | awk '{print $1}')"
[ -n "$LAN" ] && echo "局域网 http://$LAN:$PORT · Ctrl-C 停"

# ---- 5. 有图形界面就顺手开一下浏览器 ----
#     等网页真的能连上再开，不是「过 2 秒就开」：串口打不开时 server.py 会立刻退出，
#     定时开的话浏览器已经弹出来了，指着一个连不上的页面。这里最多等 15 秒，等不到就不开。
#     探测用 bash 自带的 /dev/tcp，不依赖 curl。
if [ "${NO_OPEN:-0}" != "1" ] && command -v xdg-open >/dev/null 2>&1 \
        && [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    (
        for _ in $(seq 1 60); do
            if (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then
                xdg-open "$URL" >/dev/null 2>&1 || true
                exit 0
            fi
            sleep 0.25
        done
    ) &
fi

exec "$VENV/bin/python" server.py "$@"
