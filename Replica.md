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

### 4.3 待修：板上端口配错了

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

启动路径全程只读（`sync_read` 读固件版本/限位/扭矩状态/增益，一次 `sync_write` 都没有），进控制循环前还会显式关扭矩，所以接管总线本身不会让舵机动。真正要动，得 `allow_motion` 为 true，而那要求 `installation.json` 里 `calibration_verified` 和 `imu_pose_verified` 都是 true —— 现在两个都是 false。

### 4.4 顺带记一笔：板上没有 BNO085

`feetech.json` 里 `imu_bus: /dev/i2c-4`、`imu_address: 75` 是上游用 BNO085 的配置。这块板上 **i2c-4 整条总线是空的**（`i2cdetect -y 4` 全 `--`），IMU 是挂在**舵机总线上冒充 ID 200** 的那块小板。

好在这不影响启动：`open_imu()` 只建对象、不校验芯片在不在，所以 IMU 缺失不会挡住 robotd，只是 `hardware_health.imu.ready` 会一直是 false。

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
