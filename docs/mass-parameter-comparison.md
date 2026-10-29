# 鸭子质量参数对照：四份工程

记录日期 2026-10-28。对照四份：

- **A** = 本仓库 [microduck_rl](../microduck_rl)，pollen-robotics 上游 + HD1910 集成
- **B** = `/Users/wangtao/Work/Microduck-kissqy/microduck-hd1910/training-console`，kissqy 训练控制台
- **C** = `/Users/wangtao/Work/microduck/microduck_rl_wt`，另一份 microduck_rl 工作树，**已按实称质量改过模型**
- **D** = `/Users/wangtao/Work/microduck_rl_retrain`，**A 的 HD1910 框架 + C 的实测质量模型**，已训完 8000 迭代并导出 ONNX

用途：训前按实机校正质量参数时，知道该信哪个数、改哪个文件。

## 结论

15 个刚体的 CAD 拓扑和舵机 M6 动力学参数**四份完全一致**。差异全在舵机单颗质量：

| | 舵机单颗 | 整机 | 依据 |
|---|---|---|---|
| A | 21 g | 0.782243 kg | 数据手册标称 |
| B | 23 g | 0.820000 kg | 用户 2026-10-01 确认 |
| **C** | **24.6 g** | **0.875873 kg** | **2026-10-09 实称整机 841.2 g 反推** |
| **D** | **24.6 g** | **0.875873 kg** | 直接沿用 C 的 XML（字节相同），已实际用于训练 |

三个数不是互相矛盾，是**统计口径不同**：

```
21 g   = 厂家标称（容差 ±2 g）
22.8 g = C 实称「裸舵机」
23 g   = B 的「本机 23g」——约等于裸舵机
24.6 g = C 实称「装好的舵机」，含舵盘、含线
```

**仿真里那颗 XL330 的 18 g 是裸舵机重量**，所以换成 HD1910 时应该用 **24.6 g**（含舵盘）+ 线，
而不是裸重。C 用 24.6 g 后，实称 841.2 g 与模型只差 5 g（0.6%），这 5 g 残差反过来证明
CAD 的结构件质量是对的。

**D 是这四份里唯一把 0.875873 kg 真正拿去训练过的**：4096 个环境、8000 次迭代、
导出 `hd1910_slowgait_retrain_model_7999.onnx`。它不是"另一套参数"，就是 C 的模型。

## 1. 整机质量

| 项目 | A | B | C | D |
|---|---|---|---|---|
| CAD 基准（15 刚体） | 0.737243 kg | 0.737243180 kg | 0.737243 kg（同一份） | 与 C 同一文件（`bbc69197`） |
| 舵机替换基准 | XL330 18 g → **21 g** | XL330 18 g → **23 g** | XL330 18 g → **24.6 g** | **24.6 g**（同 C） |
| 每颗增量 | +3 g | +5 g | **+6.6 g** | **+6.6 g** |
| 15 颗总计 | +45 g | +75 g | **+99 g** | **+99 g** |
| 头部电子件 | 无 | 无 | **+39.63 g**（Radxa 15.2 + 总线板 29.7 − CAD 板 5.27） | **+39.63 g** |
| 额外闭合修正 | 无 | jaw_soft +3 g、trunk_base +4.75682 g | 无（5 g 残差只记录不入模型） | 无 |
| **训练用总质量** | **0.782243 kg** | **0.820000 kg** | **0.875873 kg** | **0.875873 kg** |
| rollers 变体 | 0.736830 | — | 0.875460 | 0.875460 |
| 来源性质 | 数据手册 | 用户口头确认 | **实称 841.2 g** | 复用 C，**已实际训练** |

C 的推导（commit `96f5682` 提交信息原文）：

```
实机（含电池、不含头部电子件）实称 841.2 g，仿真 737.24 g，差 104.0 g
    15 × (24.6 − 18.0) = +99.0 g   ← 舵机
                        + 5.0 g   ← 结构件残差（≈0，说明 CAD 结构是对的）

841.2 g 里没有装那 44.9 g 电子件（Radxa 15.2 + 飞特总线板/降压 29.7）
```

出处：

| 数值 | 位置 |
|---|---|
| CAD 求和（A/B/C 相同） | `*/src/mjlab_microduck/robot/microduck/robot_walk.xml` 15 个 `<inertial>` |
| 21 g（A） | A `actuator/reference_hd1910_profile.json:9`，容差 `mass_tolerance_kg: 0.002` |
| +3 g/颗（A） | A `actuator/reference_hd1910.py:136` |
| 23 g（B） | B `training/recipe.py:12` |
| 820 g（B） | B `training/recipe.py:23` |
| 24.6 g（C） | C `scripts/set_hd1910_masses.py:47` `SERVO_NEW_G` |
| 实称 841.2 g（C） | C `scripts/set_hd1910_masses.py:5`、commit `96f5682` |
| 24.6 g / 875.873 kg（D） | D `robot/microduck/robot_walk.xml`（SHA `bbc69197` = C） |
| D 的训练规模与产物 | D `logs/rsl_rl/microduck_hd1910_velocity/…_hd1910_slowgait/model_7999.pt`、`exports/hd1910_slowgait_retrain_model_7999.onnx` |

## 2. 逐刚体质量

「A / B / C」都是应用后的最终值。A 按代码推算（无编译快照），B 和 C 是从模型或脚本读出的。

**D 不单列一列**：D 的 `robot_walk.xml` 与 C 字节相同（SHA `bbc69197e39a…`），
所以 D 的 15 个刚体质量、质心、惯量与上表 C 列**逐位一致**，没有第二套数。

| 刚体 | 舵机数 | CAD | A（21 g） | B（23 g+闭合） | C（24.6 g+电子件） |
|---|---|---|---|---|---|
| trunk_base | 2 | 0.199224 | 0.205224 | 0.213981 | **0.212424** |
| yaw2roll | 1 | 0.0230406 | 0.0260406 | 0.0280406 | 0.0296406 |
| hip_l | 0 | 0.00618934 | 0.00618934 | 0.0061893 | 0.00618934 |
| upper_leg_left | 2 | 0.0482067 | 0.0542067 | 0.0582067 | 0.0614067 |
| leg | 1 | 0.0215844 | 0.0245844 | 0.0265844 | 0.0281844 |
| ankle_left | 0 | 0.0300246 | 0.0300246 | 0.0300246 | 0.0300246 |
| neck | 2 | 0.0368414 | 0.0428414 | 0.0468414 | 0.0500414 |
| neck_pitch | 0 | 0.00572 | 0.00572 | 0.00572 | 0.00572 |
| yaw_roll_motion | 1 | 0.0486 | 0.0516 | 0.0536 | 0.0552 |
| jaw_soft（头） | 2 | 0.188766 | 0.194766 | 0.201766 | **0.241596** |
| bearing_roll | 1 | 0.0230406 | 0.0260406 | 0.0280406 | 0.0296406 |
| hip_l_2 | 0 | 0.00618934 | 0.00618934 | 0.0061893 | 0.00618934 |
| upper_leg_right | 2 | 0.0482067 | 0.0542067 | 0.0582067 | 0.0614067 |
| leg_2 | 1 | 0.0215844 | 0.0245844 | 0.0265844 | 0.0281844 |
| ankle_right | 0 | 0.0300251 | 0.0300251 | 0.0300251 | 0.0300251 |
| **合计** | **15** | **0.737243** | **0.782243** | **0.820000** | **0.875873** |

三份的 15 颗舵机分配刚体表完全一致：

```
trunk_base 2 | yaw2roll 1 | upper_leg_left 2 | leg 1 | neck 2
yaw_roll_motion 1 | jaw_soft 2 | bearing_roll 1 | upper_leg_right 2 | leg_2 1
```

都不是按 14 个策略关节均分——嘴巴那颗也算。

> ⚠️ C 的 jaw_soft 比 B 多 39.83 g，差在头部电子件（C 把 CAD 里的 Pi Zero 3.47 g + HAT 1.80 g
> 换成了实机的 Radxa 15.2 g + 总线板/降压 29.7 g）。A/B 里这块仍按 CAD 原值算。

## 3. 增重算法差异

| | A | B | C | D |
|---|---|---|---|---|
| 位置 | `actuator/reference_hd1910.py:117-155` | `training/recipe.py:42-52` | `scripts/set_hd1910_masses.py` | 离线同 C；**运行时不做任何增重** |
| 质量 | `old + delta × mesh数` | `old + (count×5 + extra)/1000` | `old − Σold_i + Σnew_i`（逐零件替换） | 已在 XML 里 |
| 质心 | 重算 `(m·c₀ + Σδ·pᵢ)/m` | 不动 | **重算**（先剥结构件再回加） | 已在 XML 里 |
| 惯量 | 平行轴定理重算 | 按比例缩放 | **平行轴定理重算**，结构件惯量单独剥离 | 已在 XML 里 |
| 幂等 | 每次加载都加 | 每次训练都加 | 脚本改一次 XML，改前断言 = XL330 原值，重复运行报错 | XML 已是终值 |
| 头部电子件 | 不处理 | 不处理 | **处理**（Radxa / 总线板替换 CAD 板） | 已在 XML 里 |
| 粒度 | 单颗舵机质点 | 单颗舵机增量 | **每零件独立质量**（舵机/两块板分别给数） | 同 C |

C 最严谨：它是**一次性改写 XML 并带幂等断言**，不是运行时补丁。A 次之（质心会动但只有舵机）。
B 最粗（质心不动、按比例缩惯量）。D 沿用 C 的结果，但**把运行时的补丁整个关掉了**。

### D 的双重计数问题：当前没触发，但是个地雷

D 里仍然保留着 A 的 `reference_hd1910.py` 和 `reference_hd1910_profile.json`，
后者第 9 行还是 `"mass_kg": 0.021`、第 88 行还是 `0.018`——也就是说 **delta 仍然是 3 g/颗**。

D 之所以没算错，是因为它注册的 `Mjlab-Velocity-{Flat,Rough}-MicroDuck-HD1910` 任务
（`tasks/__init__.py:104-110`）用的 `make_microduck_hd1910_velocity_env_cfg`
**只替换执行器、不碰 `spec_fn`**，所以 `adapt_hd1910_spec` 从来没被调用。

⚠️ 一旦有人把 `-HD1910-Reference*` 那套任务重新打开（它会 `robot.spec_fn = make_hd1910_spec`），
就会在这个已经 875.873 g 的 XML 上再加 45 g → **0.920873 kg**，比实测重 4.5%。
D 里的 `hd1910_suite.py` 目前没有被任何模块导入，是安全的；改之前请先确认这一点没变。

## 4. 舵机 M6 动力学参数：四份完全相同

| 参数 | 值 |
|---|---|
| `kt` | 0.6237611235393989 |
| `R` | 4.910191564179625 |
| `armature` | 0.0017819049974475338 |
| `q_offset` | 0.032748109622048015 |
| `command_delay` | 0.006005265953265889 |
| `friction_base` | 0.015803365327573494 |
| `friction_stribeck` | 0.009590058942953754 |
| `max_velocity` | 100.0 |
| `model` / `actuator` | `m6` / `sts3215` |

`actuator: "sts3215"` 只用于选择 BAM 里的飞特控制器实现，不代表参数来自 STS3215。
原始出处是 LuwuDynamics/xgoduck_rl，见 `microduck_rl/src/mjlab_microduck/robot/hd1910/README.md`。

| | 文件 | SHA-256 |
|---|---|---|
| A | `actuator/hd1910_1910_m6.json` | `2ab7e3c5…9ac1`（重新序列化） |
| A radxa 版 | `actuator/radxa_1910_m6.json` | `5cd21f3c…5d84`，仅 `q_offset = 0.0` |
| B | `training-console/training/hd1910_m6.json` | `ef2d51ad…afceb`（上游原件） |
| C | `robot/hd1910/1910_m6.json` | `ef2d51ad…afceb`（上游原件） |
| D（任务实际加载） | `robot/hd1910/1910_m6.json`，由 `microduck_hd1910_env_cfg.py:17,31` 引用 | `ef2d51ad…afceb`（上游原件） |
| D（另一份，未用） | `actuator/hd1910_1910_m6.json` | `2ab7e3c5…9ac1` |

四份**解析后全等**，已逐字段验证。

## 5. 执行器 / 训练配置

| 参数 | A | B | C | D |
|---|---|---|---|---|
| `kp_fw` | 5（`MICRODUCK_BAM_KP` 可覆盖） | 5.0 | 5.0 | 5.0 |
| `vin_range` | (7.4, 8.0) V | [7.4, 8.0] V | (7.4, 8.0) V | (7.4, 8.0) V |
| `vin_min` | 7.0 V | 7.0 V | 7.0 V | 7.0 V |
| `vin_drop_gain` | (0.0, 0.2) | [0.0, 0.2] | (0.0, 0.2) | (0.0, 0.2) |
| 命令延迟 | 3–6 物理步 | 3–6 | 3–6 | 3–6 |
| 控制频率 | 50 Hz | 50 Hz | 50 Hz | 50 Hz |
| 观测 / 动作 | 61 / 14 | 61 / 14 | 61 / 14 | 61 / 14 |
| 齿隙 | **±0.25°** | **±1.0°** | **±1.0°**（XML 注释「2 deg total play (symmetric ±1 deg)」） | **±1.0°**（同 C） |

齿隙这里 **A 是唯一的异类**：B、C、D 都是上游默认的 ±1°；A 按数据手册 ≤0.5° 总间隙取了 ±0.25°
（`reference_hd1910.py:155`）。选了 A 就等于假设齿隙只有别人的 1/4。

D 还多了一条 C 没有的奖励项（不是 DR，是步态整形）：`hd_contact_frequency`，
权重 0.2，惩罚 >8 次/秒的触地切换（约 2 Hz 步频），见
D `src/mjlab_microduck/tasks/microduck_hd1910_env_cfg.py:42-56`。

## 6. 质量域随机化

| | A | B | C | D |
|---|---|---|---|---|
| 默认 | trunk_base ±5% | trunk_base ±5% | 各任务 ±5%（`MASS_INERTIA_RANDOMIZATION_RANGE = (0.95, 1.05)`） | trunk_base ±5%（同 C） |
| 增强包线 | `gait_payload_v8` 时 12 刚体 0.8–1.2 / 0.9–1.1，COM ±8 mm | 无 | 无 | 无 |
| 场景回放 | 4 个 head/back/light 场景 | 无 | 无 | 无 |

## 7. 该用哪个数

**用 C / D 的 24.6 g / 875.87 g**，理由：

1. 只有 C/D 是**实称**，A 是手册值，B 是用户确认但没留下误差分析。
2. 24.6 g 含舵盘和线，与仿真里 18 g 的口径一致（18 g 也是整只舵机件，不是裸机芯）。
3. 换完之后 104 g 的差距只剩 5 g（0.6%），这是四份里唯一有闭合验证的。
4. **D 已经用这套参数跑完了 8000 次迭代并导出 ONNX**，是最接近可用的产物；
   A、B、C 都没有任何训练产物。

但要留意 C 的实称是**那台机器**的数：电池型号、有没有装头部电子件都会变。
你如果换了电池或板子，需要重称。

## 8. 回家在训练机上的执行顺序

训练机要求 Linux + NVIDIA CUDA、Python 3.12。**macOS 跑不了**：mjlab 的
`select_gpus()` 需要 CUDA，脚本硬编码 `cuda:0` 与 `MUJOCO_GL=egl`。

> **先看 D。** `/Users/wangtao/Work/microduck_rl_retrain` 已经有 0.875873 kg 的模型、
> 8000 次迭代的检查点（`model_7999.pt`）和导出好的 ONNX。如果你的实称接近
> 841 g（含电池、不含头部电子件），**不需要从头再来**：直接 play / 部署验证 D 的产物，
> 或者拿它当 warm start。只有实称差得多时，才按下面重做质量。

1. **装电池，量电压。** 记满电与带载电压，确认落在 7.4–8.0 V、低负载不低于 7.0 V。
2. **称整机**（含电池，记清楚头部电子件装没装）。
3. **二选一：**

   **方案甲（推荐，最完整）：移植 C 的质量脚本**
   ```bash
   cp /Users/wangtao/Work/microduck/microduck_rl_wt/scripts/set_hd1910_masses.py \
      microduck_rl/scripts/
   # 按你的实称数改脚本顶部的 SERVO_NEW_G / RADXA_G / BUSBOARD_G
   cd microduck_rl && uv run python scripts/set_hd1910_masses.py --check   # 先看
   uv run python scripts/set_hd1910_masses.py                              # 再改
   ```

   ⚠️ **改完必须关掉 A 自己的增重**，否则**双重计数**：
   A 的 `make_hd1910_spec` 每次加载都会在 XML 现有质量上再加 `mass_kg − old_servo_nominal_mass_kg`。
   XML 已经是 24.6 g 时再 +3 g/颗 = 总质量 0.9209 kg，全错。
   关法：把 `actuator/reference_hd1910_profile.json` 的
   `hardware.mass_kg` 和 `simulation.old_servo_nominal_mass_kg` **都设成 0.0246**，
   delta 归零，`adapt_hd1910_spec` 变成 no-op。
   同时改三条断言的期望值（原为 `.045`）：
   `tests/test_hd1910_reference.py:24`、`tests/test_hd1910_bounded.py:229`、
   `tests/test_radxa_alignment.py:39` → 改成 `pytest.approx(0)` 或按新总质量断言。

   **方案乙（最省事）：只改 profile 一个数**
   `reference_hd1910_profile.json:9` `"mass_kg": 0.021` → `0.0246`，
   整机 0.836243 kg。头部电子件 39.63 g 不建模（比 C 轻 4.5%），交给 DR 包线。
   三条测试断言改成 `.099`。

4. **冒烟训练。**
   ```bash
   cd microduck_rl && uv sync --locked && uv run list-envs

   # BAM M6 入口（推荐）。它先 register_task，再把参数转给 mjlab train。
   uv run python scripts/train_hd1910_bam.py \
     --env.scene.num-envs 64 --agent.max-iterations 5

   # 非 BAM 的平地基线入口，来自原作者文档 hd1910-baseline.md
   uv run --frozen train Mjlab-Velocity-Flat-MicroDuck-HD1910 \
     --env.scene.num-envs 64 --agent.max-iterations 5 \
     --agent.logger tensorboard --agent.run-name hd1910_m6_smoke
   ```

   ⚠️ `docs/training.md` 里写的 `uv run train Mjlab-Velocity-Flat-MicroDuck-HD1910-XgoBam-P6-Slew`
   **跑不起来**：该 task id 只在 `scripts/train_hd1910_bam.py` 里 `register_task()` 之后才存在，
   `mjlab.tasks` 入口不会静态注册它。`uv run list-envs` 查不到属正常。

   日志里确认 `kt=0.6238`、`R=4.9102`，`Episode_Termination/nan_state` 每轮为 0。

5. **正式训练前仍缺的实机数据**（不是质量）：逐关节零位/方向/限位硬校准、IMU 安装变换、
   实测 P/D 与总线时序。见 `microduck_rl/src/mjlab_microduck/robot/hd1910/README.md`
   和 `reference_hd1910_profile.json` 的 `unknown` 数组。

## 9. 怎么自己复核这些数字

```bash
# 各份的 15 刚体求和：A = 0.737243，C/D = 0.875873（B 没有 XML）
for d in /Users/wangtao/Work/microduck-hd1910m/microduck_rl \
         /Users/wangtao/Work/microduck/microduck_rl_wt \
         /Users/wangtao/Work/microduck_rl_retrain; do
  python3 -c "
import re;t=open('$d/src/mjlab_microduck/robot/microduck/robot_walk.xml').read()
m=re.findall(r'<inertial[^>]*mass=\"([^\"]+)\"',t);print('$d'.split('/')[-1],len(m),round(sum(map(float,m)),6))"
done

# C 与 D 的模型文件是否字节相同
shasum -a256 /Users/wangtao/Work/microduck/microduck_rl_wt/src/mjlab_microduck/robot/microduck/robot_walk.xml \
             /Users/wangtao/Work/microduck_rl_retrain/src/mjlab_microduck/robot/microduck/robot_walk.xml

# B 官方 run 的编译质量
python3 -c "
import json;d=json.load(open('/Users/wangtao/Work/Microduck-kissqy/microduck-hd1910/training-console/data/task-configs/Mjlab-VelStand-Rough-Backlash-MicroDuck.json'))
print(d['inspection']['model']['total_mass_kg'])"

# 四份 M6 参数是否同源（D 用得到的那份是 robot/hd1910/1910_m6.json）
python3 -c "
import json
p=['/Users/wangtao/Work/microduck-hd1910m/microduck_rl/src/mjlab_microduck/actuator/hd1910_1910_m6.json',
   '/Users/wangtao/Work/Microduck-kissqy/microduck-hd1910/training-console/training/hd1910_m6.json',
   '/Users/wangtao/Work/microduck/microduck_rl_wt/src/mjlab_microduck/robot/hd1910/1910_m6.json',
   '/Users/wangtao/Work/microduck_rl_retrain/src/mjlab_microduck/robot/hd1910/1910_m6.json']
d=[json.load(open(x)) for x in p];print(d[0]==d[1]==d[2]==d[3])"
```

## 10. 附录：逐项数据位置

缩写：

- **A** = `/Users/wangtao/Work/microduck-hd1910m/microduck_rl`
- **B** = `/Users/wangtao/Work/Microduck-kissqy/microduck-hd1910/training-console`
- **C** = `/Users/wangtao/Work/microduck/microduck_rl_wt`

### 10.1 逐刚体质量（15 项）

**A、C、D 的行号完全相同**（C 和 D 是在 A 的同一份上游 XML 上改的数，D 与 C 字节相同），
文件都是 `src/mjlab_microduck/robot/microduck/robot_walk.xml`。下表 **C 列同时就是 D 的值**：

| 刚体 | 行号 | A 值（未改） | C 值（已改） | B 出厂值<br>`official-compiled-physics.json` | B 应用后<br>task-config 行 |
|---|---|---|---|---|---|
| trunk_base | 86 | 0.199224 | 0.212424 | :111 | :43443 |
| yaw2roll | 114 | 0.0230406 | 0.0296406 | :145 | :43477 |
| hip_l | 127 | 0.00618934 | 0.00618934 | :179 | :43511 |
| upper_leg_left | 136 | 0.0482067 | 0.0614067 | :213 | :43545 |
| leg | 151 | 0.0215844 | 0.0281844 | :247 | :43579 |
| ankle_left | 161 | 0.0300246 | 0.0300246 | :281 | :43613 |
| neck | 182 | 0.0368414 | 0.0500414 | :315 | :43647 |
| neck_pitch | 195 | 0.00572 | 0.00572 | :349 | :43681 |
| yaw_roll_motion | 204 | 0.0486 | 0.0552 | :383 | :43715 |
| jaw_soft | 217 | 0.188766 | 0.241596 | :417 | :43749 |
| bearing_roll | 267 | 0.0230406 | 0.0296406 | :451 | :43783 |
| hip_l_2 | 280 | 0.00618934 | 0.00618934 | :485 | :43817 |
| upper_leg_right | 289 | 0.0482067 | 0.0614067 | :519 | :43851 |
| leg_2 | 304 | 0.0215844 | 0.0281844 | :553 | :43885 |
| ankle_right | 314 | 0.0300251 | 0.0300251 | :587 | :43919 |

B 的两个文件都在 `data/` 与 `docs/audit-0151/` 下：

- 出厂值：`docs/audit-0151/official-compiled-physics.json`，`mass_kg` 行（`:111` 起，每刚体 +34 行）
- 应用后：`data/task-configs/Mjlab-VelStand-Rough-Backlash-MicroDuck.json`，
  `inspection.model.bodies[].mass_kg`（`:43443` 起，每刚体 +34 行）

### 10.2 舵机与电子件质量

| 数据 | 值 | 位置 |
|---|---|---|
| A 舵机标称 | 0.021 kg | A `src/mjlab_microduck/actuator/reference_hd1910_profile.json:9` |
| A 标称容差 | ±0.002 kg | 同上 `:10` |
| A 原 CAD 舵机质量 | 0.018 kg | 同上 `:88` |
| A 增量算法 | `delta = 0.021 − 0.018` | A `src/mjlab_microduck/actuator/reference_hd1910.py:136` |
| A 写回质量 | `body.mass` | A 同文件 `:151` |
| B 舵机替换值 | 23 g | B `training/recipe.py:12-17`（`SERVO_MASS_PROFILE`） |
| B 参考值 18 g | 18 g | B 同处 `'reference_g': 18.0` |
| B 分配刚体表 | 15 颗 | B 同处 `'body_counts': {...}` |
| B 用户确认来源 | — | B `data/hardware.json:364`「用户2026-10-01 20:17确认」 |
| B 实称说明 | 本机23g；厂规21±2g | B `data/servo_references.json:111` |
| C 旧舵机 | 18.0 g | C `scripts/set_hd1910_masses.py:47` |
| C 新舵机 | 24.6 g（裸机 22.8） | C 同文件 `:48` |
| C Radxa | 15.2 g | C 同文件 `:49` |
| C 总线板/降压 | 29.7 g | C 同文件 `:50` |
| C CAD 里 Pi Zero | 3.47 g（估） | C 同文件 `:54` |
| C CAD 里 HAT | 1.80 g（估） | C 同文件 `:55` |
| C 替换规则表 | 舵机 + 两块板 | C 同文件 `:58-65` |
| C 幂等断言基准 | 15 个 XL330 原值 | C 同文件 `:69-75`，断言在 `:180-183` |
| D 的同一脚本 | 与 C 相同（D 也有 `scripts/set_hd1910_masses.py`） | D `scripts/set_hd1910_masses.py` |
| D 运行时舵机质量 | 不由代码设置，XML 里已是 24.6 g 对应的刚体质量 | D `robot/microduck/robot_walk.xml:86` 等 15 处 |

### 10.3 整机合计与实测

| 数据 | 值 | 位置 |
|---|---|---|
| CAD 合计 | 0.737243 kg | 三份 `robot_walk.xml` 15 个 `<inertial>` 求和 |
| CAD 合计（B 官方） | 0.7372431799999999 | B `docs/audit-0151/official-compiled-physics.json:106` |
| A 最终 | 0.782243 kg | 计算：CAD + 15×3 g |
| B 最终 | 0.82 | B task-config `:43438`；B `data/hardware.json:350` |
| B 闭合目标 | 820 g | B `training/recipe.py:23-27`（`MASS_CLOSURE_PROFILE`） |
| B 头部补 3 g / 躯干补 4.75682 g | — | B 同处 `'body_additions_g'`；B `data/hardware.json:352-353` |
| C 最终 | 0.875873 kg | C `robot_walk.xml` 15 项求和 |
| C rollers 变体 | 0.875460 kg | C `robot_groundcontact_rollers_backlash.xml` |
| C 实称（含电池，不含头部电子件） | 841.2 g | C `scripts/set_hd1910_masses.py:5`；commit `96f5682` 提交信息 |
| C 实测推导 | 104 g = 99 g 舵机 + 5 g 残差 | 同上 `:5-13` |
| C 文档记录 | 737.24 → 875.87 g | C `训练HD1910.md:223` |
| C commit | `96f5682` | `git show 96f5682`（修质量的前后值都在提交信息里） |
| D 最终 | 0.875873 kg | D `robot_walk.xml` 15 项求和（与 C 同文件） |
| D 训练规模 | 4096 envs × 8000 iters | D `logs/…/2026-10-28_08-13-28_hd1910_slowgait/params/env.yaml:3`；`run_to_end.sh` 里 `TARGET=8000` |
| D 检查点 | `model_7999.pt` | D `logs/rsl_rl/microduck_hd1910_velocity/2026-10-28_08-13-28_hd1910_slowgait/model_7999.pt` |
| D 导出 | `hd1910_slowgait_retrain_model_7999.onnx` | D `exports/`；导出日志 `exports/export_retrain.log` 显示是在 `/home/ai/microduck_rl_retrain` 上跑的 |

### 10.4 舵机 M6 参数（每个字段都在文件第 1 行，单行 JSON）

| 文件 | 行 | 说明 |
|---|---|---|
| A `actuator/hd1910_1910_m6.json` | 1 | 全部 20 个字段单行。`kt`、`R`、`armature`、`q_offset`、`command_delay`、`friction_base`、`friction_stribeck`、`max_velocity`、`model`、`actuator` 全在此行 |
| A `actuator/radxa_1910_m6.json` | 1 | 同上，唯一差异 `q_offset: 0.0` |
| B `training/hd1910_m6.json` | 1 | 上游原件，SHA `ef2d51ad…` |
| C `src/mjlab_microduck/robot/hd1910/1910_m6.json` | 1 | 上游原件，SHA `ef2d51ad…` |

参数出处说明：A `src/mjlab_microduck/robot/hd1910/README.md:3-19`；B `UPSTREAM.md` 同段。

### 10.5 执行器 / 训练配置

| 数据 | A | B | C | D |
|---|---|---|---|---|
| `kp_fw=5` | `tasks/hd1910_bam.py:730`；常量 `actuator/cpu_hd1910_bam.py:18`（`MICRODUCK_BAM_KP`，默认 5） | `data/task-configs/…Backlash….json:280` | `tasks/microduck_hd1910_env_cfg.py:33` | `tasks/microduck_hd1910_env_cfg.py:33` |
| `vin_range=(7.4, 8.0)` | `hd1910_bam.py:730` | task-config `:281` 附近 | `microduck_hd1910_env_cfg.py:34` | `microduck_hd1910_env_cfg.py:34` |
| `vin_min=7.0` | `hd1910_bam.py:730` | 同上 | `microduck_hd1910_env_cfg.py:36` | `microduck_hd1910_env_cfg.py:36` |
| `vin_drop_gain=(0, 0.2)` | `hd1910_bam.py:730` | 同上 | `microduck_hd1910_env_cfg.py:35` | `microduck_hd1910_env_cfg.py:35` |
| 延迟 3–6 步 | `hd1910_bam.py:730` 后 `delay_*` | task-config `delay_min_lag` / `delay_max_lag` | `microduck_hd1910_env_cfg.py:37-38` | `microduck_hd1910_env_cfg.py:37-38` |
| 50 Hz / 61 / 14 | `hd1910_bam.py:1080-1086` 元数据 | `data/hardware.json` `runtime` 段 | `microduck_hd1910_env_cfg.py` 继承上游 | 同 C（继承上游） |
| 步态整形奖励 | 无 | 无 | 无 | `microduck_hd1910_env_cfg.py:42-56` `hd_contact_frequency`（0.2，8 次/s） |
| M6 文件引用 | `hd1910_bam.py` 内 | `recipe.py` / task-config | `microduck_hd1910_env_cfg.py` | `microduck_hd1910_env_cfg.py:17`（`robot/hd1910/1910_m6.json`） |

### 10.6 齿隙

| 数据 | 值 | 位置 |
|---|---|---|
| A 总齿隙上限 | 0.5°（取 ±0.25°） | A `actuator/reference_hd1910_profile.json:17` |
| A 注释 | 不要用旧 XML 的 ±1° | A `actuator/reference_hd1910.py:155-156` |
| B 齿隙 | ±1.0° | B task-config `:43373`（`gear_backlash.ranges_rad`） |
| C 齿隙生成默认 | 2.0° 总间隙（±1°） | C `src/mjlab_microduck/robot/microduck/add_backlash.py:63` |
| C XML 实际值 | ±1° | C `robot_groundcontact_backlash.xml:82` 注释 |
| D 齿隙 | ±1°，与 C 的 XML **字节相同**（`26ee9e67d6eb`） | D `robot_groundcontact_backlash.xml:82` |

### 10.7 质量域随机化

| 数据 | 位置 |
|---|---|
| A/B/C/D 默认 ±5% | A/C/D `src/mjlab_microduck/tasks/microduck_velocity_env_cfg.py:73`；B task-config `randomize_mass_inertia` |
| D 的 `randomize_mass_inertia` 实际值 | D `train_slowgait_20261027.log:96-100`（startup 事件表）与 task-config 同源 |
| A 挂载点 | A `microduck_velocity_env_cfg.py:461-462` |
| A 增强包线（12 刚体） | A `actuator/payload_uncertainty.py:25-41`；范围表 `actuator/replica_mass_20261002.json:15-28` |
| A 包线开关 | A `tasks/hd1910_bam.py:433-439`（仅 `--repair-variant gait_payload_v8`） |
| A COM 偏移 ±8 mm | A `replica_mass_20261002.json:29-30` |
| A 导出元数据 | A `tasks/hd1910_bam.py:1104-1107` |

### 10.8 受这些数影响的测试断言

| 文件 | 行 | 当前断言 | 改 XML 会红吗 | 改 profile 会红吗 |
|---|---|---|---|---|
| A/D `tests/test_hd1910_reference.py` | 24 | `updated − original == 0.045` | 不会 | **会** |
| A/D `tests/test_hd1910_bounded.py` | 229 | `spec_fn() − original_factory() == 0.045` | 不会 | **会** |
| A/D `tests/test_radxa_alignment.py` | 39 | `adapted − source == 0.045` | 不会 | **会** |

原因：三条比的都是**相对差**，两边读的是同一份 XML。所以 D 把 XML 改成 875.873 g 之后，
这三条断言原样保留也不会红（差值仍是 45 g）。

- **只改 XML** → 不红，别动测试。
- **改 profile 的 `mass_kg`**（方案乙，21→24.6）→ 会红，把 `.045` 换成 `.099`。
- **方案甲**（改 XML + 把 profile 的 delta 归零）→ 会红，把 `.045` 换成 `pytest.approx(0)`。

### 10.9 文档里提到但**没有**参数的地方（别白找）

| 想找的东西 | 实际情况 |
|---|---|
| A `microduck_constants.py` 里的质量 | 没有任何质量常量，全是关节/几何常数 |
| A `replica_mass_20261002.json:12` `whole_robot_measured_mass_kg` | 无代码读取，只参与 sha256 和 ONNX 元数据 |
| B 的 MJCF | 项目里 **0 个 `.xml`**，质量只存在于编译快照 JSON |
| C 的运行时质量补丁 | 没有；C 是**一次性改 XML**，见 10.2 |
