#!/bin/bash
# bt-watch —— AIC8800 蓝牙活体记录器
#
# 为什么有这个东西：这颗蓝牙挂死时，内核只在主机发命令时才报 "command tx timeout"，
# 真正出事的那一刻（手柄掉线）没有任何日志。本脚本每 5 秒采一次样，环形缓冲保留最近
# 一小时；一旦发现 tx timeout 或手柄掉线，就把缓冲落盘，事后能看到掉线前后的现场。
#
# 只读：不发 HCI 命令、不改任何状态、不碰机器人。

set -u

INTERVAL=5
KEEP=720                     # 720 × 5 秒 = 1 小时
RING=/run/bt-watch.ring
DEST=/var/log/bt-watch

mkdir -p "$DEST"
: > "$RING"

# 采一行样本：时间、ttyS1 中断数、控制线、手柄设备数、hci0 收发计数
take_sample() {
    local ts irq hci js g n v

    ts=$(date +%H:%M:%S)

    # ttyS1 的中断计数。主机发了命令它却不涨，就是芯片聋了。
    irq=$(grep -w ttyS1 /proc/interrupts 2>/dev/null | awk '{print $2}')
    [ -z "${irq:-}" ] && irq="-"

    # hci0 的收发计数。hciconfig 走 ioctl，不发 HCI 命令，卡死时也不会挂住。
    hci=$(hciconfig hci0 2>/dev/null | awk '/RX bytes|TX bytes/ {sub(/^[ \t]+/, ""); printf "%s | ", $0}')
    [ -z "${hci:-}" ] && hci="hci0 不存在"

    # 控制线：11=芯片→主机唤醒 12=主机→芯片唤醒 17=复位 77=RTS
    g=""
    for n in 11 12 17 77; do
        v=$(awk -v pat="gpio-$n " '$0 ~ pat {
                 for (i = 4; i <= NF; i++)
                     if ($i == "in" || $i == "out") { print $i "/" $(i+1); exit }
             }' /sys/kernel/debug/gpio 2>/dev/null)
        g="$g g$n=${v:-?}"
    done

    # 手柄在不在：掉线时 /dev/input/js* 会消失，这是唯一一个「手柄侧」的信号。
    js=$(ls /dev/input/js* 2>/dev/null | wc -l)

    printf "%s irq=%s js=%s%s hci=%s\n" "$ts" "$irq" "$js" "$g" "$hci"
}

# 把当前环形缓冲和 dmesg 尾部落盘
dump() {
    local kind=$1 why=$2 out
    out="$DEST/$kind-$(date +%Y%m%d-%H%M%S).log"
    {
        echo "=== $why ==="
        echo "落盘时间：$(date '+%F %T')"
        echo "--- 环形缓冲（最近一小时，5 秒一行）---"
        cat "$RING"
        echo "--- dmesg 最近 40 行 ---"
        dmesg -T 2>/dev/null | tail -40
    } > "$out"
    # 只留最近 20 份，避免无限增长
    ls -1t "$DEST"/*.log 2>/dev/null | tail -n +21 | while read -r f; do rm -f "$f"; done
}

echo "bt-watch 启动：间隔 ${INTERVAL}s，保留 ${KEEP} 行" >> "$RING"

# -1 表示还没对齐基线：第一次采样只记数，不落盘，
# 否则启动时会把 dmesg 里已有的旧超时当成刚发生的事件。
last_js=-1
last_to=-1

while :; do
    line=$(take_sample)
    printf "%s\n" "$line" >> "$RING"
    tail -n "$KEEP" "$RING" > "$RING.tmp" && mv "$RING.tmp" "$RING"

    js=$(printf "%s\n" "$line" | sed -n 's/.* js=\([0-9]*\).*/\1/p')
    to=$(dmesg 2>/dev/null | grep -c "tx timeout")

    # 手柄掉线：这是「开始卡死」的那一刻
    if [ "$last_js" -ge 0 ] && [ "$js" -lt "$last_js" ]; then
        dump "drop" "手柄输入设备消失（js $last_js → $js）"
    fi

    # 出现新的 tx timeout：卡死第一次暴露
    if [ "$last_to" -ge 0 ] && [ "$to" -gt "$last_to" ]; then
        dump "wedge" "出现新的 tx timeout（$last_to → $to）"
    elif [ "$last_to" -ge 0 ] && [ "$to" -lt "$last_to" ]; then
        last_to=$to          # dmesg 环形缓冲翻页了，重新对齐基线
    fi

    last_js=$js
    last_to=$to
    sleep "$INTERVAL"
done
