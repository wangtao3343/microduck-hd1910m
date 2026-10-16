# three.js（本地副本，0.160.0）

调试台以前从 jsDelivr 拉 three.js。台架上经常没网，而 importmap 只认那个 CDN 地址，
拉不到就整个 3D 视图白屏 —— 修舵机的时候最不该出的问题。所以拷一份进来。

| 文件 | 来源 |
|---|---|
| `three.module.js` | `three@0.160.0/build/three.module.js` |
| `addons/controls/OrbitControls.js` | `three@0.160.0/examples/jsm/controls/OrbitControls.js` |
| `LICENSE` | `three@0.160.0/LICENSE`（MIT） |

只放了**用得到**的那一个 addon：`viewer.js` 只 import 了 `three` 和
`three/addons/controls/OrbitControls.js`（后者自己只 import `three`，没有别的依赖）。
要用别的 addon，从同一个版本拷进来，放对应的 `addons/` 子目录 —— 因为 importmap 里
`three/addons/` 指的是**目录前缀**，路径必须跟上游 `examples/jsm/` 一致。

换版本：两条 URL 都改，`vendor/three/` 整个换掉，再照上面把 addon 补齐。别只换一半 ——
`three.module.js` 和 addon 版本不一致是最难查的一类错。
