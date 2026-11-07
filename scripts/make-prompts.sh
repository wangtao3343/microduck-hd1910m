#!/usr/bin/env bash
# Generate the fixed Chinese spoken prompts robotd plays for operator-facing events.
#
# These are *generated data*, not checked in: edge-tts needs the network, and the wavs
# belong on the robot (/var/lib/robot/prompts), not in the release tree. The file names
# below are the lookup keys — keep them in sync with the `play_prompt` call sites in
# microduck/robotd/src/main.rs.
#
# Voice: zh-CN-XiaoxiaoNeural. A female voice sits above the duck speaker's ~300 Hz
# roll-off; the high-pass and loudness normalisation are for that same driver.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "$0")/.." && pwd)"
OUT="${1:-$ROOT/out/prompts}"
VOICE="${VOICE:-zh-CN-XiaoxiaoNeural}"
RATE="${RATE:-+0%}"

command -v ffmpeg >/dev/null || { echo 'ffmpeg is required' >&2; exit 1; }
if command -v edge-tts >/dev/null; then
  TTS=(edge-tts)
elif command -v uvx >/dev/null; then
  TTS=(uvx --from edge-tts edge-tts)
else
  echo 'edge-tts not found; install it, or install uv (https://docs.astral.sh/uv/)' >&2
  exit 1
fi

mkdir -p "$OUT"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# name|sentence
while IFS='|' read -r name text; do
  [ -n "$name" ] || continue
  raw="$work/$name.mp3"
  # stdin is the heredoc: a child that reads it (uvx has) eats the next entry's first
  # byte, so both commands get /dev/null explicitly.
  "${TTS[@]}" --voice "$VOICE" --rate="$RATE" --text "$text" --write-media "$raw" </dev/null
  ffmpeg -y -v error -i "$raw" \
    -af "highpass=f=180,loudnorm=I=-16:TP=-1.5:LRA=11" \
    -ar 48000 -ac 1 -c:a pcm_s16le "$OUT/$name.wav" </dev/null
  printf '%-14s %s\n' "$name" "$text"
done <<'EOF'
boot|开机完成
ready|准备
policy_enabled|解锁
policy_paused|锁定
relax|我累了，休息一下
battery_low|电量低
battery_critical|我快要噶了，立即给我充电
fault|伺服故障，请检查
fault_reset|伺服故障，已复位，请按开始键继续
shutdown|正在关机
battery_empty|电量耗尽，即将关机
EOF

echo "wrote $(find "$OUT" -name '*.wav' | wc -l) prompts to $OUT"
echo "install: scp \"$OUT\"/*.wav duck@<robot>:/var/lib/robot/prompts/"
