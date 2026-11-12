# -*- coding: utf-8 -*-
"""编号加载器的测试：ID 只认 installation.json，其余（中文名、分组、顺序）认 servo.json。
这几个用例保证「换编号只改一个文件」这件事真的成立。"""
import json
import os
import tempfile
import unittest

import joints


def write_installation(path, pairs, direction=-1):
    data = {"joints": [{"name": n, "id": i, "zero_ticks": 2048, "direction": direction}
                       for n, i in pairs]}
    with open(path, "w", encoding="utf-8") as f:
        json.dump(data, f)
    return path


class DefaultLoadTests(unittest.TestCase):
    def setUp(self):
        self.j = joints.load()

    def test_reads_the_repo_installation_file(self):
        self.assertTrue(self.j.source.endswith(os.path.join("radxa", "installation.json")))
        self.assertEqual(len(self.j.items), 15)

    def test_order_is_servo_json_order_not_id_order(self):
        # 左腿 → 颈/头/嘴 → 右腿；按 ID 升序会变成「踝→膝→髋」
        self.assertEqual(self.j.order[:5], [10, 9, 8, 7, 6])
        self.assertEqual(self.j.order[5:10], [12, 11, 14, 13, 15])
        self.assertEqual(self.j.order[10:], [5, 4, 3, 2, 1])

    def test_names_and_cn_round_trip(self):
        self.assertEqual(self.j.id_of("left_knee"), 7)
        self.assertEqual(self.j.name_of(7), "left_knee")
        self.assertEqual(self.j.cn_of(7), "左膝")

    def test_groups_cover_every_joint_exactly_once(self):
        got = [i for g in self.j.payload()["groups"] for i in g["ids"]]
        self.assertEqual(sorted(got), sorted(self.j.ids))

    def test_direction_comes_from_installation(self):
        self.assertEqual(set(self.j.dir_by_id.values()), {-1})


class CustomInstallationTests(unittest.TestCase):
    """换一份 installation.json：编号跟着变，表和顺序不用动。"""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        base = [("left_hip_yaw", 10), ("left_hip_roll", 9), ("left_hip_pitch", 8),
                ("left_knee", 7), ("left_ankle", 6), ("neck_pitch", 12), ("head_pitch", 11),
                ("head_yaw", 14), ("head_roll", 13), ("mouth", 15), ("right_hip_yaw", 5),
                ("right_hip_roll", 4), ("right_hip_pitch", 3), ("right_knee", 2),
                ("right_ankle", 1)]
        # 膝盖 7 → 21，其余不动
        self.pairs = [(n, 21 if n == "left_knee" else i) for n, i in base]
        self.path = write_installation(os.path.join(self.tmp.name, "installation.json"), self.pairs)

    def tearDown(self):
        self.tmp.cleanup()

    def test_new_id_wins(self):
        j = joints.load(self.path)
        self.assertEqual(j.id_of("left_knee"), 21)
        self.assertIsNone(j.name_of(7))
        self.assertEqual(j.cn_of(21), "左膝")

    def test_order_still_from_servo_json(self):
        j = joints.load(self.path)
        self.assertEqual(j.order, [10, 9, 8, 21, 6, 12, 11, 14, 13, 15, 5, 4, 3, 2, 1])

    def test_joint_missing_from_servo_json_is_appended_with_its_english_name(self):
        path = write_installation(os.path.join(self.tmp.name, "extra.json"),
                                  self.pairs + [("tail_wag", 30)])
        j = joints.load(path)
        self.assertEqual(j.id_of("tail_wag"), 30)
        self.assertEqual(j.cn_of(30), "tail_wag")
        self.assertEqual(j.order[-1], 30)
        self.assertIn("tail_wag", j.missing_from_config)

    def test_explicit_path_must_exist(self):
        with self.assertRaises(FileNotFoundError):
            joints.load(os.path.join(self.tmp.name, "nope.json"))


class BadFileTests(unittest.TestCase):
    def test_duplicate_id_is_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            path = write_installation(os.path.join(d, "dup.json"),
                                      [("left_knee", 7), ("right_knee", 7)])
            with self.assertRaises(ValueError):
                joints.load(path)


if __name__ == "__main__":
    unittest.main()
