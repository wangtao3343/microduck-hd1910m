#!/usr/bin/env python3
"""训练进度显示器 —— watch a training run from its TensorBoard events.

Big iteration counter, progress bar, ETA, and the handful of scalars that say whether the
gait is getting better. Read-only: it opens the run's `events.out.tfevents.*` and never
touches the trainer. When the trainer dies and `run_to_end.sh` (or a fresh launch) starts a
new run directory, the newest events file is picked up without a restart.

Two front ends, same data. `--serve` opens a window in the browser; the default is the
terminal one.

    microduck_rl/.venv/bin/python scripts/watch_training_progress.py --serve
    microduck_rl/.venv/bin/python scripts/watch_training_progress.py --logs ~/microduck_rl_retrain/logs

Run it with the training environment's interpreter, which is the one that has tensorboard.
Ctrl-C exits.
"""

import argparse
import glob
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

DEFAULT_TARGET = 8000  # iterations this run is aimed at

# What to watch: (scalar tag, label, higher is better)
WATCH = [
    ('Train/mean_reward', '总奖励', True),
    ('Metrics/twist/error_vel_xy', '速度跟踪误差 xy', False),
    ('Metrics/twist/error_vel_yaw', '角速度误差 yaw', False),
    ('Train/mean_episode_length', '回合长度', True),
    ('Episode_Termination/fell_over', '摔倒/回合', False),
    ('Metrics/slip_velocity_mean', '脚底滑移', False),
    ('Episode_Reward/hd_contact_frequency', '碎步惩罚', True),
    ('Loss/value', '价值损失', False),
    ('Perf/total_fps', '样本/秒', True),
]

GREEN, YELLOW, DIM, BOLD, RESET = '\033[32m', '\033[33m', '\033[2m', '\033[1m', '\033[0m'

# Seven-segment digits, 7 rows by 5 columns, one character per stroke. Drawn rather than
# hand-copied so the strokes always meet: hand-drawn block fonts made 3, 4 and 8 look alike
# on a real terminal.
SEGMENTS = {
    #      a      b      c      d      e      f      g
    '0': (True, True, True, True, True, True, False),
    '1': (False, True, True, False, False, False, False),
    '2': (True, True, False, True, True, False, True),
    '3': (True, True, True, True, False, False, True),
    '4': (False, True, True, False, False, True, True),
    '5': (True, False, True, True, False, True, True),
    '6': (True, False, True, True, True, True, True),
    '7': (True, True, True, False, False, False, False),
    '8': (True, True, True, True, True, True, True),
    '9': (True, True, True, True, False, True, True),
}
GLYPH_ROWS = 7
GLYPH_WIDTH = 5


def segment_glyph(ch):
    """A digit's seven-segment grid, or None when the character is not a digit."""
    seg = SEGMENTS.get(ch)
    if seg is None:
        return None
    a, b, c, d, e, f, g = seg
    on, off = '█', ' '
    return [
        ' ' + (on * 3 if a else off * 3) + ' ',
        (on if f else off) + '   ' + (on if b else off),
        (on if f else off) + '   ' + (on if b else off),
        ' ' + (on * 3 if g else off * 3) + ' ',
        (on if e else off) + '   ' + (on if c else off),
        (on if e else off) + '   ' + (on if c else off),
        ' ' + (on * 3 if d else off * 3) + ' ',
    ]


SYMBOLS = {
    '%': ['█   █', '   █ ', '  █  ', ' █   ', '█   █', '     ', '     '],
    '/': ['    █', '   █ ', '  █  ', ' █   ', '█    ', '     ', '     '],
    ' ': ['     '] * GLYPH_ROWS,
}


def big(text, scale=2):
    """Draw `text` as seven-segment block letters; `scale` widens each column."""
    rows = [''] * GLYPH_ROWS
    for index, ch in enumerate(text):
        glyph = segment_glyph(ch) or SYMBOLS.get(ch, SYMBOLS[' '])
        gap = ' ' * (2 * scale)
        for i in range(GLYPH_ROWS):
            rows[i] += ''.join(c * scale for c in glyph[i])
        if index < len(text) - 1:
            for i in range(GLYPH_ROWS):
                rows[i] += gap
    return [r.rstrip() for r in rows]


def newest_events(logs_dir):
    paths = glob.glob(os.path.join(logs_dir, 'rsl_rl', '*', '*', 'events.out.tfevents.*'))
    return max(paths, key=os.path.getmtime) if paths else None


def load_accumulator(path):
    from tensorboard.backend.event_processing.event_accumulator import EventAccumulator

    acc = EventAccumulator(path, size_guidance={'scalars': 0})
    acc.Reload()
    return acc


def value_at(acc, tag, step=None):
    """The value of `tag` at `step`, or at the closest earlier step."""
    if tag not in acc.Tags().get('scalars', []):
        return None
    scalars = acc.Scalars(tag)
    if not scalars:
        return None
    if step is None:
        return scalars[-1]
    for s in reversed(scalars):
        if s.step <= step:
            return s
    return None


def checkpoints(logs_dir, run_dir=None):
    """Every model_*.pt ordered by iteration; `run_dir` keeps it to the run being watched.

    Without the filter, an older run's model_1250.pt in the same logs tree reads as
    "the latest checkpoint" while the live run is still at iteration 74.
    """
    out = []
    if run_dir:
        pattern = os.path.join(run_dir, 'model_*.pt')
    else:
        pattern = os.path.join(logs_dir, 'rsl_rl', '*', '*', 'model_*.pt')
    for p in glob.glob(pattern):
        m = re.search(r'model_(\d+)\.pt$', p)
        if m:
            out.append((int(m.group(1)), p, os.path.getmtime(p)))
    return sorted(out)


def training_alive(needles=('.venv/bin/train', 'scripts/train_hd1910.py')):
    """Trainer processes, so a dead run does not look like a paused one."""
    alive = []
    for pid in os.listdir('/proc'):
        if not pid.isdigit():
            continue
        try:
            with open(f'/proc/{pid}/cmdline', 'rb') as fh:
                cmd = fh.read().replace(b'\0', b' ').decode(errors='replace')
        except OSError:
            continue
        if any(n in cmd for n in needles):
            elapsed = time.time() - os.stat(f'/proc/{pid}').st_mtime
            alive.append(f'pid {pid} · {elapsed / 60:.0f} 分钟')
    return alive


def gpu_line():
    try:
        out = subprocess.run(
            ['nvidia-smi', '--query-gpu=memory.used,memory.total,utilization.gpu',
             '--format=csv,noheader,nounits'],
            capture_output=True, text=True, timeout=3,
        ).stdout.strip().splitlines()
        if out:
            used, total, util = [x.strip() for x in out[0].split(',')]
            return f'GPU {used}/{total} MiB · {util}%'
    except Exception:
        pass
    return 'GPU 未知'


def bar(frac, width):
    frac = max(0.0, min(1.0, frac))
    filled = int(frac * width)
    return '█' * filled + '░' * (width - filled)


def fmt(v):
    if v is None:
        return '—'
    a = abs(v)
    if a == 0:
        return '0'
    if a < 0.001 or a >= 10000:
        return f'{v:.2e}'
    if a < 1:
        return f'{v:.4f}'
    if a < 100:
        return f'{v:.3f}'
    return f'{v:.1f}'


def human_duration(seconds):
    seconds = int(max(0, seconds))
    h, m = divmod(seconds // 60, 60)
    return f'{h} 小时 {m:02d} 分' if h else f'{m} 分'


def layout(it, pct, scale, width):
    """Pick the largest size that fits: requested scale, then 2, then 1, then drop `%`."""
    for s in (scale, 2, 1):
        left = big(str(it), s)
        right = big(pct, s)
        if len(left[0]) + len(right[0]) + 4 <= width:
            return left, right
        if len(left[0]) + 2 <= width:
            return left, None
    return big(str(it), 1), None


def render(logs_dir, target, scale=2):
    width = shutil.get_terminal_size((100, 40)).columns
    lines = []
    now = time.time()
    path = newest_events(logs_dir)
    lines.append(f'{BOLD}训练进度{RESET}  {time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(now))}')
    if not path:
        lines.append(f'  {logs_dir}/rsl_rl 下没有 events 文件')
        return '\n'.join(lines)

    acc = load_accumulator(path)
    lines.append(f'{DIM}  run  {os.path.basename(os.path.dirname(path))}{RESET}')

    latest = value_at(acc, 'Train/mean_reward')
    it = latest.step if latest else 0

    # Seconds per iteration over the last ~100 iterations, so startup does not skew it.
    speed = None
    if latest and it > 0:
        ref = None
        for cand in range(it - 100, it):
            probe = value_at(acc, 'Train/mean_reward', cand)
            if probe is not None and probe.step < it:
                ref = probe
                break
        if ref is not None and ref.step < it:
            speed = (latest.wall_time - ref.wall_time) / (it - ref.step)

    frac = it / target if target else 0.0
    finished = it >= target
    pct = f'{min(100, int(frac * 100))}%'

    left, right = layout(it, pct, scale, width)
    color = GREEN if finished else BOLD
    if right is not None:
        pad = max(2, width - (len(left[0]) + len(right[0])))
        for l, r in zip(left, right):
            lines.append(f'  {color}{l}{RESET}{" " * pad}{color}{r}{RESET}')
    else:
        for l in left:
            lines.append(f'  {color}{l}{RESET}')

    lines.append('')
    bar_width = max(20, min(60, width - 22))
    lines.append(f'  {color}{bar(frac, bar_width)}{RESET}  {it} / {target}   {frac * 100:5.1f}%')
    if finished:
        lines.append(f'  {GREEN}已经跑满 {target} 次迭代{RESET}')
    elif speed:
        eta = time.strftime('%H:%M', time.localtime(now + (target - it) * speed))
        lines.append(
            f'  {speed:.2f} s/迭代 · 剩余 {human_duration((target - it) * speed)} · 预计 {eta} 完成'
        )
    else:
        lines.append('  （还在热身，速度待测量）')

    ckpts = checkpoints(logs_dir, os.path.dirname(path) if path else None)
    if ckpts:
        last_it, _, last_mtime = ckpts[-1]
        lines.append(
            f'  {DIM}最近存档 model_{last_it}.pt'
            f'  {time.strftime("%H:%M", time.localtime(last_mtime))}'
            f'  （共 {len(ckpts)} 份）{RESET}'
        )

    lines.append('')
    lines.append(f'  {BOLD}{"指标":<26}{"当前":>11}{"100 次前":>12}   趋势{RESET}')
    lines.append(f'  {DIM}{"-" * 58}{RESET}')
    for tag, label, higher_is_better in WATCH:
        cur = value_at(acc, tag)
        if cur is None:
            continue
        prev = value_at(acc, tag, max(0, cur.step - 100))
        trend, mark = '', ''
        if prev is not None and prev.step != cur.step:
            delta = cur.value - prev.value
            if abs(delta) > 1e-9:
                improving = delta > 0 if higher_is_better else delta < 0
                trend = '↑' if delta > 0 else '↓'
                mark = GREEN if improving else YELLOW
        lines.append(
            f'  {label:<24}{fmt(cur.value):>12}{fmt(prev.value if prev else None):>12}'
            f'   {mark}{trend}{RESET}'
        )

    lines.append('')
    procs = training_alive()
    if procs:
        lines.append(f'  {GREEN}● 训练进程在跑{RESET}  {"  ".join(procs)}')
    else:
        lines.append(f'  {YELLOW}● 没有训练进程{RESET} —— 跑完了，还是挂了？看 run_to_end.log')
    lines.append(f'  {DIM}{gpu_line()}{RESET}')
    return '\n'.join(lines)


_CACHE = {'path': None, 'acc': None}


def snapshot(logs_dir, target):
    """One JSON-friendly picture of the run; the web page renders this and nothing else."""
    path = newest_events(logs_dir)
    now = time.time()
    out = {
        'clock': time.strftime('%H:%M:%S', time.localtime(now)),
        'found': bool(path),
        'run': None,
        'iter': 0,
        'target': target,
        'pct': 0.0,
        'speed': None,
        'remaining': None,
        'eta': None,
        'finished': False,
        'ckpt': None,
        'ckpt_count': 0,
        'metrics': [],
        'alive': training_alive(),
        'gpu': gpu_line(),
    }
    if not path:
        return out

    # Reload() only reads the bytes appended since the last call, so this stays cheap even
    # with a multi-megabyte events file.
    if _CACHE['path'] != path:
        _CACHE['path'] = path
        _CACHE['acc'] = load_accumulator(path)
    acc = _CACHE['acc']
    acc.Reload()

    latest = value_at(acc, 'Train/mean_reward')
    it = latest.step if latest else 0
    out['run'] = os.path.basename(os.path.dirname(path))
    out['iter'] = it
    out['pct'] = min(100.0, it / target * 100) if target else 0.0
    out['finished'] = it >= target

    if latest and it > 0:
        ref = None
        for cand in range(it - 100, it):
            probe = value_at(acc, 'Train/mean_reward', cand)
            if probe is not None and probe.step < it:
                ref = probe
                break
        if ref is not None and ref.step < it:
            speed = (latest.wall_time - ref.wall_time) / (it - ref.step)
            out['speed'] = round(speed, 2)
            out['remaining'] = human_duration((target - it) * speed)
            out['eta'] = time.strftime('%H:%M', time.localtime(now + (target - it) * speed))

    ckpts = checkpoints(logs_dir, os.path.dirname(path))
    if ckpts:
        last_it, _, _ = ckpts[-1]
        out['ckpt'] = last_it
        out['ckpt_count'] = len(ckpts)

    for tag, label, higher_is_better in WATCH:
        cur = value_at(acc, tag)
        if cur is None:
            continue
        prev = value_at(acc, tag, max(0, cur.step - 100))
        change = None
        if prev is not None and prev.step != cur.step:
            delta = cur.value - prev.value
            if abs(delta) > 1e-9:
                change = 'up' if delta > 0 else 'down'
                good = delta > 0 if higher_is_better else delta < 0
                change = f'{change}-{"good" if good else "bad"}'
        out['metrics'].append({
            'label': label,
            'now': fmt(cur.value),
            'prev': fmt(prev.value if prev is not None else None),
            'change': change,
        })
    return out


PAGE = r"""<!doctype html>
<meta charset="utf-8">
<title>训练进度显示器</title>
<style>
  :root {
    --bg: #0e1116; --card: #161a22; --line: #232936;
    --text: #e8ebf2; --dim: #7d8699; --good: #46d18a; --bad: #e8804a; --accent: #6ea8fe;
  }
  * { box-sizing: border-box; }
  body {
    margin: 0; min-height: 100vh; background: var(--bg); color: var(--text);
    font-family: "Ubuntu", "Noto Sans CJK SC", "PingFang SC", system-ui, sans-serif;
    padding: 28px 32px 24px;
  }
  .top { display: flex; justify-content: space-between; align-items: baseline; }
  .title { font-size: 15px; letter-spacing: .22em; color: var(--dim); }
  .clock { font-variant-numeric: tabular-nums; color: var(--dim); font-size: 15px; }
  .run { margin-top: 6px; color: var(--dim); font-size: 13px;
         font-family: ui-monospace, "DejaVu Sans Mono", monospace; }
  .hero { display: flex; align-items: flex-end; gap: 28px; margin: 10px 0 4px; flex-wrap: wrap; }
  .iter { font-size: clamp(72px, 15vw, 220px); line-height: .9; font-weight: 700;
          font-variant-numeric: tabular-nums; letter-spacing: -.03em; }
  .iter.finished { color: var(--good); }
  .of { font-size: 22px; color: var(--dim); padding-bottom: 12px;
        font-variant-numeric: tabular-nums; }
  .pct { margin-left: auto; font-size: clamp(40px, 7vw, 96px); font-weight: 600;
         color: var(--accent); font-variant-numeric: tabular-nums; line-height: 1; }
  .track { height: 14px; background: var(--card); border: 1px solid var(--line);
           border-radius: 99px; overflow: hidden; margin: 18px 0 14px; }
  .fill { height: 100%; width: 0; border-radius: 99px;
          background: linear-gradient(90deg, #3b82f6, #46d18a); transition: width .6s ease; }
  .stats { display: flex; gap: 26px; flex-wrap: wrap; color: var(--dim); font-size: 15px;
           font-variant-numeric: tabular-nums; }
  .stats b { color: var(--text); font-weight: 600; }
  .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
          gap: 10px; margin-top: 22px; }
  .row { display: flex; align-items: baseline; gap: 10px; background: var(--card);
         border: 1px solid var(--line); border-radius: 10px; padding: 10px 14px; }
  .label { color: var(--dim); font-size: 14px; flex: 1; }
  .val { font-variant-numeric: tabular-nums; font-size: 19px; font-weight: 600; }
  .prev { color: var(--dim); font-size: 13px; font-variant-numeric: tabular-nums;
          min-width: 62px; text-align: right; }
  .arrow { font-size: 15px; width: 16px; text-align: center; }
  .up-good, .down-bad { color: var(--good); }
  .up-bad, .down-good { color: var(--bad); }
  .foot { display: flex; gap: 22px; align-items: center; margin-top: 22px;
          color: var(--dim); font-size: 14px; flex-wrap: wrap; }
  .dot { width: 9px; height: 9px; border-radius: 99px; background: var(--good);
         display: inline-block; margin-right: 8px; }
  .dot.dead { background: var(--bad); }
</style>
<div class="top"><div class="title">训练进度显示器</div><div class="clock" id="clock"></div></div>
<div class="run" id="run"></div>
<div class="hero">
  <div class="iter" id="iter">—</div><div class="of" id="of"></div>
  <div class="pct" id="pct"></div>
</div>
<div class="track"><div class="fill" id="fill"></div></div>
<div class="stats" id="stats"></div>
<div class="grid" id="grid"></div>
<div class="foot" id="foot"></div>
<script>
const ARROW = {up: '↑', down: '↓'};
async function tick() {
  let d;
  try { d = await (await fetch('/data', {cache: 'no-store'})).json(); }
  catch (e) { document.getElementById('foot').textContent = '连不上看板进程了'; return; }
  document.getElementById('clock').textContent = d.clock;
  document.getElementById('run').textContent = d.run ? 'run  ' + d.run : '没有找到训练记录';
  const iter = document.getElementById('iter');
  iter.textContent = d.iter || 0;
  iter.classList.toggle('finished', d.finished);
  document.getElementById('of').textContent = '/ ' + d.target;
  document.getElementById('pct').textContent = d.pct.toFixed(1) + '%';
  document.getElementById('fill').style.width = d.pct + '%';

  const stats = [];
  if (d.speed) stats.push('每轮 <b>' + d.speed.toFixed(2) + ' s</b>');
  if (d.remaining) stats.push('剩余 <b>' + d.remaining + '</b>');
  if (d.eta) stats.push('预计 <b>' + d.eta + '</b> 跑完');
  if (d.ckpt !== null) stats.push('最近存档 <b>model_' + d.ckpt + '</b>（共 ' + d.ckpt_count + ' 份）');
  document.getElementById('stats').innerHTML = stats.join('');

  document.getElementById('grid').innerHTML = d.metrics.map(m => {
    const arrow = m.change ? ARROW[m.change.split('-')[0]] : '';
    const cls = m.change || '';
    return '<div class="row"><span class="label">' + m.label + '</span>' +
           '<span class="prev">' + m.prev + '</span>' +
           '<span class="val">' + m.now + '</span>' +
           '<span class="arrow ' + cls + '">' + arrow + '</span></div>';
  }).join('');

  const alive = d.alive.length;
  document.getElementById('foot').innerHTML =
    '<span><span class="dot' + (alive ? '' : ' dead') + '"></span>' +
    (alive ? '训练进程在跑 · ' + d.alive.join('  ') : '没有训练进程 —— 跑完了，还是挂了？') +
    '</span><span>' + d.gpu + '</span>';
}
tick(); setInterval(tick, 1000);
</script>
"""



def run_gui(logs_dir, target, interval=1.0):
    """A native window. tkinter only, so the only requirement is python3-tk."""
    import tkinter as tk
    from tkinter import font as tkfont

    bg, card, line = '#0e1116', '#161a22', '#202633'
    text, dim = '#e8ebf2', '#7d8699'
    good, bad, accent = '#46d18a', '#e8804a', '#6ea8fe'

    root = tk.Tk()
    root.title('训练进度显示器')
    root.configure(bg=bg)
    root.geometry('1280x900')
    root.minsize(760, 560)

    families = set(tkfont.families(root))
    ui = next((f for f in ('Noto Sans CJK SC', 'Noto Sans SC', 'Droid Sans Fallback')
               if f in families), 'TkDefaultFont')
    mono = next((f for f in ('DejaVu Sans Mono', 'Noto Sans Mono CJK SC', 'Liberation Mono')
                 if f in families), 'TkFixedFont')

    big_font = tkfont.Font(root=root, family=mono, size=120, weight='bold')
    digits = {'iter': '0'}

    def text_label(parent, message='', size=12, color=dim, family=ui, weight='normal'):
        return tk.Label(parent, text=message, bg=parent['bg'], fg=color,
                        font=(family, size, weight), anchor='w')

    def card_widget(parent, label):
        frame = tk.Frame(parent, bg=card, highlightbackground=line, highlightthickness=1)
        frame.grid(sticky='nsew')
        name = tk.Label(frame, text=label, bg=card, fg=dim, font=(ui, 11), anchor='w')
        name.pack(fill='x', padx=12, pady=(7, 0))
        value = tk.Label(frame, text='—', bg=card, fg=text, font=(mono, 16, 'bold'), anchor='w')
        value.pack(fill='x', padx=12)
        foot = tk.Frame(frame, bg=card)
        foot.pack(fill='x', padx=12, pady=(0, 7))
        prev = tk.Label(foot, text='', bg=card, fg=dim, font=(mono, 10), anchor='w')
        prev.pack(side='left')
        arrow = tk.Label(foot, text='', bg=card, fg=dim, font=(ui, 12), anchor='e')
        arrow.pack(side='right')
        return value, prev, arrow

    header = tk.Frame(root, bg=bg)
    header.pack(fill='x', padx=22, pady=(18, 0))
    tk.Label(header, text='训练进度显示器', bg=bg, fg=dim,
             font=(ui, 11), anchor='w').pack(side='left')
    clock = text_label(header, size=11)
    clock.pack(side='right')

    run_label = text_label(root, size=11, family=mono)
    run_label.pack(fill='x', padx=22)

    hero = tk.Frame(root, bg=bg)
    hero.pack(fill='x', padx=22, pady=(2, 0))
    iter_label = tk.Label(hero, text='—', bg=bg, fg=text, font=big_font, anchor='w')
    iter_label.pack(side='left')
    of_label = text_label(hero, size=15)
    of_label.pack(side='left', padx=(14, 0), pady=(0, 14))
    pct_label = tk.Label(hero, text='', bg=bg, fg=accent,
                         font=(mono, 40, 'bold'), anchor='e')
    pct_label.pack(side='right', padx=(0, 6))

    bar = tk.Frame(root, height=16, bg=card, highlightthickness=1,
                   highlightbackground=line, bd=0)
    bar.pack(fill='x', padx=22, pady=(10, 12))
    bar.pack_propagate(False)
    bar_fill = tk.Frame(bar, bg=accent)
    bar_fill.place(x=0, y=0, relwidth=0.0, relheight=1.0)

    stats = text_label(root, size=12)
    stats.pack(fill='x', padx=22)

    grid = tk.Frame(root, bg=bg)
    grid.pack(fill='both', expand=True, padx=16, pady=(10, 2))
    for column in range(3):
        grid.columnconfigure(column, weight=1, uniform='cards')
    for row in range(3):
        grid.rowconfigure(row, weight=1, uniform='cards')
    cards = {}
    for index, (_, label, _) in enumerate(WATCH):
        cards[label] = card_widget(grid, label)
        cards[label][0].master.grid(row=index // 3, column=index % 3,
                                    padx=4, pady=4, sticky='nsew')

    footer = tk.Frame(root, bg=bg)
    footer.pack(fill='x', padx=22, pady=(0, 16))
    status = text_label(footer, size=11)
    status.pack(side='left')
    gpu = text_label(footer, size=11)
    gpu.pack(side='right')

    def fit_big(_event=None):
        # 宽度留出右边百分比的地方，高度不超过窗口的三成，否则卡片和底栏会被挤出屏幕
        available_w = max(200, root.winfo_width() * 0.55)
        available_h = max(90, root.winfo_height() * 0.30)
        for size in range(300, 39, -6):
            big_font.configure(size=size)
            if big_font.measure('0' * len(digits['iter'])) <= available_w and size * 1.4 <= available_h:
                return

    root.bind('<Configure>', fit_big)

    def tick():
        snap = snapshot(logs_dir, target)
        digits['iter'] = str(snap['iter'])
        iter_label.config(text=digits['iter'])
        clock.config(text=snap['clock'])
        run_label.config(text='run  ' + snap['run'] if snap['run'] else '没有找到训练记录')
        of_label.config(text='/ ' + str(target))
        pct_label.config(text=f"{snap['pct']:.1f}%")
        iter_label.config(fg=good if snap['finished'] else text)

        bar_fill.place_configure(relwidth=max(0.0, min(1.0, snap['pct'] / 100)))
        bar_fill.config(bg=good if snap['finished'] else accent)

        bits = []
        if snap['speed']:
            bits.append(f"每轮 {snap['speed']:.2f} s")
        if snap['remaining']:
            bits.append(f"剩余 {snap['remaining']}")
        if snap['eta']:
            bits.append(f"预计 {snap['eta']} 跑完")
        if snap['ckpt'] is not None:
            bits.append(f"最近存档 model_{snap['ckpt']}（共 {snap['ckpt_count']} 份）")
        stats.config(text='   ·   '.join(bits))

        seen = set()
        for metric in snap['metrics']:
            value, prev, arrow = cards[metric['label']]
            seen.add(metric['label'])
            value.config(text=metric['now'])
            prev.config(text=f"100 次前 {metric['prev']}")
            change = metric['change'] or ''
            if change.startswith('up'):
                arrow.config(text='↑', fg=good if change.endswith('good') else bad)
            elif change.startswith('down'):
                arrow.config(text='↓', fg=good if change.endswith('good') else bad)
            else:
                arrow.config(text='', fg=dim)
        for label, (value, prev, arrow) in cards.items():
            if label not in seen:
                value.config(text='—')

        if snap['alive']:
            status.config(text='● 训练进程在跑   ' + '  '.join(snap['alive']), fg=good)
        else:
            status.config(text='● 没有训练进程 —— 跑完了，还是挂了？', fg=bad)
        gpu.config(text=snap['gpu'])

        fit_big()
        root.after(int(interval * 1000), tick)

    tick()
    root.mainloop()
    return 0


def serve(logs_dir, target, port=8765):
    """Serve the dashboard on localhost and open it in the browser."""
    import http.server
    import socketserver
    import webbrowser

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path.startswith('/data'):
                body = json.dumps(snapshot(logs_dir, target)).encode()
                kind = 'application/json'
            else:
                body = PAGE.encode()
                kind = 'text/html; charset=utf-8'
            self.send_response(200)
            self.send_header('Content-Type', kind)
            self.send_header('Content-Length', str(len(body)))
            self.send_header('Cache-Control', 'no-store')
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    for attempt in range(20):
        try:
            httpd = socketserver.ThreadingTCPServer(('127.0.0.1', port), Handler)
            break
        except OSError:
            port += 1
    else:
        raise SystemExit('no free port near 8765')

    url = f'http://127.0.0.1:{port}/'
    print(f'dashboard: {url}', flush=True)
    threading.Timer(0.6, lambda: webbrowser.open(url)).start()
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        httpd.server_close()


def main():
    ap = argparse.ArgumentParser(description='Terminal dashboard for a training run')
    ap.add_argument('--logs', default=str(ROOT / 'microduck_rl' / 'logs'),
                    help='logs directory holding rsl_rl/<experiment>/<run>/')
    ap.add_argument('--interval', type=float, default=10, help='refresh seconds (default 10)')
    ap.add_argument('--target', type=int, default=DEFAULT_TARGET,
                    help=f'iterations this run aims at (default {DEFAULT_TARGET})')
    ap.add_argument('--scale', type=int, default=2, help='big-digit scale, 1-3 (default 2)')
    ap.add_argument('--once', action='store_true', help='print once and exit')
    ap.add_argument('--gui', action='store_true', help='native window (tkinter)')
    ap.add_argument('--serve', action='store_true', help='serve the dashboard page and open a browser')
    ap.add_argument('--port', type=int, default=8765, help='port for --serve (default 8765)')
    args = ap.parse_args()

    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    if args.gui:
        return run_gui(args.logs, args.target, args.interval)
    if args.serve:
        serve(args.logs, args.target, args.port)
        return 0
    try:
        while True:
            text = render(args.logs, args.target, args.scale)
            if args.once:
                print(text)
                return 0
            sys.stdout.write('\033[H\033[2J' + text + '\n')
            sys.stdout.flush()
            time.sleep(args.interval)
    except KeyboardInterrupt:
        return 0


if __name__ == '__main__':
    raise SystemExit(main())
