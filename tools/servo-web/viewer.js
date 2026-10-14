import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { ImuAttitude, eulerDegrees } from './imu_attitude.js';

let imuAttitude = new ImuAttitude(), imuRevision = -1;
const imuNumber = (n, digits = 0) => typeof n === 'number' && Number.isFinite(n) ? n.toFixed(digits) : '—';
/* 面板上带 ± 的那几个数：角度、加速度、角速度。两个讲究：
   ① 先按显示精度取整、再判正负。「-0.0 和 0.0 老来回闪」就是这里来的 ——
      角度在零点附近飘，-0.004 让 toFixed 印出 "-0.00"、+0.004 印出 "+0.00"，
      同一堆零来回换脸。取整后 -0 加 0 归成 +0，落进 >= 0 那一支，就一直印 "+0.00"。
      （直接对原数判 >= 0 没用：-0.004 本来就是负数，加了 0 还是负数。）
   ② 正数带 +：一列数上下对齐时，+ / - 让「往哪边偏」不用看数字长度。 */
const imuFixed = (n, digits = 2) => {
  if (typeof n !== 'number' || !Number.isFinite(n)) return '—';
  const r = Number(n.toFixed(digits)) + 0;
  return (r >= 0 ? '+' : '') + r.toFixed(digits);
};
const imuVector = v => Array.isArray(v) && v.length === 3 ? v.map(n => imuFixed(n, 2)).join(' / ') : '—';
/* 哪条路能填哪些格子。规矩是「这条路肯定填不上的，整格不画」——
   挂个「—」看着像掉数据或者坏了，其实那条路上压根没这个量。
   总线（ID 200 那 15 个字节）只捎陀螺 + 四元数 + 采样计数 + 状态位：
   没有加速度、没有 SPI / FIFO 计数、没有参考电压。（板子的寄存器表里是有加速度的 ——
   control_table.c 把 raw_accel 放在 136、spi_errors 在 172、fifo_overflows 在 176，
   读回调读哪个地址都行；是现在读的那个窗口「地址 56 起 15 字节」避开了它们，
   不是板子没这份数据。哪天要让总线也显示，改的是 imu200.py 去读哪个地址，不是这张表。）
   live（J-Link / pyOCD）走整块 RAM 快照，上面全都有；demo 是软件合成的，
   除了参考电压（那是调试口量的，合成不出来）也都有。 */
const IMU_CAPS = {
  bus: ['samples', 'hz', 'gyro', 'errImu'],
  demo: ['samples', 'hz', 'accel', 'gyro', 'errImu', 'errSpi', 'errFifo'],
  live: ['samples', 'hz', 'voltage', 'accel', 'gyro', 'errImu', 'errSpi', 'errFifo'],
};
let imuCapsShown;
/* 收格子。先把填不上的整格收掉，再让分隔号跟着两边的邻居一起收 ——
   不然收掉一格会剩下孤零零一个「·」。一行里格子全收光了，这一行也收掉。 */
function renderImuCaps(caps) {
  if (caps === imuCapsShown) return;   // 30 Hz 调进来，路没变就别碰 DOM（对比的是 IMU_CAPS 里那几个常量数组本身）
  imuCapsShown = caps;
  const fields = [...document.querySelectorAll('#imuPanel .imuF')];
  for (const f of fields) f.hidden = !!caps && !caps.includes(f.dataset.cap);
  for (const sep of document.querySelectorAll('#imuPanel .sep')) {
    const prev = sep.previousElementSibling, next = sep.nextElementSibling;
    sep.hidden = !prev || !next || prev.hidden || next.hidden;
  }
  for (const row of new Set(fields.map(f => f.parentElement)))
    row.hidden = [...row.querySelectorAll('.imuF')].every(f => f.hidden);
}
function renderImu() {
  $('imuPanel').hidden = !imuFeed.enabled;
  if (!imuFeed.enabled) return;
  const p = imuAttitude.latest || {}, valid = imuAttitude.valid, demo = p.mode === 'demo';
  /* 先按当前这条路收起填不上的格子。还不知道走哪条（第一帧之前）就传 null，全都留着 ——
     「还不知道」最多只到第一帧：服务器的每帧都带 backend / mode，连「正在连接」那帧也带
     （imu_bridge.py 里那句 setdefault，就是为了别让标签先显示成 J-Link）。 */
  renderImuCaps(demo ? IMU_CAPS.demo : p.backend ? (IMU_CAPS[p.backend] || IMU_CAPS.live) : null);
  $('imuMode').textContent = demo ? '演示 · 非硬件' : p.backend === 'bus' ? '总线 · ID 200' : p.backend === 'swd' ? `${p.probe_name || 'pyOCD'} · SWD` : 'J-Link · SWD';
  $('imuMode').style.color = demo ? 'var(--warn)' : 'var(--muted)';
  const status = valid ? (demo ? '演示跟随' : '实时跟随') :
    ({ stale: '数据已停滞', error: '数据无效', disconnected: '连接中断' }[imuAttitude.status] || '等待有效数据');
  $('imuStatus').textContent = status; $('imuStatus').style.color = valid ? 'var(--acc)' : 'var(--warn)';
  $('imuZero').disabled = !valid; $('imuRaw').disabled = !valid;
  $('imuSamples').textContent = imuNumber(p.sample_count); $('imuHz').textContent = valid ? imuNumber(p.hz, 1) : '—';
  $('imuVoltage').textContent = imuNumber(typeof p.voltage_mv === 'number' ? p.voltage_mv / 1000 : null, 3);
  $('imuErrors').textContent = imuNumber(p.error ?? p.errors?.imu); $('imuSpiErrors').textContent = imuNumber(p.errors?.spi); $('imuFifoErrors').textContent = imuNumber(p.errors?.fifo);
  $('imuAccel').textContent = valid ? imuVector(p.accel_g) : '—'; $('imuGyro').textContent = valid ? imuVector(p.gyro_dps) : '—';
  /* 三个数各占一个 span、由 #imuAngles 的 flex 撑开（style.css），不再用「 · 」当分隔号：
     那三个「 · 」连空格白占 26px，而这一行是本块的主读数，字号不能一缩再缩 ——
     腾出这 26px 才排得进 ±179.99° 这种三位数。撑开还有个好处：数值变宽变窄时
     三段是往两边让，不是整行重新折行。*/
  const angles = valid ? eulerDegrees(imuAttitude.target).map(n => imuFixed(n, 2)) : ['—', '—', '—'];
  $('imuRoll').textContent = `横滚 ${angles[0]}°`;
  $('imuPitch').textContent = `俯仰 ${angles[1]}°`;
  $('imuYaw').textContent = `偏航 ${angles[2]}°`;
  $('imuReference').textContent = (imuAttitude.raw ? '原始姿态' : '相对姿态（首次有效数据自动归零）') + '；显示归零只影响本页，不改写板子或舵机。';
  /* 这行改成「出事了才说话」。正常打通时桥给的是一句
     「正在读取真实板卡；姿态可跟随板子运动。」—— 而上一行的实时跟随徽章、参考电压、
     读取 Hz 已经把同一件事说了三遍，再复述一句只是占地方，所以正常就整行收起来。
     用 hidden 而不是写空字符串：空 <p> 还留着 2px 边距，收不干净。
     真正只有这里看得到的，是故障那几句：错误码、快照时间戳不一致、采样过期多少毫秒。
     桥在 live 那条路上只发那一句固定的话（imu_bridge.decode_snapshot 最后一个分支），
     所以「有效且非演示」就等于没话说；哪天 live 路上开始带别的提示，改的是这里。*/
  const note = valid
    ? (demo ? '软件合成姿态，不是板子读数。' : '')
    : `${status}，躯干保留最后一帧；${imuFeed.connected ? p.message || '等待新的有效采样。' : '与服务的 WebSocket 已断开，正在重连。'}`;
  $('imuMessage').textContent = note;
  $('imuMessage').hidden = !note;
}
function acceptImuFeed() {
  if (imuRevision === imuFeed.revision) return;
  imuRevision = imuFeed.revision;
  if (!imuFeed.enabled) imuAttitude = new ImuAttitude();
  else if (!imuFeed.connected || !imuFeed.packet) imuAttitude.disconnect();
  else imuAttitude.receive(imuFeed.packet, performance.now());
  renderImu();
}
window.addEventListener('imu-feed', acceptImuFeed); acceptImuFeed();
setInterval(() => { imuAttitude.expire(performance.now()); renderImu(); }, 250);
/* 安装方向那个下拉删了（8 个选项，实机用的一直是默认的 y90，没人动过；而且它本来就不保存，
   刷新就回默认）。换基那步照旧，取的是 imu_attitude.js 里的 DEFAULT_MOUNT —— 芯片要是哪天
   换个方向装，改那个常量就行，不必再从页面上选。setMount()/mountQuaternion() 留着：
   test_imu_attitude.cjs 靠它们验换基的数学。*/
$('imuZero').addEventListener('click', () => { if (imuAttitude.zero()) { renderImu(); addLog('IMU 当前姿态设为显示零位（仅本页，不写硬件）', 'IMU'); } });
$('imuRaw').addEventListener('click', () => { if (imuAttitude.showRaw()) { renderImu(); addLog('IMU 切换到原始姿态显示（仅本页）', 'IMU'); } });
// 整块姿态浮在 3D 画布上挡着鸭子，给个折叠：只藏细节和说明，标题与角度留下。
// 纯粹是本页的显示开关 —— 不碰 imuAttitude，读数、3D 躯干姿态、日志都照常。
$('imuFold').addEventListener('click', () => {
  const folded = document.getElementById('imuPanel').classList.toggle('folded');
  $('imuFold').textContent = folded ? '展开' : '收起';
});

const canvas = document.getElementById('view');
const renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
const scene = new THREE.Scene();
scene.background = new THREE.Color(0x0f1412);
const camera = new THREE.PerspectiveCamera(40, 1, 0.01, 10);
camera.up.set(0, 0, 1);
const controls = new OrbitControls(camera, canvas);
controls.enableDamping = true;
// 视线中心压低一点（target.z 0.1 → 0.02），鸭子整体在画面上抬高约 80px，
// 好让底部那条通栏滑块浮层不压住它的脚。相机位置没动，只是看的目标点低了。
function homeCam() { camera.position.set(0.62, -0.52, 0.34); controls.target.set(0, 0, 0.02); controls.update(); }
homeCam();
window.duckResetCam = homeCam;
scene.add(new THREE.HemisphereLight(0xdfe8e2, 0x1a221e, 1.1));
const key = new THREE.DirectionalLight(0xffffff, 1.4); key.position.set(0.5, -0.6, 0.9); scene.add(key);
const fill = new THREE.DirectionalLight(0x8fb8ff, 0.5); fill.position.set(-0.6, 0.4, 0.3); scene.add(fill);
const grid = new THREE.GridHelper(0.6, 12, 0x2c3a33, 0x1e2925); grid.rotation.x = Math.PI / 2; scene.add(grid);
// A parent at the trunk origin rotates the whole kinematic tree. Joint pivots stay independent.
const trunkAttitude = new THREE.Group(), robotContent = new THREE.Group();
trunkAttitude.name = 'imu-trunk-attitude'; trunkAttitude.add(robotContent); scene.add(trunkAttitude);
// 官方姿势叠加：同一套网格再搭一份，半透明蓝色，按 OFFICIAL_POSES 的角度摆；躯干跟实体共用 IMU 姿态
const ghostRoot = new THREE.Group(), ghostJoints = {};
ghostRoot.name = 'official-pose-ghost'; ghostRoot.visible = false; trunkAttitude.add(ghostRoot);
const ghostMat = new THREE.MeshStandardMaterial({ color: 0x5aa9ff, transparent: true, opacity: 0.28, depthWrite: false, roughness: 0.8 });

const joints = {};   // servo id -> {pivot, axis, dir}
const hud = document.getElementById('viewHud');
const massInfo = [], soleInfo = [];   // 重心：每个 body 的质量/质心；脚底：sole 网格包围盒角点（踝体系）
let comMark = null, comLine = null, comRing = null, soleLoops = [], supportBox = null, comText = document.createElement('div');
// 给个 id：它一行通栏钉在画布底边，窄屏（CSS 里画布不再是一屏高）要改成贴窗口底边，得有个能选中的钩子
comText.id = 'comText';
comText.style.cssText = 'position:absolute;left:10px;right:10px;bottom:10px;font-size:12px;color:#8fa098;background:rgba(22,29,26,.85);padding:6px 10px;border-radius:6px;pointer-events:none';
document.getElementById('viewWrap').appendChild(comText);
function buildComMarkers() {
  if (!massInfo.length) return;
  comMark = new THREE.Mesh(new THREE.SphereGeometry(0.005, 16, 12), new THREE.MeshBasicMaterial({ color: 0x3df0a3 })); scene.add(comMark);
  comLine = new THREE.Line(new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(), new THREE.Vector3()]), new THREE.LineDashedMaterial({ color: 0x3df0a3, dashSize: 0.006, gapSize: 0.004 })); scene.add(comLine);
  comRing = new THREE.Mesh(new THREE.RingGeometry(0.004, 0.007, 24), new THREE.MeshBasicMaterial({ color: 0x3df0a3, side: THREE.DoubleSide })); scene.add(comRing);
  soleInfo.forEach(() => { const l = new THREE.LineLoop(new THREE.BufferGeometry().setFromPoints([0, 1, 2, 3].map(() => new THREE.Vector3())), new THREE.LineBasicMaterial({ color: 0xf0b43d })); scene.add(l); soleLoops.push(l); });
  supportBox = new THREE.LineLoop(new THREE.BufferGeometry().setFromPoints([0, 1, 2, 3].map(() => new THREE.Vector3())), new THREE.LineBasicMaterial({ color: 0x8fa098, transparent: true, opacity: 0.6 })); scene.add(supportBox);
}
const _v = new THREE.Vector3();
function updateCom() {
  if (!comMark) return;
  let total = 0; const c = new THREE.Vector3();
  massInfo.forEach(m => { _v.copy(m.com); m.inner.localToWorld(_v); c.addScaledVector(_v, m.mass); total += m.mass; });
  c.divideScalar(total);
  // Include the IMU parent transform: all masses and sole corners are in world coordinates.
  // This is a model AABB projection, not measured foot contact or a physical stability test.
  let gz = Infinity, xmin = Infinity, xmax = -Infinity, ymin = Infinity, ymax = -Infinity;
  soleInfo.forEach((s, i) => {
    const pts = s.corners.map(p => { const w = p.clone(); s.inner.localToWorld(w); return w; });
    pts.forEach(p => { gz = Math.min(gz, p.z); });
    const fx = pts.map(p => p.x), fy = pts.map(p => p.y);
    const a = Math.min(...fx), b = Math.max(...fx), c2 = Math.min(...fy), d = Math.max(...fy);
    xmin = Math.min(xmin, a); xmax = Math.max(xmax, b); ymin = Math.min(ymin, c2); ymax = Math.max(ymax, d);
    soleLoops[i].userData = { a, b, c2, d };
  });
  soleLoops.forEach(l => { const u = l.userData; l.geometry.setFromPoints([new THREE.Vector3(u.a, u.c2, gz + 0.0005), new THREE.Vector3(u.b, u.c2, gz + 0.0005), new THREE.Vector3(u.b, u.d, gz + 0.0005), new THREE.Vector3(u.a, u.d, gz + 0.0005)]); });
  supportBox.geometry.setFromPoints([new THREE.Vector3(xmin, ymin, gz + 0.0003), new THREE.Vector3(xmax, ymin, gz + 0.0003), new THREE.Vector3(xmax, ymax, gz + 0.0003), new THREE.Vector3(xmin, ymax, gz + 0.0003)]);
  const inside = c.x >= xmin && c.x <= xmax && c.y >= ymin && c.y <= ymax;
  const col = inside ? 0x3df0a3 : 0xf05a5a;
  comMark.material.color.setHex(col); comRing.material.color.setHex(col); comLine.material.color.setHex(col);
  comMark.position.copy(c); comRing.position.set(c.x, c.y, gz + 0.001);
  comLine.geometry.setFromPoints([c, new THREE.Vector3(c.x, c.y, gz)]); comLine.computeLineDistances();
  const toe = (xmax - c.x) * 1000, heel = (c.x - xmin) * 1000, left = (ymax - c.y) * 1000, right = (c.y - ymin) * 1000;
  comText.textContent = `模型重心投影${inside ? '在' : '不在'}脚底包围矩形内 · 世界 +X/−X/+Y/−Y 余量 ${toe.toFixed(0)}/${heel.toFixed(0)}/${left.toFixed(0)}/${right.toFixed(0)} mm（模型 ${(total * 1000).toFixed(0)} g；非实物平衡判定${imuFeed.enabled && !imuAttitude.valid ? '；IMU 保留最后帧' : ''}）`;
  comText.style.color = inside ? '#8fa098' : '#f05a5a';
}

async function loadModel() {
  const mj = await (await fetch('model/model.json')).json();
  const bin = await (await fetch('model/meshes.bin')).arrayBuffer();
  const geoms = {};
  for (const [name, m] of Object.entries(mj.meshes)) {
    const q = new Int16Array(bin, m.voff, m.nverts * 3);
    const pos = new Float32Array(m.nverts * 3);
    for (let i = 0; i < m.nverts; i++) for (let k = 0; k < 3; k++) {
      const span = m.bbox_max[k] - m.bbox_min[k] || 1;
      pos[i * 3 + k] = m.bbox_min[k] + (q[i * 3 + k] + 32768) / 65535 * span;
    }
    const idx = m.index_bytes === 2 ? new Uint16Array(bin, m.ioff, m.ntris * 3) : new Uint32Array(bin, m.ioff, m.ntris * 3);
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
    g.setIndex(new THREE.BufferAttribute(idx, 1));
    g.computeVertexNormals();
    geoms[name] = g;
  }
  const objs = [];
  const rootBody = mj.bodies.find(b => b.parent < 0);
  if (rootBody) {
    trunkAttitude.position.set(...rootBody.pos);
    robotContent.position.copy(trunkAttitude.position).negate();
  }
  const mjq = q => new THREE.Quaternion(q[1], q[2], q[3], q[0]);  // MuJoCo w x y z
  mj.bodies.forEach((b, i) => {
    const node = new THREE.Group(); node.name = b.name;
    node.position.set(...b.pos); node.quaternion.copy(mjq(b.quat));
    let inner = node;
    if (b.joint) {
      const pivot = new THREE.Group(); pivot.name = 'joint:' + b.joint.name; node.add(pivot); inner = pivot;
      if (b.joint.id != null) joints[b.joint.id] = { pivot, axis: new THREE.Vector3(...b.joint.axis).normalize(), name: b.joint.name };
    }
    for (const g of b.geoms) {
      const mat = new THREE.MeshStandardMaterial({
        color: new THREE.Color(g.rgba[0], g.rgba[1], g.rgba[2]), roughness: 0.6, metalness: 0.05, flatShading: true,
        transparent: g.rgba[3] < 1, opacity: g.rgba[3]
      });
      const mesh = new THREE.Mesh(geoms[g.mesh], mat);
      mesh.position.set(...g.pos); mesh.quaternion.copy(mjq(g.quat)); inner.add(mesh);
    }
    objs.push({ node, inner });
    if (b.parent < 0) robotContent.add(node); else objs[b.parent].inner.add(node);
    if (b.mass) massInfo.push({ inner, mass: b.mass, com: new THREE.Vector3(...b.com) });
    if (b.sole) {
      const mb = mj.meshes[b.sole.mesh], lo = mb.bbox_min, hi = mb.bbox_max, R = mjq(b.sole.quat), P = new THREE.Vector3(...b.sole.pos);
      const corners = [];
      for (const x of [lo[0], hi[0]]) for (const y of [lo[1], hi[1]]) for (const z of [lo[2], hi[2]]) corners.push(new THREE.Vector3(x, y, z).applyQuaternion(R).add(P));
      soleInfo.push({ inner, corners });
    }
  });
  // 官方姿势的半透明副本：只要网格和关节，不参与重心计算
  ghostRoot.position.copy(robotContent.position);
  const ghostObjs = [];
  mj.bodies.forEach(b => {
    const node = new THREE.Group(); node.position.set(...b.pos); node.quaternion.copy(mjq(b.quat));
    let inner = node;
    if (b.joint) {
      const pivot = new THREE.Group(); node.add(pivot); inner = pivot;
      if (b.joint.id != null) ghostJoints[b.joint.id] = { pivot, axis: new THREE.Vector3(...b.joint.axis).normalize() };
    }
    for (const g of b.geoms) {
      const mesh = new THREE.Mesh(geoms[g.mesh], ghostMat); mesh.renderOrder = 2;
      mesh.position.set(...g.pos); mesh.quaternion.copy(mjq(g.quat)); inner.add(mesh);
    }
    ghostObjs.push({ inner });
    if (b.parent < 0) ghostRoot.add(node); else ghostObjs[b.parent].inner.add(node);
  });
  buildComMarkers();
  needRender = true;
  // hud.textContent = `模型：${mj.source}（${Object.keys(joints).length} 个关节跟随舵机${(mj.notes || []).length ? '；' + mj.notes.join('；') : ''}）`;
}
loadModel().catch(e => { hud.textContent = '模型没加载：' + e.message + '（跑 build_model.py 生成 model/）'; });

const RAD = 2 * Math.PI / 4096;
// 有变化才画：模型 40 多万个三角面，一直按 60 帧画会把显卡和主线程占满（点按钮半天才弹确认框）。
// 关节角、官方叠加、躯干姿态、视角、窗口大小有一样变了才重画；静止时每帧只比一下签名
let lastPoseSig = '', lastTrunkSig = '', needRender = true;
controls.addEventListener('change', () => { needRender = true; });
function pose() {
  let sig = showWhat;
  for (const [id, j] of Object.entries(joints)) {
    const src = showWhat === 'goal' ? goalPos : rawPos;
    const raw = src[id]; if (raw === undefined) continue;
    const ang = (raw - 2048) * RAD * (dirs[id] || 1);
    j.pivot.quaternion.setFromAxisAngle(j.axis, ang);
    sig += ',' + raw + (dirs[id] || 1);
  }
  // 官方姿势直接是模型约定的弧度，不经过舵机读数和 ±
  const key = window.duckGhost, official = key && OFFICIAL_POSES[key];
  ghostRoot.visible = !!official;
  if (official) for (const [id, j] of Object.entries(ghostJoints)) j.pivot.quaternion.setFromAxisAngle(j.axis, official.rad[id] ?? 0);
  sig += '|' + (key || '');
  if (sig === lastPoseSig) return false;
  lastPoseSig = sig; return true;
}
function resize() {
  const w = canvas.clientWidth, h = canvas.clientHeight;
  if (canvas.width !== w * renderer.getPixelRatio() || canvas.height !== h * renderer.getPixelRatio()) { renderer.setSize(w, h, false); camera.aspect = w / h; camera.updateProjectionMatrix(); needRender = true; }
}
function frame() {
  resize();
  let dirty = pose();
  if (!imuFeed.enabled) trunkAttitude.quaternion.identity();
  else if (imuAttitude.valid) trunkAttitude.quaternion.fromArray(imuAttitude.target);
  const q = trunkAttitude.quaternion, qsig = `${q.x.toFixed(5)},${q.y.toFixed(5)},${q.z.toFixed(5)},${q.w.toFixed(5)}`;
  if (qsig !== lastTrunkSig) { lastTrunkSig = qsig; dirty = true; }
  if (controls.update()) dirty = true;          // 阻尼还在滑的时候返回 true
  if (dirty || needRender) { scene.updateMatrixWorld(true); updateCom(); renderer.render(scene, camera); needRender = false; }
  requestAnimationFrame(frame);
}
frame();
