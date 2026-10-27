# 来源与许可

本仓库是本地集成快照，不代表上游原作者发布，也不将第三方文件统一改授为本项目许可。

| 部分 | 来源 | 许可/边界 |
| --- | --- | --- |
| Rust 基础工程 | https://github.com/pollen-robotics/microduck | 保留 `microduck/LICENSE` Apache-2.0；本地 HD1910/BNO/App 适配包含修改 |
| RL 基础工程及几何 | https://github.com/pollen-robotics/microduck_rl | 保留 `microduck_rl/LICENSE` Apache-2.0 及源文件声明 |
| BAM 执行器实现 | https://github.com/Rhoban/bam | 通过 RL 的锁文件安装，不将第三方实现另行改授许可 |
| 当前四个 ONNX | https://github.com/LuwuDynamics/xgoduck_runtime_arduino/tree/8cdbbd84710d856581982c9eaf0d5e2970666232/python | 固定来源、未修改；所核对版本未附仓库 LICENSE。公开再分发/商用授权需另行确认，下载脚本不授予额外权利 |
| kissqy 上游策略六张 ONNX（walk / stand / sitstand / kick_left / ground_pick / roulade） | https://github.com/fanhao375/Microduck-kissqy 的 `microduck-hd1910/firmware/policies/`（本地检出的远端；各包内 `deployment-contract.json` 记 `training_source: pollen-robotics/microduck_rl`） | 原样拷入 `radxa/references/kissqy_20261027/`，sha256 与源一致、未修改。随包 `LICENSE` 为 **Personal Non-Commercial License v1.0（个人非商业）**，明确不覆盖上游代码/几何/第三方权利。本仓库按原样保存并随包保留 `LICENSE` 与署名；对该批权重的第三方再分发或商用仍需先取得授权 |
| M6 参数参考 | https://github.com/LuwuDynamics/xgoduck_rl | 外部参考参数，不等于本机已完成执行器辨识 |
| 舵机调试工具 `tools/servo-web` | https://github.com/fanhao375/microduck-replica | 按该仓库的双许可 `LICENSE`：`tools/` 下的代码为 Apache-2.0；`tools/servo-web/model/` 的 3D 几何是上游 Microduck 模型的衍生作品，为 CC BY-NC-SA 4.0（**禁止商用**）。本地做过修改：舵机 ID 改成本机的 1–15、姿态与方向表调整、前端拆成 `style.css` / `app.js` / `viewer.js`。模型来源见 `tools/servo-web/model/NOTICE.md` |
| APK、演示与照片 | 本地工作区及用户提供素材 | 不包含 APK 对应前端源码，不声明第三方板卡照片的额外授权 |

感谢 [fanhao375/microduck-replica](https://github.com/fanhao375/microduck-replica) 提供舵机调试工具 `tools/servo-web` —— 装机、校零、台架验收都靠它；结构件与装配模型来自 [microduck-replica-cad](https://github.com/fanhao375/microduck-replica-cad)。

导出的 Rust 与 RL 源码来自含本地未提交修改的工作区，不能仅凭上游 HEAD 重建本地修改。发布提交及文件校验清单才是本次集成快照的依据。
