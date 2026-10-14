"""--imu-bus：从舵机总线读 IMU 小板 ID 200。

两层：BusImuService 的判据（跟主控 / imu200.py 同一套），以及 server 的状态轮询
走真的 feetech.py + 协议级模拟器 sim_bus + 假小板 imu200.SimImu200。不接硬件。
Run: python -m pytest -q test_imu_bus.py
"""
import contextlib
import io
import math
import struct
import unittest
from unittest.mock import patch

import feetech
import imu200            # 同目录兄弟：以前在 tools/imu200/，要 sys.path 绕过去，现在直接 import
import imu_bus
import server
from sim_bus import SimSerial

JOINTS = [10, 9, 8, 7, 6, 12, 11, 14, 13, 15, 5, 4, 3, 2, 1]


def block(gyro=(100, -50, 3), quat=(0.0, 0.0, 0.3827), counter=7, bits=0, reserved=0):
    return (struct.pack("<3h", *gyro) + struct.pack("<3e", *quat) + bytes([counter, bits, reserved]))


class DecisionTests(unittest.TestCase):
    def setUp(self):
        self.svc = imu_bus.BusImuService()

    def test_valid_block_is_live_with_raw_quaternion(self):
        f = self.svc.feed(block(), now=1.0)
        self.assertTrue(f["live"])
        self.assertEqual((f["status"], f["backend"]), ("live", "bus"))
        self.assertAlmostEqual(f["quaternion_xyzw"][2], 0.3827, places=3)
        self.assertAlmostEqual(f["quaternion_xyzw"][3], math.sqrt(1 - 0.3827 ** 2), places=3)
        self.assertAlmostEqual(f["gyro_dps"][0], 1.75)
        self.assertEqual(f["sample_count"], 7)

    def test_wrapping_sample_counter_is_not_a_restart(self):
        # 协议 §3：byte 12 是 u8、255→0 回绕。页面把「计数变小」当成数据流重来，并丢掉
        # 归零参考（imu_attitude.js 的 receive）—— 不展开的话鸭子静置时每 256 个块
        # （120 Hz 下约 2.1 秒）画面就跳回恒等姿态，现场看到的「又站直了」。
        self.svc.feed(block(counter=250), now=1.0)
        f = self.svc.feed(block(counter=6), now=1.1)     # 250 再走 12 个块：回绕后的样子
        self.assertEqual(f["sample_count"], 262)         # 展开成单调：不是重来
        self.assertEqual(f["counter_raw"], 6)            # 线上那个字节原样给，不动它

    def test_live_frame_satisfies_the_page_gate(self):
        # 【下游改动 A 的回归】imu_attitude.js 的 validPacket 要求 ready / configured 都是 1。
        # 上游总线这条路不填这两个字段，200 读得再健康页面也显示「数据无效」。
        live = self.svc.feed(block(), now=1.0)
        self.assertEqual((live["ready"], live["configured"]), (1, 1))
        not_ready = self.svc.feed(bytes(12) + bytes([0, imu_bus.BIT_FUSION_NOT_READY, 0]), now=1.02)
        self.assertEqual((not_ready["ready"], not_ready["configured"]), (0, 1))
        bad = self.svc.feed(block(reserved=9), now=1.04)
        self.assertEqual((bad["ready"], bad["configured"]), (0, 0))

    def test_spec_vector_blocks(self):
        # 总线协议.md §10 的两个应答数据段
        ready = bytes.fromhex("6400ceff03000000000020360700 00".replace(" ", ""))
        self.assertTrue(self.svc.feed(ready, now=1.0)["live"])
        not_ready = bytes(12) + bytes([0, 0x01, 0])
        f = self.svc.feed(not_ready, now=1.02)
        self.assertEqual(f["status"], "stale")
        self.assertIn("融合还没出数", f["message"])

    def test_board_faults_and_bad_blocks(self):
        cases = [(block(bits=imu_bus.BIT_IMU_COMM_FAIL), "通信失败"),
                 (block(bits=imu_bus.BIT_SELF_TEST_FAIL), "自检失败"),
                 (block(reserved=9), "保留字节"),
                 (block(quat=(1.0, 1.0, 0.0)), "主控会拒绝")]
        for data, hint in cases:
            with self.subTest(hint=hint):
                f = imu_bus.BusImuService().feed(data, now=1.0)
                self.assertEqual(f["status"], "error")
                self.assertFalse(f["live"])
                self.assertIn(hint, f["message"])

    def test_slow_reader_bit_is_a_warning_not_a_fault(self):
        f = self.svc.feed(block(bits=imu_bus.BIT_READER_TOO_SLOW), now=1.0)
        self.assertTrue(f["live"])
        self.assertIn("BIT3", f["message"])
        # 调试台自己 10 Hz 读：BIT3 每帧都亮是正常的，不能提示"读得太慢"（审查第 2 条）
        slow_console = imu_bus.BusImuService(poll_hz=10)
        f = slow_console.feed(block(bits=imu_bus.BIT_READER_TOO_SLOW), now=1.0)
        self.assertTrue(f["live"])
        self.assertNotIn("BIT3", f["message"])
        self.assertEqual(f["status_bits"], imu_bus.BIT_READER_TOO_SLOW)   # 原始状态位照样给
        self.assertIn("BIT3", imu_bus.BusImuService(poll_hz=50).feed(
            block(bits=imu_bus.BIT_READER_TOO_SLOW), now=1.0)["message"])

    def test_frozen_window_like_master(self):
        for k in range(imu_bus.FROZEN_FRAMES - 1):
            f = self.svc.feed(block(counter=k), now=1 + k * 0.02)   # 计数在变也没用：它在 12 字节窗口外
            self.assertTrue(f["live"], k)
        f = self.svc.feed(block(counter=99), now=2.0)
        self.assertEqual(f["status"], "stale")
        self.assertIn("frozen", f["message"])
        f = self.svc.feed(block(gyro=(101, -50, 3)), now=2.02)     # 低位一跳就恢复
        self.assertTrue(f["live"])

    def test_misses_take_200_out_and_ping_brings_it_back(self):
        for k in range(imu_bus.MISS_DEGRADE - 1):
            self.svc.feed(None, now=1 + k * 0.05)
            self.assertTrue(self.svc.include_in_sync())
        f = self.svc.feed(None, now=2.0)
        self.assertFalse(self.svc.include_in_sync())
        self.assertIn("单独 PING", f["message"])
        self.assertTrue(self.svc.due_probe(now=2.0))
        self.assertFalse(self.svc.due_probe(now=2.0 + imu_bus.RETRY_S / 2))
        self.assertFalse(self.svc.probe_result(False))
        self.assertTrue(self.svc.due_probe(now=2.0 + imu_bus.RETRY_S))
        self.assertTrue(self.svc.probe_result(True))
        self.assertTrue(self.svc.include_in_sync())
        self.assertTrue(self.svc.feed(block(), now=5.0)["live"])

    def test_host_stale_when_polling_stops(self):
        self.svc.feed(block(), now=1.0)
        self.svc.last_publish -= 1.0
        s = self.svc.get_status()
        self.assertEqual(s["status"], "stale")
        self.assertIsNone(s["quaternion_xyzw"])


class ServerPollingTests(unittest.TestCase):
    def setUp(self):
        stack = contextlib.ExitStack()
        self.addCleanup(stack.close)
        stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
        stack.enter_context(patch.object(server, "LOG", []))
        self.svc = imu_bus.BusImuService()

    def sim_bus(self, **imu_kw):
        port = SimSerial(JOINTS)
        port.servos[imu_bus.IMU_ID] = imu200.SimImu200(imu_ready_ms=0, **imu_kw)
        bus = feetech.FeetechBus(None, ser=port)
        calls = []
        real = bus.sync_read

        def spy(ids, addr, n):
            calls.append((list(ids), addr, n))
            return real(ids, addr, n)
        bus.sync_read = spy
        return port, bus, calls

    def test_single_transaction_with_200_last(self):
        # 【下游改动 C】200 排最后，不是最前 —— 排最前时板子和舵机抢总线（见 server.py 里的说明）
        port, bus, calls = self.sim_bus()
        with patch.object(server, "BUS", bus):
            port.clock = 100.0
            st = server.read_states_with_imu(JOINTS, self.svc)
            port.clock = 150.0
            st = server.read_states_with_imu(JOINTS, self.svc)
        ids, addr, n = calls[-1]
        self.assertEqual(len(calls), 2)                               # 一条事务，不是拆成两条
        self.assertEqual((ids[-1], addr, n), (200, 56, 15))           # 200 在队尾，地址 56，长度 15
        self.assertEqual(ids[:-1], JOINTS)
        self.assertTrue(all(st[i] is not None and "pos" in st[i] for i in JOINTS))
        status = self.svc.get_status()
        self.assertTrue(status["live"], status["message"])
        self.assertEqual(status["backend"], "bus")

    def test_silent_board_does_not_starve_servos(self):
        port, bus, calls = self.sim_bus(answer=False)
        with patch.object(server, "BUS", bus):
            for k in range(imu_bus.MISS_DEGRADE + 2):
                port.clock += 20
                st = server.read_states_with_imu(JOINTS, self.svc)
            self.assertNotIn(200, calls[-1][0])                        # 出列后舵机单独读
            self.assertTrue(all(st[i] is not None for i in JOINTS))
            self.assertEqual(self.svc.get_status()["status"], "disconnected")
            # 板子恢复应答：下一次 PING 把它放回同一条 sync_read
            port.servos[imu_bus.IMU_ID].answer = True
            self.svc.last_probe = None
            server.read_states_with_imu(JOINTS, self.svc)
            port.clock += 20
            server.read_states_with_imu(JOINTS, self.svc)
        self.assertEqual(calls[-1][0][-1], 200)
        self.assertTrue(self.svc.get_status()["live"])

    def test_polls_without_servos_and_never_sends_an_empty_list(self):
        # 审查第 1 条：台架上只接小板，扫描没扫到舵机（PRESENT 空）也要轮询
        with patch.object(server, "CLIENTS", {object()}), patch.object(server, "PRESENT", []), \
                patch.object(server, "IMU_SERVICE", self.svc):
            self.assertTrue(server.stream_should_poll())
        with patch.object(server, "CLIENTS", {object()}), patch.object(server, "PRESENT", []), \
                patch.object(server, "IMU_SERVICE", None):
            self.assertFalse(server.stream_should_poll())                 # 不开 IMU 时照旧
        with patch.object(server, "CLIENTS", set()), patch.object(server, "IMU_SERVICE", self.svc):
            self.assertFalse(server.stream_should_poll())                 # 没网页连着不读
        # 只有小板、而且它出列了：不发空 ID 表的 sync_read，只按时 PING
        port, bus, calls = self.sim_bus(answer=False)
        with patch.object(server, "BUS", bus):
            for _ in range(imu_bus.MISS_DEGRADE + 3):
                port.clock += 20
                self.assertEqual(server.read_states_with_imu([], self.svc), {})
        self.assertTrue(calls and all(ids for ids, _, _ in calls))
        self.assertEqual(len(calls), imu_bus.MISS_DEGRADE)

    def test_fake_bus_demo(self):
        bus = server.FakeBus(JOINTS)
        bus.imu200 = imu_bus.FakeImu200(ready_after_s=0)
        with patch.object(server, "BUS", bus):
            st = server.read_states_with_imu(JOINTS, self.svc)
        self.assertEqual(st[10]["pos"], 2048)
        self.assertTrue(self.svc.get_status()["live"])


class CliTests(unittest.TestCase):
    def test_flag_and_service(self):
        args = server.parse_args(["--fake", "--imu-bus"])
        self.assertTrue(args.imu_bus)
        svc = server.create_imu_service(args)
        self.assertEqual((svc.backend, svc.demo), ("bus", False))
        self.assertEqual(server.imu_backend(svc), "bus")

    def test_mutually_exclusive(self):
        for other in (["--imu-demo"], ["--imu-swd"], ["--imu-jlink", "x.json"]):
            with self.subTest(other=other), contextlib.redirect_stderr(io.StringIO()), \
                    self.assertRaises(SystemExit):
                server.parse_args(["--imu-bus", *other])


if __name__ == "__main__":
    unittest.main()
