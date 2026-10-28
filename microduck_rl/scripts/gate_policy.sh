#!/usr/bin/env bash
# 一分钟门槛：把一个 ONNX 丢进 MuJoCo（BAM M6 舵机模型）跑几秒，判它会不会走、会不会摔。
#
# 用法：  scripts/gate_policy.sh <policy.onnx> [前进命令 m/s, 默认 0.3] [秒数, 默认 4]
# 退出码：0 过；1 不过；2 用法或环境有问题。
#
# 为什么不直接用 --case forward：内置那条命令是 0.1 m/s，落在上游走路策略的死区里，
# 已知能走的策略跑出来是原地不动，看着像工具坏了。默认 0.3。
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
py=$root/.venv/bin/python

policy=${1:-}
cmd=${2:-0.3}
secs=${3:-4}

[ -n "$policy" ] || { echo "用法: $0 <policy.onnx> [命令 m/s] [秒数]" >&2; exit 2; }
policy=$(cd "$(dirname "$policy")" && pwd)/$(basename "$policy")
[ -r "$policy" ] || { echo "读不到策略: $policy" >&2; exit 2; }
[ -x "$py" ] || { echo "找不到 venv: $py" >&2; exit 2; }

out=${OUT_DIR:-/tmp/duck-gate}
mkdir -p "$out"
tag=$(basename "$policy" .onnx)-$(date +%H%M%S)
report=$out/$tag.json
trace=$out/$tag.csv
log=$out/$tag.log

echo "策略 $(sha256sum "$policy" | cut -c1-12)  命令 ${cmd} m/s  仿真 ${secs} s"
echo "报告 $report  轨迹 $trace"

( cd "$root" && "$py" scripts/replay_hd1910.py --policy "$policy" \
    --reference-policy --reference-action-alpha 0 --bam-reference \
    --case forward --case-lin-vel-x "$cmd" --seconds "$secs" \
    --report "$report" --trace "$trace" ) > "$log" 2>&1
rc=$?
if [ $rc -ne 0 ] || [ ! -s "$report" ] || [ ! -s "$trace" ]; then
  echo "✗ 回放没跑完（exit $rc），看 $log 末尾："
  tail -5 "$log" | sed 's/^/    /'
  exit 2
fi

"$py" - "$report" "$trace" <<'PY'
import csv, json, statistics, sys
report, trace = sys.argv[1], sys.argv[2]
case = json.load(open(report))["cases"][0]
rows = list(csv.DictReader(open(trace)))[len(list(csv.DictReader(open(trace)))) // 2:]

vx = case["mean_body_velocity_after_1s"][0]
fall = case["first_fall_s"]
tilt = case["max_tilt_deg"]
std = statistics.pstdev([float(r["target_left_knee_rad"]) for r in rows])
step = statistics.mean(abs(float(b["target_left_knee_rad"]) - float(a["target_left_knee_rad"]))
                       for a, b in zip(rows, rows[1:]))

checks = [
    (fall is None, f"不摔（{('满 ' + str(round(case['steps'] / 50, 1)) + ' s') if fall is None else str(round(fall, 2)) + ' s 摔了'}，峰值倾角 {tilt:.0f}°）"),
    (vx >= 0.15, f"真的前进（平均 {vx:+.2f} m/s，要求 ≥0.15）"),
    (std >= 0.2, f"真有步态（膝目标 std {std:.2f} rad，要求 ≥0.20）"),
]
for ok, text in checks:
    print(("  ✓ " if ok else "  ✗ ") + text)

passed = all(ok for ok, _ in checks)
print(("过" if passed else "不过") + f"   （膝目标每拍变化 {step:.3f} rad）")
sys.exit(0 if passed else 1)
PY
