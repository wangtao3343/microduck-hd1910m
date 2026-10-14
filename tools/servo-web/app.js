// 跟 server.py 的 VERSION 一起改。连上后比对，不一致说明浏览器拿的是旧页面
const PAGE_VERSION = "0.15.9";
const $ = id => document.getElementById(id);
const v = id => Number($(id).value);
let ws, names = {}, present = [], ids = [], lastDump = null, sendTimer = {}, lastSent = {};
const rawPos = {}, goalPos = {};
let dirs = {};   // 每颗 +1 / -1，来自服务端 directions.json，所有页面共用
let isServoDemo = false;
// The module can load after hello/the first frame: retain the latest state for it.
const imuFeed = { enabled:false, connected:false, packet:null, revision:0 };
function announceImu() { imuFeed.revision++; window.dispatchEvent(new Event('imu-feed')); }

function connect() {
  ws = new WebSocket((location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws');
  ws.onopen = () => { $('dot').classList.add('on'); $('conn').textContent = '已连接'; };
  ws.onclose = () => { imuFeed.connected=false; announceImu(); $('dot').classList.remove('on'); $('conn').textContent = '断开，3 秒后重连'; pushLog({ t: new Date().toTimeString().slice(0, 8), cat: '页面', level: 'warn', msg: '跟服务的连接断了，3 秒后重连' }); setTimeout(connect, 3000); };
  ws.onmessage = e => {
    const m = JSON.parse(e.data);
    if (m.type === 'hello') {
      imuFeed.enabled=m.imu_enabled === true; imuFeed.connected=true; imuFeed.packet=null; announceImu();
      showSimImu(m.sim_imu);
      const sv = m.version || '旧版', same = sv === PAGE_VERSION;
      $('ver').textContent = same ? `v${PAGE_VERSION}` : `页面 v${PAGE_VERSION} · 服务 v${sv} 不一致`;
      $('ver').style.color = same ? '' : 'var(--bad)';
      $('ver').title = same ? '页面和服务版本一致' : '浏览器里是旧页面或服务没重启：先重启 server.py，再 Ctrl+F5';
      if (!same) hint(`页面 v${PAGE_VERSION} 跟服务 v${sv} 不一致：重启 server.py 后按 Ctrl+F5`);
      dirs = {}; for (const [k, v] of Object.entries(m.dirs || {})) dirs[k] = v; names = m.names; ids = m.ids; present = m.present;  REGS = (m.regs || []).filter(r => r[3] === 'EEPROM' || r[3] === 'SRAM'); showPorts(m); fillLogCats(m.cats || []); build(); fillRegSelect(); (m.logs || []).forEach(pushLog); send({ op: 'poses' }); }
    else if (m.type === 'imu') { if (imuFeed.enabled) { imuFeed.packet=m; announceImu(); } }
    else if (m.type === 'imu_config') { imuFeed.enabled = m.enabled === true; imuFeed.packet = null; announceImu(); showSimImu(m.sim_imu); }
    else if (m.type === 'ports') showPorts(m);
    else if (m.type === 'reconnect') {
      showPorts(m);
      if (m.ok) { present = m.present || []; build(); hint(m.msg); }
      else hint(m.msg);
    }
    else if (m.type === 'poses') renderPoses(m.poses);
    else if (m.type === 'dirs') { dirs = {}; for (const [k, v] of Object.entries(m.dirs || {})) dirs[k] = v; }
    else if (m.type === 'calib_result') showCalibResult(m);
    else if (m.type === 'calib_plan') confirmCalib(m);
    else if (m.type === 'torque') showTorqueResult(m);
    else if (m.type === 'goals_verify') { if (m.missing.length) hint(`ID ${m.missing.join(',')} 没收到目标位置，总线丢包，再发一次`); else if (m.attempts > 1) addLog(`目标写入：重发 ${m.attempts} 轮才全到`, '运动', 'warn'); }
    else if (m.type === 'state') update(m.states, m.stats);
    else if (m.type === 'scan') { present = m.present; build(); }
    else if (m.type === 'dump') showDump(m);
    else if (m.type === 'timing') { $('timing').textContent = `n=${m.n} ids=${m.ids} min ${m.min_ms} ms · avg ${m.avg_ms} ms · p99 ${m.p99_ms} ms · max ${m.max_ms} ms · 失败 ${m.fails}`; }
    else if (m.type === 'logs') m.items.forEach(pushLog);
    else if (m.type === 'error') hint('出错：' + m.msg + '（详情看日志）');
  };
}
// 模拟 IMU 下拉：sim_imu 为 null 说明 IMU 是启动参数给的（或接了真串口），页面里不改
function showSimImu(mode) {
  const sel = $('simImu');
  sel.disabled = !mode;
  if (mode) sel.value = mode;
  sel.title = mode ? '' : '服务启动时用参数指定了 IMU，页面里不改';
}
function send(o) { if (ws && ws.readyState === 1) ws.send(JSON.stringify(o)); }
// 串口：没连上时顶栏出现下拉 + 「连接」；连上了只显示口号。拔插 USB 后口号会变，点一下就能重连
function showPorts(m) {
  if (m.fake === undefined) return;
  isServoDemo = m.fake === true;
  const box = $('portBox'), sel = $('portSel');
  $('simPanel').hidden = !m.fake;
  $('busInfo').textContent = m.fake ? '模拟：没连串口，舵机是虚拟的' : `串口 ${m.port}`;
  $('busInfo').style.color = m.fake ? 'var(--warn)' : '';
  box.style.display = '';
  if (m.ports) {
    const keep = sel.value; sel.innerHTML = '';
    (m.ports.length ? m.ports : [{ device: '', desc: '没找到串口，插上 USB 再点刷新' }]).forEach(p => {
      const o = document.createElement('option'); o.value = p.device;
      o.textContent = p.device + (p.desc ? ` · ${p.desc}` : ''); sel.appendChild(o);
    });
    const opts = [...sel.options].map(o => o.value);
    if (opts.includes(keep)) sel.value = keep; else if (m.port && opts.includes(m.port)) sel.value = m.port;
  }
}
function reconnect() {
  const port = $('portSel').value;
  if (!port) { send({ op: 'ports' }); hint('先插上 USB，再点一次'); return; }
  hint(`正在打开 ${port}…`);
  send({ op: 'reconnect', port });
}
let hintTimer;
// 右侧浮层的三个 tab：官方姿势 / 校准 / 日志。纯显示开关 —— 切走的那块只是 hidden，
// 里面的滑块、日志、幽灵表照常由各自的老代码管，连读带写都不受影响。
const TABS = { pose: 'tabPose', calib: 'tabCalib', log: 'tabLog' };
function showTab(name) {
  for (const k in TABS) $(TABS[k]).hidden = (k !== name);
  document.querySelectorAll('#tabBar .tab').forEach(b => b.classList.toggle('on', b.dataset.tab === name));
}

function hint(text) { const h = $('hint'); h.textContent = text; h.classList.add('show'); clearTimeout(hintTimer); hintTimer = setTimeout(() => h.classList.remove('show'), 5000); }
// ---- 日志 ----
// 服务端每条日志带 {n, t, cat, level, msg}，按 n 去重；页面自己的日志（站姿、序列、灵动…）发给服务端记进文件再推回来，
// 这样日志文件里是完整的时间线。连接断了才只显示在本地。
const LOG_ITEMS = []; let logLastN = 0, logWarnN = 0, logErrN = 0;
const LEVEL_TXT = { warn: '警告', error: '错误' };
function addLog(msg, cat = '页面', level = 'info') {
  if (ws && ws.readyState === 1) send({ op: 'page_log', msg: String(msg), cat, level });
  else pushLog({ t: new Date().toTimeString().slice(0, 8), cat, level, msg: String(msg) });
}
function pushLog(e) {
  if (e.n) { if (e.n <= logLastN) return; logLastN = e.n; }
  LOG_ITEMS.push(e); if (LOG_ITEMS.length > 1000) LOG_ITEMS.shift();
  if (e.level === 'warn') logWarnN++; if (e.level === 'error') logErrN++;
  showLogBadges();
  if (logMatch(e)) { const l = $('log'), atEnd = l.scrollHeight - l.scrollTop - l.clientHeight < 30; l.appendChild(logLine(e)); if (atEnd) l.scrollTop = l.scrollHeight; }
}
function showLogBadges() {
  $('logWarn').style.display = logWarnN ? '' : 'none'; $('logWarn').textContent = `警告 ${logWarnN}`;
  $('logErr').style.display = logErrN ? '' : 'none'; $('logErr').textContent = `错误 ${logErrN}`;
}
function logMatch(e) {
  const c = $('logCat').value;
  return (!c || e.cat === c) && (!$('logBad').checked || e.level === 'warn' || e.level === 'error');
}
function logText(e) { return `${e.t} [${e.cat}]${LEVEL_TXT[e.level] ? '[' + LEVEL_TXT[e.level] + ']' : ''} ${e.msg}`; }
function logLine(e) {
  const d = document.createElement('div'); d.className = 'll ' + (e.level || 'info');
  const t = document.createElement('span'); t.className = 'lt'; t.textContent = e.t + ' ';
  const c = document.createElement('span'); c.className = 'lc'; c.textContent = `[${e.cat}]` + (LEVEL_TXT[e.level] ? `[${LEVEL_TXT[e.level]}]` : '') + ' ';
  d.append(t, c, document.createTextNode(e.msg)); return d;
}
function renderLog() { const l = $('log'); l.innerHTML = ''; LOG_ITEMS.filter(logMatch).forEach(e => l.appendChild(logLine(e))); l.scrollTop = l.scrollHeight; }
function clearLog() { LOG_ITEMS.length = 0; logWarnN = logErrN = 0; showLogBadges(); renderLog(); }
function copyLog() {
  const text = LOG_ITEMS.filter(logMatch).map(logText).join('\n');
  navigator.clipboard.writeText(text).then(() => hint(`已复制 ${text ? text.split('\n').length : 0} 行日志`), () => hint('浏览器不让复制，点「今天的日志文件」'));
}
function fillLogCats(cats) {
  const sel = $('logCat'), keep = sel.value; sel.innerHTML = '<option value="">全部</option>';
  [...cats, '页面'].forEach(c => { const o = document.createElement('option'); o.value = c; o.textContent = c; sel.appendChild(o); });
  sel.value = keep;
}

// ---- 单颗操作的下拉 ----
const CN = { 10: '左髋 yaw', 9: '左髋 roll', 8: '左髋 pitch', 7: '左膝', 6: '左踝', 12: '颈 pitch', 11: '头 pitch', 14: '头 yaw', 13: '头 roll', 15: '嘴',
             5: '右髋 yaw', 4: '右髋 roll', 3: '右髋 pitch', 2: '右膝', 1: '右踝' };
// 关节顺序（左腿 → 颈/头/嘴 → 右腿），跟 server.py 的 JOINT_NAMES 同一个顺序，也是本仓库
// radxa/installation.json 里 joints 数组要求的顺序。编号只是编号，别按大小重排。
const ORDER = [10, 9, 8, 7, 6, 12, 11, 14, 13, 15, 5, 4, 3, 2, 1];
let REGS = [];   // [[地址, 名字, 字节数, 区]]，hello 里来，只留 EEPROM 和 SRAM（能写的）
function idLabel(id) { return `${id} · ${CN[id] || '非标准 ID'}${present.includes(id) ? '' : '（离线）'}`; }
function fillSelect(sel, list, label, prefer) {
  const keep = sel.value; sel.innerHTML = '';
  list.forEach(id => { const o = document.createElement('option'); o.value = id; o.textContent = label(id); sel.appendChild(o); });
  const opts = [...sel.options].map(o => o.value);
  if (opts.includes(keep)) sel.value = keep; else if (prefer !== undefined && opts.includes(String(prefer))) sel.value = String(prefer);
}
function fillIdSelects() {
  const extra = present.filter(id => !ORDER.includes(id)).sort((a, b) => a - b);
  fillSelect($('opId'), [...ORDER.filter(id => ids.includes(id) || present.includes(id)), ...extra], idLabel);
  // 改 ID：旧 ID 只列在线的，非标准的排前面（新舵机出厂 ID 1 一般在这）；新 ID 列 15 个标准位，占用的标出来，默认第一个空位
  fillSelect($('oldId'), [...extra, ...ORDER.filter(id => present.includes(id))], idLabel);
  fillSelect($('newId'), ORDER, id => `${id} · ${CN[id]}${present.includes(id) ? '（已占用）' : ''}`, ORDER.find(id => !present.includes(id)));
}
function setIdClick() {
  const o = $('oldId').value, n = $('newId').value;
  if (!o) { hint('总线上没有在线的舵机，先扫描'); return; }
  if (o === n) { hint('新旧 ID 一样'); return; }
  const clash = present.includes(Number(n)) ? `\n\n⚠️ 总线上已经有 ID ${n} 了，改完两颗会撞 ID！` : '';
  if (confirm(`把 ID ${o} 改成 ${n}（${CN[n]}）？\n总线上最好只接这一颗。写进 EEPROM。` + clash)) send({ op: 'set_id', old: Number(o), new: Number(n) });
}
function fillRegSelect() {
  const sel = $('wAddr'), keep = sel.value || '41'; sel.innerHTML = '';
  REGS.forEach(([a, name, size, area]) => { const o = document.createElement('option'); o.value = a; o.textContent = `${a} · ${name}（${size} 字节 ${area}）`; sel.appendChild(o); });
  if ([...sel.options].some(o => o.value === keep)) sel.value = keep;
  regChanged();
}
function regChanged() {
  const r = REGS.find(x => String(x[0]) === $('wAddr').value); if (!r) return;
  $('wSize').value = String(r[2]); $('wUnlock').checked = r[3] === 'EEPROM';
}

function build() {
  ['g20', 'g30', 'g10', 'gx'].forEach(g => $(g).innerHTML = '');
  // 按关节顺序（ORDER）排，不按 ID 大小：本机编号 1–15 升序是「踝→膝→髋」，跟关节顺序反的。
  // 认不出的 ID（不在 ORDER 里，比如小板 200）排最后。
  const rank = id => { const k = ORDER.indexOf(id); return k < 0 ? 999 : k; };
  const all = [...new Set([...ids, ...present])].sort((a, b) => rank(a) - rank(b) || a - b);
  let other = false;
  all.forEach(id => {
    // 按关节名分组，不按 ID 区间：本机编号是连续的 1–15，区间分不出左右腿和头颈
    const nm = names[id] || '';
    const g = nm.startsWith('left_') ? 'g20' : nm.startsWith('right_') ? 'g10' : nm ? 'g30' : (other = true, 'gx');
    const on = present.includes(id);
    const d = document.createElement('div');
    d.className = 'servo' + (on ? '' : ' off'); d.id = 's' + id;
    d.innerHTML = `<div class="id">${id}</div>
      <div class="track"><div class="mid"></div><div class="mark" id="m${id}" style="left:50%"></div>
        <input type="range" min="0" max="4095" value="${goalPos[id] ?? 2048}" id="r${id}" ${on ? '' : 'disabled'}></div>
      <div class="val" id="p${id}">–</div>
      <input type="checkbox" class="tq" id="t${id}" title="扭矩" ${on ? '' : 'disabled'}>
      <button class="dir" id="d${id}" title="3D 显示方向取反">${(dirs[id] || 1) > 0 ? '+' : '−'}</button>
      <div class="name"><span>${names[id] || ''}</span><span id="i${id}"></span></div>`;
    $(g).appendChild(d);
    const r = $('r' + id);
    r.oninput = () => { goalPos[id] = +r.value; $('p' + id).textContent = r.value; if ($('live').checked) throttledGoal(id, +r.value); if (!$('t' + id).checked) hint(`ID ${id} 扭矩没开，舵机不会动。勾上右边的扭矩框，或把 3D 的"显示"切成「滑块目标」预览`); };
    r.onchange = () => sendGoal(id, +r.value);
    $('t' + id).onchange = e => send({ op: 'torque', id, on: e.target.checked ? 1 : 0 });
    $('d' + id).onclick = () => { const d = -(dirs[id] || 1); dirs[id] = d; $('d' + id).textContent = d > 0 ? '+' : '−'; send({ op: 'set_dir', id, dir: d }); };
  });
  $('other').style.display = other ? '' : 'none';
  fillIdSelects();
}
function throttledGoal(id, pos) {
  const now = Date.now();
  if (!lastSent[id] || now - lastSent[id] > 50) { sendGoal(id, pos); }
  else { clearTimeout(sendTimer[id]); sendTimer[id] = setTimeout(() => sendGoal(id, pos), 60); }
}
function sendGoal(id, pos) { lastSent[id] = Date.now(); send({ op: 'goal', id, pos }); }

// 电流峰值保持：50 Hz 刷屏，起步那一下的峰肉眼读不出来，这里替人记着。
// peakSum 是 15 颗之和（跟 robotd 报的 mA 同一个口径），peakOne 是单颗最大。
let peakSum = 0, peakOne = 0, peakOneId = 0;
window.peakReset = function () { peakSum = 0; peakOne = 0; peakOneId = 0; };
function update(states, stats) {
  for (const [id, s] of Object.entries(states)) {
    const el = $('s' + id); if (!el) continue;
    if (!s) { $('i' + id).innerHTML = '<span class="bad">无应答</span>'; continue; }
    rawPos[id] = s.pos;
    const pos = Math.max(0, Math.min(4095, s.pos));
    $('m' + id).style.left = (pos / 4095 * 100) + '%';
    if (document.activeElement !== $('r' + id)) $('p' + id).textContent = s.pos;
    const st = s.status ? `<span class="bad">状态:${s.status_text || s.status}</span>` : '';
    const deg = (dirs[id] || 1) * (s.pos - 2048) * 360 / 4096;
    $('i' + id).innerHTML = `<b style="color:var(--text)">${deg >= 0 ? '+' : ''}${deg.toFixed(1)}°</b> ${s.volt.toFixed(1)}V ${s.temp}°C 负载${s.load / 10}% ${s.current_ma.toFixed(0)}mA ${s.moving ? '动' : ''} ${st}`;
  }
  if (stats && !isServoDemo) $('busInfo').textContent = `超时 ${stats.timeout} · 校验错 ${stats.bad_checksum} · 坏帧 ${stats.bad_length ?? 0}`;
  const live = Object.values(states).filter(Boolean);
  let curSum = 0;
  for (const [id, s2] of Object.entries(states)) {
    if (!s2) continue;
    curSum += s2.current_ma;
    if (s2.current_ma > peakOne) { peakOne = s2.current_ma; peakOneId = +id; }
  }
  if (curSum > peakSum) peakSum = curSum;
  $('stOnline').textContent = `${live.length}/${ids.length}`;
  $('stOnline').style.color = live.length === ids.length ? 'var(--acc)' : 'var(--warn)';
  const now = performance.now(); frameTimes.push(now); while (frameTimes.length && now - frameTimes[0] > 2000) frameTimes.shift();
  if (frameTimes.length > 1) $('stHz').textContent = ((frameTimes.length - 1) / ((now - frameTimes[0]) / 1000)).toFixed(1) + ' Hz';
  if (live.length) $('stVolt').textContent = (live.reduce((a, x) => a + x.volt, 0) / live.length).toFixed(1) + ' V';
  if (live.length) $('stPeak').textContent = (peakSum / 1000).toFixed(2) + ' A'
    + (peakOneId ? ` / 单颗 id${peakOneId} ${peakOne.toFixed(0)} mA` : '');
}
let frameTimes = [];
function parseIdList(text) {
  const out = [];
  for (const part of text.split(',')) {
    const p = part.trim(); if (!p) continue;
    if (p.includes('-')) { const [a, b] = p.split('-').map(Number); for (let i = a; i <= b; i++) out.push(i); } else out.push(Number(p));
  }
  return out.filter(id => present.includes(id));
}
// 基准：'' = 零位 2048；'@official:FOLD' = 官方折叠；否则是 poses/ 里的文件名。这里只给摆法，
// 每颗写成多少由服务端算（calib_plan），跟真校准用同一份，页面不再各算各的
function calibRefText() {
  const f = $('calibRef').value;
  if (!f) return { ref: null, text: '零位（腿伸直、脚板垂直、头平视、嘴闭上）' };
  if (f === '@official:FOLD')
    return { ref: f, text: '官方折叠（头颈摆正）：髋 pitch ±90°（直角尺把大腿摆到跟躯干垂直）、膝 ±90°（折到踝刚好贴上大腿舵机，别再往里顶）、'
      + '髋 yaw / roll 0°（腿不外撇、不外张）、踝 0°（脚板跟小腿成直角，跟伸直站着时一样）、脖子竖直、头平视朝正前不歪、嘴闭上' };
  const p = posesCache.find(q => q.file === f);
  return p ? { ref: f, text: `姿态「${p.name}」（${f}）的样子` } : { ref: null, text: null };
}
// 零位、官方折叠都是 15 颗全标；存的姿态保留自己填的列表（缩着定不住的关节可能被拿掉了）
function calibRefChanged() {
  const f = $('calibRef').value;
  if (!f || f === '@official:FOLD') $('calibIds').value = '1-15';
  if (f === '@official:FOLD') {
    hint('官方折叠（头颈摆正）：膝折到踝刚贴上大腿舵机就停，髋 pitch 用直角尺摆垂直，髋 yaw/roll、踝、头颈摆正，嘴闭上，15 颗一起标。3D 叠加了要摆的样子');
    if (!$('ghostPose').value) { $('ghostPose').value = 'FOLD_CALIB'; setGhost(); }
  }
}
let calibPending = null;   // 点了校准、等服务端回预览
function calibAll() {
  const list = parseIdList($('calibIds').value);
  if (!list.length) { hint('校准列表里没有在线的舵机'); return; }
  const r = calibRefText(); if (!r.text) { hint('基准姿态找不到了，重新选'); return; }
  calibPending = { for: 'all', ids: list, ref: r.ref, text: r.text };
  send({ op: 'calib_plan', ids: list, ref: r.ref, for: 'all' });
}
function calibOne() {
  const id = v('opId'), r = calibRefText(); if (!r.text) { hint('基准姿态找不到了，重新选'); return; }
  calibPending = { for: 'one', id, ids: [id], ref: r.ref, text: r.text };
  send({ op: 'calib_plan', ids: [id], ref: r.ref, for: 'one', id });
}
// 服务端回来的预览：逐颗「现在 → 写成、差几度」。差得多的不拦，再确认一次（装的时候转了半圈 vs 方向 ± 设反了）
function confirmCalib(m) {
  const p = calibPending; calibPending = null;
  if (!p || p.for !== m.for || (p.ref || null) !== (m.ref || null)) return;
  const deg = d => (d >= 0 ? '+' : '') + d.toFixed(1) + '°';
  const nm = id => `#${id} ${CN[id] || ''}`;
  const big = m.rows.filter(r => r.diff_deg !== null && Math.abs(r.diff_deg) > m.big_deg);
  const lines = m.rows.map(r => r.target === null ? `${nm(r.id)}  基准里没有，跳过`
    : r.pos === null ? `${nm(r.id)}  读不到位置（目标 ${r.target}）`
    : `${nm(r.id)}  ${r.pos} → ${r.target}  差 ${deg(r.diff_deg)}${Math.abs(r.diff_deg) > m.big_deg ? '  ⚠️' : ''}`);
  const shifts = !p.ref || p.ref.startsWith('@official:');
  const head = p.for === 'one' ? `只校准 ID ${p.id}（${CN[p.id] || ''}）` : `校准这 ${m.rows.length} 颗`;
  const tail = m.fake ? '\n\n模拟：只改虚拟舵机，备份放临时目录，不碰 calib/ 和存的姿态。'
    : '\n\n写进舵机 EEPROM；原来的偏移会备份，可以撤销' + (shifts ? '；存的其它姿态自动换算。' : '。');
  if (!confirm(`${head}\n\n确认鸭子已经摆成${p.text}\n\n现在 → 写成（差多少）：\n${lines.join('\n')}${tail}`)) return;
  if (big.length && !confirm(`${big.length} 颗差 ${m.big_deg}° 以上：\n${big.map(r => `${nm(r.id)}  差 ${deg(r.diff_deg)}`).join('\n')}\n\n`
      + '常见两种原因：\n1. 装舵盘时转了半圈（或错了几个齿）：没关系，校准就是把它纠回来，照常校。\n'
      + '2. 这颗的方向 ± 设反了：折叠时 ±90° 的关节方向反了，也会差 180° 左右，这样校进去的零点是错的。\n\n'
      + '分不清就先取消：扭矩关着用手把这颗往一边掰一点，看 3D 里的关节是不是跟着往同一边转；不是就点这一行的 ± 改过来再校。\n\n确定照常校？')) return;
  if (p.for === 'one') send({ op: 'calibrate', id: p.id, ref: p.ref });
  else send({ op: 'calib_mid_all', ids: p.ids, ref: p.ref });
}
function showCalibResult(m) {
  const bad = Object.entries(m.bad || {}), big = Object.keys(m.big || {});
  // 每颗的过程服务端已经逐行记了，这里只弹提示
  if (bad.length) hint(`${bad.length} 颗没成功：${bad.map(([k]) => k).join(' ')}，日志选「校准」看每颗卡在哪一步`);
  else if (m.ok.length) hint(`${m.ok.length} 颗完成` + (big.length ? `；${big.join(' ')} 差 30° 以上，确认一下方向 ± 没设反（日志里有说明）` : ''));
  setTimeout(syncSliders, 300);
}
// 开扭矩的回包：服务端先把目标对准了当前位置，滑块跟过去；没开成的把勾去掉
function showTorqueResult(m) {
  for (const [id, p] of Object.entries(m.aligned || {})) {
    if (!$('r' + id)) continue;
    const c = Math.max(0, Math.min(4095, p)); $('r' + id).value = c; goalPos[id] = c; $('p' + id).textContent = p;
  }
  const bad = Object.entries(m.bad || {});
  bad.forEach(([id]) => { const t = $('t' + id); if (t) t.checked = false; });
  if (bad.length) hint(`${bad.map(([k]) => k).join(' ')} 没开扭矩：${bad[0][1]}（详情看日志）`);
}
function torqueAll(on) { send({ op: 'torque', on }); present.forEach(id => { const t = $('t' + id); if (t) t.checked = !!on; }); }
function syncSliders() { present.forEach(id => { const p = rawPos[id]; if (p !== undefined) { const c = Math.max(0, Math.min(4095, p)); $('r' + id).value = c; goalPos[id] = c; } }); }
// 官方 model.rs DEFAULT_POSITION，弧度，零位 = 笔直站
const DEFAULT_POSITION = { 10: 0, 9: -0.0873, 8: -0.4579, 7: -0.0049, 6: 0.4530, 12: 0.3491, 11: 0.3491, 14: 0, 13: 0, 15: 0,
                           5: 0, 4: 0.0873, 3: 0.4579, 2: 0.0049, 1: -0.4530 };
// 按钮文案跟存下来的姿态名不总是一模一样：按钮写「站姿」
// 只按 p.name.includes(key) 找的话这两种都找不到 —— 「站姿」看着能用，其实是掉进了下面官方 DEFAULT_POSITION 的兜底，
// 发的一直是官方默认值、不是你调好的那个姿态。这里改成两边互相包含也算命中；
// 但要按「完全相等 > 名字更长」挑，免得将来同时存在「站」和「站姿」时挑错。
function findPose(key) {
  const cand = posesCache.filter(p => !p.steps && p.name);
  return cand.find(p => p.name === key)
      || cand.filter(p => p.name.includes(key) || key.includes(p.name))
             .sort((a, b) => b.name.length - a.name.length)[0]
      || null;
}
function standPose() {
  const p = findPose('站姿');
  if (p) { sendPose(posesCache.indexOf(p)); return; }
  if (!confirm('poses/ 里没有存的「站姿」，发官方 DEFAULT_POSITION？髋 pitch ±26°、踝 ∓26°、颈/头 +20°，按每颗的 ± 换算。第一次托着试')) return;
  const goals = {};
  present.forEach(id => { if (DEFAULT_POSITION[id] === undefined) return; goals[id] = Math.round(2048 + DEFAULT_POSITION[id] * 4096 / (2 * Math.PI) * (dirs[id] || 1)); });
  const sec = Number($('rampSec').value) || 0; addLog(`站姿（${sec} s）→ ` + JSON.stringify(goals), '运动'); rampTo(goals, sec);
}
// 官方姿势：microduck_rl src/mjlab_microduck/robot/microduck/scene.xml 的 keyframe（d424a0c），弧度、模型约定。
// 嘴不在官方模型里：只有零位带嘴（2048）
const OFFICIAL_POSES = {
  INIT:  { label: '零位', rad: { 10: 0, 9: 0, 8: 0, 7: 0, 6: 0, 12: 0, 11: 0, 14: 0, 13: 0, 15: 0, 5: 0, 4: 0, 3: 0, 2: 0, 1: 0 } },
  STAND: { label: '站', rad: DEFAULT_POSITION },
  SIT:   { label: '坐', rad: { 10: 0, 9: 0, 8: -0.5236, 7: 1.0472, 6: 0, 12: 0.5, 11: 1.6, 14: 0, 13: 0, 5: 0, 4: 0, 3: 0.5236, 2: -1.0472, 1: 0 } },
  FOLD:  { label: '折叠', rad: { 10: 0, 9: 0, 8: 1.57, 7: 1.57, 6: 0, 12: 1, 11: 1, 14: 0, 13: 0, 5: 0, 4: 0, 3: -1.57, 2: -1.57, 1: 0 } },
};
// 校准用折叠：跟服务端 FOLD_CALIB 同一份，腿按官方折叠，头颈摆正、嘴闭上。只用来叠加，没有按钮发它
OFFICIAL_POSES.FOLD_CALIB = { label: '校准用折叠（头颈摆正）', rad: { ...OFFICIAL_POSES.FOLD.rad, 12: 0, 11: 0, 15: 0 } };
// 精确网格求交（官方 XL330 几何，tools/fold_contact/fold_contact.py）：髋 ±90°、踝 0° 时膝折到 89.6° 踝就碰上大腿舵机，
// FOLD 的 90° 会顶进去 0.36°。真机发折叠时膝只折 88°，不让舵机顶着零件使劲。3D 叠加仍显示官方原值
const FOLD_KNEE_SEND = 88 * Math.PI / 180;
function officialGoals(key) {
  const rad = { ...OFFICIAL_POSES[key].rad };
  if (key === 'FOLD') { rad[7] = FOLD_KNEE_SEND; rad[2] = -FOLD_KNEE_SEND; }
  const goals = {};
  present.forEach(id => { if (rad[id] !== undefined) goals[id] = Math.round(2048 + rad[id] * 4096 / (2 * Math.PI) * (dirs[id] || 1)); });
  return goals;
}
function goOfficial(key) {
  const p = OFFICIAL_POSES[key], goals = officialGoals(key), n = Object.keys(goals).length, sec = Number($('rampSec').value) || 0;
  if (!n) { hint('没有在线的舵机'); return; }
  const extra = key === 'FOLD' ? '\n膝只折 88°：官方 90° 会让踝顶着大腿上的舵机。\n⚠️ 零点还没校准的话别用这个按钮：膝差几度就顶上零件了（88° 离碰上只剩 1.6°）。校准前要折叠，扭矩关着用手折。' : '';
  if (!confirm(`官方「${p.label}」→ ${n} 颗，${sec} 秒过渡，按现在的零点和每颗的 ± 换算。${extra}\n零点没校准好的话，到的位置跟官方会差一截。扭矩开着的会动，先托住鸭子`)) return;
  peakReset();
  addLog(`官方姿势「${p.label}」（${sec} s）→ ` + JSON.stringify(goals), '运动'); rampTo(goals, sec);
}
function setGhost() {
  const key = $('ghostPose').value;
  window.duckGhost = key;
  addLog(key ? `3D 叠加官方「${OFFICIAL_POSES[key].label}」` : '3D 不叠官方姿势', '姿态');
  renderGhostTable();
}
// 每颗「实际 − 官方」：实际按 (读数 − 2048) × ± 换成模型约定的角度
function renderGhostTable() {
  const key = $('ghostPose').value, box = $('ghostTbl');
  if (!key) { box.innerHTML = ''; return; }
  const rad = OFFICIAL_POSES[key].rad;
  const rows = ORDER.filter(id => rad[id] !== undefined).map(id => {
    const off = rad[id] * 180 / Math.PI, raw = rawPos[id];
    if (raw === undefined) return `<tr><td>${id} ${CN[id]}</td><td class="n">${off.toFixed(1)}</td><td class="n muted">—</td><td class="n muted">—</td></tr>`;
    const act = (dirs[id] || 1) * (raw - 2048) * 360 / 4096, diff = act - off;
    const col = Math.abs(diff) > 5 ? 'var(--bad)' : Math.abs(diff) > 2 ? 'var(--warn)' : 'var(--acc)';
    return `<tr><td>${id} ${CN[id]}</td><td class="n">${off.toFixed(1)}</td><td class="n">${act.toFixed(1)}</td><td class="n" style="color:${col}">${diff >= 0 ? '+' : ''}${diff.toFixed(1)}</td></tr>`;
  });
  box.innerHTML = `<table class="ghost"><tr><th>关节</th><th>官方 °</th><th>实际 °</th><th>差 °</th></tr>${rows.join('')}</table>`;
}
setInterval(() => { if ($('ghostPose').value) renderGhostTable(); }, 500);
function centerAll() { const sec = Number($('rampSec').value) || 0; if (!confirm(`全部到 2048（零位），${sec} 秒过渡？扭矩开着的会动`)) return; const goals = {}; present.forEach(id => { goals[id] = 2048; }); addLog(`零位（${sec} s）`, '运动'); rampTo(goals, sec); }

function showDump(m) {
  lastDump = m; showTab('calib'); $('dumpSec').hidden = false; $('dumpId').textContent = 'ID ' + m.id;
  const t = $('dumpTbl');
  t.innerHTML = '<tr><th>地址</th><th>名称</th><th>区</th><th>字节</th><th>值</th><th>hex</th></tr>' +
    m.rows.map(r => `<tr><td class="n">${r.addr}</td><td>${r.name}</td><td class="area-${r.area}">${r.area}</td><td class="n">${r.size}</td><td class="n">${r.value}</td><td class="n muted">0x${r.value.toString(16).toUpperCase().padStart(r.size * 2, '0')}</td></tr>`).join('');
  $('dumpSec').scrollIntoView({ behavior: 'smooth' });
}
function downloadDump() {
  if (!lastDump) return;
  const a = document.createElement('a');
  a.href = URL.createObjectURL(new Blob([JSON.stringify(lastDump, null, 1)], { type: 'application/json' }));
  a.download = `HD-1910-id${lastDump.id}-regs-${new Date().toISOString().slice(0, 10)}.json`; a.click();
}
function copyDumpMd() {
  if (!lastDump) return;
  const md = ['| 地址 | 名称 | 区 | 值 |', '|---|---|---|---|', ...lastDump.rows.map(r => `| ${r.addr} | ${r.name} | ${r.area} | ${r.value} |`)].join('\n');
  navigator.clipboard.writeText(md).then(() => addLog('已复制 Markdown 表'));
}
// robotd.toml 的 [bus.feetech] directions：关节顺序是左腿 6–10、颈头嘴 11–15、右腿 1–5
function copyRobotdDirs() {
  const order = [10, 9, 8, 7, 6, 12, 11, 14, 13, 15, 5, 4, 3, 2, 1];
  const line = 'directions = [' + order.map(id => (dirs[id] || 1) < 0 ? -1 : 1).join(', ') + ']';
  navigator.clipboard.writeText(line).then(() => addLog('已复制：' + line), () => addLog(line));
}
let posesCache = [], rampTimer = null, seqAbort = false;
function stopRamp() {
  if (rampTimer) { clearInterval(rampTimer); clearTimeout(rampTimer); rampTimer = null; }
  seqAbort = true;
  // 平滑模式下舵机已经拿到终点在自己走，把"现在的位置"当新目标发过去，让它停在原地
  const g = {}; present.forEach(id => { if (rawPos[id] !== undefined) g[id] = Math.max(0, Math.min(4095, rawPos[id])); });
  if (Object.keys(g).length) send({ op: 'goals_verify', goals: g });
  addLog('已停止：目标改成当前位置', '运动');
}
// 从当前实际位置线性插到 goals，20 Hz 发 sync_write；返回 Promise
function rampTo(goals, seconds) {
  return new Promise(resolve => {
    if (rampTimer) { clearInterval(rampTimer); rampTimer = null; }
    const ids = Object.keys(goals).filter(id => present.includes(+id) && !(idleRunning && idleCenter && idleCenter[id] !== undefined));   // 灵动开着时头归灵动管
    if (!ids.length) { resolve(); return; }
    if (($('moveMode').value === 'profile') && seconds > 0) {
      // 平滑：一次把终点+速度+加速度发给舵机，舵机自己走；后端等它走完再回 goals_verify
      const g = {}; ids.forEach(id => { g[id] = goals[id]; const c = Math.max(0, Math.min(4095, goals[id])); $('r' + id).value = c; goalPos[id] = c; $('p' + id).textContent = c; });
      send({ op: 'goals_profile', goals: g, seconds, acc: Number($('accReg').value) || 30 });
      rampTimer = setTimeout(() => { rampTimer = null; send({ op: 'goals_verify', goals: g }); send({ op: 'release', ids: ids.map(Number) }); resolve(); }, seconds * 1000 + 400);
      return;
    }
    const start = {}; ids.forEach(id => { start[id] = rawPos[id] !== undefined ? rawPos[id] : (goalPos[id] ?? 2048); });
    const t0 = performance.now();   // 按墙钟算进度：浏览器切到后台把定时器降到 1 Hz 也能按时到终点
    const tick = () => {
      const t = seconds <= 0 ? 1 : Math.min(1, (performance.now() - t0) / (seconds * 1000)); const g = {};
      ids.forEach(id => { g[id] = Math.round(start[id] + (goals[id] - start[id]) * t); const c = Math.max(0, Math.min(4095, g[id])); $('r' + id).value = c; goalPos[id] = c; $('p' + id).textContent = c; });
      if (t >= 1) { send({ op: 'goals_verify', goals: g }); send({ op: 'stream_end', goals: g, ids: ids.map(Number) }); clearInterval(rampTimer); rampTimer = null; resolve(); }
      else send({ op: 'goals_stream', goals: g, dt: 0.05 });
    };
    if (seconds <= 0) tick(); else rampTimer = setInterval(tick, 50);
  });
}
function fillCalibRef() {
  const sel = $('calibRef'), keep = sel.value;
  sel.innerHTML = '<option value="">零位 2048（鸭子摆直）</option><option value="@official:FOLD">官方折叠（腿折好趴稳、头颈摆正）</option>';
  posesCache.filter(p => !p.steps).forEach(p => { const o = document.createElement('option'); o.value = p.file; o.textContent = `姿态「${p.name}」· ${(p.time || '').slice(5, 16)}`; sel.appendChild(o); });
  if ([...sel.options].some(o => o.value === keep)) sel.value = keep;
}
function renderPoses(list) {
  posesCache = list || [];
  fillCalibRef();
  const box = $('poseList');
  if (!posesCache.length) { box.innerHTML = '<span class="muted">还没有存的姿态</span>'; return; }
  box.innerHTML = '';
  // 同名的姿态后面带上日期，不然一排「xxx」分不清
  const count = {}; posesCache.forEach(p => { count[p.name] = (count[p.name] || 0) + 1; });
  posesCache.forEach((p, i) => {
    const wrap = document.createElement('span');
    wrap.style.cssText = 'display:inline-flex;align-items:stretch';
    const b = document.createElement('button');
    const date = (p.time || '').slice(5, 10);
    b.textContent = (p.steps ? '▶ ' : '') + p.name + (count[p.name] > 1 && date ? ` · ${date}` : '');
    b.title = (p.note ? p.note + '\n' : '') + `${p.time} · ${p.steps ? p.steps.length + ' 步' : Object.keys(p.goals).length + ' 颗'} · ${p.file}`;
    b.onclick = () => p.steps ? runSequence(i) : sendPose(i);
    b.style.borderRadius = '6px 0 0 6px';
    const x = document.createElement('button');
    x.textContent = '×'; x.title = `删除 ${p.file}`;
    x.style.cssText = 'border-left:0;border-radius:0 6px 6px 0;padding:6px 8px;color:var(--muted)';
    x.onclick = () => deletePose(i);
    wrap.appendChild(b); wrap.appendChild(x);
    box.appendChild(wrap);
  });
}
function deletePose(i) {
  const p = posesCache[i]; if (!p) return;
  // 序列按名字引用姿态：删最后一个同名的之前提醒一句
  const usedBy = posesCache.filter(q => q.steps && JSON.stringify(q.steps).includes(`"pose":"${p.name}"`)).map(q => q.name);
  const same = posesCache.filter(q => q !== p && !q.steps && q.name === p.name).length;
  const warn = usedBy.length && !same ? `\n\n⚠️ 序列 ${usedBy.join('、')} 用到了「${p.name}」，删了它们就跑不了。` : '';
  if (!confirm(`删除「${p.name}」？\n文件 ${p.file}\n会挪进 poses/.trash/，要找回就挪回来。` + warn)) return;
  send({ op: 'delete_pose', file: p.file });
}
async function sendPose(i) {
  const p = posesCache[i]; if (!p) return;
  const goals = {};
  present.forEach(id => { if (p.goals[id] !== undefined) goals[id] = p.goals[id]; });
  const n = Object.keys(goals).length, sec = Number($('rampSec').value) || 0;
  if (!confirm(`发「${p.name}」到 ${n} 颗，${sec} 秒过渡？扭矩开着的会动，先托住鸭子`)) return;
  addLog(`姿态「${p.name}」→ ${n} 颗，${sec} s`, '运动'); await rampTo(goals, sec); addLog(`「${p.name}」到位`, '运动');
}
// 序列里的目标可以写成 {"pose":"站姿","delta_deg":-30}：以那个姿态为基准，加模型约定下的角度，按每颗的 ± 换成真机方向
function resolveGoal(id, v) {
  if (typeof v === 'number') return v;
  const base = findPose(v.pose);
  if (!base || base.goals[id] === undefined) return null;
  return Math.round(base.goals[id] + (v.delta_deg || 0) * 4096 / 360 * (dirs[id] || 1));
}
async function runSequence(i) {
  const p = posesCache[i]; if (!p || !p.steps) return;
  const total = p.steps.reduce((s, st) => s + (st.seconds || 0), 0);
  if (!confirm(`跑序列「${p.name}」，${p.steps.length} 步共 ${total} 秒？\n${p.note || ''}\n第一次托着鸭子，随时点「停」`)) return;
  seqAbort = false;
  for (const st of p.steps) {
    if (seqAbort) break;
    const goals = {}; let missing = [];
    present.forEach(id => { if (st.goals[id] !== undefined) { const g = resolveGoal(id, st.goals[id]); if (g === null) missing.push(id); else goals[id] = g; } });
    if (missing.length) { addLog(`序列 ${st.name}：基准姿态缺 ${missing.join(',')}，先存「站姿」/「坐姿」`, '运动', 'warn'); break; }
    addLog(`序列 ${st.name}（${st.seconds} s）`, '运动'); await rampTo(goals, st.seconds || 0);
    await new Promise(r => setTimeout(r, 300));
  }
  addLog(seqAbort ? '序列已中断' : `序列「${p.name}」完成`, '运动', seqAbort ? 'warn' : 'info');
}
function savePose() {
  const name = $('poseName').value.trim() || 'pose';
  if (!confirm(`把现在 ${present.length} 颗的实际位置存成「${name}」？`)) return;
  send({ op: 'save_pose', name });
}
// ── 灵动：头部小动作 ──────────────────────────────────────────────
// 每个动作是一条平滑指令（舵机自己走梯形曲线），动作之间不空等、下一条紧接着发，靠舵机的加速度自然衔接。
// 幅度故意大一点：这批 320:1 减速箱的舵机在几十步/秒的低速下会粘滑，小幅慢动反而顿。
let idleRunning = false, idleCenter = null;
const D = 4096 / 360;   // 1° 的步数
const rnd = (a, b) => a + Math.random() * (b - a);
const pick = arr => arr[Math.floor(Math.random() * arr.length)];
const wait = ms => new Promise(r => setTimeout(r, ms));
function moveHead(deltaDeg, sec, acc) {
  const goals = {};
  for (const [id, deg] of Object.entries(deltaDeg)) {
    if (!present.includes(+id) || !idleCenter || idleCenter[id] === undefined) continue;
    const raw = Math.round(idleCenter[id] + deg * D * (dirs[id] || 1));
    goals[id] = Math.max(0, Math.min(4095, raw)); $('r' + id).value = goals[id]; goalPos[id] = goals[id]; $('p' + id).textContent = goals[id];
  }
  if (Object.keys(goals).length) send({ op: 'goals_profile', goals, seconds: sec, acc: acc || 35 });
  return wait(sec * 1000 + 60);   // 只比走的时间多等 60 ms，下一条紧接着发
}
// 每个动作：[相对中心的度数, 秒]。14 yaw、13 roll、11 头 pitch、12 颈 pitch、15 嘴（正 = 张）。{} = 停住不动
const GESTURES = [
  { w: 3, steps: () => { const s = pick([-1, 1]); return [[{ 13: 14 * s, 14: 8 * s }, 0.5], [{}, rnd(0.6, 1.4)], [{ 13: 0, 14: 0 }, 0.6]]; } },            // 歪头
  { w: 3, steps: () => { const s = pick([-1, 1]); return [[{ 14: 30 * s, 13: -5 * s, 11: 4 }, 0.6], [{}, rnd(0.8, 1.8)], [{ 14: 0, 13: 0, 11: 0 }, 0.7]]; } }, // 看一边
  { w: 2, steps: () => [[{ 14: -20 }, 0.3], [{ 14: 20 }, 0.38], [{ 14: -12 }, 0.32], [{ 14: 0 }, 0.35]] },                                            // 摇头
  { w: 2, steps: () => [[{ 11: 12 }, 0.3], [{ 11: -7 }, 0.34], [{ 11: 6 }, 0.3], [{ 11: 0 }, 0.35]] },                                               // 点头
  { w: 2, steps: () => [[{ 15: 16 }, 0.22], [{ 15: 0 }, 0.26], [{ 15: 13 }, 0.22], [{ 15: 0 }, 0.28]] },                                             // 嘎嘎两下
  { w: 2, steps: () => { const s = pick([-1, 1]); return [[{ 13: 12 * s, 11: -8, 15: 9 }, 0.55], [{}, rnd(0.5, 1.0)], [{ 15: 0 }, 0.25], [{ 13: 0, 11: 0 }, 0.6]]; } }, // 好奇
  { w: 2, steps: () => { const s = pick([-1, 1]); return [[{ 14: 18 * s, 11: 6 }, 0.45], [{ 14: -18 * s, 11: 6 }, 0.7], [{ 14: 0, 11: 0 }, 0.5]]; } },   // 扫视
  { w: 1, steps: () => [[{ 13: 7, 14: 6 }, 0.18], [{ 13: -7, 14: -6 }, 0.2], [{ 13: 0, 14: 0 }, 0.25]] },                                            // 抖一下
];
async function toggleIdle() {
  if (idleRunning) { idleRunning = false; $('idleBtn').textContent = '🦆 灵动'; return; }
  const headIds = [12, 11, 14, 13, 15].filter(id => present.includes(id));
  if (headIds.length < 3) { hint('头部舵机不在线'); return; }
  if (!headIds.every(id => $('t' + id).checked) && !confirm('头部扭矩没全开，要先开吗？')) return;
  send({ op: 'torque', on: 1, ids: headIds }); headIds.forEach(id => { $('t' + id).checked = true; });
  idleCenter = {}; headIds.forEach(id => { idleCenter[id] = rawPos[id] !== undefined ? rawPos[id] : 2048; });
  idleRunning = true; $('idleBtn').textContent = '⏹ 停灵动'; addLog('灵动开始，中心 ' + JSON.stringify(idleCenter), '运动');
  const bag = GESTURES.flatMap(g => Array(g.w).fill(g));
  let last = null;
  while (idleRunning) {
    let g = pick(bag); if (g === last) g = pick(bag); last = g;
    for (const [delta, sec] of g.steps()) {
      if (!idleRunning) break;
      if (Object.keys(delta).length) await moveHead(delta, sec); else await wait(sec * 1000);
    }
    if (idleRunning) await wait(rnd(300, 1500));
  }
  await moveHead({ 12: 0, 11: 0, 14: 0, 13: 0, 15: 0 }, 0.7);
  send({ op: 'release', ids: Object.keys(idleCenter).map(Number) });
  addLog('灵动结束，头回中心', '运动'); idleCenter = null;
}
let showWhat = 'present';
function setShowWhat() { showWhat = $('showWhat').value; }
function resetCam() { if (window.duckResetCam) window.duckResetCam(); }
connect();
