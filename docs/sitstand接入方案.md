# 接入 kissqy 的 sitstand（坐下/站起）

**2026-10-29 记录，状态：尚未改代码，待开发机编译 + 上板验证。**

文件在 `radxa/references/kissqy_20261027/`，git 已跟踪。同一个目录里的
`stand.onnx` **不要接**，原因见下。

## 1. 两张图分别是什么

| 图 | sha256 | manifest 关键字段 | 能不能用 |
|---|---|---|---|
| `sitstand.onnx` | `1c94934c6421ec72ed48079cea74966868a7a59f7d732e0207d0f7bdaeed3c50` | `task: Mjlab-SitStand-Flat-MicroDuck`，`deployment_verified: false` | ✅ 是真的坐/起网络 |
| `stand.onnx` | `a481d9f211f31d9476ab1a319fe362a21ef0879d9fa59442a122845af598268c` | `entry_pose: "fallen"`，`teacher_only: true`，`task: Mjlab-StandUp-Rough-Backlash-MicroDuck` | ❌ 是"从倒地起身"的老师网络 |

`stand.onnx` 不是原地站立策略，当 `stand` 槽用会一直试图从倒地姿态起身。
kissqy 自己的配置也没用它：`firmware/packaging/robotd-profile.toml` 里
`sitstand = ".../1c94934c.../policy.onnx"`、**`stand = "none"`**。

sitstand 的 ONNX 元数据和 walk 同一套：`joint_names` 是 14 个（顺序一致，跳过嘴）、
有 `default_joint_pos`。训练时用 twist 的 vx 槽当姿态标志（坐下 `[1,0,0]`，
起身/站立 `[0,0,0]`）—— 本地 `control.rs` 的 `Sit::Sitting` / `Sit::Rising` 已经是这么发的。

## 2. 为什么现在加载不了

1. `duck-control/src/policy.rs:449` 明确拒绝：

   ```rust
   if paths.stand.is_some() || paths.sitstand.is_some() || paths.skills.len() != 2 {
       return Err(PolicyError::Inference("Reference requires walk, pick, recovery, roulade; no stand/sitstand graph".into()));
   }
   ```

2. `REPLICA.md:112` 已有结论：reference_native 只吃 walk / pick / getup / roulade，
   另三张要跑得换 profile，会连带改 IMU 滤波、速度滤波、目标钳位。
3. 这张 sitstand 的 manifest 只有 5 个字段，没有 `obs_len` / `action_len` / `encoding`，
   通用加载器也过不了 manifest 校验。

## 3. 最小改法：只放行 sitstand，仍然拒绝 stand

### 3.1 `duck-control/src/policy.rs` — `load_reference`

a) 拒绝条件去掉 sitstand：

```rust
if paths.stand.is_some() || paths.skills.len() != 2 {
```

b) 文件和哈希白名单改成可追加（现在是 `[&paths.walk, pick, &skills[0], &skills[1]]`
和 4 项 `hashes` 数组）：

```rust
let mut files = vec![&paths.walk, pick, &paths.skills[0], &paths.skills[1]];
let mut hashes: Vec<&[&str]> = vec![&WALK_HASHES, &PICK_HASHES, &GETUP_HASHES, &ROULADE_HASHES];
if let Some(sit) = paths.sitstand.as_ref() {
    files.push(sit);
    hashes.push(&SITSTAND_HASHES); // = &["1c94934c6421ec72ed48079cea74966868a7a59f7d732e0207d0f7bdaeed3c50"]
}
```

循环体（读文件、校验哈希、`joint_names`、`default_joint_pos`、预热推理）不用改。

c) 取会话和 home（`sessions` / `homes` 的顺序是 walk、pick、getup、roulade、[sitstand]）：

```rust
let sitstand = if paths.sitstand.is_some() {
    Some(sessions.pop().expect("sitstand session"))
} else {
    None
};
let walk = sessions.remove(0);
let ground_pick = Some(sessions.remove(0));
```

`homes` 不用 pop：它已经按同一顺序收集了每一张图自己的 `default_joint_pos`，
sitstand 的 home 留在索引 4。

d) `home()`（policy.rs:434 附近）加一个分支，别让它借 walk 的 home：

```rust
Some(homes) => homes[match net {
    Net::GroundPick => 1,
    Net::Skill(i) => 2 + i,
    Net::SitStand if homes.len() > 4 => 4,
    _ => 0,
}],
```

e) 构造 `Self` 时把 `sitstand: None` 换成 `sitstand,`。

### 3.2 配置

`radxa/reference_native.toml` 的 `[policy]` 加一行（路径按板上实际部署位置改）：

```toml
sitstand = "<部署目录>/kissqy_20261027/sitstand.onnx"
```

注意 [REPLICA.md:110](../REPLICA.md) 的坑：板上跑的是 `local/params.toml`，
由 `scripts/configure.py` 从 `radxa/reference_native.toml` 生成。改完要么重新
`configure.py` 再部署，要么直接改板上 `local/params.toml` 后重启。

### 3.3 不用改的

- `robotd` 的 reference 契约检查：`skills` 仍然只有 recovery + roulade 两个，不受影响
  （`main.rs:1648` 那段只校验 skills 列表和 pick/roulade 参数）。
- `sitstand_rise_s` / `sitstand_ramp_s`：没有 `[[policy.skill]]` 条目时用默认值
  **1.0 s / 2.0 s**（`robotd-params/src/lib.rs:1040,1043`），和"2 s 斜坡"的训练一致。
  要调再在 `[policy]` 里加同名键。
- `padd`：十字键 ↓ 已经在发 `sit_toggle`，robotd 有 sitstand 后自然就通了。

## 4. 编译和部署

开发机（Linux + aarch64 sysroot，见 REPLICA §6）：

```bash
cd microduck && ./scripts/build.sh      # 编 robotd / robotctl / app-server
# 或只编 robotd:
cargo build --release --locked --target aarch64-unknown-linux-gnu -p robotd
```

上板前先备份板上 `out/arm64/bin/robotd` 和 `local/params.toml`，再替换、重启
`microduck-hd1910` 服务。

## 5. 上板验证

1. `journalctl -u robotd -f` 看启动日志：应有 `policy loaded … sitstand=Some("…")`。
   哈希或元数据不对会直接 `policy load failed`，**整个策略集加载失败**（reference 是
   全有或全无），机器人不会动。
2. 把机器人放垫子上，手放 Select。按 Start 两次：第一次起身，第二次策略开。
3. 按 **十字键 ↓**：日志应出现 `sit toggle`，direction `"sit"`；再按一次 direction `"stand"`。
4. 策略开着时 **Select 按住 2 s**：日志
   `shutdown: sitting down before power off` → 坐下 → `sit complete: cutting torque` → 关机。
   （`main.rs:2395` 的 `can_sit` 要求 `snapshot.enabled`，策略没开时仍是直接断电。）

## 6. 风险

- `deployment_verified: false`，manifest 没有 obs/action 契约字段，只能实机验证。
- sitstand 的 `default_joint_pos` 必须用它自己的：借 walk 的 home 会让观测和动作基线
  整体偏移，可能一按 ↓ 就趴下。
- 第一次试一定垫子上、手放 Select；如果起身方向反了或抖动，直接卸力。
- 加载失败等于整套策略起不来（walk 也没了），所以先备份二进制和配置。

## 7. 回滚

1. 配置里删掉 `sitstand` 那行，重启；
2. 或换回备份的 `robotd`；
3. 代码改动用 `git revert` 回退。

## 8. 相关背景（同一天查到的）

- 手柄实体键和代码名是反的：**物理 X = 头模式（North）**、**物理 Y = roulade（配置里的 `x`）**。
  原因和细节见 [手柄连接.md](手柄连接.md#L88)。
- 身体姿态（B 键）在这台是空的：`reference_native` 把 body 指令强制清零
  （`robotd/src/control.rs:665`），且没有 `stand` 网络，按 B 只等于停车保持站姿。
