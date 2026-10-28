#!/usr/bin/env bash
# 重启板子上的运行时（robotd + 手柄客户端 padd），重启前先核对策略文件都在。
#
# 用法（板上，root）：  sudo duck-restart
# 或者从仓库跑：        sudo bash scripts/restart_hardware.sh
#
# 退出码：0 起来了；1 预检没过或服务没起来。
set -uo pipefail

SERVICE=${SERVICE:-microduck-hd1910.service}
PAD=${PAD:-padd.service}
ROOT=${ROOT:-/home/duck/workspace/microduck-hd1910m}
PARAMS=${PARAMS:-$ROOT/local/params.toml}

log() { printf '%s  %s\n' "$(date +%H:%M:%S)" "$*"; }
die() { log "✗ $*"; exit 1; }

[ "$(id -u)" -eq 0 ] || die "要以 root 运行（sudo $0）"
[ -r "$PARAMS" ] || die "读不到 $PARAMS"

# 1. 预检：params.toml 里引用的每个 .onnx 都得在。
#    路径写错的话 robotd 起得来但加载不了策略，鸭子会静默退回 limp——
#    与其等下次开机才发现，不如在重启前挡住。
missing=0
while IFS= read -r f; do
  if [ -e "$f" ]; then
    log "✓ ${f#"$ROOT"/}"
  else
    log "✗ 缺失 $f"
    missing=1
  fi
done < <(grep -oE '"/[^"]*\.onnx"' "$PARAMS" | tr -d '"' | sort -u)
[ "$missing" -eq 0 ] || die "有策略文件缺失，先修好再重启"

# 2. 先停手柄：它会跟着 robotd 一起断，停掉能少一条 Broken pipe 的噪音。
log "停止 $PAD"
systemctl stop "$PAD" 2>/dev/null

# 3. 重启运行时，然后只看这次启动之后写的日志。
#    用 journal 游标而不是 "--since 时间戳"：这颗板子没有 RTC，开机后 timesync
#    会把时钟猛拽一下，按时间切的窗口可能整段是空的。
CURSOR=$(journalctl -u "$SERVICE" -n 0 --show-cursor -o cat 2>/dev/null | sed -n 's/^-- cursor: //p')
svc_log() {
  if [ -n "$CURSOR" ]; then
    journalctl -u "$SERVICE" --after-cursor "$CURSOR" --no-pager 2>/dev/null
  else
    journalctl -u "$SERVICE" -n 200 --no-pager 2>/dev/null
  fi
}

log "重启 $SERVICE"
systemctl restart "$SERVICE" || die "systemctl restart 失败"

ok=0
for _ in $(seq 1 30); do
  systemctl is-active --quiet "$SERVICE" || die "服务没起来，看 journalctl -u $SERVICE -n 50"
  if svc_log | grep -q "control loop running"; then
    ok=1
    break
  fi
  sleep 1
done
[ "$ok" -eq 1 ] || die "30 秒内没看到 control loop running，看 journalctl -u $SERVICE -n 50"

# 4. 起手柄，把关键几行贴出来。
systemctl start "$PAD"
sleep 2

log "状态：$SERVICE=$(systemctl is-active "$SERVICE")  $PAD=$(systemctl is-active "$PAD")"
svc_log | grep -E "policy loaded|control loop running|seated boot|Reference role|hash mismatch|error=" \
  | tail -5 | sed 's/^/    /'
journalctl -u "$PAD" -n 40 --no-pager 2>/dev/null \
  | grep -E "pad connected|button bindings" | tail -2 | sed 's/^/    /'

# 5. 加载失败要显式报错：robotd 起得来但没有策略，就是 limp。
if ! svc_log | grep -q "policy loaded"; then
  die "服务在跑，但没看到 policy loaded —— 策略没加载，机器人不会动"
fi
log "完成"
