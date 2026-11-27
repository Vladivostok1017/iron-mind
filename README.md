# 钢心 / Iron Mind

> **你只能发电报，不能拖部队。** 一个 AI 驱动的战役级大战略引擎：将军们按自己的性格理解你的命令，可能追问、可能打折扣、也可能抗命。
>
> 引擎只提供框架——每个战役都是可导入的**模块**，引擎里不认识任何一场具体的战争。

[![CI](https://github.com/Vladivostok1017/iron-mind/actions/workflows/ci.yml/badge.svg)](https://github.com/Vladivostok1017/iron-mind/actions/workflows/ci.yml)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![engine 0.12](https://img.shields.io/badge/engine-0.12-2f6f4f)

**三个仓库：**

| 仓库 | 作用 |
|---|---|
| **iron-mind**（这里） | 引擎：内核 / 系统 / 规则 / 服务端 / 单文件前端 |
| [iron-mind-sdk](https://github.com/Vladivostok1017/iron-mind-sdk) | 作者工具：存盘即重载的开发循环、零警告脚手架 |
| [nanxun-1938](https://github.com/Vladivostok1017/nanxun-1938) | 首个战役模块：1938 年 7–10 月的南浔线，87 省 / 18 个单位 |

> **接手开发请看 [`HANDOFF.md`](HANDOFF.md)** —— 项目现状、代码地图、设计决策、已知的坑、下一步建议。10 分钟能上手。

**个人项目，非商业，免费发布。** 不设商业模式，不做付费内容，不做在线平台。

**产品形态：引擎 + 模块。** 游戏本体只提供框架，每个战役都是可导入的模块，玩家可以自己创作。
本仓库只有引擎：Kernel / Systems / Rules / 服务端 / 前端。
里面**不应该出现"1938""日本""中国"这类字样**——那是模块的事。

```powershell
git clone https://github.com/Vladivostok1017/nanxun-1938 ..\nanxun-1938
$env:IRONMIND_MODULES = "..\nanxun-1938"   # 告诉引擎去哪个目录找模块
cargo run -p sim-server                    # 默认捡到的就是南浔线
```

---

## 当前状态

v1.3 · **上 iPhone**：同一个内核，第三种壳。iOS 这层是 **Swift + WKWebView**，
不是 wry 的 iOS 后端——键盘顶页面、`confirm()` 弹窗、安全区、禁掉页面级双指缩放
这四件事都得 UIKit 管（理由见 [`docs/25-iOS.md`](docs/25-iOS.md)）。Rust 那边只多了一个
出站 HTTP 的口子：iOS 上没有 `curl`，模型请求改走 URLSession。
战役模块和安卓一样**打进二进制**，装上就能玩，不需要电脑也不需要联网。

v1.2 · **上手机**：同一个内核编成安卓 APK（arm64），战役模块**打在 .so 里**，
装上就能玩——不需要电脑、不需要联网。手机上地图占满屏幕，电报台/公文堆/部队/战报
收进底部抽屉；单指平移、双指缩放；存档与日志落在 app 私有目录。

v1.1.1 · **插件可更新**：同名模块按版本判断，新版本直接覆盖（`0.10.0 > 0.9.0`，
按数字比不比字符串）；同版本重装、降级、撞名先问一句；正在玩的那一局会被重开。

v1.1 · **可选阵营**：模块声明 `sides [{id,name,hq}]` + `playable_sides`，玩家可以打任意一边，
AI 接手另一边（用它自己的司令官性格）。**引擎内部只有 A/B 两方**——国名、番号、战区名
一个字都不在引擎里，全在模块。193 个测试（含黄金回放 + 一条"引擎里不许出现具体战争"的分层测试）。

v1.0 · **可玩**：确定性内核、真实地形与县界、指挥即通信（电报 → 将领 → 抗命）、
LLM 将领、敌方统帅部（AI 玩家）、空海战、补给与合围、编制变动、
模块声明结局、跨省行军、浏览器内切模块。四个机制（弹药油料 / 铁路被炸 /
派系人事 / 连续正面）从预览版并入主线，**默认全开**；默认值跟 `scale` 走。

v0.10 · **能装**：`install.ps1` 装成桌面程序（原生窗口 + 开始菜单 + 卸载），
有首页、有六步上手引导、**首次启动就问要不要接模型**（填地址/模型/密钥，可当场测试），
每 5 天自动存档。**不填 key 也能玩**：将领走本地规则判断（会下令、会请示、会抗命），
填了 key 就接真模型、保存即生效不用重启。见 `docs/24-桌面版.md`。

### 手机上怎么出包

```powershell
# 默认内嵌南浔线，出 release APK 并装到连着的手机
pwsh -File tools\build-android.ps1 -Install

# 内嵌你自己的模块（一个模块目录或 .imod 文件）
pwsh -File tools\build-android.ps1 -Module F:\我的战役\my_mod.imod -Install
```

需要 NDK（脚本里默认 `C:\Android\android-ndk-r27c`，可用 `-Ndk` 改）、
JDK 17+、以及本机缓存过一份 Gradle 8.9+。产物在 `dist\IronMind-android-release.apk`。

**iOS**（要 Mac + 完整 Xcode，不是 Command Line Tools）：

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
tools/build-ios.sh                              # 未签名 .ipa → dist/
tools/build-ios.sh --module ../nanxun-1938/nanxun_1938.imod
tools/build-ios.sh --simulator                  # 模拟器里直接跑，不需要 Apple ID
```

产物是**未签名** `.ipa`（签名要你自己的 Apple ID，那种东西不该进仓库）。
装到手机上走 Xcode 直接 Run，或者用 AltStore / Sideloadly 重签。细节与已知限制见
[`docs/25-iOS.md`](docs/25-iOS.md)。

---

## 已拍板的关键决定

| 决定 | 选择 | 理由 |
|---|---|---|
| 项目性质 | 个人非商业，免费发布 | 不做平台、不设商业模式，专注做出来的东西本身 |
| 产品形态 | 引擎 + 可导入模块 | 内容生命力交给社区；更新不必等官方 |
| AI 部署 | 纯云端 API，不做本地模型 | 小模型判断质量不足以支撑将领决策 |
| 首作战役 | 1938 武汉会战（示例模块） | 五条并行战线 + 一份不够用的预备队 + 派系政治 |
| 胜负判定 | 不判输赢，给结局 | 史实上说得通；避免诱导刷分；平衡容错大一个数量级 |
| 阵营 | 引擎只认 A/B；哪两方、叫什么，模块声明 | 换一场战役（甚至换一个时代）不用改引擎代码；玩家也能挑着打 |
| 排期 | 先做 3-4 个月垂直切片 | 业余投入下完整路线 = 3-4 年，先验证核心假设 |

---

## 设计文档

| 文档 | 内容 |
|---|---|
| `docs/00-方向方案与技术提纲.md` | 总纲：定位、六条设计原则、架构选型、AI 三层分级、里程碑、风险 |
| `docs/01-武汉会战场景设计.md` | 首作战役：五条战线、战斗序列、派系系统、事件清单、MVP 裁剪 |
| `docs/04-地图数据方案.md` | 地图数据从哪来、四层管线、工作量、**引擎与模块的地图归属与体积** |
| `docs/05-结局系统设计.md` | 12 种结局、状态变量、判定规则、历史基线对照 |
| `docs/07-框架与模块架构.md` | 引擎与模块边界、模块格式、版本契约、作者工具链 |
| `docs/08-情报与单位类型.md` | 三层信息模型、侦察精度、将领独立思考、跨时代兼容、空军/海军 |
| `docs/09-规则引擎与扩展边界.md` | 模块自定义规则的机制、「万物皆模块」的四层边界 |
| `docs/24-桌面版.md` | **怎么装成桌面程序**：外壳/内核分工、首页、引导、配置、已知限制 |
| `docs/25-iOS.md` | **怎么上 iPhone**：为什么不用 wry、Swift 壳做哪四件事、iOS 特有的五个坑、出包与签名 |
| `docs/26-模块作者-AI提示词指南.md` | **给模块作者的提示词规矩**：哪些字段进提示词、`persona`/`agenda` 各写什么、为什么不能写"保存实力"、正反例对照、自检方法 |
| **[content/README.md](content/README.md)** | **模块开发手册**：四个 JSON 的完整字段表、结局/规则/通信的写法、33 条校验码、发布清单 |
| `docs/06-战役数据底稿.md` | 三源合并的兵力/时间线，以及"兵力数据缺失"的处理方案 |
| `docs/03-史料核对-战斗序列.md` | 中方编制树（师级+师长）、派系、时间线矛盾、伤亡数据 |
| `docs/02-视频精读-战斗记录.md` | 视频逐帧精读：战斗记录 + 14 段空间态势 + 10 种战术要点原型 |

---

## 核心技术判断

**模拟核心用 Rust 编译到 WASM，渲染用 TypeScript + PixiJS，壳用 Tauri。**
不选 Bevy，因为这类游戏"UI 即游戏"，Rust 全栈会把编辑器迭代拖慢 2-3 倍。

**模拟核心对 AI 零依赖。** 拔掉 AI，游戏照样能跑，将领退回启发式。

**AI 分三层，权限严格隔离。** L3 规则层只能产出候选参数、L2 决策层只能在合法动作空间发指令、L1 叙事层无写入权限。

**将领不是每 tick 调 LLM。** 平时跑本地效用函数，只有战役转折、补给危机、政治冲突这类关键节点才请求。一局几十到几百次。

---

## 下一步

**垂直切片进行中。** 目标是回答一个问题：意图式指挥到底好不好玩。

- [x] 引擎地基：确定性 tick + 状态哈希（Rust → WASM）
- [x] 定点数、多流 RNG、世界状态、补给/移动/战斗/士气
- [x] 黄金回放测试（21 单元 + 14 集成，全过）
- [x] WASM 目标编译通过
- [x] **模块加载器**：从 `.imod` 目录读地图与编制，带静态校验
- [x] 示例模块：南浔线 1938，20 省，真实地名
- [x] **规则 AI**：意图（固守/进攻/推进/撤退）、目标承诺、集中兵力、打残撤退
- [x] **情报系统**：三层信息模型、侦察（战斗接触/航空/邻接）、情报衰减
- [x] **空军**：侦察半径与打击强度，不参与地面战斗
- [x] **将领独立思考**：两级决策、自主主张、抗命判定在引擎侧
- [x] **战报延迟**：情报要经通信链路传回，在途期间上级一无所知
- [x] **跨时代兼容**：引擎不认年代，只认延迟参数；已用 1805 模块实证
- [x] **规则引擎**：模块可写声明式规则，扩展引擎没有的机制
- [ ] 海军（只有类型定义，无行为）、空军打击（只有参数）
- [x] **海军**：海域省份、海陆分离通行、舰炮支援（邻接水域 +60% 上限）
- [x] **空军**：侦察半径 + 对地打击（strike_power 生效）
- [x] **前端**：本地 HTTP 服务 + 单文件页面，只渲染情报层
- [ ] 空军战损、海军登陆与封锁、装甲机动差异
- [ ] 前线识别：让双方主力真正撞上（当前会穿插）

切片不通过就不扩展。通过之后，再按 `docs/00` 的完整路线图走。

---

## 模拟核心的三个已知短板

写下来免得忘掉，也免得把参数当解药：

**1. 战斗是单变量的阈值系统。** 总战力比决定一切。日军战斗力系数调 1.7 全灭、调 2.5 碾压，中间几乎没有过渡。没有集中突破、没有战场宽度、没有预备队投入的次第。

**2. 没有前线概念。** 部队会穿过对方的空档，双方在同一张图上互相追逐。已经加了"对向遭遇战"判定，但主力仍常常错身而过。

**3. 没有补给枢纽与前沿仓库。** 只有"补给源 + BFS 衰减"。日军靠 `supply_reach` 缓解，但这只是一个补丁，不是真正的后勤模型。

这三条都不是调参能解决的，需要模型本身变复杂。这也是"意图式指挥"要解决的问题——玩家通过集中兵力、选主攻方向，把线性比拼变成非线性的博弈。

---

## 怎么跑

### 玩（不装 Rust，直接下载）

去 [Releases](https://github.com/Vladivostok1017/iron-mind/releases) 下 `IronMind-<版本>-win64.zip`，
解压双击 `IronMind.exe`。原生窗口、离线可玩（接模型才需要联网），模块已经打在 `modules\` 里。

> exe 没有代码签名，SmartScreen 第一次会拦：**更多信息 → 仍要运行**。
> 全部源码都在这，不放心可以自己 `cargo build --release`。

### 玩（装成桌面程序，带开始菜单与卸载）

```powershell
pwsh -File install.ps1 -Modules F:\dsh\nanxun-1938 -DefaultModule nanxun_1938.imod
```

装好之后开始菜单和桌面会有「钢心」，双击就是**原生窗口**：首页 → 选战役 → 上手引导。
配置（模型 key、敌方统帅部节奏、模块目录）在 `%LOCALAPPDATA%\IronMind\settings.env`，记事本可改。
`pwsh -File install.ps1 -Uninstall` 卸载，存档会先备份到「我的文档」。
**macOS** 是同一个外壳（wry 在 Mac 上就是系统的 WKWebView），只是打包方式不同：

```bash
tools/build-mac.sh          # → dist/IronMind.app + dist/IronMind-<版本>-mac.zip
open dist/IronMind.app      # 存档在 ~/Library/Application Support/IronMind/
```

细节与已知限制见 [`docs/24-桌面版.md`](docs/24-桌面版.md)。

### 开发：做模块（改完存盘就重载）

作者工具在另一个仓库 [`../iron-mind-sdk`](../iron-mind-sdk)——**引擎里不装开发行为**：

```powershell
cd ..\iron-mind-sdk
node tools\module-init.js --id org.you.mymod --name "我的战役"   # 生成骨架
node imdev.mjs --module F:\dsh\nanxun-1938\nanxun_1938.imod     # 起内核 + 存盘即重载
```

一存盘就重载、回到第 0 日；开着的页面 3 秒内自己换地图。
存了一半的坏 JSON 只报错、不毁现场。重载会清空命令流（这是开发工具，不是读档）。

它只是拿引擎的**公开 HTTP 接口**（`POST /api/module`）驱动本引擎，所以引擎内部怎么重构
都不用改它；引擎升级后跑 `node imdev.mjs --check` 确认接口没缺（缺了退出码 2）。
模块格式规范仍然在 [`content/README.md`](content/README.md)——它必须和 `validate.rs`/`migrate.rs`
住在一起，否则文档和实现必然漂移。

### 开发：改引擎（浏览器 + 随手改页面）

```powershell
$env:IRONMIND_MODULES = "F:\dsh\nanxun-1938"   # 模块仓库在哪
$env:IRONMIND_LLM = "mock"                     # 不联网，用本地判断
cargo run -p sim-server                        # 然后打开 http://127.0.0.1:8899/
```

玩法：**你不能拖动部队**。右侧电报台选一个收报人（兵团/军/师都行），
写一句话发出去；将领按自己的判断执行，可能追问、可能抗命。
空格 = 推进一天，回车 = 推进到有事。左侧是真实地形地图，右侧是电报台/公文堆/部队/态势/战报。

**注意**：地图上敌方省份初始是灰的——因为你还没侦察到。派空军出去，情报过一两天才会到。

### 无头跑批

```bash
# 测试
cargo test --workspace

# 跑示例模块
cargo run -p sim-testbed -- --module content/scenarios/nanxun_1938.imod --ticks 120 --verbose

# 内置演示场景
cargo run -p sim-testbed -- --ticks 120 --verbose

# 换种子
cargo run -p sim-testbed -- --ticks 200 --seed 7

# 编 WASM 给前端
cargo build --target wasm32-unknown-unknown -p sim-core
```

---

## 代码结构

```
crates/sim-core/src/
├─ fixed.rs      定点数 Q16.16（模拟层禁用浮点）
├─ rng.rs        SplitMix64 + 多流派生（每系统/每实体独立流）
├─ hash.rs       FNV-1a 状态哈希（黄金回放用）
├─ world.rs      省份 / 单位 / 世界状态
├─ systems.rs    补给 → 移动 → 战斗 → 士气
└─ scenario.rs   内置测试场景（假数据，不是史实）

crates/sim-content/src/
├─ model.rs      模块数据模型（引擎与内容的契约）
├─ validate.rs   静态校验：结构、引用、邻接、连通性
└─ lib.rs        加载 → 校验 → 构建 World

crates/sim-server/src/
├─ lib.rs        HTTP 端点：状态 / 推进 / 电报 / 模块 / 存档
├─ llm.rs        将领的大脑：提示词、解析、后台问询线程
└─ http.rs       出站 HTTP：桌面/安卓走 curl，iOS 由外壳装 URLSession

crates/sim-ios/          iOS 壳的 Rust 半边（staticlib + 4 个 C 函数）
crates/sim-bundle/       外壳 build.rs 的公共部分：模块目录 → .imod

crates/sim-content/tests/   模块加载与校验（10 个）
crates/sim-core/tests/      黄金回放（4 个）

content/scenarios/nanxun_1938.imod/   示例模块
content/README.md                     模块开发指南
```

**模拟层的三条铁律**：不用浮点、不用 HashMap、遍历顺序即定义顺序。

---

## 目录结构（规划）

```
iron-mind/
├─ crates/
│  ├─ sim-core/          # 确定性模拟核心
│  ├─ sim-content/       # 数据加载与 schema 校验
│  ├─ mapgen/            # 地图管线 CLI
│  └─ sim-testbed/       # 无头模拟跑批
├─ apps/
│  ├─ desktop/           # Tauri 壳
│  ├─ client/            # 渲染与 UI
│  └─ ai-orchestrator/   # AI 编排 sidecar
├─ content/
│  ├─ maps/
│  ├─ scenarios/
│  ├─ events/
│  └─ generals/
├─ mods/
└─ docs/
```





### 前端设计来源

界面布局参考了三个开源项目的做法（2026-11 查）：

| 项目 | 借鉴的点 |
|---|---|
| [Open-Doctrines](https://github.com/Pr1nted/Open-Doctrines) | 地图占满、UI 贴边；**底部地图模式切换**（同一张图按不同维度着色）；右下时间 |
| [Archon-Engine](https://github.com/ForgottenHistory/Archon-Engine) | 确定性模拟与渲染分离（我们已经是这个结构）；作者也卡在渲染上 |
| [Late-Eastern-Han](https://github.com/CtxPilot/Late-Eastern-Han-Dynasty) | 中国题材的信息密度与用词 |

三个直接落地的改动：

1. **操作逻辑**：原来是"先选命令模式 → 再点省份"，反直觉。
   现在是"**点单位 → 相邻可达省亮金色虚线圈 → 点就执行**"，
   `Shift`+点 = 推进，`Alt`+点 = 撤退，点自己所在省 = 固守。
2. **地图模式切换**：归属 / 情报 / 补给 / 地形 四档，底部切换。
3. **视觉**：深色底 + 低饱和省份色 + 高饱和单位标记；情报精度用环线样式区分
   （精确实线、近似虚线、传闻点线）。

**没抄的部分**：他们的地图是真实省份多边形，我们只有邻接数据，所以用力导向布局。
这是硬约束——要做得像地图，得先有省份几何数据（见 `docs/04` 的地图管线）。

