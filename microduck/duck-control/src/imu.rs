//! The `imu_to_dxl` v2 board (LSM6DSV16X), decoded.
//!
//! One IMU, one code path. The board rides the motor bus and its 12-byte block is fetched
//! in the same `sync_read` as the servos, so there is no separate sensor to poll and no
//! fusion to run on the host — the chip's SFLP block ships a game-rotation quaternion and
//! estimates its own gyro bias.
//!
//! 数据块布局（地址 = 舵机自己那一段状态块的起点）：
//!
//! | 字节 | 内容 |
//! |---|---|
//! | 0..6  | 陀螺 x/y/z，`i16` 小端原始计数，±500 dps |
//! | 6..12 | SFLP 四元数 x/y/z，IEEE 半精度；`w = √(1 − x² − y² − z²)` |
//!
//! 小板真正的诊断块比这长（后面还有采样计数和状态位）。控制循环只取前 12 字节，
//! 这样这次读才能跟舵机挤进同一条事务。
//!
//! 有两条总线喂这个块，差别**全在「怎么问」**：
//!
//! * [`crate::bus`] —— 官方 XL330 那条，走 Dynamixel 协议读地址 124，小板排在 id 表**队首**。
//! * [`crate::feetech`] —— HD1910M 那条，走 FT-SCS 读地址 56，小板排在 id 表**队尾**，
//!   因为飞特舵机的自然应答延时比小板短。
//!
//! 十二个字节到手之后，解码是一模一样的 —— 这正是 [`SflpDecoder`] 和
//! [`StaleImuTracker`] 住在这里、而不是住在任何一个后端里的原因。

use std::time::{Duration, Instant};

use crate::io::{ImuStale, IoError};
use crate::model::NUM_JOINTS;

/// Bytes of the v2 board's block consumed per tick.
pub const IMU_BLOCK_LEN: usize = 12;

/// What the control loop knows about the robot's orientation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImuData {
    /// Angular velocity in the trunk frame, rad/s.
    pub gyro: [f64; 3],
    /// Projected gravity in the trunk frame, unit vector. Upright is `[0, 0, -1]`.
    ///
    /// This is what the policy observes, and what fall detection thresholds on.
    pub gravity: [f64; 3],
    /// Orientation, trunk→world, scalar-first `[w, x, y, z]`.
    pub quat: [f64; 4],
}

impl Default for ImuData {
    fn default() -> Self {
        Self {
            gyro: [0.0; 3],
            gravity: [0.0, 0.0, -1.0],
            quat: [1.0, 0.0, 0.0, 0.0],
        }
    }
}

/// ±500 dps at 17.5 mdps/LSB, in rad/s.
const GYRO_RAD_PER_LSB: f64 = 0.0175 * std::f64::consts::PI / 180.0;

/// Decodes the board's block into [`ImuData`].
///
/// Stateful only for spike rejection and for holding the last good quaternion — there is
/// no filter here. Keeping the state explicit means [`crate::io::FakeIo`] can drive the
/// same decoder in tests.
pub struct SflpDecoder {
    /// Sensor→trunk mounting rotation, scalar-first.
    mount: [f64; 4],
    /// Last quaternion the board actually produced. Held across blocks that arrive before
    /// SFLP has written its table — snapping to identity would report a robot as upright
    /// when its orientation is simply unknown, which is the worst possible lie to tell
    /// fall detection.
    last_quat: [f64; 4],
    /// Blocks carrying a live quaternion. Gates [`SflpDecoder::ready`].
    quat_samples: u32,
    gyro_history: [[f64; 3]; 2],
    gravity_history: [[f64; 3]; 2],
}

impl Default for SflpDecoder {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MOUNT)
    }
}

impl SflpDecoder {
    /// The board is mounted so that trunk = `[+raw_z, +raw_y, −raw_x]`, a +90° rotation
    /// about Y.
    pub const DEFAULT_MOUNT: [f64; 4] = [
        std::f64::consts::FRAC_1_SQRT_2,
        0.0,
        std::f64::consts::FRAC_1_SQRT_2,
        0.0,
    ];

    pub fn new(mount: [f64; 4]) -> Self {
        Self {
            mount,
            last_quat: [1.0, 0.0, 0.0, 0.0],
            quat_samples: 0,
            gyro_history: [[0.0; 3]; 2],
            gravity_history: [[0.0, 0.0, -1.0]; 2],
        }
    }

    /// Whether the chip has demonstrably produced fused output — roughly 0.25 s at 100 Hz.
    ///
    /// Until this is true the orientation is a default, not a measurement. Slice 2's fall
    /// detection must not run before it.
    pub fn ready(&self) -> bool {
        self.quat_samples >= 25
    }

    /// 已经拿到过多少个带有效四元数的块。给状态上报用，不参与判断。
    pub(crate) fn quat_samples(&self) -> u32 {
        self.quat_samples
    }

    pub fn decode(&mut self, raw: &[u8; IMU_BLOCK_LEN]) -> ImuData {
        let gyro_sensor = [
            i16::from_le_bytes([raw[0], raw[1]]) as f64 * GYRO_RAD_PER_LSB,
            i16::from_le_bytes([raw[2], raw[3]]) as f64 * GYRO_RAD_PER_LSB,
            i16::from_le_bytes([raw[4], raw[5]]) as f64 * GYRO_RAD_PER_LSB,
        ];
        let gyro = rotate(self.mount, gyro_sensor);

        // All-zero quaternion bytes mean SFLP has not written its table yet — the board
        // just powered up, or its init failed. Keep the last good value.
        let packed = [
            u16::from_le_bytes([raw[6], raw[7]]),
            u16::from_le_bytes([raw[8], raw[9]]),
            u16::from_le_bytes([raw[10], raw[11]]),
        ];
        if packed != [0, 0, 0] {
            let (x, y, z) = (half(packed[0]), half(packed[1]), half(packed[2]));
            let norm_sq = x * x + y * y + z * z;
            // A quaternion the chip never produced would fail this; ≤ 1.02 allows for
            // half-precision rounding at full scale.
            if x.is_finite() && y.is_finite() && z.is_finite() && norm_sq <= 1.02 {
                let w = (1.0 - norm_sq).max(0.0).sqrt();
                let mount_inv = [
                    self.mount[0],
                    -self.mount[1],
                    -self.mount[2],
                    -self.mount[3],
                ];
                let q = mul([w, x, y, z], mount_inv);
                let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
                if norm > 0.5 {
                    self.last_quat = [q[0] / norm, q[1] / norm, q[2] / norm, q[3] / norm];
                    self.quat_samples = self.quat_samples.saturating_add(1);
                }
            }
        }

        // Normalise *before* the median, matching the runtime. Note the consequence: a
        // component-wise median across three unit vectors is not itself unit-norm, so
        // during a transient the policy sees a slightly short vector. Steady state is
        // exact. Whether the training env expects strict unit norm is worth settling when
        // slice 2 wires up observations — normalising after the median would guarantee it,
        // but that is a behaviour change to a path that currently walks.
        let gravity = normalise(rotate_inverse(self.last_quat, [0.0, 0.0, -1.0]));

        let out = ImuData {
            gyro: median3_each(&self.gyro_history, gyro),
            gravity: median3_each(&self.gravity_history, gravity),
            quat: self.last_quat,
        };
        self.gyro_history = [self.gyro_history[1], gyro];
        self.gravity_history = [self.gravity_history[1], gravity];
        out
    }
}

/// Single-sample spike rejection. A dropped or corrupted block shows up as one wild value;
/// a median over three discards it without the lag of an average.
fn median3_each(history: &[[f64; 3]; 2], now: [f64; 3]) -> [f64; 3] {
    let m = |a: f64, b: f64, c: f64| a.max(b).min(c).max(a.min(b));
    [
        m(history[0][0], history[1][0], now[0]),
        m(history[0][1], history[1][1], now[1]),
        m(history[0][2], history[1][2], now[2]),
    ]
}

/// IEEE 754 binary16 → f64. The LSM6DSV16X ships quaternion components in this format.
fn half(bits: u16) -> f64 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((bits >> 10) & 0x1F) as i32;
    let frac = (bits & 0x3FF) as f64;
    match exp {
        0 => sign * frac * 2.0_f64.powi(-24),
        0x1F if frac == 0.0 => sign * f64::INFINITY,
        0x1F => f64::NAN,
        _ => sign * (1.0 + frac / 1024.0) * 2.0_f64.powi(exp - 15),
    }
}

/// Hamilton product, scalar-first.
pub(crate) fn mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let ([aw, ax, ay, az], [bw, bx, by, bz]) = (a, b);
    [
        aw * bw - ax * bx - ay * by - az * bz,
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
    ]
}

/// `q · v · q⁻¹`
pub(crate) fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let (t, c) = cross_terms(q, v);
    [
        v[0] + q[0] * t[0] + c[0],
        v[1] + q[0] * t[1] + c[1],
        v[2] + q[0] * t[2] + c[2],
    ]
}

/// `q⁻¹ · v · q` — world vector expressed in the body frame.
pub(crate) fn rotate_inverse(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let (t, c) = cross_terms(q, v);
    [
        v[0] - q[0] * t[0] + c[0],
        v[1] - q[0] * t[1] + c[1],
        v[2] - q[0] * t[2] + c[2],
    ]
}

fn cross_terms(q: [f64; 4], v: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let (x, y, z) = (q[1], q[2], q[3]);
    let t = [
        (y * v[2] - z * v[1]) * 2.0,
        (z * v[0] - x * v[2]) * 2.0,
        (x * v[1] - y * v[0]) * 2.0,
    ];
    let c = [
        y * t[2] - z * t[1],
        z * t[0] - x * t[2],
        x * t[1] - y * t[0],
    ];
    (t, c)
}

/// Unit vector, falling back to "upright" rather than dividing by ~zero.
fn normalise(v: [f64; 3]) -> [f64; 3] {
    let mag = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if mag > 0.1 {
        [v[0] / mag, v[1] / mag, v[2] / mag]
    } else {
        [0.0, 0.0, -1.0]
    }
}

/// 连续读到多少个「一模一样」的块，就该在日志里说一声。
///
/// 25 次在 50 Hz 下是半秒 —— 跟 [`SflpDecoder::ready`] 愿意承认芯片输出算数之前等的
/// 那段一样长，且远长于任何正常的抖动。低于它的重复一律不吭声，是故意的：头一次重复
/// 就报警，正是这条消息被所有人学会忽略的原因。得跟 `ImuHealth::FROZEN_RUN` 保持一致，
/// 健康上报用的是同一个阈值 —— 本 crate 是硬件层，故意不依赖 IPC 那边的词表，所以
/// 这个数字在两边各存一份。
pub(crate) const STALE_RUN_WARN: u64 = 25;

/// 认出一块「在应答但没在刷新」的小板：记住上一个块，看它是不是又发了一遍。
///
/// 从读路径里拆出来，是为了不开串口也能测：它描述的这个故障，机器人上别处都报不出来，
/// 不拆就只能拿坏硬件来验证。
///
/// 两条总线共用：Dynamixel 那条把小板排**队首**，Feetech 这条排**队尾**，但「同一个
/// 块发两遍」在两边是同一种故障。
#[derive(Debug, Default)]
pub(crate) struct StaleImuTracker {
    /// 拿到第一个块之前是 `None`。这里不能用一个固定的初值：那只能是全零，而全零块恰恰
    /// 就是 SFLP 表还没写好时小板发出来的东西 —— 等于拿一个从来没存在过的前驱去判重复。
    last: Option<[u8; IMU_BLOCK_LEN]>,
    pub(crate) stale: ImuStale,
}

impl StaleImuTracker {
    /// 记录一个块，返回它所属那一段连续重复的长度；块是新鲜的（绝大多数情况）时返回 0。
    pub(crate) fn observe(&mut self, block: &[u8; IMU_BLOCK_LEN]) -> u64 {
        if self.last.replace(*block) == Some(*block) {
            self.stale.total = self.stale.total.saturating_add(1);
            self.stale.run = self.stale.run.saturating_add(1);
        } else {
            self.stale.run = 0;
        }
        self.stale.run
    }
}

/// 解码出来的样本，多久之后就不再算数。
///
/// 跟 `Bno08x` 用同一个 150 ms —— 同一个传感器、同一个问题，不该有两个答案。
pub(crate) const IMU_SAMPLE_MAX_AGE: Duration = Duration::from_millis(150);

/// 小板连续多少次不应答，就把它从读表里摘掉。
///
/// 不摘的话，一块不吭声的小板**每一轮**都要把整次读拖到超时为止，还会顺手把舵机的
/// 反馈一起带下去 —— `sync_read` 要等表里每一个 id 都答完才返回。
pub(crate) const IMU_MISS_LIMIT: u32 = 5;

/// 摘掉之后隔多久，放一轮进去试探一下它回来没有。
pub(crate) const IMU_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// 舵机总线上的那块 `imu_to_dxl` 小板，加上控制循环判断「要不要信它」所需的全部状态。
///
/// 跟 `Bno08x` 不一样，这里没有自己的线程、也没有自己的文件描述符：块是搭着舵机本来
/// 就要付的那条 `sync_read` 回来的，所以这里的活儿只剩两件 —— 解码，和记住上一次
/// 拿到它是什么时候。[`BusImu::observe`] 由**拥有串口的那条线程**调用；其余方法都在
/// 自己的锁后面，谁调都行。
///
/// `pub` 是因为它出现在 [`crate::feetech::ImuSource`] 那个公开枚举里；字段和下面所有
/// 方法都是 `pub(crate)`，外面拿到它也没法乱来。
pub struct BusImu {
    decoder: SflpDecoder,
    stale: StaleImuTracker,
    /// 最近一次解码出来的样本，以及它后面那个块是什么时候到的。
    sample: Option<(ImuData, Instant)>,
    /// 连续多少次读没等到小板。
    misses: u32,
    /// 小板被摘出读表期间是 `Some`，记的是**上一次问它**的时刻，用来算下一次试探。
    dropped_at: Option<Instant>,
    reference_filter: bool,
    /// 上一个陀螺样本，用来算参考滤波的衰减。跟 `bno08x` 那份同一个用法。
    last_gyro: Option<([f64; 3], Instant)>,
    block_reads: u64,
    miss_total: u64,
    reconnects: u64,
    last_error: Option<String>,
}

impl BusImu {
    /// `mount` 是安装文件里的 传感器→躯干 旋转。
    ///
    /// [`SflpDecoder::new`] 不校验它，所以在这里归一化：一个不是单位四元数的 mount 会
    /// 悄悄把策略看到的每一个重力向量都缩放一遍。跟 `Bno08x::open` 的处理对齐 ——
    /// 校验不过就退回小板默认朝向，并且**说出来**，不装作没事。
    pub(crate) fn new(mount: [f64; 4], reference_filter: bool) -> Self {
        let norm = mount.iter().map(|v| v * v).sum::<f64>().sqrt();
        let mount = if norm.is_finite() && (0.9..=1.1).contains(&norm) {
            mount.map(|v| v / norm)
        } else {
            tracing::warn!(?mount, "imu mount 不是单位四元数；改用小板默认朝向");
            SflpDecoder::DEFAULT_MOUNT
        };
        Self {
            decoder: SflpDecoder::new(mount),
            stale: StaleImuTracker::default(),
            sample: None,
            misses: 0,
            dropped_at: None,
            reference_filter,
            last_gyro: None,
            block_reads: 0,
            miss_total: 0,
            reconnects: 0,
            last_error: None,
        }
    }

    /// 这一轮读要不要带上小板。
    ///
    /// 一直要，直到它连续缺席 [`IMU_MISS_LIMIT`] 次；之后不要，除了每隔
    /// [`IMU_RETRY_INTERVAL`] 放一轮进去探一下。
    pub(crate) fn wants_block(&self, now: Instant) -> bool {
        self.dropped_at
            .is_none_or(|at| now.duration_since(at) >= IMU_RETRY_INTERVAL)
    }

    /// 记下这一轮读回来的块，返回它的内容已经冻了多久。
    pub(crate) fn observe(&mut self, block: &[u8; IMU_BLOCK_LEN], now: Instant) -> u64 {
        if self.dropped_at.take().is_some() {
            self.reconnects += 1;
            tracing::info!("imu 小板回来了，重新加入读表");
        }
        self.misses = 0;
        self.block_reads += 1;
        self.last_error = None;
        let run = self.stale.observe(block);
        let mut data = self.decoder.decode(block);
        // 参考滤波：`reference_native` 在 I²C 那条路上就是这个意思（`bno08x` 的
        // `parse`），所以这里实现同一个衰减，而不是把那个开关变成一句空话。
        if self.reference_filter {
            if let Some((old, at)) = self.last_gyro {
                let weight = 0.5f64.powf(now.duration_since(at).as_secs_f64() / 0.01);
                for i in 0..3 {
                    data.gyro[i] = weight * old[i] + (1.0 - weight) * data.gyro[i];
                }
            }
            self.last_gyro = Some((data.gyro, now));
        }
        self.sample = Some((data, now));
        run
    }

    /// 记下「问了它、它没答」。
    ///
    /// 连续 [`IMU_MISS_LIMIT`] 次之后把块从读表里摘掉：一块不存在的小板，不该让舵机的
    /// 反馈付出任何代价。摘掉之后每失败一次就把试探计时器重置一回，所以两次试探之间
    /// 至少隔 [`IMU_RETRY_INTERVAL`]。
    pub(crate) fn record_miss(&mut self, now: Instant, error: &str) {
        self.misses = self.misses.saturating_add(1);
        self.miss_total += 1;
        self.last_error = Some(error.to_owned());
        if self.misses < IMU_MISS_LIMIT {
            return;
        }
        let first = self.dropped_at.is_none();
        self.dropped_at = Some(now);
        if first {
            self.sample = None;
            tracing::warn!(
                misses = self.misses,
                "imu 小板不应答了；先把它从舵机读表里摘掉，等它回来"
            );
        }
    }

    /// 最近一次样本，或者一个说明为什么现在没有可用样本的错误。
    pub(crate) fn snapshot(&self) -> Result<(ImuData, Instant), IoError> {
        let (data, at) = self
            .sample
            .ok_or_else(|| IoError::ImuUnavailable(self.unavailable_reason()))?;
        if at.elapsed() > IMU_SAMPLE_MAX_AGE {
            return Err(IoError::ImuUnavailable(format!(
                "总线 IMU 样本已经 {:.0} ms 没更新了",
                at.elapsed().as_secs_f64() * 1000.
            )));
        }
        Ok((data, at))
    }

    pub(crate) fn read(&self) -> Result<ImuData, IoError> {
        Ok(self.snapshot()?.0)
    }

    /// 出厂至今有没有拿到过足够多带四元数的块，**且**现在手上这份还是新鲜的。
    ///
    /// 前半个条件是 [`SflpDecoder::ready`] 的本意（大约 0.25 秒），后半个跟 I²C 那条
    /// 路上 `read().is_ok()` 的语义对齐 —— 刚上电那 0.25 秒里报「就绪」，等于让跌倒
    /// 检测拿一个默认朝向去判。
    pub(crate) fn ready(&self) -> bool {
        self.decoder.ready() && self.snapshot().is_ok()
    }

    pub(crate) fn stale(&self) -> ImuStale {
        self.stale.stale
    }

    pub(crate) fn set_reference_filter(&mut self, enabled: bool) {
        self.reference_filter = enabled;
    }

    /// 显式的人工复位：忘掉手上的样本，并且给被摘掉的小板一次试探机会。
    pub(crate) fn reconnect(&mut self) {
        self.sample = None;
        self.misses = 0;
        self.dropped_at = None;
        self.reconnects += 1;
    }

    fn unavailable_reason(&self) -> String {
        match (&self.last_error, self.dropped_at.is_some()) {
            (Some(error), true) => format!("imu 小板已被移出舵机读表：{error}"),
            (Some(error), false) => format!("还没有总线 IMU 样本：{error}"),
            (None, _) => "还没有总线 IMU 样本".into(),
        }
    }

    /// 键名跟 `Bno08x::report_stats` 对齐，再加几个总线自己的。
    ///
    /// 这个函数**不会失败**：`feedback()` 里原来是 `.ok()?`，IMU 一打嗝整份报告就变
    /// `None` —— 位置、关节、错误全没。读不到的计数器就说自己读不到，不是不再描述
    /// 这台机器人的理由。
    pub(crate) fn report_stats(&self) -> serde_json::Value {
        serde_json::json!({
            "source": "bus",
            "requested_hz": 100,
            "gyro_reports": self.block_reads,
            "quaternion_reports": self.decoder.quat_samples(),
            "block_reads": self.block_reads,
            "misses": self.miss_total,
            "frozen_run": self.stale.stale.run,
            "frozen_total": self.stale.stale.total,
            "reconnects": self.reconnects,
            "ready": self.decoder.ready(),
            "dropped": self.dropped_at.is_some(),
            "last_error": self.last_error,
            "read_errors": self.miss_total,
            "reconnect_mode": "automatic",
            "timestamp_source": "bus_transaction",
        })
    }
}

/// Compile-time assurance that the joint count and this module stay independent — the IMU
/// block is fixed-size regardless of how many servos share the bus.
const _: () = assert!(NUM_JOINTS > 0);

#[cfg(test)]
mod tests {
    use super::*;

    /// Half-precision decoding is hand-rolled, so pin the cases that matter: zero, one,
    /// a negative, and a subnormal. A wrong exponent bias here silently tilts the horizon.
    #[test]
    fn half_precision_decodes_known_values() {
        assert_eq!(half(0x0000), 0.0);
        assert_eq!(half(0x3C00), 1.0);
        assert_eq!(half(0xBC00), -1.0);
        assert_eq!(half(0x3800), 0.5);
        assert!((half(0x3555) - 0.333).abs() < 1e-3);
    }

    /// A block of zeroes is the board saying "SFLP has not started". Reporting identity —
    /// i.e. perfectly upright — would tell fall detection the robot is fine when its
    /// orientation is simply unknown.
    #[test]
    fn all_zero_quaternion_bytes_hold_the_last_good_value() {
        let mut d = SflpDecoder::default();
        assert!(!d.ready());

        let zero = d.decode(&[0u8; IMU_BLOCK_LEN]);
        assert_eq!(zero.quat, [1.0, 0.0, 0.0, 0.0]);
        assert!(!d.ready(), "a zero block must not count as a live sample");

        // 0x3C00 = 1.0 in the x slot would exceed unit norm on its own; use a modest
        // rotation the chip could actually emit.
        let mut block = [0u8; IMU_BLOCK_LEN];
        block[6..8].copy_from_slice(&0x3000u16.to_le_bytes()); // x = 0.125
        let live = d.decode(&block);
        assert_ne!(live.quat, [1.0, 0.0, 0.0, 0.0]);

        let held = d.decode(&[0u8; IMU_BLOCK_LEN]);
        assert_eq!(held.quat, live.quat, "zero block must hold, not reset");
    }

    /// `ready()` gates fall detection in slice 2. If it were true from the first block,
    /// the robot would be judged on a default orientation for the first quarter second.
    #[test]
    fn not_ready_until_the_chip_has_produced_output() {
        let mut d = SflpDecoder::default();
        let mut block = [0u8; IMU_BLOCK_LEN];
        block[6..8].copy_from_slice(&0x3000u16.to_le_bytes());
        for _ in 0..24 {
            d.decode(&block);
        }
        assert!(!d.ready());
        d.decode(&block);
        assert!(d.ready());
    }

    /// In steady state gravity must be a unit vector at any orientation — the policy
    /// observes it directly and was trained on normalised input.
    ///
    /// Three identical blocks per orientation so the median settles. Mid-transient the
    /// median blends three different unit vectors component-wise and the result is
    /// slightly short; that is the runtime's behaviour and is deliberately preserved (see
    /// the note in `decode`).
    #[test]
    fn gravity_is_a_unit_vector_in_steady_state() {
        let mut d = SflpDecoder::default();
        let mut block = [0u8; IMU_BLOCK_LEN];
        for packed in [0x0000u16, 0x3000, 0x3800, 0xB000] {
            block[6..8].copy_from_slice(&packed.to_le_bytes());
            d.decode(&block);
            d.decode(&block);
            let g = d.decode(&block).gravity;
            let mag = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
            assert!(
                (mag - 1.0).abs() < 1e-9,
                "gravity magnitude {mag} for {packed:#06x}"
            );
        }
    }

    /// The transient case, pinned so the bound is known rather than assumed: switching
    /// orientation between ticks blends three vectors and shortens the result. If this
    /// ever gets far from 1.0 the policy is being fed something training never saw.
    #[test]
    fn gravity_stays_close_to_unit_through_a_transient() {
        let mut d = SflpDecoder::default();
        let mut block = [0u8; IMU_BLOCK_LEN];
        block[6..8].copy_from_slice(&0x0000u16.to_le_bytes());
        d.decode(&block);
        block[6..8].copy_from_slice(&0x3800u16.to_le_bytes());
        let g = d.decode(&block).gravity;
        let mag = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
        assert!(mag > 0.5, "transient gravity collapsed to {mag}");
        assert!(mag <= 1.0 + 1e-9);
    }

    /// Gyro counts are signed. Reading them as unsigned would turn every negative rate
    /// into a large positive one, which reads as a robot spinning.
    #[test]
    fn gyro_counts_are_signed() {
        let mut d = SflpDecoder::default();
        let mut block = [0u8; IMU_BLOCK_LEN];
        block[0..2].copy_from_slice(&(-1000i16).to_le_bytes());
        // Three identical blocks so the median settles on this sample.
        d.decode(&block);
        d.decode(&block);
        let out = d.decode(&block);
        let magnitude: f64 = out.gyro.iter().map(|v| v.abs()).sum();
        assert!(magnitude > 0.0);
        let expected = 1000.0 * GYRO_RAD_PER_LSB;
        assert!(
            (magnitude - expected).abs() < 1e-9,
            "expected |gyro| {expected}, got {magnitude}"
        );
    }
}
