# 项目交接文档

> 给下一个接手的人。10 分钟读完，能跑起来、能改东西、知道哪些坑已经踩过。
> 最后更新：2026-11 · 当前版本 v0.5

---

## 0 一句话

一个 AI 驱动的战役级大战略游戏。**引擎 + 模块**架构：游戏本体只提供框架，每个战役都是可导入的 JSON 模块。首个模块是 1938 年南浔线。

> **1.0 的裁决（2026-11）**：四个实验机制（弹药油料、铁路被炸、派系人事、
> 连续正面模型）不再是"预览版"，已并回主线。默认值**跟着地图尺度走**：
> `theater`/`naval` 用粗口径（占领要清空邻省），`front`/`battle`/`siege` 用连续正面模型。
> 想覆盖就写 `"preview": {"x": true/false}`，界面会挂"经典模式"角标。
> 黄金哈希为此**刻意**改过一次（`0x09fd…` → `0x6f5e…`，理由见 `golden_replay.rs`）。
>
> 经验：南浔线（`theater`，35 km/省）如果错误地打开连续正面模型，77 日内**南昌会丢**；
> 按尺度默认（粗口径）则是"九江、德安失守、南昌还在"，与史实吻合。
> 判断平衡问题时先确认尺度标对了没有——尺度错会让"平衡"看起来坏了。

技术形态：Rust 确定性模拟核心（可编译成 WASM）+ 本地 HTTP 服务 + 单文件 HTML 前端。

---

## 0 三个仓库

从 2026-11 起，**引擎、作者工具、战役分开三个仓库**：

| 仓库 | 负责 | 不该出现的东西 |
|---|---|---|
| F:\dsh\iron-mind（本仓库） | Kernel / Systems / Rules / 服务端 / 前端 | 任何"1938""日本""中国"的硬编码；任何开发行为（热重载、脚手架） |
| F:\dsh\iron-mind-sdk | 模块脚手架、开发循环、将来的可视化编辑器 | 任何引擎逻辑（只调引擎的公开 HTTP 接口） |
| F:\dsh\nanxun-1938 | 地图、序列、将领、规则、结局、战役文档 | 任何引擎逻辑 |

为什么 SDK 单独一个仓库：热重载和脚手架是**开发行为**，不该在玩家装的二进制里，
也不该混进作者的模块仓库；但它们需要的全部能力已经是引擎的公开接口，
所以 SDK 不复制引擎代码——引擎内部怎么重构，它几乎不用改
（引擎升级后跑 `node imdev.mjs --check`，缺接口就退出码 2）。

引擎通过 IRONMIND_MODULES 找模块，也可以命令行 --module 指定任意路径；
浏览器左下角的下拉框会把两边仓库的模块都列出来。

---

## 1 怎么跑

```bash
# 需要 Rust 1.80+（已装好 1.100）
cd F:\dsh\iron-mind

# 全部测试（188 个 / 31 个测试二进制；南浔线那几条找不到模块时会明确跳过）
cargo test --workspace

# 装成桌面程序（原生窗口 + 开始菜单 + 可卸载，不需要管理员）
# 引擎自带两个演示模块；南浔线在隔壁仓库，用 -Modules 指过去
pwsh -File install.ps1 -Modules F:\dsh\nanxun-1938 -DefaultModule nanxun_1938.imod
pwsh -File install.ps1 -Uninstall          # 卸载（存档先备份到「我的文档」）

# 玩（开发模式）：启动服务，然后浏览器打开 http://127.0.0.1:8899/
# 战役模块现在住在**另一个仓库**（F:\dsh\nanxun-1938），告诉引擎去哪里找：
$env:IRONMIND_MODULES = "F:\dsh\nanxun-1938"
cargo run -p sim-server                    # 默认捡到南浔线

# 做模块：开发循环在 SDK 仓库（F:\dsh\iron-mind-sdk），引擎里不放开发行为
cd ../iron-mind-sdk
node tools/module-init.js --id org.you.mymod --name "我的战役"   # 生成骨架
node imdev.mjs --module ../nanxun-1938/nanxun_1938.imod          # 起内核 + 存盘即重载
node imdev.mjs --check                                           # 引擎升级后查接口有没有缺
# 不想用 SDK 的话，等价的最小做法还是显式指定：
cargo run -p sim-server -- --module ../nanxun-1938/nanxun_1938.imod

# 无头跑批，看战局演化（同样要 IRONMIND_MODULES）
cargo run -p sim-testbed -- --module ../nanxun-1938/nanxun_1938.imod --ticks 60

# 编 WASM（还没接前端，只是验证能编）
cargo build --target wasm32-unknown-unknown -p sim-core

# 重新生成战役地图的邻接表（作者工具，会覆盖 map.json）
node tools/mapgen/gen.js --dry

# 从真实高程（DEM）重算地形，写回 map.json（详见 docs/16）
node tools/mapgen/terrain.js stat     # 只出表，不改文件
node tools/mapgen/terrain.js build

# 重新生成桌面图标（窗口图标 raw RGBA + 快捷方式 .ico，同一段画图代码）
pwsh -File tools/make-icon.ps1

# 无头截图验收界面（每次新 profile + no-cache，避开"看到旧页面"）
node tools/shot.js --url http://127.0.0.1:8899/ --out tools/_shots/x.png --wait 1400

# iOS：出未签名 .ipa（要 Mac + 完整 Xcode；见 §15 与 docs/25）
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
tools/build-ios.sh                        # → dist/IronMind-ios-1.3.0.ipa
tools/build-ios.sh --simulator            # 模拟器里直接跑，不需要 Apple ID

# iOS 那一半的内核测试（在主机的 macOS 上跑，不需要 Xcode）
cargo test -p sim-ios
```

**桌面版的分工**（`docs/24`）：`crates/sim-desktop` 只是一个外壳——
挑空闲端口、拉起 `sim-server` 子进程、关窗杀进程、读 `settings.env` 当环境变量。
它不含任何战役逻辑，改界面/规则都不用碰它。

**端口**：桌面版用随机空闲端口（不跟开发时的 8899 抢）；开发模式默认 8899。8765 被本机的 clash 占着，别用。

**操作**：地图**只能看**。指挥只有一个入口——右侧「电报台」：选收件人、写一句人话、
发出，电报在路上要走 1 天以上，将领自己决定怎么执行（`docs/14`）。
收件人可以是兵团、也可以是任何一个军/师，还可以是"全体"（`docs/17`）。
下令也不必相邻：说"开赴万家岭"，部队就一天走一省地走过去。
点省份/单位只是查看；`空格` 推进一天，`Esc` 取消选中。

---

## 2 代码地图

```
crates/sim-core/src/          模拟核心。确定性是它的全部职责
├─ fixed.rs       Q16.16 定点数。模拟层禁用浮点
├─ rng.rs         SplitMix64 + 多流派生（每系统/每实体独立流）
├─ hash.rs        FNV-1a 状态哈希，黄金回放用
├─ world.rs       省份 / 单位 / 将领 / 情报 / 世界状态（最大的文件）
├─ systems.rs     补给 → 移动 → 战斗 → 士气 → 海空支援
├─ intel.rs       情报系统：真值 → 情报视图（含在途报告）
├─ ai.rs          规则 AI + 将领解释（抗命、自主主张）
├─ rules.rs       规则引擎：模块声明的条件+动作
└─ scenario.rs    内置测试场景（假数据）

crates/sim-content/src/       模块加载
├─ model.rs       数据契约（serde 结构，改动等于改契约）
├─ validate.rs    静态校验：邻接对称、补给源、连通性
└─ lib.rs         加载 → 校验 → 构建 World

crates/sim-server/            HTTP 服务 + 前端
├─ src/main.rs    手写 HTTP（无框架），路由见 docs/24 与文件内的 match 表
├─ src/http.rs    出站 HTTP（只有 LLM 用）：桌面/安卓 curl，iOS 由外壳装 URLSession
└─ web/index.html 单文件前端（地图 + 首页 + 引导 + 各面板）

crates/sim-ios/               iOS 壳的 Rust 半边（staticlib → libsim_ios.a，见 docs/25）
├─ src/lib.rs     4 个 C 函数：start / set_http_post / version / 错误缓冲区
├─ build.rs       内嵌战役模块（和安卓共用 sim-bundle）
└─ tests/kernel.rs 在 macOS 上**真起内核、真打 HTTP** 的 5 条测试

crates/sim-bundle/            外壳 build.rs 的公共部分：模块目录 → .imod（安卓与 iOS 共用）

crates/sim-desktop/           桌面外壳（原生窗口，见 docs/24）
└─ src/main.rs    挑空闲端口 → 拉起 sim-server 子进程 → WebView2 窗口 → 关窗杀进程

install.ps1                   装成桌面程序：拷贝产物/模块、建快捷方式、登记卸载

crates/sim-testbed/           无头跑批 CLI

tools/mapgen/gen.js           地图生成器（Delaunay 算邻接，作者工具，不参与模拟）
tools/shot.js                 无头截图 + 页内跑 JS（--eval-file 绕开 PowerShell 转义）
content/README.md              **模块开发手册**（字段契约、校验码、发布清单）

ios/                          Xcode 工程（只有壳，没有游戏逻辑，见 docs/25）
├─ project.yml               工程设置的源文件（XcodeGen 生成下面那个 .xcodeproj）
├─ IronMind.xcodeproj/       生成好并提交：装了 Xcode 就能直接打开
└─ IronMind/                 AppDelegate / SceneDelegate / GameViewController /
                             Kernel / HttpBridge + Info.plist + 图标

（脚手架 tools/module-init.js 和开发循环 imdev.mjs 在 SDK 仓库，不在本仓库——
 引擎出货的二进制不带开发行为，作者工具也不该混进玩家安装包。）

content/scenarios/
├─ demo_1805.imod/          3 省，骑兵传令（跨时代兼容的实证）
└─ siege_tianjiazhen.imod/  围城/断补给的最小可玩例

（南浔线 1938 90 省 / 18 个单位在隔壁模块仓库 F:\dsh\nanxun-1938，
 用 IRONMIND_MODULES 或 install.ps1 -Modules 指过去。）
```

**测试分布**（`cargo test --workspace`，2026-11 是 201 个）：`sim-core` 内联 + 黄金回放；`sim-content` 模块加载与校验；`sim-server` 的 15 条（导入、版本比较、存档名之类）；`sim-ios` 的 5 条（真起内核、端口回退、坏模块、C 传输钩子）+ 3 条内联；`sim-bundle` 1 条（模块目录真的能压成 zip）。改完引擎先跑一遍全套，别只看自己那个 crate。

---

## 3 文档地图

| 文件 | 内容 |
|---|---|
| `docs/00-方向方案与技术提纲.md` | 总纲：立项理由、设计原则、架构选型、里程碑、风险 |
| `docs/01-武汉会战场景设计.md` | 首作战役：五条战线、战斗序列、派系、MVP 裁剪 |
| `docs/02-视频精读-战斗记录.md` | 47 分钟战役视频逐帧精读（870 行） |
| `docs/03-史料核对-战斗序列.md` | 中方编制树到师级+师长、派系、时间线矛盾（734 行） |
| `docs/04-地图数据方案.md` | 四层地图管线 + **附录 B 前端 Voronoi 方案** |
| `docs/05-结局系统设计.md` | 12 种结局、状态变量、判定规则 |
| `docs/06-战役数据底稿.md` | 三源合并的兵力与时间线 |
| `docs/07-框架与模块架构.md` | 引擎/模块边界、模块格式、版本契约 |
| `docs/08-情报与单位类型.md` | 三层信息模型、侦察、将领独立思考、跨时代兼容 |
| `docs/09-规则引擎与扩展边界.md` | 模块自定义规则、「万物皆模块」的四层边界 |
| **[content/README.md](content/README.md)** | **模块开发手册（给作者看的唯一契约）**：四个 JSON 全字段、结局/规则/通信写法、33 条校验码、发布清单 |
| `docs/10-指挥与尺度.md` | 命令分两层、将领上报、省内阵地与尺度上限 |
| `docs/23-溃退与围歼.md` | **引擎侧：按方计位、AI 先打部队、压制冲击、溃退与围歼（伤亡不对称的来源）** |
| `docs/22-结局与整补.md` | **结局由模块声明（6 条）、推进到有事、整补补人** |
| `docs/21-路线图.md` | **还差什么：按依赖排好的 P0/P1/P2/P3 清单** |
| `docs/20-补给与断粮.md` | **补给线可被切断（敌纵队踩线 ×0.3）、断粮真减员、战报走邮路** |
| `docs/19-编制变动.md` | **覆灭是状态不是删除、将领殉国/被俘/去职/突围、二次整编要人也要后方** |
| `docs/18-空海军.md` | **空军/海军到底能干什么：侦察、集中空袭、出击损耗；舰炮支援与舰队移动** |
| `docs/17-行军与奔袭.md` | **命令可指向不相邻省（一天一省）、电报可发给任何军/师、三个踩过的坑** |
| `docs/16-真实地形.md` | **地形不再手写：DEM 采样与判定阈值、33 省改判、阵地级地形是下一步** |
| `docs/15-尺度与作战域.md` | **scale/domains 声明与校验、真实地理底图、三种尺度的模块** |
| `docs/14-指挥即通信.md` | **交互模型重写：地图只作查看，唯一入口是电报；电报生命周期与回电** |
| `docs/13-界面与公文.md` | **HOI4 布局对照、公文三类、请示过期、限流饿死坑** |
| `docs/12-将领大脑.md` | **LLM 将领：确定性边界、反谄媚提示词、异步与成本、配置** |
| `docs/11-架构对标与重构.md` | **对标 OpenRA/Freeciv/NationFall/Command Ops，v2 传输/命令/事件/渲染规格** |
| `docs/24-桌面版.md` | **怎么装成桌面程序**：外壳/内核分工、首页、引导、配置、已知限制 |
| `docs/25-iOS.md` | **怎么上 iPhone**：为什么不用 wry 的 iOS 后端、Swift 壳做哪四件事、iOS 特有的五个坑、出包与签名、三端对照 |
| `docs/10-指挥与尺度.md` | 命令分两层、将领上报、省内阵地与尺度上限 |

---

## 4 核心设计决策（都有理由，别轻易推翻）

**1. 引擎 + 模块。** 每个战役是可导入的数据包，作者填表就能做战役。判据很硬：**换一个战役时，动的是数据还是代码。** 只要引擎里出现 `if scenario == "wuhan"`，边界就画错了。

**2. 确定性优先。** 定点数、固定 tick、有序容器、多流 RNG、状态哈希。这是回放、分叉、平衡测试、AI 调试的地基。黄金哈希现在是 `0x6f5e56bc7ee1cf74`，行为改动会让测试失败——**失败本身是提醒，不是让你删测试**。

**3. 不判胜负，给结局。** 12 种互斥结局 + 历史基线对照，不做加权求和。因为史实上"丢了武汉但达成战略目的"无法用分数表达。（`docs/05`，尚未实现）

**4. 阵营是内容，引擎只认 A/B。** 玩家可以挑阵营（模块写 `playable_sides`），AI 接手另一边；
但**谁是国军、谁是日军、谁是美军，引擎一个字都不知道**——全部来自模块的
`sides: [{id, name, hq}]`。1.1 之前这里写死过"只做中方可玩 / `Some("cn") => Side::A`"，
那等于把一场战争钉进框架：换一场仗就得改代码。现在分层是硬的，
`crates/sim-core/tests/no_nation_names.rs` 会扫引擎源码，代码里出现具体国名/番号就测试失败。

**5. 将领做两级决策。** 统帅部给方向，将领做解释——可能打折扣、改目标、抗命。
**判定交给 LLM，机械边界留给引擎**：模型只产出"命令 + 理由"，合不合法、怎么结算、怎么回放都由引擎定。
反谄媚**靠提示词硬顶，不靠禁止**——把"你会倾向顺从提问者"直接写进提示词，
并要求他给出反对理由与照办代价（见 `docs/12`）。最初那版"绝不交给 LLM"是错的。

**6. 情报三层。** 真值 / 情报视图 / 将领认知。玩家和将领都看第二层，战斗结算才用第一层。**省份归属也是情报**——没侦察到的敌方省显示为未知。

**7. 通信延迟是三段。** `transmission`（电报是秒级的，为 0）+ `staff`（参谋汇总决策，1 天）+ `order_delay`。**瓶颈是参谋流程不是电报**。这是这一轮最重要的修正。

**8. 引擎不认年代。** 跨时代靠参数：`comms.preset = courier / wire_telegraph / radio`。引擎代码里没有任何 `if era == 1938`。已用 1805 模块实证。

**9. 规则引擎是"模块碰代码"的正确形态。** 模块只能写声明式规则（条件+原语+阶段钩子），不能写任意代码。因为游戏要能逐位重放，任意代码会毁掉确定性。

**10. 万物皆模块有四层边界。** 内核（tick/存储/RNG/哈希）**不可模块化**——它是保证，不是功能。系统层可以 trait 化但第一版不做。规则层和内容层才是模块的地盘。

---

## 5 已完成 / 未完成

### 已完成（能跑、有测试）

- 确定性模拟核心 + 黄金回放测试
- 补给（BFS + 补给延伸）/ 移动（加权 Dijkstra）/ 战斗 / 士气
- 将领性格四维 + 抗命 + 自主主张
- 情报系统（战斗接触/航空侦察/邻接观察，四档精度，时效衰减）
- 通信延迟（三段模型 + 在途报告）
- 海域省份 + 海陆分离通行 + 舰炮支援
- 空军侦察 + 对地打击
- 模块加载 + 静态校验（22 条错误码，6 类：元信息/地图/邻接/补给/战斗序列/版本）
- 规则引擎（7 条件 / 4 原语 / 6 阶段钩子）
- 前端：Voronoi 地图 + 四档地图模式 + 单位选中/下令 + 战报流
- 90 省战役地图（真实经纬度，Delaunay 邻接，水道画成带而不是色块）
- 作战姿态 `stance`：`attack` / `defend`，由模块声明，引擎只实现语义
- 堆叠上限 3 + "邻接有敌军不能占领"——战线终于停在接触面上
- 省内阵地层：Position（阵地名/地形/展开宽度/工事），战斗逐阵地结算并按宽度封顶
- 三条原地命令：Shift（省内调兵，不耗天数）、Fortify（筑工事，三日一级）、Rest（整补）
- Unit::manual：玩家下的命令 AI 不再覆盖
- 类型化事件 `Event{kind,text,tick}`：战斗、占领、抗命都有战报，前端按 kind 上色
- **命令流 + 回放校验**：`GET /api/replay-check` 用 `(模块, 种子, 命令流)` 重建世界并比对哈希
- 相机：滚轮缩放、拖拽平移、双击复位（SVG viewBox 当相机，命中测试免费）
- 操作日志：`logs/server.log` + `GET /api/log`（含每条命令与拒绝原因）
- 公文堆：请示（可准/驳，逾期自决）/ 抗命 / 通报；`GET /api/reports` + `POST /api/report`
- 侧栏按 HOI4 重排：电报台 / 公文堆 / 部队列表（outliner）/ 阵地 / 态势 / 战报
- **指挥=通信**：玩家不能直接调兵（`/api/order` 已关闭），只能发电报；电报按通信延迟送达，番号下每个将领各自解释并回电
- 模块可声明 **scale**（theater/battle/siege/naval）与 **domains**（陆/海/空），校验器据此把关
- 真实地理底图：Natural Earth 1:50m 水系列入 8.6 KB `basemap.json`，随服务端编译
- 三种尺度的模块：`nanxun_1938`(theater) / `demo_1805`(battle) / `siege_tianjiazhen`(siege)
- 服务端线程化：每条连接一个线程 + 10s 超时（之前单线程会被浏览器预连接卡死）
- 阵地内容：万家岭 = 一省三阵地（张古山 0.4 / 扁担山 0.6 / 哔叽岭 0.5），德安/瑞昌/南昌 也拆了
- **真实地形**：`terrain` 由 DEM 采样判定（143 块 terrarium 瓦片，z10 ≈130 米），
  33 省改判，`elev`/`relief` 一并进快照；「地形」视图 = 高程图 + 色标（`docs/16`）
- **无头截图验收工具** `tools/shot.js`：新 profile + 强制 no-cache，专治"看到旧页面"
- **跨省行军**：命令可以指向任何走得到的省，一天走一省（`docs/17`）；
  快照带 `march_hops`/`march_to`，outliner 显示"→万家岭 3日"
- **电报可发给任何军/师**（含空军与"全体"），不必只发给三个兵团；
  将领的提示词里多了"更远处的战场（要走几天）"
- **结局系统**（`docs/22`）：打完 77 天给一条结局（模块声明 6 条，指标+比较+优先级），
  结局面板带关键数字；「推进到有事 ⏎」跳过没事的日子；整补会补兵
- **补给线能切断、断粮会死人**（`docs/20`）：敌地面部队踩在补给线上通行力 ×0.3；
  补给 <0.15 日损 0.5% 兵力；战报走 `in_transit` 邮路（事件日期记发生日）；
  覆灭阈值从"不足 1 千"改为"不足 200 人"（老口径会把 800–1500 人的飞行团判死）
- **编制会变**（`docs/19`）：覆灭只标记不删除（下标即命令坐标）、将领有结局且只本方可见、
  模块可声明 `reform_days`/`reform_ratio` 做二次整编（要将领生还 + 后方补给源）
- **空海军不再是摆设**（`docs/18`）：空军每天集中空袭一个省（带战报、有出击损耗、
  能用命令指定目标）；舰队能沿水道移动，舰炮支援以「敌舰炮支援 ×1.15」显示在悬停卡片上
- **四个预览机制毕业成正式基线（1.0）**：弹药与油料、铁路被炸/抢修、派系与人事、
  连续正面模型（`front_line`）默认全开；想退回旧口径的模块在 `scenario.json` 里
  `"preview": {"x": false}`，界面挂"经典模式"牌子（`content/README.md` §5.5）
- **默认值跟着 `scale` 走**：`theater`/`naval` → 粗口径（`front_line` 关）；
  `front`/`battle`/`siege` → 连续正面模型开。`scale` 不只是标签，它决定语义
- **多阵营（1.1）**：模块写 `sides: [{id, name, hq}]` + `playable_sides`，首页就出现
  "作为第九战区 / 作为第11军"；你选哪边，AI 就用另一边的 `hq` 性格替你对面下决心。
  换阵营 = 重开一局（两边的命令流不能混着重放），存档记着当时是哪一方。
  **引擎内部只有 `Side::A/B`**，国名/番号一个字都不在引擎里
  （`crates/sim-core/tests/no_nation_names.rs` 扫源码守着这条线）
- **战报与结局指标也按内容命名**：引擎只写"A方攻占某地"，服务端按模块
  `sides[].name` 换成"第11军攻占 瑞昌"；结局指标同理（"第九战区伤亡（千）"）

### 未完成（按优先级）

**P0 · 地图感** —— ✅ 已完成
90 省、真实经纬度、Delaunay 邻接、水道成链、地图从"色块拼图"变成"读得出走廊"的地图。

**P1 · 前线识别** —— 做了一半
已加：作战姿态 `stance`（守方不再主动开拔）、堆叠上限 3、邻接有敌军时不能占领。
这三条合起来让战线真的停在接触面上（同一场景，国军从丢 47 省变成只丢 7 省）。
**还缺**：ZOC（进入敌军邻接省要付代价）与"接触面"识别——现在双方仍可能互相穿插。

**P2 · 战斗模型**
现在是单变量阈值系统：日军战斗力系数 1.7 → 全灭，2.5 → 碾压，中间几乎没有过渡。缺战场宽度、预备队投入、突破与合围。

**P3 · 结局系统**
`docs/05` 设计了 12 种结局，一行代码没写。需要状态变量记录 + 规则匹配 + 历史基线对照。

**P4 · 空海军半成品**
空军没有战损、没有制空权对抗；海军只有舰炮支援，没有登陆、封锁、运输。

**P5 · 将领局部视野**
将领现在能看到整方情报，应该按辖区裁剪。

**P6 · 编辑器**
`docs/07` 规划的编辑器/校验器/预览器都没做。现在做模块要手写 JSON。

---

## 6 已知的坑（都踩过，别再踩）

| 坑 | 表现 | 根因 |
|---|---|---|
| **敌境补给惩罚过狠** | 攻方永远推不动，改多少兵力系数都没用 | 敌境补给算成 `0.25×0.6=0.19`，战力直接剩两成。改为就地征发 0.5、不额外打折 |
| **士气归零死锁** | 战线僵死，双方都不动 | 士气 0 → 战力 0 → 战斗静默跳过。给士气加 0.15 地板 |
| **单位在敌省自由穿行** | 地图归属永远不变 | 只有全歼才改 owner。改为进入无人防守的敌省即占领 |
| **每 tick 重选目标导致震荡** | 单位在几个省之间来回横跳 | 加"目标承诺"：保留上一 tick 的目标直到失效 |
| **对向穿插** | 双方擦肩而过，永远打不着 | 检测位置互换，把一方拉到对方落点强制同省 |
| **`S.units` 未定义** | 点单位后整张地图空白，再点一下恢复 | 后端快照没有顶层 `units`，单位嵌在 provinces 下。`S.units.find()` 抛异常打断渲染。**已修**：加 `allUnits()` 收集 + render 加 try/catch |
| **在途报告被无限推后** | 情报永远送不到 | 每 tick 重新侦察都会生成更晚到达的新报告，覆盖了旧报告。改为"同一省只保留最早到达的那份" |
| **情报存了战斗力而非兵力** | "敌军估计 70 千"但实际只有 37 千 | 用了 `power_at` 而不是 `strength_at` |
| **地面单位把江面当目标** | 单位对着长江"进攻"，然后原地不动 | AI 选目标时没排除水域，移动阶段又被 `can_enter` 挡住。三处都要排：`cost_field`、两处 `weakest` 扫描 |
| **路过即占领** | 六个师团四十天把九十省全涂成自己的颜色 | 占领只看"省里有谁"，没看"旁边有谁"。改为邻接有敌军地面单位就不算拿下 |
| **部队挤成一个堆** | 一个山头七个师，别处一个兵都没有 | AI 的最短路一致，天然撞车。加 `STACK_LIMIT = 3` |
| **守方主动放弃阵地** | 兵力再多也守不住，因为守军全在开拔路上 | AI 只有"朝最近敌人走"一种逻辑。加 `stance: defend` |
| **AI 每 tick 覆盖玩家的命令** | 玩家下的命令下一个 tick 就没了，等于没在指挥。这也是"只有进攻一个选项"的真实原因：点了也白点 | `run_ai` 无条件给所有单位重新下令。加 `Unit::manual`：玩家下过命令的单位 AI 不许碰 |
| **省内调兵没生效** | 点「换阵地」后部队没动 | 同上——命令在 AI 那一步就被覆盖了，`run_movement` 根本看不到 |
| **敌将的心里话出现在我方战报** | 战报里能看到日军师团长抗命、非战斗减员 | 事件没有归属方。加 `Event.side`：`None`=客观事实，`Some(s)`=某方内部文件；服务端按视角过滤 |
| **进攻绕开守军占空山头** | 双方在同一省各占一个阵地、永不交战，省的颜色来回翻，看着在打仗其实谁也没碰到谁 | 攻方按"最空的阵地"选目标。改为**必须打到有敌军的那个阵地** |
| **整个服务端卡死（点什么都没反应）** | 浏览器里界面在，但点单位、点推进全无响应；`Invoke-RestMethod` 会一直挂着 | 手写 HTTP 是**单线程顺序处理**。浏览器会开"预连接"（连上但先不发请求），`read_line` 就在那条连接上死等，后面所有请求都排在它后面。改为**每条连接一个线程 + 读写超时 10 秒** |
| **没配 API key 时电报被静默丢弃** | 玩家发了 8 封电报、推了 39 天，部队一动不动；回电、请示、抗命一个都没有；日志里连"送达"都没有，界面上还写着"将领大脑：规则 AI"。看起来像将领不听令 | `Config.enabled = !off && (mock \|\| 有key)`，而 `deliver_telegrams` / `request_due` / `hq_due` 都以 `enabled` 为闸门**直接 return**。于是"没 key"= 指挥链整条不存在。改为 `enabled = !off`，没 key 时用本地规则判断（`mock_judgement`/`mock_hq`）顶上；真关只有 `IRONMIND_LLM=off` |
| **"继续"读到旧存档** | 明明刚玩过，首页"继续"却回到很早的一天，或者显示"还没有存档" | `/api/saves` 按**路径**排序、前端取第一条；`latest_save()` 又只认 `save-<时间戳>.json`，于是起过名字的存档和自动存档都不算"最新"。改为按**修改时间**排/取 |
| **自动存档一次都没写** | 推了 6 天，`saves/` 还是空的 | 自动存档的 `tick % 5 == 0` 判断放在 tick 循环**外面**，"一次推进 6 天"只看最后一天（6），永远不是 5 的倍数。挪进循环按天判 |
| **`approve` 被当成"驳回"** | 脚本发 `{id, approve:true}`，回执却是"已驳回…维持原令" | 接口实际只认 `{choice:"yes"\|"no"}`，`approve` 字段被忽略 → `unwrap_or(false)`。改为两种都认，**都没有就报错**——悄悄反驳比报错恶劣得多 |

**通用的教训**：每次改 AI 行为，先跑 `sim-testbed --ticks 60 --units --verbose` 看位置序列。**打时序，不要打快照**——快照会骗人（"位置=目标"可能只是移动刚完成的瞬间）。

---

## 7 怎么改内容（不改代码）

### 加一个省

`content/scenarios/nanxun_1938.imod/map.json`：

```json
{
  "id": 22, "name": "马当", "terrain": "hill",
  "owner": "cn", "fort": 3, "neighbors": [0, 20],
  "lon": 116.55, "lat": 29.95,
  "prototype": "river_ridge"
}
```

> 新加的省 `terrain` 可以随手写，但**跑一次 `node tools/mapgen/terrain.js build`
> 会用真实高程覆盖它**（并补上 `elev`/`relief`）。想要手写的地形不被覆盖，
> 就写成 `water` 或 `urban`——这两个 DEM 判不出来，工具会原样保留。

注意：**id 必须从 0 连续**；**邻接必须双向**（A 里有 B，B 里必须有 A）；`lon/lat` 决定它在地图上的位置。

### 加一条规则

`scenario.json` 的 `rules` 数组：

```json
{
  "id": "armor_bogged",
  "phase": "post_movement",
  "when": { "all": [ { "terrain": "marsh" }, { "unit_kind": "armor" } ] },
  "then": [ { "modify_strength_pct": -0.02 }, { "log": "装甲陷入沼泽" } ]
}
```

详见 `content/README.md`。

### 换一个时代

```json
"comms": { "preset": "courier" }
```

参考 `content/scenarios/demo_1805.imod`。

---

## 8 术语表

| 词 | 含义 |
|---|---|
| **效忠度 / obedience** | 将领服从上级命令的倾向，1.0 = 永不抗命 |
| **主动性 / initiative** | 将领自行改变目标的倾向 |
| **补给延伸 / supply_reach** | 把补给缺口补上多少。日军 0.45（有汽车铁路），国军 0.1（人力挑运） |
| **战斗力系数 / effectiveness** | 装备+训练+组织的综合。**是玩法参数，不是历史换算** |
| **情报精度** | 精确（战斗接触/航空）/ 近似（邻接观察 ±40%）/ 传闻 / 未知 |
| **在途报告** | 已侦察到但还没传回上级的情报 |
| **报告延迟** | `transmission + staff`，不是电报速度 |
| **地形原型** | 山脊线/背水口袋/围三缺一…… 见 `docs/04` 附录 A.2 |
| **黄金哈希** | 固定场景跑 200 tick 的末哈希，任何行为改动都会让它变 |

---

## 9 技术债

1. **`world.rs` 太大**（807 行）。省份/单位/将领/情报全在一个文件里，该拆。
2. **HTTP 是手写单线程的**。够用，但一个慢请求会阻塞全部。要并发就换 axum。
3. **前端无构建、无模块化**。单文件 611 行，再长就该拆了。（`ai.rs` 537 行已经很接近了）
4. ~~**`pick_main_target` / `attack_margin` 是死代码**~~（已删）
5. **`docs/00` 的里程碑已经过时**（写的是 12-14 个月的完整路线，实际走的是"垂直切片"路子）。
6. **校验器没检查 lon/lat**。不填坐标的模块会全部叠在左上角，应该报警。
7. **`tools/mapgen` 只服务南浔线**。它是硬编码清单的一次性工具，不是通用编辑器。

---

## 10 下一步建议

如果你想继续，我建议这个顺序：

1. **兵团 + 任务分解**（3-5 天）——现在玩家还是逐个师下命令；有了兵团，`obedience/initiative` 才对玩家生效
2. **将领上报通道**（3-5 天）——战况/请示/建议/抗命声明，走既有的通信延迟，请示会过期（`docs/10` §3）
3. **战斗解算加深**（1 周）——阵地已有宽度，但结算还是"一方战力对一方战力"。下一步是预备队投入、突破与合围
4. **地毯式阵地化**（内容工作）——现在只有 4 个省有阵地；`tools/mapgen` 的 `pos` 字段已经支持。
   顺带把 `positions[].terrain` 也按 DEM 采出来：现在阵地继承省地形，
   而"万家岭"在县尺度上是丘陵、在张古山这个山头上才是山地（`docs/16` §6）
5. **海战与制空权**——现在空海军只是支援兵种：舰队之间不会互相开火，
   空军之间不会互耗，行军纵队也不会被空袭（`docs/18` §4）
6. ~~**结局系统**~~ ✅ 已做（`docs/22`）：模块声明结局，引擎按指标判定

如果你想换个方向（比如先做编辑器、或先补海空军），现在拆掉任何一块的代价都不大，因为模块和引擎的边界是清楚的。

---

## 11 一句话给下一个人

**引擎负责"这类仗怎么打"，模块负责"这一仗在哪打"。**

只要新增一个战役不需要改引擎代码，架构就是对的。所有设计决策都是为了守住这一条。

---

## 12 P0–P3 收官（2026-11）

`docs/21-路线图.md` 里 P0/P1/P2 与 P3 的两项已经全部做完，每项都有测试与文档：

| 块 | 做了什么 | 文档 |
|---|---|---|
| P0 | 结局系统（模块声明 6 条）、整补补兵、推进到有事 | `docs/22` |
| P1 | 合围判定、地图行军线、兵团实体（兵团部影响服从度）、番号需两次接触 | `docs/19` `docs/20` `docs/08` |
| P2 | 海战、制空权、行军纵队被空袭、铁路与水路运力 | `docs/18` `docs/20` |
| P3 | 浏览器内切换模块、存档槽位、**模块更新（同名新版本直接覆盖）** | `docs/07` `docs/11` |

还剩的（都不在原目标里）：
- 模块编辑器（远期）
- 存档的**回放浏览**（现在只是读档，不能逐日看命令流）
- 派系与人事（`docs/03`）：结局里说不出"谁死了"，因为将领名单还没单开一张表
- 弹药/油料分离、就地征发上限、铁路被炸的运力骤降

**接手时先读 `docs/21-路线图.md`**：那是唯一一份"还差什么"的清单，
比任何口头交接都可靠。

---

## 14 安卓（1.2，2026-12）

**同一个内核，换一层壳。** `sim-server` 早就是"HTTP 端点 + 一个进程内服务"，
安卓只是在同一进程里调 `sim_server::start(module, 0)`，再让 WebView 指 `127.0.0.1:<port>`。

```
crates/sim-mobile/        安卓壳（cdylib → libsim_mobile.so，3.8 MB）
  build.rs                把选定的战役模块打成 zip 塞进 .so
  src/android.rs          tao 事件循环 + wry WebView + JNI 胶水
android/                  Gradle 工程（只有壳，没有游戏逻辑）
tools/build-android.ps1   NDK → .so → jniLibs → APK（一条命令）
```

### 踩过的坑（每条都花过一次时间）

| 现象 | 根因 |
|---|---|
| 手机上看到一张**缩小的桌面版**，媒体查询不生效 | `index.html` 缺 `<meta name="viewport">`。安卓 WebView 默认按 980px 虚拟宽度渲染 |
| `UnsatisfiedLinkError: No implementation found for Rust.onCreate` | `tao::android_binding!` 第三个参数是**声明 external fun 的那个类**。wry 0.57 把它们放在 `object Rust` 里，不是 `WryActivity` |
| 设置面板报 `写 /system/bin/settings.env 失败` | 安卓的 `current_exe()` 是 `/system/bin/app_process64`。所有要落盘的东西（settings/saves/logs/模块）必须走 `IRONMIND_DATA`（app 的 `filesDir`） |
| APK 装不上 / 更新被拒 | release 包未签名。已配一张自签钥匙（`android/app/iron-mind.jks`，口令 `ironmind`，**不是秘密**：它只用来证明"同一份 APK 的后续版本"） |
| Gradle 报 `Minimum supported Gradle version is 8.9` | 挑 Gradle 时按**版本号**排，不能按路径字符串：`gradle-8.2` 字符串比 `gradle-8.12.1` 大，但 8.2 < 8.9 |
| 白屏 | Android 9+ 默认禁明文 HTTP。清单里要 `usesCleartextTraffic="true"`（页面和内核对的是 `http://127.0.0.1`） |
| 转屏/切分屏后进度没了 | Activity 重建会重启内核。清单里那一长串 `configChanges` 就是为这个 |
| `.so` 61 MB | 编了 debug。release 带 LTO 只有 3.8 MB（差 16 倍） |

### 手机布局的三条约束

1. **不能缩微桌面**。地图铺满，侧栏变成从底部拉起的抽屉（`#app.sheet-open`）。
2. **触屏要重做输入**：单指平移、双指捏合缩放、双指双击复位。
   鼠标那套 `wheel`/`mousedown` 在手机上不触发，但两套共用同一份 `cam` 状态。
3. **安全区**：`100dvh` + `env(safe-area-inset-*)`，否则底部按钮被手势条盖住。

### 还没做的

- **模块导入**走的是 `<input type=file webkitdirectory>`。手机上要选文件得给 WebView
  实现 `onShowFileChooser`；目前换模块只能重新出包（`-Module` 指定，模块打在 .so 里）。
- 只出 `arm64-v8a`。32 位 arm 跑这种规模不现实。
- ~~iOS 没碰~~ → 1.3 做了，见 §15。结论是**不用** wry 的 iOS 后端：Swift 壳更省事。

## 15 iOS（1.3，2026-11）

**同一个内核，第三种壳，但这次换了一门语言写壳。** 安卓那边是 tao + wry 一套 Rust
到底；iOS 这边是 **Rust 管内核、Swift 管那台机器**（理由见 `docs/25` 第 1 节，
一句话：键盘顶页面、`confirm()` 弹窗、安全区、禁页面级缩放这四件事，用跨平台
窗口库做比用 UIKit 做更绕，而它们恰好是"能装但玩不了"的全部原因）。

```
crates/sim-ios/           iOS 壳的 Rust 半边（staticlib → libsim_ios.a，26 MB）
  src/lib.rs              4 个 C 函数：ironmind_start / set_http_post / version
  tests/kernel.rs         宿主上真起内核、真打 HTTP（不需要 Xcode）
crates/sim-bundle/        安卓与 iOS 共用的"模块目录 → .imod"打包逻辑
crates/sim-server/src/http.rs  出站 HTTP 换了传输层（默认仍是 curl）
ios/                      Xcode 工程：5 个 Swift 文件 + Info.plist + 图标
tools/build-ios.sh        一条命令出未签名 .ipa / 模拟器里跑
tools/build-ios-rust.sh   Xcode 的 preBuildScript 调它，cargo → target/ios/
tools/make-icon-ios.py    18 个尺寸的图标 + Contents.json
```

### 发动机（引擎）那侧只改了一处

`llm.rs` 里原来有两段一模一样的 `curl`（`ask` 和 `hq_chat`，作者当时刻意没合并）。
iOS 上必须合并了——不是洁癖，是因为**没有 curl**：iOS 既没有 `/usr/bin/curl`，
也禁止 App fork/exec。现在统一走 `http::post(url, key, body, timeout, tag)`：

| 平台 | 传输 |
|---|---|
| 桌面 / 安卓 | `curl -K`（行为、错误文案一字未变） |
| iOS | 外壳用 URLSession 实现的 C 函数（`sim-ios` 的 `ironmind_set_http_post`） |

同时把 `base` 末尾斜杠的容错收进一个 `chat_url()`（以前两处各拼一次，
填 `.../v1/` 就会得到 `//chat/completions`）。

### 踩过的坑（iOS 版，每条都不报错）

| 现象 | 根因 |
|---|---|
| 白屏 | ATS 默认只放行 https。`Info.plist` 要 `NSAllowsLocalNetworking = true` |
| 将领突然"变木"，日志一行"调用 curl 失败" | iOS 上没有 curl，也不许 fork/exec。得装 URLSession 传输 |
| 点「换战役 → 继续」没反应 | WKWebView 不实现 `WKUIDelegate`，`confirm()` **静默返回 false** |
| 键盘盖住电报台 / 发报按钮 | iPhone 键盘弹出时窗口不变，只是盖上去。要监听 `keyboardWillChangeFrame` 抬 WebView 底边（`100dvh` 会跟着重排） |
| 双指捏一下就"越捏越歪"、松手页面留在放大状态 | iOS 从 10 起忽略 `user-scalable=no`，页面级缩放和地图的 pinch 打架。用 `WKUserScript` 拦 `gesturestart/change/end` |
| 每次启动都被上手引导拦一遍 | 端口是随机的话 origin 就变，`localStorage` 换了一份。所以端口要**先试固定值 18731**，被占才顺延 |
| 内嵌模块打进 `.a` 后想改 | `IRONMIND_BUNDLE_MODULE` 指模块目录，重出包；或者网页里「导入 .imod」（WKWebView 的文件选择器能用） |
| 「导入文件夹」在 iOS 上没反应 | 它靠 `<input webkitdirectory>`，iOS 不支持（安卓支持）。发布形态本来就该是单个 `.imod` |
| pbxproj 里塞了几百行 shell | XcodeGen 的 `path:` 会把脚本**整段内联**进工程。改成 `script: bash "$SRCROOT/../tools/build-ios-rust.sh"` |
| 装完 Xcode 还是编不了：`iOS 18.5 is not installed` | Xcode 16 起 iOS 平台包是**单独下载**（`xcodebuild -downloadPlatform iOS`，约 8 GB）。SDK 在 Xcode.app 里，平台包不在 |
| `actool` 报 `No available simulator runtimes`（和图标毫无关系） | 同一个根因：平台包没装 |
| 所有工具都回 "You have not agreed to the Xcode license" | 许可协议没同意：`sudo xcodebuild -license accept && sudo xcodebuild -runFirstLaunch` |
| `ld: library 'ironmind_ios' not found` | crate 叫 `sim-ios`，产出的是 **`libsim_ios.a`**。工程里写成了 `-lironmind_ios`（名字对不上，报错也不会告诉你找过哪） |
| 模拟器里几百行 `found architecture 'arm64', required architecture 'x86_64'` | Intel Mac 的模拟器是 x86_64；`generic/platform=iOS Simulator` 还要**两种切片**。脚本现在按 `ARCHS` 逐个编 + `lipo` |
| 改完代码重新装，截图还是旧界面 | `simctl install` 只换 bundle，**不重启**已在跑的进程。先 `simctl terminate` |
| 「钢」字徽记被灵动岛压住 | 页面按 `env(safe-area-inset-*)` 写的，但 WKWebView 里它返回 **0**。改成 WebView 贴 `safeAreaLayoutGuide` |
| 横屏底栏比安卓高一截（长标签换两行） | 贴安全区之后横屏可用宽度只剩 ~756px，而底栏是**定宽**的（208 + 5×54），三个推进按钮挤到 60–75px 就换行。改成比例分配 + `nowrap` + 隐藏键盘符号（`docs/25` 4.10）。**安卓不受影响**：它锁横屏且 WebView 全宽，放得下 |

### 验到了什么程度

做这一版时手上只有 **Command Line Tools，没有 Xcode / iOS SDK**，所以：

**2026-11 装了 Xcode 16.4 之后全部验通了**（细节见 `docs/25` 第 8 节那张表）：
Swift 类型检查零错误、5 个文件 + 26 MB 静态库链成 arm64 可执行体、
`tools/build-ios.sh` 出 `dist/IronMind-ios-1.3.0.ipa`（895 KB）、`actool` 把图标编进
`Assets.car`、**在 iPhone 16 Pro 模拟器里真跑起来并截了图**（首页、首次设置、沙盒写盘、
内核 `127.0.0.1:18731` 从宿主机直通）。

这一轮改出来的四个 bug 都不是"编译不过"，而是"能装能开但不对"：
安全区（4.7）、模拟器 fat 库（4.8）、`simctl install` 不重启（4.9）、库名 `-lsim_ios`。

**还没验**：点进地图之后的界面、键盘顶起、双指缩放、转屏、`confirm()` 弹窗、真机。

### 还没做的

- **签名与分发**：出的是未签名 `.ipa`（签名要用你自己的 Apple ID，不该进仓库）。
  上 App Store 还有一道审核，这个项目本来也不打算走商店。
- 局域网 `http://` 的模型地址连不上（只开了 `NSAllowsLocalNetworking`）。
- 崩栈要连 Mac 看（沙盒里的 `logs/server.log` 能在「文件」App 里看到）。
- iPad 上的外接键盘快捷键（`空格`/`回车`）没验过，理论上直接能用。

---

## 16 macOS（1.5.1，2026-11）

**没有新壳。** `crates/sim-desktop` 一直是跨平台的（tao + wry），Windows 用 WebView2、
macOS 用 WKWebView；只是从来没人给它打过 macOS 的包。补的：

```
tools/build-mac.sh             装配 .app（两二进制 + modules + icns + Info.plist + ad-hoc 签名）
crates/sim-desktop/src/main.rs mac_data_root()：数据落 ~/Library/Application Support/IronMind
docs/24 §7.8                   写清为什么存档不能进 bundle
```

| 踩过的坑 | 原因 |
|---|---|
| 裸二进制启动后存档写不进 | Finder 启动时工作目录是 `/`。现在没有便携布局就显式设 `IRONMIND_DATA` |
| 不设 `NSAllowsLocalNetworking` 白屏 | ATS 只放行 https，页面走的是 `http://127.0.0.1` |
| 没有 bundle 就没有 Dock 图标 / 不能双击 | 必须打成 `.app`；顺带解决了激活问题 |
| 换 bundle 后文件面板返 nil | wry 的注释里写明：`NSOpenPanel` 在 bundle 被替换后拿不到 XPC 服务。**重打包前先退出 App** |
| 导入 .imod：选完文件点「打开」没反应 | `runModal` 在非激活窗口下会吃掉那一下。**双击文件可以**；或直接放进 modules/ 目录 |
| 同一份页面，三端三种「导入坏了」 | `accept=".imod"` 的三种死法：iOS 全灰、macOS 选完静默丢弃、老安卓看不出。**结论：不设 accept**，把关交给内核的加载器 |

---

## 13 1.0 与 1.1（2026-12）

| 版本 | 做了什么 |
|---|---|
| 1.0 | 四个预览机制并入主线并**默认全开**（弹药油料 / 铁路被炸 / 派系人事 / 连续正面）；默认值跟 `scale` 走；迟到番号 `arrivals` |
| 1.1 | **多阵营**：`sides[{id,name,hq}]` + `playable_sides`，玩家可以打任意一边，AI 接手另一边；阵营名、番号、战报称呼全部来自模块 |
| 1.1.1 | **模块更新**：同名模块按版本判断——新版本直接覆盖，同版本/降级/撞名先问一句；正在玩的那一局会被重开；顺手修了 `respond` 没列 409 的老问题 |

1.1 顺手清掉的一处结构性错误：引擎里曾经写着 `Some("cn") => Side::A`、
`match!(to, "全体" | "第九战区")`、`HQ_SYSTEM = "你是一支侵华日军的军司令官"`。
这些都是"把一场战争钉进框架"，换一个时代/一场战役就会说错话——
现在它们分别变成了 `side_by_id`、本方名字匹配、和一段不提任何国名的提示词。

**引擎不认年代，也不认阵营。** 这两条现在是同一件事，由测试守着。