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
# 开发机：交叉编译（只有 robotd/robotctl 变了，app-server 没动）
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

## 7: 2026-10-16 「11 万次故障」到底是什么

### 7.1 现象

```
robot     unhealthy: 111169 consecutive bus read failures
  bus       111169 consecutive read failures
  imu       not ready, 551 stale reads — 1 in 297
```

`feedback.error = "bus transaction failed: Feetech: ID 10: alarm 0x01"`、`control_valid = false`；
journal 里 11307 行 `WARN robotd: bus read failed error=… ID 10: alarm 0x01 consecutive=N`，N 每秒涨 50。
看起来像总线炸了。**不是。**

### 7.2 总线是好的：那 11 万行是同一条旧消息被重放

`consecutive_errors` 是 **robotd 主循环**的计数器（[main.rs:2000](microduck/robotd/src/main.rs#L2000)），数的是 `safety.read()` 失败；
而 `FeetechIo::read()` 在 `state.fatal` 非空时**永远返回 Err**（[feetech.rs:571](microduck/duck-control/src/feetech.rs#L571)），
`feedback["error"]` 也来自同一个 `state.fatal`（:585）。

**锁死之后这个数字只会一直涨，它不代表总线还在出错。** 看总线真实状况要看 worker 那边的计数器：

| 证据 | 值 | 说明 |
|---|---|---|
| `transport.complete_reads` | 315972 → 334666 | 一直在涨 |
| `transport.failed_reads` | 8703 → 8707 | 18694 次读里 4 次，0.02% |
| `states[]` 15 颗 | `status` 全 0、7.7–7.8 V、28–30 °C、`valid: true` | **没有一颗在报警** |
| `imu_valid` | true | 小板在答 |

那 11 万行是**同一条** `state.fatal` 被 50 Hz 重放（[main.rs:2005](microduck/robotd/src/main.rs#L2005) 每 10 次打一行）；
里面的「ID 10」是 10:00:58 那一刻的，不是持续的。

> **判据**：故障锁死期间，`health` 的 `consecutive bus read failures` **没有诊断价值**。
> 要看 `transport.complete_reads` 涨不涨、`states[]` 里 `status` 是不是 0。

### 7.3 同时还有一件真事：小板掉线 174 次

journal 里从 10:00:58.780 起，`imu 小板不应答了` / `imu 小板回来了` 成对出现，**174 回**。
过去 4 个 boot 的掉线次数是 **0、0、0、174** —— 史上第一次。

每次都**正好 2.007 秒后回来**，那正是 `IMU_RETRY_INTERVAL`（[imu.rs:304](microduck/duck-control/src/imu.rs#L304)）。
**说明小板一直活着，只是不发。** 不是断线——是它的发送卡住了（小板要等总线空闲才发），
一个 2 秒的空档正好把它解开。

时间线：

| 时刻 | 事件 |
|---|---|
| 10:00:58.723 | 舵机瞬时告警 `ID 10: alarm 0x01` → **锁死**电机故障 |
| 10:00:58.780 | 小板第一次掉线（相隔 **56 ms**） |
| 10:00:58 → 10:36:26 | 小板反复掉/回 **174** 轮 |
| **10:36:26** | 小板最后一次回来 —— **主人这时接上了电池** |
| 10:44 | `robot.clear_fault` 被拒（见 7.4） |
| 10:46:58 | 重启 → 故障清掉 |

**两个症状同源，都指向 10:00:58 那一下；而把它彻底治好的是 10:36 接电池，不是重启。**
（一开始把「不再掉线」记在重启头上是错的。）

**根因（2026-10-16 主人确认）：10:36:26 之前整机是 5 V USB 供电，没接电池。** 接上电池，
56 ms 内一起爆掉的两件事同时停了。

但 journal 里有个反证要说清楚：**量到的电压从头到尾没动过**。`battery_v` 是舵机自己报的
总线电压（[main.rs:3325](microduck/robotd/src/main.rs#L3325) 的 `slow_sensors`，5 分钟一次、
EMA 平滑）：09:51 = 7.69 V，10:01（事故那一刻）= 7.67 V，10:36:46（接了电池之后）= 7.67 V。
**接电池那一下没有台阶。** 所以这不是「电压偏低」触发的报警，是**电源站不住**——瞬时带不动，
舵机报一次电压告警、小板被反复顶复位。同一个软脚电源，同一秒。

> 还悬着一个接线细节：既然 10:36 之前舵机总线上已经有 7.67 V，那 5 V USB 供的是 SBC
> （和/或小板那个单独的插头），舵机总线另有 ~7.7 V 来源。这条得看接线图才能钉死。

**接上电池之后也不是就没事了 —— 所以上面那条根因没定死。** 11:48:49 小板又静了一次，
**8 分 07 秒**（11:56:56 才回来），那时已经在电池上。8 分钟这个长度也不像原来那种抖：
抖动每次 2 秒就回来，而这一次是**每一轮试探都失败**，整段没人应答。主人对当时有没有碰过
接线、动过鸭子是「没准、忘了」，所以这条就先摆着。

两件事同时起、同时停仍然说明 10:00:58 那一下是**同一个原因**；但它是不是「没接电池」，
现在只能说最像，不能说就是。

### 7.4 只读配置下，故障复位是死的

`robot.clear_fault` → `reset_hardware_fault`（[main.rs:1406](microduck/robotd/src/main.rs#L1406)）
→ `Safety::clear_fault` → `FeetechIo::clear_fault`（[feetech.rs:610](microduck/duck-control/src/feetech.rs#L610)）
→ `relax_and_revalidate` → `set_torque(false)`
→ **`if !self.allow_motion { return Err("read-only configuration") }`**（[feetech.rs:1276](microduck/duck-control/src/feetech.rs#L1276)）。

实测返回：`{"accepted":false,"reason":"bus transaction failed: Feetech: read-only configuration"}`。

而 `state.fatal = None` 全文件**只有一处**（[feetech.rs:619](microduck/duck-control/src/feetech.rs#L619)），就在 `clear_fault` 里。

**结论：`allow_motion: false` 时，一旦锁死就只能重启进程。**
复位这条路在当前配置下是死的 —— 而只读恰恰是最需要能复位的那种配置。

### 7.5 收尾数据（重启后 21 分钟）

| | 值 |
|---|---|
| 读总数 / 失败 | 126890 / **9**（0.0071%，全是良性舵机帧毛刺） |
| `error` | null |
| `control_valid` / `imu_valid` | true / true |
| 小板 `misses` / `reconnects` / `dropped` | **0 / 0 / false** |
| 小板 `block_reads` | 126881 —— **一次都没漏** |
| journal「不应答了」 | **0 条** |

对照出事时：每几秒漏 5 次、累计 `misses=1300`、掉线 174 回。

### 7.6 还摆着的五个问题（没动代码）

| | 问题 | 位置 |
|---|---|---|
| **A** | 只读配置下故障复位无效（见 7.4）——锁死只能重启 | `feetech.rs:1276` / `:619` |
| **B** | 瞬时**电压**报警 `0x01` **第一次就锁死**，而**角度**报警 `0x02` 有 5 次重试——不对称 | `feetech.rs:423` vs `:431` |
| **C** | 试探失败后 `dropped_at` 已是 `Some`，`misses` 继续涨但**不再打日志**，且 200 永远留在读表里、每轮白等满 12 ms | `imu.rs:366`（`wants_block`）+ `record_miss` 里的 `first` 判据 |
| **D** | 主循环把锁死的旧消息以 50 Hz 重放——「11 万次」就是这么来的，很误导人 | `main.rs:2005` |
| **E** | 小板一掉线，`read()` 就失败，`consecutive_errors` 照涨（2026-10-16 拔板测试里爬到 1810+ 再自己归零）——**跟 B/D 是同一个坑**：那个数字不是总线诊断。好在这条会自愈，不锁存 | `feetech.rs:576` + `main.rs:2000` |

**判据（自己看的时候够用）**：`transport.last_error` 里 `missing_ids` 是 1–15 的舵机号 = 老毛刺，不用管；
出现 `200` = 小板出事。
