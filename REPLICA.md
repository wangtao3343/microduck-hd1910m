# 这里记录了整个复刻的流程

板子：Radxa ZERO 3W（aarch64 / Armbian 26.2.1 trixie / Python 3.13.5），`root@192.168.3.10`。

---

## 1: 拷贝 fanhao375 项目中的舵机调试工具 servo-web

### 1.1 改舵机 ID

调试台管 **1–15**，映射跟 `radxa/installation.json` 一致：

| ID | 关节 | ID | 关节 |
|---|---|---|---|
| 1 | right_ankle（右踝） | 9 | left_hip_roll |
| 2 | right_knee（右膝） | 10 | left_hip_yaw |
| 3 | right_hip_pitch | 11 | head_pitch |
| 4 | right_hip_roll | 12 | neck_pitch |
| 5 | right_hip_yaw | 13 | head_roll |
| 6 | left_ankle（左踝） | 14 | head_yaw |
| 7 | left_knee（左膝） | 15 | mouth（嘴） |
| 8 | left_hip_pitch | | |

分三段：**右腿 1–5、左腿 6–10、头颈嘴 11–15**。

代码里落在 `tools/servo-web/server.py` 的 `DEFAULT_IDS = "1-15"` 和 `JOINT_NAMES` 两张表；页面分组按关节名前缀走，不按 ID。

### 1.2 同步到板子

```bash
cd /home/ai/microduck-hd1910m
rsync -av \
  --exclude='.venv' --exclude='logs' --exclude='__pycache__' --exclude='.pytest_cache' \
  tools/servo-web/ root@192.168.3.10:/root/servo-web/
```

**四个 `--exclude` 一个都不能省**，尤其第一个：开发机的 `.venv` 是 **x86_64 / Python 3.12**，到 aarch64 板上根本跑不了；更坏的是它会让 `start.sh` 以为环境已就绪，跳过重建。必须让板子自己建。

源路径末尾的 `/` 是必须的，否则会套成 `servo-web/servo-web/`。

## 2: 在板子上运行舵机调试程序

### 2.1 先停 robotd

调试台和本体运行时（`robotd`）**抢同一个串口，同一时间只能一个用**：

```bash
systemctl stop microduck-hd1910
```

### 2.2 启动

```bash
cd /root/servo-web
./start.sh --port /dev/ttyS2 --http-port 9090
```

第一次运行会自己建 `.venv` 并装依赖（板上能连 PyPI，要几分钟）。板上 `python3` **没有 pip 模块**（Debian 13 的 PEP 668），但 `venv` 模块是好的，所以不用额外装 `python3-pip`。

### 2.3 实测结果

```
舵机调试台 · 真机 · /dev/ttyS2
http://127.0.0.1:9090
局域网 http://192.168.3.10:9090 · Ctrl-C 停
[系统] 舵机调试台 v0.16.10 启动，日志 logs/servo-web-2026-10-15.log
[总线] 串口 /dev/ttyS2 @ 1000000 已打开
[总线] 扫描 1..15：在线 15 颗 [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15]
[IMU] 状态 live/live：正在从舵机总线读 ID 200（跟主控同一条 sync_read）
```

15 颗全在线，IMU 小板（ID 200）也在同一条总线上。

### 2.4 看页面

浏览器开 **`http://192.168.3.10:9090`**。

3D 是从 `cdn.jsdelivr.net` 拉的 three.js —— **开页面的那台机器要能上外网**，否则 3D 空白。后端和舵机不受影响。

## 3: 串口是哪条

板子的舵机总线是 **`/dev/ttyS2`**（40 脚排针上的 UART2），中间接一块 **URT-2** 做**单线半双工转换**。

- **不要用 `/dev/hd1910-servo`** —— 那是 USB 转串口适配器的 udev 别名（`/etc/udev/rules.d/99-hd1910-servo.rules`）。板上没插 USB 适配器，这个设备根本不存在。
- **也不要以为 ttyS2 能直连舵机** —— 瑞莎的 UART 是全双工两根线，飞特总线是单线半双工，中间必须有转换。URT-2 就是干这个的。

## 4: 本体运行时（robotd）

### 4.1 板上真正在跑的是什么

```
out/arm64/bin/robotd                 ← 核心。50 Hz 控制循环：读舵机 + 跑 ONNX 策略 + 写目标角
out/arm64/bin/microduck-app-server   ← 后端 API，绑 0.0.0.0:38880
out/arm64/bin/robotctl               ← 命令行工具，经 Unix socket 跟 robotd 说话，按需跑
```

策略是 `radxa/references/reference_runtime_20261005/` 里那四个 ONNX：**walk / getup / pick / roulade**。
`.venv` 在板上唯一的用途是提供 `libonnxruntime.so`（见 `scripts/run_hardware.sh`）。

### 4.2 部署哪些

`scripts/deploy.py` 只搬三样：

```
scripts   radxa   out/arm64
```

`microduck/` 里的 Rust 源码在**开发机交叉编译**，**源码不上板**，只搬 `out/arm64/bin` 下的二进制。

### 4.3 板上端口配错了（2026-10-15 已修）

板上 `local/feetech.json` 写的是：

```json
"port": "/dev/hd1910-servo",
```

这是 `scripts/configure.py` 的**默认值**（给 USB 适配器用的），不是为这块板子选的。板上没这个设备，所以 robotd 永远起不来，日志里每秒一条：

```
WARN robotd: waiting for native Feetech/BNO085; startup reads only
     error=bus transaction failed: Feetech: No such file or directory  attempt=N
```

改成：

```json
"port": "/dev/ttyS2",
```

再 `systemctl restart microduck-hd1910`。**改前先 `cp` 一份备份，改错了覆盖回去重启就还原。**

> **2026-10-15**：改完了，板上现在是 `"port": "/dev/ttyS2"`，服务从 degraded 转 healthy。
> 备份在 `local/feetech.json.bak-preimu`。**注意 `scripts/configure.py` 第 49 行的 `--port`
> 默认值还是那个坏别名** —— 哪天重跑它配一遍，这个修复会被冲掉。

启动路径全程只读（`sync_read` 读固件版本/限位/扭矩状态/增益，一次 `sync_write` 都没有），进控制循环前还会显式关扭矩，所以接管总线本身不会让舵机动。真正要动，得 `allow_motion` 为 true，而那要求 `installation.json` 里 `calibration_verified` 和 `imu_pose_verified` 都是 true —— 现在两个都是 false。

### 4.4 顺带记一笔：板上没有 BNO085

`feetech.json` 里 `imu_bus: /dev/i2c-4`、`imu_address: 75` 是上游用 BNO085 的配置。这块板上 **i2c-4 整条总线是空的**（`i2cdetect -y 4` 全 `--`），IMU 是挂在**舵机总线上冒充 ID 200** 的那块小板。

好在这不影响启动：`open_imu()` 只建对象、不校验芯片在不在，所以 IMU 缺失不会挡住 robotd，只是 `hardware_health.imu.ready` 会一直是 false。

> **2026-10-15**：这条已经被第 6 节取代 —— 现在不用 BNO085 了，从总线读 ID 200。

### 4.5 一共有哪些 daemon，装了哪些（2026-10-17 查）

`microduck-hd1910.service` 是板上**唯一**的 unit，`ExecStart` 跑 `scripts/run_hardware.sh`，
那个脚本起两个进程就结束。仓库 `microduck/` 里其实有 **7 个 daemon**，`microduck_app` 那边还有个后端：

| unit | 干什么 | 板上 | 需要吗 |
|---|---|---|---|
| `robotd` | 控制循环 / 舵机 / IMU / 策略 | **跑着** | **必需**，没它鸭子就是块铁 |
| `microduck-app-server`（`microduck_app/`） | 手机 app 后端，绑 `0.0.0.0:38880` | **跑着**（同一个 unit 里起） | 要用手机 app 就要 |
| `padd` | 手柄意图客户端（`gilrs`） | **跑着**（2026-10-17 起） | **加手柄时要它** |
| `btd` | 蓝牙 BLE 传输适配 | 没装 | 手机走 LAN 就不用；走蓝牙才要 |
| `configd` | 配 wifi / 身份 | 没装 | 现在是手改配置文件，不用 |
| `tofd` | 头上 ToF 传感器 | 没装 | 有 ToF 硬件才要 |
| `mediad` | 摄像头 + WebRTC 网关 | 没装 | 见 §9，没摄像头不做 |
| `updaterd` | 更新守护 | **masked** | 我们是手动部署，不用 |

**让鸭子走路，只需要 `robotd`。** 其余全是可选的，缺了不影响它走。

unit 文件在各组件自己的 `systemd/` 下（`microduck/{btd,configd,padd,tof,mediad,updater}/systemd/`）。
`scripts/build.sh` 只编 `-p robotd -p robotctl`（外加 app-server）；`padd` 单独编（见 §6.5），
`btd` / `configd` / `tofd` / `mediad` / `updaterd` **从来没生成过**。

板上另外有两个非仓内工具（2026-10-17 装）：
`padd.service`（手柄，单元在 `microduck/padd/systemd/padd-runtime.service`）和
`bt-watch.service`（蓝牙活体记录，脚本在 `tools/bt-watch/`，卡死时把现场落到 `/var/log/bt-watch/`）。

## 5: 关节的正负方向（`direction`）

### 5.1 这个字段是什么

`installation.json` 每条关节一个 `direction`，**只能是 ±1**（`feetech.rs:79` 校验）。读和写都乘它：

```
读：q     = (ticks - zero_ticks) × 2π/4096 × direction
写：ticks = zero_ticks + q ÷ (2π/4096) × direction
```

15 条的 `zero_ticks` 全是 2048（舵机自己的电气中点，4096 线/圈），所以这个文件里唯一携带信息的，就是这一列。

`+1` = 读数变大就是模型里的正方向，`-1` = 反的。它由**舵机在支架里的装配朝向**决定 —— 同一颗 HD1910M，翻 180° 装，符号就相反。

### 5.2 同一个约定存了三份，前两份互相矛盾

| 文件 | 谁在用 | 11/12 的值 |
|---|---|---|
| `radxa/installation.json` | 本体运行时 robotd | **+1** |
| `tools/servo-web/directions.json` | 调试台（3D、站姿、序列都按它算） | **-1** |
| `microduck_rl/config/static_home_20261004/reference.json` | 训练/仿真侧的参考记录 | **+1** |

`directions.json` 的 `_note` 自己就写着这个矛盾，并注明「本机编号 1-15 还没在实物上核过」。

### 5.3 实测：本机 15 颗全是 -1

**结论：`radxa/installation.json` 里 `neck_pitch`(12) 和 `head_pitch`(11) 的 `+1` 是错的，应该是 `-1`。**

测法（就是 `_note` 里给的那条）：在调试台拖 ID 11/12 的滑块，看真机动的方向和 3D 里转的方向是不是同一边 —— 同边就对，反了就把 `±` 翻过来。

这个测法**能**分辨符号，靠的是滑块走**原始 ticks**（`app.js:191`，`min="0" max="4095"`），而 `±` 只改 3D 怎么解释读数（按钮 title：「3D 显示方向取反」）。真机动的方向不受 `±` 影响，两边一比就知道谁对。

> 反例：如果滑块走的是角度就不行了 —— 指令和显示套的是同一张方向表，怎么翻都一致，永远测不出来。

### 5.4 为什么这个错要紧

符号反了是**双重反**：策略说「抬头」，鸭子低头；鸭子真抬头时，策略读到的是低头。正反馈，自己跟自己打架。策略是按正确符号训出来的，一开 motion 这两个关节就会失控。

现在没发作，只因为 `calibration_verified: false` 把 motion 锁着（`feetech.rs:96`）。

### 5.5 那两行 `+1` 是哪来的

`static_home_20261004/reference.json` 里也是 `+1/+1`，一模一样。那份的 `source` 写着：

```
/home/robot/workspace/yannuo/huggingface/radxa/reports/home_upright_20261004_121919/capture.jsonl
```

是从**上游那台鸭子**的真实采集里提取的（1016 帧、30 秒 HOLD）。所以 `direction` 这一列记的是**那台机器当时的映射**，被照抄进了本仓库的 `installation.json`。

本机实测是 `-1`，说明**上游那台和这台，颈/头两处舵机的装配方向是相反的**。

所以：`reference.json` 是采集记录，**不能改**（改了就是伪造记录）；要改的是本机的 `installation.json`。

### 5.6 该改哪里、不该改哪里

**该改：**

- ~~板上 `/home/duck/workspace/microduck-hd1910m/radxa/installation.json`~~ —— **2026-10-15 已改**，板上现在 15 颗全 `-1`。目录里没留备份，但原件在开发机仓库里（sha256 `89af0f61…`），丢不了。
- 开发机 `radxa/installation.json` 第 14、15 行：`"direction": 1` → `-1`。**目前是 `-1`，跟板上一致了。**
- `tools/servo-web/directions.json` 的 `_note`：把「还没在实物上核过……跟这里不一致」改成「已核过，全 -1」。

**不该改：**

`microduck_rl/config/static_home_20261004/reference.json`。它是**记录**不是**配置**：

- 由 `microduck_rl/scripts/calibrate_static_home.py` 从一份 `capture.jsonl` 提取出来的，`direction` 是把采集里的 `feedback.joints` 原样抄下来的。改了就跟它自己的 `source_sha256` 对不上了。
- 全仓库唯一的消费者是 `microduck_rl/scripts/run_static_voltage_ab.py` —— 开发机上跑 MuJoCo 训练的电压域 A/B，把 `name/id/direction/zero_ticks` 抽出来写进一个**临时训练目录**。跟板子、串口、robotd 全无关系；**机器人运行时只读 `installation.json`，从不读它。**
- 它的 sha256（`183aedcd…`）钉在 `docs/source_manifest.json` 里，跟文件当前内容一致 —— 是发布快照的一份子。（`scripts/audit_release.py` 是**重新生成**这份 manifest 的，所以改它当场不会报错，静悄悄地就把快照改掉了。）

**哪天想在你这台上跑这个电压实验**，正确做法是重新生成一份：用这台的 `installation.json` 采一份 `capture.jsonl`，跑 `calibrate_static_home.py` 输出到新目录（比如 `static_home_<日期>/`），不是就地改这一份。

改上面那三处不影响任何现在能跑的东西（motion 还锁着），只是排掉一个埋着的雷。

## 6: 让 IMU 走舵机总线（`imu_source`）

### 6.1 为什么

上游 `feetech.rs` 把 IMU 换成独立 I²C 上的 BNO085，这块板上没有那颗芯片，所以 IMU 一直死值、控制永远判为不可用。板上真 IMU 是 `imu_to_dxl` 小板：STM32 + LSM6DSV16X，挂在**舵机总线上冒充 ID 200**，数据块在地址 56、长度 15（= 舵机自己那个窗口），波特率跟舵机一样 1 Mbps。

### 6.2 接口

`feetech.json` 加一格：

```json
"imu_source": "bus"          // "i2c"（默认）| "bus"
```

`#[serde(default)]`，缺省 `i2c` —— 老配置不写这一格，行为跟以前**逐字一样**，还是走 BNO085。

### 6.3 三个关键点

**① 200 排最后，跟舵机共用同一条 `sync_read`。** 这不是性能问题，是电气问题：小板收到指令 50 µs 后就开口答（21 字节 = 210 µs），而**飞特舵机的自然应答延时比 50 µs 还短** —— 200 放前面时，舵机会在小板还在驱动半双工总线的时候就开口，两个器件同时驱动，字节互相冲掉，**每轮固定坏两帧**。200 排最后时小板走排队分支，等 15 颗答完再发，16 帧 336 B 全过，只多 210 µs。

**② 寄存器 65 那条报警规则要给 200 开旁路。** 原代码有条规则：读的窗口跨过寄存器 65，就把 `row[65-address]` 当报警字节。三种帧形 `(40,31)` `(55,16)` `(56,15)` 下这一格**都**落在 IMU 块的**四元数 y 高字节**上 —— 不排除 200 的话，每帧都会误报一个报警。所以要按 id 判。

**③ 小板缺席必须能降级，这是配套不是可选项。** 同包读意味着小板的死活进了舵机那次读的结果。连续 5 次因为它缺席而失败，就把 200 从请求表里摘掉、`imu_valid = false`，**舵机照读**；之后每 2 秒试挂回一次。全程**永不锁电机故障** —— 本仓库的既定态度是 IMU 坏了不能把电机带下去。

### 6.4 mount：埋着 90° 的那一格

**`installation.imu_mount_wxyz` 是配给 BNO085 的，不是配给小板的。** 这颗雷难排就在这里：两个都是**单位四元数**，所以 `BusImu::new` 的归一化那道闸**拦不住拿错**，只会静悄悄地把侧躺的重力喂给策略。

| mount | 谁在用 | 站着时反推的重力 | 离正立 |
|---|---|---|---|
| `[√0.5, 0, √0.5, 0]`（`SflpDecoder::DEFAULT_MOUNT`） | `bus.rs`，以及现在的 bus 臂 | `[-0.08, -0.01, -1.00]` | **3.9–4.8°** ✅ |
| `[0.5, -0.5, 0.5, -0.5]`（`installation.json`） | `Bno08x::open` —— 这格唯一真正的读者 | `[-0.08, -1.00, 0.01]` | 90.4° ❌ |

反向也验过：鸭子侧躺时运行时报 `gravity = [-1, 0, 0]`、`fallen = true`；放正后自己清成 3.9°、`fallen = false`。**一正一反两次都对上**，这条钉死了。

**`radxa/installation.json` 没动** —— 那一格对 BNO085 是对的。bus 那条路改成硬编码 `DEFAULT_MOUNT`，跟官方 `bus.rs` 读同一块小板时一致。加了测试 `the_bus_arm_uses_the_boards_own_mount_not_the_installations` 钉住，以后有人「顺手」把它接回安装文件那一格会当场红。

### 6.5 上板部署：顺序不能反

`Config` 是 `deny_unknown_fields`，所以**先送新二进制，再加配置项**。反过来 robotd 会直接拒绝加载配置，整个运行时起不来。

```bash
# 开发机：cargo 不在默认 PATH 里，先 export —— 否则 build.sh 第 15 行报
# `cargo: command not found`。每个新 shell 都要来一次。
export PATH="$HOME/.cargo/bin:$PATH"

# 开发机：交叉编译（只有 robotd/robotctl 变了，app-server 没动）
# 产物：out/arm64/bin/{robotd,robotctl,microduck-app-server}
bash scripts/build.sh arm64

# 板上：备份 → 停 → 送。配置先不动，确认新二进制本身没毛病
ssh root@192.168.3.10
  cp -a .../out/arm64/bin/robotd .../out/arm64/bin/robotd.bak-20261013
  systemctl stop microduck-hd1910
# 开发机：
rsync -a --chown=duck:duck out/arm64/bin/robotd out/arm64/bin/robotctl \
      root@192.168.3.10:/home/duck/workspace/microduck-hd1910m/out/arm64/bin/

# 板上：先按现状起一次（预期还是报 BNO085，但进程活着）—— 把「新二进制的毛病」
# 和「总线 IMU 的毛病」分开
systemctl start microduck-hd1910

# 板上：加 "imu_source": "bus"，再重启一次
python3 -c '...'      # 改 local/feetech.json；备份留在 feetech.json.bak-preimu
systemctl restart microduck-hd1910
```

**`--chown=duck:duck` 不能省**：以 root 直接 rsync 会把二进制写成 root 属主，而服务是 duck 跑的。

回退：`mv robotd.bak-20261013 robotd` + 删掉那一格 + 重启。

**`build.sh` 只编 `robotd` / `robotctl` / `microduck-app-server`。** `padd`（手柄）带 C 依赖，
要单独编 —— 先 export sysroot（做法见 `手柄连接.md`「编 padd」）：

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cd ~/microduck-hd1910m/microduck
export PKG_CONFIG_SYSROOT_DIR=/home/ai/arm64-sysroot
export PKG_CONFIG_PATH=/home/ai/arm64-sysroot/usr/lib/aarch64-linux-gnu/pkgconfig
export PKG_CONFIG_ALLOW_CROSS=1
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
cargo build --release --locked --target aarch64-unknown-linux-gnu -p padd
# 产物：target/aarch64-unknown-linux-gnu/release/padd
# 放进 out/arm64/bin —— build.sh 不管 padd，deploy.py 又只搬 out/arm64，
# 少了这一步板上会缺 padd
install -m755 target/aarch64-unknown-linux-gnu/release/padd ../out/arm64/bin/
scp ../out/arm64/bin/padd root@192.168.3.10:/tmp/padd.new
ssh root@192.168.3.10 'install -o duck -g duck -m0755 /tmp/padd.new \
      /home/duck/workspace/microduck-hd1910m/out/arm64/bin/padd && systemctl restart padd'
```

链接时的 `libcap.so.2 … not found` 无害，板上运行时解析。
`cargo test -p padd` 在开发机上跑不起来（本机没装 x86_64 的 `libudev-dev`），不是代码问题。

### 6.6 验收（2026-10-15 实测）

| 项 | 结果 |
|---|---|
| `imu_valid` / `imu_age_s` | ✅ true / 1.5–6.3 ms（阈值 0.15 s） |
| 有没有 `ID 200: alarm 0x??` | ✅ **0 次** —— 旁路①在真硬件上守住了 |
| 姿态 | ✅ 站着 3.9°；`fallen` 随真实姿态 true↔false |
| 读预算（`SCHEDULED_READ_BUDGET` = 12 ms） | ✅ `transport.read_ms = 5.27 ms` |
| 出错率 | 54097 次读：`read_errors=2`、`misses=2`、`dropped=false`、`reconnects=0`、`frozen_run=0` |
| 日志 | ✅ 切到 bus 之后一条 WARN/ERROR 都没有 |

**一件已补做、一件没说清的：**

- **拔板测试**（计划里第 6 条）：**2026-10-16 补做了，见第 6.7 节。** 顺带回答了原来那个顾虑——小板
  **不串在总线中间**，拔它一颗舵机都不影响。（2026-10-16 那场事故等于**非受控地**跑了一遍，见第 7.3 节。）
- `transport` 里有 **6 次 `incomplete feedback ... missing_ids=[15, 3]`**（54097 分之 6，0.011%）。缺的是**舵机 15 和 3，不是 200** —— 所以不是小板把舵机挤掉了。加小板之前是不是也这样，**没有基线，未知**；频率很低且自恢复（`recovery.attempts = 0`）。

### 6.7 拔板测试（2026-10-16 实测）

计划里第 6 条，补做。做法：拔小板**自己那个插头** → 数十几秒 → 插回；板上跑一个采样器，
每 ~0.5 秒把 `feedback` 的关键字段记一行（`/tmp/pulltest.log`，跑完即弃）。

| 时刻（板子本地） | 事件 |
|---|---|
| ~11:21:45 | 拔掉 |
| 11:21:55.245 | `WARN duck_control::imu: imu 小板不应答了；先把它从舵机读表里摘掉，等它回来 misses=5` |
| 11:21:55 → 11:22:31 | 小板缺席 **36 秒** |
| 11:22:31.592 | `INFO duck_control::imu: imu 小板回来了，重新加入读表` |

| 验收项 | 结果 |
|---|---|
| 舵机有没有被带下去 | ✅ 缺席那 36 秒里 `complete_reads` 207557 → 212425，一次没断 |
| 缺的到底是谁 | ✅ 每次失败帧都是 `missing_ids=[200]`、`received_bytes=330`（= 15 颗 × 22 B）——**15 颗全答了** |
| 降级痕迹 | ✅ WARN + `dropped=true` + `imu_valid=false` + 本地 `misses` 到 5 |
| 试探节奏 | ✅ 36 秒里 17 次试探 ≈ 2.1 秒一次（对上 `IMU_RETRY_INTERVAL`），期间 `reconnects` 一直 0 |
| 插回去多快回来 | ✅ 2 秒内；`reconnects` 0→1、`imu_valid`→true、`error`→null |
| 有没有锁电机故障 | ✅ 没有。`state.fatal` 只有[一个写点](microduck/duck-control/src/feetech.rs#L317)，而它[放过 `ImuUnavailable`](microduck/duck-control/src/feetech.rs#L316) |

**代价**：36 秒里 17 次失败读 / 约 5000 次 = 0.34%，每次白等 12 ms；这期间 `control_valid=false`
（这是要的效果，不是副作用）。

**两处读表时别误会的：**

- `imu_transport.misses` 是 [`miss_total`](microduck/duck-control/src/imu.rs#L483)，**累计值，永不归零**——
  拔过一次，它就一直 ≥ 那一次的总数。要看「现在还在不在丢」得看 `dropped`。
- `imu_transport.ready` 是 [`decoder` 见过够多四元数没](microduck/duck-control/src/imu.rs#L487)，
  **不是小板在不在**；小板拔了它照样 `true`。

**中途我说错一句，收回：** 按「`feedback.error` 变非 null = 锁死」喊了停。其实不是。`error` 平时确实是
`state.fatal`（[feetech.rs:585](microduck/duck-control/src/feetech.rs#L585)），但
[feetech.rs:601](microduck/duck-control/src/feetech.rs#L601) 在 `fatal` 为 null 时会把 **IMU 自己的错**
填进去——所以那 36 秒里的 `error` 是降级消息，不是故障锁存。

**这次没验到的**：故意重插**没有复现抖动**——一次干净的掉线、一次干净的回来。所以 7.3 节那 174 次抖动，
**不能**就此归因于插头接触不良。

### 6.8 改了哪些、没 commit

```
duck-control/src/feetech.rs    imu_source / ImuSource / 报警旁路 / 缺席降级 / 读 200
duck-control/src/imu.rs        BusImu、SflpDecoder 抽出、StaleImuTracker 挪进来共用
duck-control/src/bus.rs        纯搬家（改用共用的 StaleImuTracker）
duck-control/src/lib.rs        导出 ImuSource / ImuSourceKind
robotd/src/main.rs             native_imu 类型换成 ImuSource
robotd/src/imu_only.rs         bus 模式下 imu: 模式明确报错，不装作能读
scripts/configure.py           生成的 config 里加 imu_source='i2c'
```

测试：`cargo test -p duck-control` **139 通过 0 失败**；`robotd` **145 通过 + 3 ignored**，集成 **7 通过**。

**全程没 commit、没 push。** `radxa/installation.json` 没动，板上 `/root` 没动，`allow_motion` 全程 false，没写 EEPROM、没开过扭矩。

---

## 7: 2026-10-16 的两件真事

> **原来这一大段「11 万次故障」已经删了 —— 那是测试过程里的假象，不是故障。**
> 锁死之后 `consecutive bus read failures` 只是同一条 `state.fatal` 被 50 Hz 重放
> （[main.rs:2005](microduck/robotd/src/main.rs#L2005)），数字每秒涨 50，**没有诊断价值**。
> 要判断总线真实状况，看 `transport.complete_reads` 涨不涨、`states[]` 里 `status` 是不是 0。

### 7.1 IMU 小板老是掉 —— **接线不良（已定案）**

症状：`imu ready, N stale reads — 1 in M` 的 M 很小；journal 里 `imu 小板不应答了` /
`imu 小板回来了` 成对刷，最狠一次静了 **8 分 07 秒**。

**根因：小板那个连接器的焊接/压接不可靠。换成原厂 AMP2.0 线就好了，鸭子能走。**

| | `1 in M` |
|---|---|
| 换线前（2026-10-16 21:01 实测） | **13** |
| 换线后（2026-10-16 22:21 重启后累计） | **1133** |

差约 87 倍。当时很像、但**不是**的两个方向：

- **不是小板本身** —— 它的 `block_reads` 一次都没漏（126881 / 126881）
- **不是时钟** —— 小板上的 `X1`（16 MHz 有源晶振）虽然按硬件说明 2026-09-08 起默认贴片、
  使能脚固定接 3.3 V（MCU 关不掉它），但固件 `BOARD_USE_HSE_BYPASS 0` 走内部 HSI16，
  **它没进时钟树**（`tools/imu_to_dxl/firmware/Core/Inc/board_config.h:7`）。
  有源振荡器输出脚虚焊的症状是「时钟整个没了」，跟这里那种零星坏字节对不上。

`0x80` 那批报警大概率也是同一处：接头松的时候小板会去抢那条半双工总线，
冲掉的字节落在谁的帧里就算谁的。代码那边已经改过，见 7.3。

### 7.2 只读配置下，故障复位是死的

`robot.clear_fault` → `reset_hardware_fault`（[main.rs:1406](microduck/robotd/src/main.rs#L1406)）
→ `Safety::clear_fault` → `FeetechIo::clear_fault`
→ `relax_and_revalidate` → `set_torque(false)`
→ **`if !self.allow_motion { return Err("read-only configuration") }`**（`feetech.rs` 的 `clear_fault`）。

实测返回：`{"accepted":false,"reason":"bus transaction failed: Feetech: read-only configuration"}`。

而 `state.fatal = None` 全文件**只有一处**，就在 `clear_fault` 里。

**结论：`allow_motion: false` 时，一旦锁死就只能重启进程。**
复位这条路在当前配置下是死的 —— 而只读恰恰是最需要能复位的那种配置。

### 7.3 五个问题（B 已修，其余没动代码）

| | 问题 | 位置 |
|---|---|---|
| **A** | 只读配置下故障复位无效（见 7.2）——锁死只能重启 | `feetech.rs` 的 `clear_fault` |
| **B** | ~~瞬时**电压**报警 `0x01` **第一次就锁死**，而**角度**报警 `0x02` 有 5 次重试——不对称~~ **2026-10-16 已修并上板**（commit `ba26c5a`）：`is_transient_alarm` 让 `0x01` / `0x02` / `0x80` 都走五次重试（50 Hz 下约 100 ms），过温 `0x04`、过流 `0x08` 仍**第一下就锁**。`0x80` 不是飞特定义的任何报警位，全天落在 5 个互不相干的舵机上（14/13/9/4/5），而 `0x08` 只落在腿上 —— 代价不对称，忽略它的最坏结果是继续走，采信它的是断扭矩、鸭子摔 | `is_transient_alarm` |
| **C** | 试探失败后 `dropped_at` 已是 `Some`，`misses` 继续涨但**不再打日志**，且 200 永远留在读表里、每轮白等满 12 ms | `imu.rs:366`（`wants_block`）+ `record_miss` 里的 `first` 判据 |
| **D** | 主循环把锁死的旧消息以 50 Hz 重放——「11 万次」就是这么来的，很误导人 | `main.rs:2005` |
| **E** | 小板一掉线，`read()` 就失败，`consecutive_errors` 照涨（2026-10-16 拔板测试里爬到 1810+ 再自己归零）——**跟 B/D 是同一个坑**：那个数字不是总线诊断。好在这条会自愈，不锁存 | `feetech.rs:576` + `main.rs:2000` |

**判据（自己看的时候够用）**：`transport.last_error` 里 `missing_ids` 是 1–15 的舵机号 = 老毛刺，不用管；
出现 `200` = 小板出事。

## 8: 2026-10-16 打开 motion —— 鸭子真的站起来了

### 8.1 复核方向：实物上 15 颗逐颗掰过

把鸭子放平、**扭矩全关**，用手逐颗掰 1–15，同时看调试台的 3D 是不是往同一边转。
读数用一个只读的 `/ws` 跟随脚本（`/tmp/wstail.py`，**只收不发**，一条指令都不发），
把每颗的变化打成 `ID n 旧 → 新  ±delta` 一行。

每颗都出了干净的单关节 delta（例：ID 4 `1990→1869`、ID 12 `1783→1347`、ID 11 `1920→2100` 穿过 2048）。

**结论：15 颗全 `-1`，跟 `tools/servo-web/directions.json` 和板上 `radxa/installation.json`
两边都一致，一颗没翻。**

顺带清掉一处过期说明：`directions.json` 的 `_note` 原来写着「本机 1-15 还没在实物上核过：
radxa 里写的是 11/12 为 +1、其余 -1，跟这里不一致」。**那份不一致早就不存在了** ——
板上 11/12 早在 `34effa3` 就翻成了 −1，只是 `0f19aa7` 写的说明没跟着改。`_note` 已按本次实测重写。

### 8.2 零点：只到「旁证级」，**没做过硬校准**

手法：把鸭子摆成折叠的样子放桌上，读 15 颗，跟官方折叠位比。做了三次（越摆越像）。

| 第几次 | 除头颈外 13 颗平均\|偏差\| | 最大 |
|---|---|---|
| 1（摆得不像） | 15.4° | 63.0°（右踝） |
| 2 | **3.0°** | 12.7°（左踝） |
| 3 | **2.5°** | 6.7°（左踝） |

**左右对称性比绝对偏差更能说明问题**：膝左右差 **0.26°**（读到 −2043 tick，官方镜像值 −2046）——
两颗膝是同一个机械限位顶住的，对得这么齐说明**膝的零点没问题**。髋横滚 0.9°、髋偏航 2.6° 也好。
踝左右差 15.7°、髋俯仰差 9.6°，但这两个在折叠位**不是接触点**（踝是 0、脚悬着），
差的是**手摆的余量**，不是零点。

> **一个会骗人的坑：头/颈那 −56°/−60° 不是零点错。** 页面用的基准 `FOLD_CALIB` 把 ID 11/12
> **故意改成 0**，基准名字就叫「官方折叠（头颈摆正）」—— 理由是官方 FOLD 里颈/头的 57°
> 不是接触点、眼睛摆不准（[server.py:828](tools/servo-web/server.py#L828)）。
> 而实物按真折叠摆时头颈就是收到 1 rad = 57.3°（1396 tick），所以那一列会显示差 57°。
> 按 `OFFICIAL_FOLD` 比才对：三次读到 11/12 = `1411/1366`、`1479/1396`，**差 1.3° / 0.0°**。
> **拿页面那列直接读，会以为头颈零点偏了 60°。**

**判决：零点「大致对」，落在旁证级** ——
顶零件的（膝、髋横滚/偏航）1–3°、左右对称到 0.26°；靠重力的（头/颈）1–3°；悬空的（踝、髋俯仰）十几度。
**没硬校准过**（硬校准要机械接触自定位的那种折叠位，这三次都是手摆的）。

### 8.3 打开 motion：三个开关，**一行代码没改**

| 文件 | 字段 | 改成 |
|---|---|---|
| `local/feetech.json` | `allow_motion` | `true` |
| `radxa/installation.json` | `calibration_verified` | `true` |
| `radxa/installation.json` | `imu_pose_verified` | `true` |

然后 `systemctl restart microduck-hd1910`。**三个必须一起改**：`allow_motion` 是那道闸，
另外两个是钥匙 —— 只翻 `allow_motion` 而不翻那两个，[feetech.rs:113](microduck/duck-control/src/feetech.rs#L113)
会直接返回 `motion requires verified joint and IMU installation`，**服务根本起不来**。

这是上游故意设的**人工签字闸门**（设计意图：没人确认装好了就别让鸭子动）。三个开关合起来松开三处：
`set_torque`（[:1276](microduck/duck-control/src/feetech.rs#L1276)）、`write`（[:1240](microduck/duck-control/src/feetech.rs#L1240)）、
`feedback()["motion_available"]`（[:1197](microduck/duck-control/src/feetech.rs#L1197)）。

改前备份在 `local/feetech.json.bak-premotion` 和 `radxa/installation.json.bak-premotion`，
覆盖回去 + restart 就回到只读。

**实测（2026-10-16）**：
- 重启后**一颗舵机都没自己动**（前后两帧逐颗比过）—— 启动路径读当前姿势当参考并保持
- app 上「使能站立」从灰变可点，**点完鸭子自己站了起来**
- 随后 `motion_available` / `homed` / `policy_enabled` 全 true，`error: null`，`servo_gains_verified` true
- 跟站姿的差：14 颗 ±2° 内；踝（−6.5/+6.8）和髋俯仰（+6.1/−5.6）差 6–7° ——
  **那是重力压着 PD 的静态误差，不是零点问题**（腿在承重）
- 重力 `[0.026, −0.016, −1.0]`，立得正、水平 → 顺带再确认 ID 200 的 mount 是对的（见 6.4）

> **使能 = 只把关节摆到站姿，鸭子不会自己起来 —— 起点必须是「大致站着」。**
>
> `robot.init` / app 的「使能站立」做的永远是同一件事：开扭矩，把 15 个关节**从当前读数
> 线性插值**到 `DEFAULT_POSITION`（站姿，[model.rs:39](microduck/duck-control/src/model.rs#L39)），
> `HOME_RAMP = 2 秒`（[main.rs:94](microduck/robotd/src/main.rs#L94)）。
> **纯插值 —— 没有接触判断、没有平衡，而且摔倒了也不拦**
> （[main.rs:2391](microduck/robotd/src/main.rs#L2391)：`the fall verdict gates nothing, so the pose
> ramp moves a robot lying on the floor like any other tick`）。
>
> **实测（2026-10-16，两种起点）**：
>
> | 起点 | 结果 |
> |---|---|
> | **折着趴着** | **站起来了** —— 腿在身子底下，把腿驱到站姿就是把身子**顶**起来（脚着地 = 站起来） |
> | **躺着** | **起不来** —— 只是「关节摆成了站姿、鸭子还在地上」，得人工扶起来。躺平时身子在地上、腿在旁边，驱到站姿只是把腿摆成那个形状；随后行走策略接手的是一个「躺着但腿摆着站姿」的怪状态 |
>
> **规则：使能之前鸭子得先大致是站着的姿势，不是就先用手摆。**
> 躺着进了那个怪状态时，先按停止 / relax，别让它跑策略。
>
> 本来有安全网 —— 启动时若判定「坐着」（`leg_deviation > 0.30 rad`），代码会走 **sitstand 网络**
> 起身、绕过这条盲 ramp（`begin_boot_rise()`，[main.rs:2069](microduck/robotd/src/main.rs#L2069)）——
> 但本机 `params.toml` 里 `sitstand = "none"`，`has_sitstand()` 为假，那条路**没装**
> （[main.rs:1779](microduck/robotd/src/main.rs#L1779)）；`limp_fall: false`，「摔下去之前先变软」
> 那套也关着。**开机前把鸭子摆到站姿附近，或人在旁边接。**

**遗留的诚实说明**：`calibration_verified: true` 是**我们替它签的字**，不是测出来的
（见 8.2，只到旁证级）。`radxa/installation.json` 自己的 `purpose` 字段原文仍是
"Read-only wiring/mapping example."。

### 8.4 手机 app：板上只有后端，没有界面

`microduck_app/backend/`（`microduck-app-server`）是**手机 app 的后端**，不是 app 本身 ——
unit 名字就写着 `MicroDuck HD1910 native runtime and APK backend`。

| 它干什么 | 怎么做 |
|---|---|
| **发界面** | `ServeDir` 指向 `MICRODUCK_APP_WEB`，默认 `../web/dist`。**板上 `/home/duck/workspace/web/dist` 不存在**，所以它实际没在发界面，纯当 API 用 |
| **让 app 找到鸭子** | UDP 38882 收广播 `MICRODUCK_DISCOVER_V1`，回 JSON（名字、端口、要不要 token） |
| **转发命令** | WebSocket `/ws`，bind `0.0.0.0:38880`（[run_hardware.sh](microduck/scripts/run_hardware.sh) 里 `MICRODUCK_APP_ALLOW_OPEN_LAN=1` 且不设 token → 局域网谁都能连），经 `MICRODUCK_ROBOTD_SOCKET` 转给 robotd |

它跟 `microduck/` 那个 workspace 是**两个独立工程**。板上搜不到 `.apk`，手机 app 是装在手机上的。

命令白名单 + 限幅在 [main.rs:20](microduck_app/backend/src/main.rs#L20)：
`move`(vx≤0.30 / vy≤0.04 / vyaw≤0.8)、`mouth`、`head`、`skill`、`stop`、`enable`、`clear_fault`、`claim_control`。
`claim_control` 决定唯一控制者；每条命令带 `_time`，**超过 300 ms 丢弃 = 死手开关**（app 卡住或断网自动停）。

> **`clear_fault` 就是 app 上那个「复位故障」按钮**（[main.rs:221](microduck_app/backend/src/main.rs#L221)），
> 转给 robotd 的正是不久前把总线锁死的 `robot.clear_fault`（7.2 节）。**打开 motion 之后这条路通了**
> —— `set_torque(false)` 不再被拒，所以那个死锁自己解开了，以后遇到故障按它有效，不用重启服务。

## 9: 待办 —— 装了摄像头之后跑 `mediad`

**触发条件：给鸭子装上摄像头（`/dev/video*` 出现）。在那之前不做。**

`mediad` 是上游 workspace 里的一个 Rust 守护进程（[microduck/mediad/](microduck/mediad/)），
摄像 + 麦克风 + WebRTC。它**没编、也没装到这台板子上** —— [scripts/build.sh:15](scripts/build.sh#L15)
只编 `-p robotd -p robotctl`。这是故意的，不是漏了。

它提供什么：

| | |
|---|---|
| `http://<robot>:8080/` | **控制台网页**（`--web-port`，默认 8080）—— 就是之前记得的那个 8080 |
| `<robot>:8443` | WebRTC 信令（`--port`，默认 8443） |
| 网页里 | 摄像头画面 + 一条 `control` 数据通道直通机器人 API，**浏览器里就能遥控鸭子** |
| 配置 | `robotd.toml` 的 `[media]` / `[detect]` 两节，跟 robotd 读同一个文件 |

**现在不做，是因为这三条：**

1. **板上没有摄像头** —— `/dev/video*` 不存在，它最主要的那一半是空的
2. **缺 `webrtcsink` 插件** —— GStreamer 1.26.2 在（它要求 ≥1.22），但这个插件要从
   gst-plugins-rs 自己编；交叉编译还得用 `scripts/cross-sysroot.sh` 那套 sysroot，
   整个 workspace 都跟着它
3. **它不鉴权** —— 源码原话：「Anyone who reaches the signalling port can drive the robot and
   see its camera」，而且默认绑 `0.0.0.0`

**装好摄像头之后要做的（顺序别反）：**

1. 先确认 `/dev/video0` 出得来、`v4l2-ctl --list-formats-ext` 能看到格式
2. 编 `webrtcsink`（gst-plugins-rs，版本跟板上 1.26.2 对齐）
3. `cargo build -p mediad`，加进 `scripts/build.sh` 的 `-p` 列表，并补一个 systemd unit
4. `robotd.toml` 里配 `[media]`（camera / 分辨率 / 帧率 / 码率）
5. 上板验证：`http://<board>:8080/` 能开、能出画面、网页里的 control 能驱动
6. **鉴权没有就是没有** —— 只在内网用，别直接暴露到公网

它自己写着「**不在恢复路径上**」：mediad 起不来，鸭子照走、照能更新、照样能用蓝牙。
所以它坏了随时停掉就行，不影响控制。

参考：`microduck/docs/design/remote-webrtc.md`、`microduck/docs/design/webrtc-console.md`。
