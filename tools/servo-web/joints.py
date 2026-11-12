# -*- coding: utf-8 -*-
"""舵机编号的唯一入口。ID 从 radxa/installation.json 来（robotd 读的是同一份），
中文名、分组、显示顺序从 servo-web/servo.json 来。

为什么要这一层：网页、3D、姿态、校准、报表原先把「膝盖 = 7 号」写死在各自的表里，
换一颗舵机的编号就要改五六个文件，漏一个就两边不一致。现在所有地方都按关节名查，
编号只在安装文件里写一次。改了 installation.json 里某个 id，这台机器上所有工具一起变。

用法：
    import joints
    J = joints.load()                  # 默认找 servo.json 里写的路径
    J.id_of("left_knee")               # 7
    J.name_of(7)                       # "left_knee"
    J.cn_of(7)                         # "左膝"
    J.order                            # 显示顺序的 ID 列表
    J.payload()                        # 给网页的 {order, names, cn, groups}
"""
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))
CONFIG_FILE = os.path.join(HERE, "servo.json")

# 找不到安装文件时依次试这些相对位置。最后两个是板子上的实际布局：
# servo-web 在 /root/servo-web，运行时仓库在 /home/duck/workspace/microduck-hd1910m。
FALLBACKS = [
    os.path.join(HERE, "..", "..", "radxa", "installation.json"),
    "/home/duck/workspace/microduck-hd1910m/radxa/installation.json",
]


class JointMap:
    def __init__(self, items, groups, source):
        self.items = items
        self.groups = groups
        self.source = source
        self.name_by_id = {j["id"]: j["name"] for j in items}
        self.id_by_name = {j["name"]: j["id"] for j in items}
        self.cn_by_id = {j["id"]: j["cn"] for j in items}
        self.cn_by_name = {j["name"]: j["cn"] for j in items}
        self.group_by_id = {j["id"]: j["group"] for j in items}
        self.dir_by_id = {j["id"]: j["direction"] for j in items}
        self.dir_by_name = {j["name"]: j["direction"] for j in items}
        self.order = [j["id"] for j in items]
        self.ids = list(self.order)

    def id_of(self, name, default=None):
        return self.id_by_name.get(name, default)

    def name_of(self, sid, default=None):
        return self.name_by_id.get(sid, default)

    def cn_of(self, sid, default=""):
        return self.cn_by_id.get(sid, default)

    def payload(self):
        """给网页的公开表。网页只认这些键，不再自己维护中文名和顺序。"""
        return {
            "order": self.order,
            "names": self.name_by_id,
            "cn": self.cn_by_id,
            "groups": self.groups,
            "installation": self.source,
        }


def _read_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def find_installation(explicit=None, config=None):
    """按「命令行 > 环境变量 > servo.json > 默认位置」找 installation.json。
    明确指定的（命令行 / 环境变量）找不到就直接报错 —— 它写错了，退回默认编号比起不来更危险；
    servo.json 里写的找不到时退回默认位置（换台机器拷过去还能跑）。"""
    def full(cand):
        return os.path.normpath(cand if os.path.isabs(cand) else os.path.join(HERE, cand))

    for label, cand in (("--installation 指定的", explicit),
                        ("环境变量 SERVO_INSTALLATION 指定的", os.environ.get("SERVO_INSTALLATION"))):
        if not cand:
            continue
        path = full(cand)
        if not os.path.exists(path):
            raise FileNotFoundError(f"{label}文件不存在：{path}")
        return path

    tried, seen = [], set()
    for cand in [(config or {}).get("installation")] + FALLBACKS:
        if not cand:
            continue
        path = full(cand)
        if path in seen:
            continue
        seen.add(path)
        tried.append(path)
        if os.path.exists(path):
            return path
    raise FileNotFoundError("找不到舵机编号文件 radxa/installation.json，试过：\n  "
                            + "\n  ".join(tried)
                            + "\n用 --installation 指定，或设环境变量 SERVO_INSTALLATION")


def load(explicit=None):
    config = {}
    if os.path.exists(CONFIG_FILE):
        config = _read_json(CONFIG_FILE)
    path = find_installation(explicit, config)
    raw = _read_json(path)

    by_name = {}
    extra = []                       # installation 里有、servo.json 里没写的：补在后面
    for j in raw.get("joints", []):
        name, sid = j.get("name"), j.get("id")
        if name is None or sid is None:
            continue
        by_name[name] = {"id": int(sid), "direction": -1 if j.get("direction") == -1 else 1}

    items, seen = [], set()
    for s in config.get("servos", []):
        name = s.get("name")
        if name not in by_name:
            continue                 # 安装文件里没有这颗：跳过，不凭空造 ID
        items.append({"name": name, "id": by_name[name]["id"],
                      "cn": s.get("cn") or name, "group": s.get("group") or "",
                      "direction": by_name[name]["direction"]})
        seen.add(name)
    for name, info in by_name.items():
        if name in seen:
            continue
        extra.append(name)
        items.append({"name": name, "id": info["id"], "cn": name, "group": "",
                      "direction": info["direction"]})

    ids = [j["id"] for j in items]
    dup = {i for i in ids if ids.count(i) > 1}
    if dup:
        raise ValueError(f"installation.json 里编号重复：{sorted(dup)}（同一个 ID 给了两个关节）")

    labels = {g.get("key"): g.get("label") or g.get("key") for g in config.get("groups", [])}
    groups = []
    for g in config.get("groups", []):
        key = g.get("key")
        gids = [j["id"] for j in items if j["group"] == key]
        if gids:
            groups.append({"key": key, "label": labels[key], "ids": gids})
    leftovers = [j["id"] for j in items if j["group"] not in labels]
    if leftovers:
        groups.append({"key": "", "label": "其它", "ids": leftovers})

    jm = JointMap(items, groups, path)
    jm.missing_from_config = extra
    return jm


if __name__ == "__main__":
    j = load()
    print(f"编号来源：{j.source}")
    for it in j.items:
        print(f"  {it['id']:>3}  {it['name']:<16} {it['cn']:<10} {it['group']}")
