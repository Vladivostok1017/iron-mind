# 模块开发手册

> 这是**内容的全部契约**。引擎只提供框架：地图、编制、将领、规则、结局全部由模块声明。
> 读完这份文档，你不需要写一行 Rust 就能做出一个战役。
>
> 字段名、默认值、校验码以 `crates/sim-content/src/model.rs` 和 `validate.rs` 为准；
> 两者不一致，以代码为准，并请报 bug。

---

## 0 三十秒版

一个模块 = 一个目录，四个 JSON。**不需要写代码、不需要编译**。

```
mymod.imod/              ← 目录名随便起，.imod 只是约定，不是压缩包
├─ module.json           元数据（这是什么战役、谁在打、敌方统帅是谁）
├─ scenario.json         战役定义（几天、种子、通信、规则、结局）
├─ map.json              省份、地形、邻接、补给源
├─ oob.json              部队：番号、兵力、位置、将领
└─ provinces_geo.json    （可选）真实省界多边形，没有就用抽象多边形
```

做完之后，在游戏里「新的战役 → 导入模块…」选这个目录就能玩；
作者自己调试用 `cargo run -p sim-testbed -- --module 路径`。

> **`.imod` 不是 zip。** 引擎读的是目录。旧文档写"压成 zip 改后缀"，那是错的，
> 压过之后就加载不了——目录名带 `.imod` 只是为了让它在文件管理器里像个整体。

---

## 1 最小可加载模块

四个文件都写全，就能通过校验。复制成新目录、改个 `id` 就是你的模块。

**module.json**

```json
{
  "id": "org.yourname.mymod",
  "name": "我的战役",
  "version": "0.1.0",
  "author": "你的名字",
  "description": "一句话说明。会显示在首页的战役卡上。",
  "engine": { "min": "0.10.0" },
  "content_schema": 1,
  "playable_side": "cn",
  "scale": "theater",
  "domains": ["land"]
}
```

**map.json**（三个省一条线）

```json
{
  "bbox": [115.9, 29.0, 116.3, 29.6],
  "projection": "wgs84",
  "provinces": [
    { "id": 0, "name": "甲", "terrain": "plain", "owner": "jp",
      "supply_source": true, "neighbors": [1], "lon": 116.0, "lat": 29.5 },
    { "id": 1, "name": "乙", "terrain": "hill", "owner": "cn",
      "neighbors": [0, 2], "lon": 116.1, "lat": 29.3 },
    { "id": 2, "name": "丙", "terrain": "urban", "owner": "cn",
      "supply_source": true, "neighbors": [1], "lon": 116.2, "lat": 29.1 }
  ]
}
```

**oob.json**

```json
{
  "units": [
    { "id": "jp.1", "name": "第1联队", "side": "jp", "strength": 3.0,
      "position": 0, "stance": "attack", "effectiveness": 1.4, "supply_reach": 0.45 },
    { "id": "cn.1", "name": "第1师", "side": "cn", "strength": 8.0,
      "position": 2, "stance": "defend", "effectiveness": 1.0, "supply_reach": 0.12 }
  ]
}
```

**scenario.json**

```json
{
  "id": "scen.mymod",
  "name": "我的战役",
  "start_date": "1938-07-26",
  "days": 30,
  "seed": 1,
  "comms": { "preset": "radio" }
}
```

验证：

```powershell
cargo run -q --release -p sim-testbed -- --module 路径\mymod.imod --ticks 30
```

跑通就说明数据合法。`--units` 看每 15 天的部队状态，`--trace 第101师团` 逐 tick
打某个单位的位置序列（"敌人怎么跑到我后方去了"这类问题只能看时序，不能看快照）。

### 开发循环：改完存盘就重载

做模块最耗人的不是写数据，是"改一行 → 手动导入 → 手动点一天 → 看结果"这个循环。
用开发模式就没有这个循环：

```powershell
pwsh -File tools/dev.ps1 -Module ..\nanxun-1938\nanxun_1938.imod
```

它会起内核并**盯着模块目录**（`module.json` / `scenario.json` / `map.json` /
`oob.json` / `provinces_geo.json`）：

- 你一存盘，内核就重载模块、回到第 0 日，控制台打一行
  `[重载] 南浔线 1938 · 87 省 · 18 单位 · 77 天`；
- 开着的游戏页面会在 3 秒内自己换地图（省界、模块列表一起刷），不用刷新；
- **存了一半的坏 JSON 不会毁掉现场**：加载失败只打印错误，继续用旧世界，
  改完再存一次就重载。

两条注意：重载会**清空命令流和电报**（旧命令在新数据上重放没有意义），
所以这是开发工具，不是"换剧本接着打"。要保住进度请用存档。

---

## 2 module.json

| 字段 | 必填 | 说明 |
|---|---|---|
| `id` | 是 | 全局唯一，反域名。**决定导入时的目录名**（取最后一段） |
| `name` | 是 | 显示名。首页战役卡、顶栏都用它 |
| `version` | 是 | 模块自己的版本 |
| `description` | 否 | 首页战役卡的正文。写清"这是哪一仗、你扮演谁" |
| `author` | 否 | 署名 |
| `engine.min` | 是 | 需要的最低引擎版本，如 `0.10.0` |
| `engine.max` | 否 | 最高支持版本。不填就一直不管 |
| `content_schema` | 否 | 模块格式版本，当前 `1`。高于引擎支持值会报 `meta.future_schema` |
| `playable_side` | 否 | 玩家操作的一方：`cn` / `jp`，默认 `cn`。**当前只支持单方可玩** |
| `scale` | 否 | `theater`（默认）/ `battle` / `siege` / `naval`。只影响校验严格程度 |
| `domains` | 否 | `["land","naval","air"]` 的子集。声明了却没有对应单位会报错/告警 |
| `hq` | 否 | **非玩家阵营的统帅部（AI 玩家）**，见下 |

### 什么是 `hq`

"敌方也有一个玩家"的实现：一个有目标、有性格的 LLM 指挥官，会按自己的方针
给自己的师团下命令——走的是**和玩家一样的通道**（电报 → 师团长判断 → 执行），
所以师团长照样可以不听他的。

```json
"hq": {
  "name": "岡村寧次",
  "title": "第11军司令官",
  "persona": "情报参谋出身……用兵讲章法，不爱为抢功而分兵；对后勤与气象极敏感。",
  "agenda": "他想打通南浔线，但更怕师团被各个击破——第106师团在万家岭陷进去过一次。",
  "objectives": [
    "沿铁路南下夺取德安，打开通往南昌的门户",
    "保护长江水路补给线",
    "避免单个师团孤军深入"
  ]
}
```

| 字段 | 说明 |
|---|---|
| `name` / `title` | 谁、什么职务。会出现在战报与指挥链里 |
| `persona` | 他是什么人、怎么打仗。决定他的判断风格 |
| `agenda` | 他要什么、怕什么。决定他会不会为抢功而冒险 |
| `objectives` | 战役目标，按优先级。每天开会时喂给他 |

不写 `hq`，敌方就只有师团长各自为战。**写它，敌军才会像一个整体在打仗。**

---

## 3 map.json

```json
{
  "bbox": [115.7, 28.6, 116.3, 29.9],
  "projection": "wgs84",
  "provinces": [ { "...": "见下" } ]
}
```

| 顶层字段 | 说明 |
|---|---|
| `bbox` | 西/南/东/北。用于地图自动取景，不写也能跑 |
| `projection` | 目前只支持 `wgs84`（等距圆柱投影），不写当 `wgs84` |

### 省份字段

| 字段 | 必填 | 说明 |
|---|---|---|
| `id` | 是 | **必须 0..n-1 连续**，引擎按索引访问 |
| `name` | 是 | 显示名。战报里点名靠它，也是将领认地名的唯一依据 |
| `terrain` | 是 | `plain` / `hill` / `mountain` / `marsh` / `water` / `urban` |
| `owner` | 是 | `cn` / `jp`，不能留空（`map.unowned_province`） |
| `neighbors` | 是 | 邻接省 id，**必须双向**，最多 12 个 |
| `supply_source` | 否 | 补给源。**每一方至少一个** |
| `rail` | 否 | 有铁路/公路干线，补给沿它传几乎不掉（×0.95），**地图上会画出来** |
| `supply_throughput` | 否 | 0-1，默认 1.0。这个省的转运能力，越小越拖后腿 |
| `fort` | 否 | 0-5 初始工事 |
| `victory_points` | 否 | 要点权重，目前只做标注 |
| `prototype` | 否 | 地形原型，目前只做标注（见下） |
| `positions` | 否 | 省内阵地，不写自动生成一个 |
| `lon` / `lat` | 否 | 经纬度。不写就摆不好地图（前端靠它定位与取景） |
| `elev` / `relief` | 否 | 平均海拔、相对高差（米）。**模拟不读**，只用来解释"为什么这里是山地" |
| `note` | 否 | 出处备注。值得写 |

### 地形与地形原型

| 地形 | 防御乘数 | 说明 |
|---|---|---|
| `plain` | 1.0 | |
| `hill` | 1.2 | |
| `mountain` | 1.4 | 行军慢、有非战斗减员 |
| `marsh` | 1.1 | |
| `water` | 1.6 | 只有海军能进 |
| `urban` | 1.3 | |

工事每级再 +0.15。

`prototype` 是从战场推演里提炼的要点类型，目前只做标注：
`ridge_line` / `river_ridge` / `pocket` / `isolated_height` / `village_node` /
`road_cut` / `city_gate` / `beachhead` / `encircle_gap` / `retreat_fan`。

### 阵地（positions）

省是**作战**尺度（约 35 公里），阵地是**战术**尺度（2-5 公里）。
不写 `positions` 就自动生成一个（省名 + 省地形 + 全宽），所以小模块不用管。

```json
{
  "id": 18, "name": "万家岭", "terrain": "mountain",
  "positions": [
    { "name": "张古山", "terrain": "mountain", "width": 0.4 },
    { "name": "扁担山", "width": 0.6 },
    { "name": "哔叽岭", "width": 0.5, "fort": 2 }
  ]
}
```

| 字段 | 说明 |
|---|---|
| `name` | 阵地名。出现在单位牌、悬停卡、战报里 |
| `terrain` | 不写就沿用省地形 |
| `width` | **展开正面 0.2-1.0**。每方战力先按 `width × 40 千人` 封顶，多出来的只能当预备队。窄阵地 = 只能硬啃 |
| `fort` | 初始工事 0-5，之后靠「筑工事」命令提升（三日一级） |

判断标准：**省的数量由行军速度定，阵地的有无由战场形态定。**
两个地名相距不到 10 公里，做成两个省会打不出"反复易手"，做成一个省的两个阵地才有意义。

### 地图的四条硬约束（都直接影响玩法）

1. **一个省每方最多 3 个地面单位**（`STACK_LIMIT`）。省太少 → 部队挤成一堆。
2. **邻接有敌军时不能占领**。所以"绕过守军去涂色"不成立。
3. **地面部队不能在空省里穿行深入敌后**：进入敌占省要求这一省挨着本方控制区
   （或者省里有敌军——那是进攻，不拦）。想深入就得一省一省占下来。
4. **补给要从补给源沿己方省份走得到**，否则那个省里的部队直接断粮。

→ 所以地图必须留出**可以迂回的省**，而不是一条从九江到南昌的直线。

---

## 4 oob.json

```json
{
  "units": [
    {
      "id": "jp.106",
      "name": "第106师团",
      "side": "jp",
      "parent": "第11军",
      "kind": "infantry",
      "strength": 16.0,
      "position": 0,
      "stance": "attack",
      "effectiveness": 1.5,
      "supply_reach": 0.45,
      "estimated": true,
      "reform_days": 20,
      "reform_ratio": 0.5,
      "commander": {
        "name": "松浦淳六郎",
        "title": "第106师团长",
        "obedience": 0.8,
        "initiative": 0.5,
        "aggression": 0.6,
        "caution": 0.55,
        "persona": "熊本人，做过第6师团旅团长，熟悉山地战……",
        "agenda": "第106是新编师团，他急于一战立威，敢把部队往深山里带。"
      },
      "note": "史实约 1.6 万人"
    }
  ]
}
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `id` | 是 | 模块内唯一 |
| `name` | 是 | 显示名。**玩家发电报就是点这个名字**，写番号不是人名 |
| `side` | 是 | `cn` / `jp`，**双方都必须有单位** |
| `strength` | 是 | **千人**。`16.0` = 一万六千人；写 `16000` 就是一千六百万 |
| `position` | 是 | 所在省 id |
| `kind` | 否 | `infantry`（默认）/ `armor` / `air` / `naval` |
| `stance` | 否 | `attack`（默认）/ `defend`，见下 |
| `effectiveness` | 否 | 战斗力系数，默认 1.0 |
| `supply_reach` | 否 | 补给延伸能力 0-1，默认 0 |
| `parent` | 否 | 上级番号，只做展示 |
| `estimated` | 否 | 是否推断值。`false` 表示有确切记载 |
| `reform_days` | 否 | 覆灭后多少天可在后方重新成军。**0 = 不可重建** |
| `reform_ratio` | 否 | 重建后补到满编的几成（0.5 = 五成） |
| `recon_radius` | 否 | 航空侦察半径（跳数）。仅 `air` |
| `strike_power` | 否 | 空中打击强度。仅 `air`，0 表示只侦察 |
| `commander` | 否 | 将领。不写就是完全服从的默认将领 |
| `note` | 否 | 出处备注 |

### stance：为什么防守方不会自己跑掉

AI 默认"邻接有敌就进攻，没有就朝最近敌人推进"。给防守方也套这套逻辑，它会
**主动放弃阵地去迎敌**——兵力再多也守不住，因为守军全在路上。

| 值 | 行为 |
|---|---|
| `attack`（默认） | 邻接有敌省就进攻；没有就朝最近的敌人推进。适合进攻方 |
| `defend` | 守在原地、吃地形与工事加成；反攻交给玩家（或将领抗命） |

**引擎不认国家，只认姿态。**"这一方该守"是史实知识与玩法设计，属于模块。

### strength 的口径（最容易写错的一处）

这条踩过一个大坑：最初把国军的军写成 8–15 千，比日军一个师团还少，于是
"中国兵多、装备差"这个基本事实在数据里是**反的**——国军比日军还少，怎么打都输。

史实近似值（推演用）：

| 单位 | 实有兵力 |
|---|---|
| 国军一个师 | 约 7–9 千（1938 年实编，缺额严重） |
| 国军一个军（2–3 师） | 约 15–27 千 |
| 日军一个常备师团 | 约 20–25 千 |
| 日军一个支队/旅团 | 约 4–6 千 |

所以**国军总兵力应当明显多于日军**（南浔线模块：159.8 千 vs 96.5 千 = 1.66 倍），
代差靠 `effectiveness` 拉开。校验器在"可玩方兵力少于敌方"时报
`balance.lopsided`——那不是引擎的平衡表，是给作者的体检提醒。

### effectiveness 与 supply_reach：两把不同的刀

| `effectiveness` | 建议 |
|---|---|
| 日军常备师团 | 1.4–1.7 |
| 日军支队 | 1.3–1.5 |
| 国军中央军 | 0.9–1.15 |
| 国军地方部队 | 0.6–0.8 |

| `supply_reach` | 建议 |
|---|---|
| 日军（汽车 + 铁路 + 水路） | 0.45–0.50 |
| 国军中央军（人力挑运） | 0.14–0.18 |
| 国军地方部队 | 0.08–0.12 |

`effectiveness` 是装备与训练的代差；`supply_reach` 是"能把补给缺口补上多少"。
**别让双方共用一套补给衰减**——那等于抹掉了机械化部队对人力挑夫的优势。

⚠️ `effectiveness` 有阈值效应：调到某个点以下进攻方全灭，调上去就碾压，中间过渡很窄。
这是战斗模型还浅导致的，别指望靠调参做出拉锯。

### 将领：四个数字 + 三段文本

| 字段 | 含义 |
|---|---|
| `obedience` | 服从度。越低越可能抗命、打折扣 |
| `initiative` | 主动性。越高越可能自行改变目标 |
| `aggression` | 进攻倾向。高的人愿意强攻，低的人偏好稳守 |
| `caution` | 谨慎。高的人劣势时倾向保存实力 |
| `title` | 职务。只给 LLM 看 |
| `persona` | 人物志：出身、经历、用兵习惯。只给 LLM 看 |
| `agenda` | 他图什么、怕什么。只给 LLM 看——**这是让将领有私心的关键** |

四个数字参与模拟（进 `state_hash`）；三段文本**不参与模拟**，只在接模型时进提示词。
没有它们，模型只能按四个小数臆想一个军官；有了它们，它才会自己想到
"这是临时编成的师团，装备最差，不能当消耗品用"。

写法建议：`persona` 写"他是怎么打仗的人"，`agenda` 写"他为什么可能不听你的"。
两条都要具体到能拿来权衡，别写成性格形容词的堆砌。

三条硬约束：将领只能改方向、不会投敌；抗命只朝"更保守"或"更激进"偏；
`obedience = 1.0` 的将领永不抗命。

### 二次整编

部队覆灭（兵力打到 200 人以下，或战斗中被全歼）不会从编制簿上消失：

- `reform_days > 0` → 过这么多天在本方**补给源**重新成军，兵力补到 `reform_ratio`；
- `reform_days = 0` → 这个番号就没了（史实上有些部队打光就再没有了）。

将领会殉国/被俘（引擎掷骰），殉国的将领跟着番号一起进编制簿。

---

## 5 scenario.json

| 字段 | 说明 |
|---|---|
| `id` | 场景 id |
| `name` | 场景名 |
| `start_date` | 起始日期，只做展示 |
| `days` | 战役长度。**到这一天战役结束并判定结局**，不是"只做展示" |
| `seed` | 随机种子。省略或 0 时由模块 id 派生，保证同模块同结果 |
| `comms` | 通信条件，见 §8 |
| `rules` | 自定义规则，见 §7 |
| `endings` | 结局，见 §6。**不写就只有"战役期满"一个默认结局** |

---

## 6 结局（endings）

引擎不认识"天炉""苦撑"这些词。它只认三件事：**指标、比较、优先级**。
所以结局完全由内容决定：同一套引擎可以描述 1938 年的南浔线，也可以描述别的战役。

```json
"endings": [
  {
    "id": "hold_the_line",
    "name": "南浔线未破",
    "text": "日军未能打通南浔线，德安仍在手里……",
    "priority": 10,
    "at_end_only": true,
    "require": [
      { "metric": "provinces", "side": "cn", "cmp": "ge", "value": 50 },
      { "metric": "holds_province", "side": "cn", "province": "德安" }
    ]
  },
  {
    "id": "line_collapsed",
    "name": "南浔线失守",
    "text": "德安易手，南昌门户洞开……",
    "priority": 5,
    "at_end_only": false,
    "require": [
      { "metric": "holds_province", "side": "jp", "province": "德安" }
    ]
  }
]
```

| 字段 | 说明 |
|---|---|
| `id` / `name` / `text` | 结局标题与正文。正文是给玩家看的**历史评价**，不是比分 |
| `priority` | 越小越先判。多条同时命中时取优先级最高的一条 |
| `at_end_only` | `true`（默认）只在战役期满时判；`false` 每天都可以提前触发（崩盘、速溃） |
| `require` | 全部满足才算命中。**留空 = 永远命中**，会被跳过 |

### 判定子句（Clause）

```json
{ "metric": "provinces", "side": "cn", "cmp": "ge", "value": 50 }
```

| 字段 | 说明 |
|---|---|
| `metric` | 见下表 |
| `side` | `cn` / `jp`。`holds_province` 必须写 |
| `cmp` | `ge`（默认）/ `gt` / `le` / `lt` |
| `value` | 阈值 |
| `province` | 仅 `holds_province` 用：**省名，必须和 map.json 里的 `name` 完全一致** |

| metric | 含义 |
|---|---|
| `day` | 当前天数 |
| `provinces` | 控制省数 |
| `strength` | 总兵力（千） |
| `losses` | 累计损失（千） |
| `units_alive` | 在编单位数 |
| `commanders_lost` | 殉国/被俘的将领数 |
| `holds_province` | 是否控制某省（1/0），配合 `province` |

**写结局的建议**：不要写"你赢了"。写 5–12 条不同走向，让同一个局面在不同判断下
落到不同的历史上。这是这个引擎和别的战争游戏最大的不同。

---

## 7 自定义规则（rules）

模块能力最强的一块：在 `scenario.json` 里声明规则，引擎按固定时机执行。
**不需要写代码，不需要改引擎。**

```json
"rules": [
  {
    "id": "armor_bogged_in_marsh",
    "phase": "post_movement",
    "priority": 10,
    "when": { "all": [ { "terrain": "marsh" }, { "unit_kind": "armor" } ] },
    "then": [
      { "modify_strength_pct": -0.02 },
      { "modify_morale": -0.05 },
      { "log": "装甲部队陷入沼泽，损失惨重" }
    ]
  }
]
```

### 阶段（phase）

| phase | 时机 |
|---|---|
| `pre_supply` | 补给计算之前 |
| `post_supply` | 补给计算之后、整编之前 |
| `post_movement` | 移动之后、战斗之前 |
| `pre_combat` | 战斗解算之前 |
| `post_combat` | 战斗之后 |
| `post_tick` | 整个 tick 结束 |

### 条件（when）

| 写法 | 含义 |
|---|---|
| `{"always": true}` | 总是 |
| `{"never": true}` | 永不（调试用） |
| `{"terrain": "mountain"}` | 所在省是山地 |
| `{"unit_kind": "armor"}` | 单位是装甲 |
| `{"strength_below": 0.35}` | 兵力低于编制的 35% |
| `{"province_owner": "cn"}` | 所在省归中方 |
| `{"all": [...]}` | 全部满足 |
| `{"any": [...]}` | 任一满足 |
| `{"not": {...}}` | 取反 |

地形：`plain` / `hill` / `mountain` / `marsh` / `water` / `urban`
兵种：`infantry` / `armor` / `air` / `naval`
阵营：`cn` / `jp`

### 动作（then）

| 写法 | 含义 |
|---|---|
| `{"modify_strength_pct": -0.02}` | 按当前兵力百分比增减 |
| `{"modify_morale": 0.05}` | 士气增减（0-1） |
| `{"modify_supply": -0.1}` | 补给增减（0-1） |
| `{"log": "文本"}` | 写一条战报 |

### 三条约束

1. **规则不能有随机**。要随机只能用引擎的确定性随机源——否则回放会崩。
2. **执行顺序固定**：先 phase，再 priority（小的先），最后 id 字典序。**不依赖你写的顺序。**
3. **规则只能调用上面的动作**，不能直接改世界状态。

---

## 8 通信（comms）

引擎只认**延迟天数**，不认年代。不同时代的模块配不同数值——这是跨时代兼容的核心。

```json
"comms": { "preset": "radio" }
```

| preset | 适用 | `transmission` | `staff` | `order_delay` |
|---|---|---|---|---|
| `courier` | 前电报时代（骑兵传令） | 2 | 1 | 1 |
| `wire_telegraph` | 有线电报（1930 年代） | 0 | 1 | 0 |
| `radio` | 无线电时代 | 0 | 1 | 0 |

不用预设时直接写三个数字：

```json
"comms": { "transmission": 0, "staff": 2, "order_delay": 1 }
```

| 字段 | 含义 |
|---|---|
| `transmission` | 技术传输延迟（天）。电报/无线电是 0，骑兵 2–3 |
| `staff` | 参谋汇总、核实与决策延迟。**这才是 1938 年的真正瓶颈** |
| `order_delay` | 命令逐级下达的额外延迟 |

两条派生量：

- **战报延迟** = `transmission + staff`（情报多久进入你的视野）；
- **命令延迟** = `transmission + order_delay`（你的电报多久到达执行单位）。

> ⚠️ **只有这三个键名有效。** `local_delay` / `adjacent_delay` / `air_delay` 这类
> 按"远近"分档的写法**不存在**——写了会被静默忽略，你拿到的还是默认值。
> （这条踩过：旧版本的手册就是这么写的，照着抄的人以为自己在改延迟。）

**不要用年代名当配置**，只写天数。引擎不认识年代。

---

## 9 provinces_geo.json（可选：真实省界）

```json
{
  "source": "DataV 行政区划（县界）",
  "simplify": 0.008,
  "polygons": {
    "0":  [[[116.0829, 29.7612], [116.0725, 29.7607]]],
    "18": [[["...每一环都是 [lon, lat] 数组"]]]
  }
}
```

- 键是**省份 id 的字符串**，值是若干条环（外环 + 空洞），每条环是 `[lon, lat]` 数组；
- 有它就用真形状，没有前端退回抽象多边形（所以小模块不用做这一步）；
- 生成脚本：`tools/mapgen/boundaries.js`（南浔线用 DataV 县界，87 个省 345 KB）。

体积参考：**每省约 4 KB**。100 个省 ≈ 400 KB，可以接受；别塞到几 MB。

---

## 10 校验：加载时会挡下什么

`Error` 级拒绝加载（游戏里表现为"加载失败"），`Warning` 只在控制台提示。

| 代号 | 级别 | 含义 / 怎么改 |
|---|---|---|
| `meta.empty_id` | Error | `module.json` 的 `id` 不能为空 |
| `meta.empty_engine_min` | Error | `engine.min` 不能为空 |
| `meta.future_schema` | Error | `content_schema` 高于引擎支持值 |
| `map.empty` | Error | 地图没有省份 |
| `map.empty_name` | Error | 省份没写 `name` |
| `map.duplicate_id` | Error | 省 id 重复 |
| `map.id_out_of_range` | Error | 省 id 必须 0..n-1 连续 |
| `map.fort_range` | Error | `fort` 超出 0-5 |
| `map.throughput_range` | Error | `supply_throughput` 超出 0-1 |
| `map.neighbor_out_of_range` | Error | 邻接指向不存在的省 |
| `map.self_loop` | Error | 省不能邻接自己 |
| `map.asymmetric_neighbor` | Error | **邻接必须双向**（A 写了 B，B 必须写 A） |
| `map.too_many_neighbors` | Error | 一个省邻接超过 12 个 |
| `map.no_supply_source` | Error | 某一方没有任何补给源 |
| `map.unowned_province` | Error | 有省没写 `owner` |
| `map.owner_orphans` | Warning | 有己方省份从补给源走不到（那里的部队会断粮） |
| `oob.empty` | Error | `oob.json` 没有单位 |
| `oob.duplicate_id` | Error | 单位 id 重复 |
| `oob.strength_nonpositive` | Error | `strength` 必须 > 0 |
| `oob.strength_extreme` | Warning | 兵力异常大/小，多半写错单位（千人 vs 人） |
| `oob.position_out_of_range` | Error | `position` 指向不存在的省 |
| `oob.starts_in_enemy_land` | Warning | 开局就站在敌方领土里，多半位置写错 |
| `oob.side_without_units` | Error | 某一方没有任何单位 |
| `oob.unit_unreachable` | Warning | 某个单位开局即断补给 |
| `balance.lopsided` | Warning | 可玩方兵力明显少于敌方 |
| `scale.too_few_provinces` | Warning | `battle`/`siege` 的省数太少 |
| `scale.too_many_provinces` | Warning | `siege` 的省数太多 |
| `scale.missing_positions` | Warning | 小尺度战役却没拆阵地 |
| `domain.land_without_units` | Warning | 声明陆地域却没有地面单位 |
| `domain.naval_without_units` | Error | 声明海军域却没有海军单位 |
| `domain.naval_without_water` | Error | 声明海军域却没有任何水域省 |
| `domain.air_without_units` | Warning | 声明空军域却没有空军单位 |
| `engine.too_old` | Error | 引擎版本低于 `engine.min` |
| `engine.too_new` | Warning | 引擎版本高于 `engine.max` |

---

## 11 平衡：先跑一局，别只看数字

```powershell
# 每 15 天打一次双方兵力与省数
cargo run -q --release -p sim-testbed -- --module 路径\mymod.imod --ticks 77 --verbose
```

看三件事：

1. **战线动没动**。77 天里地图颜色几乎不变 → AI 卡住了，多半是地形/邻接/姿态写错；
2. **双方还活着没有**。一边归零说明 `effectiveness` 或兵力差得太远；
3. **进攻方推得动但没有碾压**。兵力曲线应该缓慢下滑，不是断崖。

再在游戏里玩一局（`新的战役 → 导入模块…`），确认：地图能读、番号能认、
电报有回应、结局能触发。

---

## 12 发布前的清单

- [ ] `id` 全局唯一，`engine.min` 填了
- [ ] 每方至少一个 `supply_source`，且从补给源能走到本方所有省
- [ ] 邻接双向、id 从 0 连续
- [ ] 每个省有 `lon` / `lat`（否则地图摆不好）
- [ ] `strength` 单位是千人
- [ ] 双方都有单位，且可玩方兵力 ≥ 敌方
- [ ] 写了 `endings`（至少 3 条不同走向），且都有 `require`
- [ ] `sim-testbed --ticks 天数 --verbose` 跑通，战线在动
- [ ] 游戏里导入一次，能开局、能推进、能出结局
- [ ] 写清"哪些数字是史实、哪些是推断"（用 `note` / 模块文档）

---

## 13 数据从哪来

**别编。** 找不到就写 `"estimated": true` 并在 `note` 里说明推断依据。

参考模块仓库里 `docs/03-史料核对-战斗序列.md`：中方编制已经落到师级 + 师长姓名，
两源交叉，冲突处并列。但**几乎所有师的实有兵力都没有记载**——这不是你没找到，
是史料就缺。把缺口标出来，比编一个漂亮数字有用。

地图数据（县界、高程、水系）的完整管线见 `docs/04-地图数据方案.md` 与
`docs/16-真实地形.md`；`tools/mapgen/` 下的脚本可以直接用。

---

## 14 常见错误速查

| 现象 | 原因 |
|---|---|
| 加载失败，说 `map.asymmetric_neighbor` | 邻接忘了一边。用 `gen.js` 生成就不会犯 |
| 部队全挤在一个省 | 省太少，撞 `STACK_LIMIT`（每方 3 个） |
| 地图上什么也没画 | 省缺 `lon`/`lat` |
| 兵力写成 16000 | 单位是千人，写 `16` |
| 开局就全军断粮 | 没给本方标 `supply_source`，或补给源与本方省份不连通 |
| 敌人突然出现在后方 | 那一带有空省走廊。要么补省，要么留拒止部队——这是玩法问题不是 bug |
| `comms` 里写的延迟没生效 | 键名写错了（只有 `transmission` / `staff` / `order_delay`） |
| 结局永远不触发 | `require` 里的省名写错（必须和 `map.json` 的 `name` 完全一致） |
| 压成 zip 改后缀 `.imod` 后加载不了 | **`.imod` 是目录**，引擎不读压缩包 |
| 将领从不抗命 | `obedience` 全是 1.0，或没写 `persona` / `agenda`（模型没有抓手） |
