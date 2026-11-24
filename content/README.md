# 如何做一个战役模块

模块就是一个目录，里面四个 JSON 文件。**不需要写代码**。

```
mymod.imod/
├─ module.json     元数据
├─ scenario.json   战役定义
├─ map.json        地图
└─ oob.json        部队
```

做好了把整个目录压成 zip，改后缀为 `.imod`，就是可分发的模块。

---

## 快速开始

```
1. 复制 content/scenarios/nanxun_1938.imod 为 mymod.imod
2. 改 module.json 里的 id 和 name（id 必须全局唯一）
3. 改 map.json 的省份、oob.json 的部队
4. 跑 cargo run -p sim-testbed -- --module mymod.imod 看能不能通过校验
```

---

## module.json

```json
{
  "id": "org.yourname.mymod",
  "name": "我的战役",
  "version": "0.1.0",
  "author": "你的名字",
  "description": "一句话说明",
  "engine": { "min": "0.1.0" },
  "content_schema": 1,
  "playable_side": "cn"
}
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `id` | 是 | 全局唯一，建议反向域名 |
| `engine.min` | 是 | 需要的最低引擎版本 |
| `engine.max` | 否 | 最高支持版本，超出会警告 |
| `content_schema` | 是 | 模块格式版本，当前是 1 |
| `playable_side` | 否 | 玩家操作的一方，`cn` 或 `jp`，默认 `cn` |

**注意版本**：引擎升级时不会静默破坏你的模块。不兼容会明确告诉你"需要 0.4.x，当前 0.6.0"。

---

## scenario.json

```json
{
  "id": "scen.mymod",
  "name": "我的战役",
  "start_date": "1938-07-26",
  "days": 77,
  "seed": 20250924
}
```

| 字段 | 说明 |
|---|---|
| `days` | 战役长度。目前只做展示，不影响模拟 |
| `seed` | 随机种子。填 0 或省略时由模块 id 派生，保证同模块同结果 |

---

## map.json

```json
{
  "bbox": [115.7, 28.6, 116.3, 29.9],
  "projection": "wgs84",
  "provinces": [
    {
      "id": 0,
      "name": "九江",
      "terrain": "plain",
      "owner": "jp",
      "supply_source": true,
      "fort": 0,
      "supply_throughput": 1.0,
      "neighbors": [1, 5],
      "victory_points": 3,
      "prototype": null,
      "note": "出处备注"
    }
  ]
}
```

### 省份字段

| 字段 | 必填 | 说明 |
|---|---|---|
| `id` | 是 | **必须从 0 连续**，引擎按索引访问 |
| `name` | 是 | 显示名 |
| `terrain` | 是 | `plain` / `hill` / `mountain` / `marsh` / `water` / `urban` |
| `owner` | 是 | `cn` / `jp`。当前必须填，不能留空 |
| `supply_source` | 否 | 补给源。每方**至少一个** |
| `fort` | 否 | 工事 0-5，越界报错 |
| `supply_throughput` | 否 | 0-1，该省传递补给的能力 |
| `neighbors` | 是 | 邻接省 id。**必须双向** |
| `victory_points` | 否 | 战役要点权重（目前只做标注） |
| `prototype` | 否 | 地形原型，见下 |
| `note` | 否 | 资料来源备注，强烈建议写 |

### 地形原型

从战场推演里提炼的十种要点类型，目前只做标注，将来会驱动战斗解算：

| 原型 | 含义 | 典型 |
|---|---|---|
| `ridge_line` | 山脊线，两侧不可迂回 | 富金山、小界岭 |
| `river_ridge` | 沿江狭长山脊，一线天 | 长山 |
| `pocket` | 背水口袋，三面水一面山 | 田家镇 |
| `isolated_height` | 孤立高地，两坡可上 | 张古山 |
| `village_node` | 村落要点 | 孔家祠 |
| `road_cut` | 公路截断点 | 松阳桥 |
| `city_gate` | 城垣单点突破 | 瑞昌东门 |
| `beachhead` | 湖岸滩头 | 姑塘 |
| `encircle_gap` | 围三缺一 | 万家岭 |
| `retreat_fan` | 多路撤退 | 小界岭转进 |

### 地形效果

| 地形 | 防御乘数 | 说明 |
|---|---|---|
| plain | 1.0 | |
| hill | 1.2 | |
| mountain | 1.4 | |
| marsh | 1.1 | |
| water | 1.6 | |
| urban | 1.3 | |

工事每级额外 +0.15。

---

## 邻接表怎么来（生成器）

`tools/mapgen/gen.js` 是一份引导工具：给它"地名 + 经纬度 + 地形 + 归属"的清单，
它用 **Delaunay 三角剖分**算出邻接，直接写出 `map.json`。

为什么要用 Delaunay：前端把省份画成多边形用的是 Voronoi（见 `docs/04` 附录 B），
而 **Voronoi 的邻接恰好就是 Delaunay 的边**。所以生成的邻接表和你在屏幕上看到的
"这两块挨着"天然一致。手写邻接表做不到这一点，而且很容易 A 写了 B、B 忘了 A，
触发 `map.asymmetric_neighbor`。

```bash
node tools/mapgen/gen.js --dry   # 只打印统计（省份数/边数/度数分布/警告）
node tools/mapgen/gen.js         # 写入 nanxun_1938.imod/map.json
```

生成之后 `map.json` 就是唯一真相。后续手改省份**不必再跑它**——跑一次会覆盖手改。

### 引擎对地图的两条硬约束

1. **一个省最多容纳 3 个地面单位**（`systems::STACK_LIMIT`）。
   省切得太少，部队会挤成一堆，地图上会出现"七个师在一个山头"。
2. **邻接有敌军时不能占领**（`systems::enemy_adjacent`）。
   所以"绕过守军去涂色"是不成立的。这也是为什么地图必须留出可以迂回的省份，
   而不是一条从九江到南昌的直线。

### 阵地（`positions`）

省是**作战**尺度（约 35 公里），阵地是**战术**尺度（2-5 公里）。
一个省可以不写 `positions`（引擎自动生成一个，省名 + 省地形 + 全宽度），
写了之后战斗、工事、调兵都在阵地粒度上进行。

```json
{
  "id": 18, "name": "万家岭", "terrain": "mountain",
  "positions": [
    { "name": "张古山", "terrain": "mountain", "width": 0.4 },
    { "name": "扁担山", "terrain": "mountain", "width": 0.6 },
    { "name": "哔叽岭", "terrain": "mountain", "width": 0.5 }
  ]
}
```

| 字段 | 说明 |
|---|---|
| `name` | 阵地名。会出现在单位牌、悬停卡片、战报里 |
| `terrain` | 阵地地形，不写就沿用省地形 |
| `width` | **展开正面 0.2-1.0**。每方战力先按 `width × 40 千人` 封顶，再乘地形与工事——多出来的兵只能当预备队。窄阵地 = 只能硬啃 |
| `fort` | 初始工事 0-5，之后由「筑工事」命令提升 |

什么时候该拆阵地：两个地名相距不到 10 公里，做成两个省会打不出"反复易手"，
做成一个省的两个阵地才有意义。**省的数量由行军速度定，阵地的有无由战场形态定。**

### 三条原地命令（玩家侧）

| 命令 | 作用 |
|---|---|
| 筑工事 | 原地挖，三日一级，工事记在当前**阵地**上；敌军进同省自动停 |
| 整补 | 原地恢复补给与士气 |
| 换阵地 | 省内把部队从 A 阵地挪到 B 阵地，**不耗天数**——这是预备队的用法 |

玩家的命令会锁住该单位（`manual`），AI 不再覆盖；推进/进攻到点后自动交还 AI。

## oob.json

```json
{
  "units": [
    {
      "id": "jp.106",
      "name": "第106师团",
      "side": "jp",
      "parent": "第11军",
      "strength": 16.0,
      "position": 0,
      "effectiveness": 2.5,
      "estimated": true,
      "note": "松浦淳六郎，史实约1.6万人"
    }
  ]
}
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `id` | 是 | 模块内唯一 |
| `name` | 是 | 显示名 |
| `side` | 是 | `cn` / `jp`，**双方都必须有单位** |
| `strength` | 是 | **千人**。`16.0` = 16000 人 |
| `position` | 是 | 所在省 id |
| `effectiveness` | 否 | 战斗力系数，默认 1.0。见下 |
| `supply_reach` | 否 | 补给延伸能力 0-1，默认 0（纯人力）。见下 |
| `commander` | 否 | 将领。留空则用完全服从的默认将领 |
| `estimated` | 否 | 是否推断值。`false` 表示有确切记载 |
| `parent` | 否 | 上级番号，只做展示 |
| `stance` | 否 | 作战姿态，默认 `attack`。见下 |

### 关于 stance（作战姿态）

只影响 AI 怎么下命令，不影响战斗数值。**引擎不认国家，只认姿态**——
"这一方该守"是史实知识与玩法设计，属于模块。

| 值 | 行为 |
|---|---|
| `attack`（默认） | 邻接有敌省就进攻；没有就朝最近的敌人推进 |
| `defend` | 守在原地不动，吃地形与工事加成。反攻交给玩家（或将领抗命） |

为什么要有这一条：AI 的默认逻辑是"朝最近的敌人走"。给防守方也套这套逻辑，
它会主动放弃阵地去迎敌——**兵力再多也守不住**，因为守军全在路上。
南浔线这个模块里，国军全部是 `defend`，日军全部是 `attack`。

```json
{ "id": "cn.74", "name": "第74军", "side": "cn", "stance": "defend", ... }
```

### 关于 supply_reach

表示"把补给不足的缺口补上多少"。机械化、汽车运输、铁路网越强，值越高。

| 部队 | 建议 |
|---|---|
| 日军（有汽车与铁路） | 0.40-0.50 |
| 国军中央军 | 0.14-0.18 |
| 国军地方部队 | 0.08-0.12 |

这是武汉会战最关键的后勤不对称。**别让双方共用一套衰减**——那等于抹掉了机械化部队对人力挑夫的优势。

### 关于 commander（将领）

这是"意图式指挥"的核心。上级（统帅部或玩家）给的是**意图**，将领做的是**解释**：

```json
"commander": {
  "name": "波田重一",
  "obedience": 0.70,
  "initiative": 0.70,
  "aggression": 0.85,
  "caution": 0.25
}
```

| 字段 | 含义 |
|---|---|
| `obedience` | 服从度。越低越可能抗命、打折扣 |
| `initiative` | 主动性。越高越可能自行改变目标 |
| `aggression` | 进攻倾向。高的人愿意强攻，低的人偏好稳守 |
| `caution` | 谨慎。高的人在劣势时倾向保存实力 |
| `title` | 职务（第106师团长）。只给 LLM 看 |
| `persona` | 人物志：出身、经历、用兵习惯。只给 LLM 看 |
| `agenda` | 他图什么、怕什么。只给 LLM 看——**这是让将领有私心的关键** |

`title` / `persona` / `agenda` 三段文本**不参与模拟**（不进 `state_hash`），
只在接 LLM 时进提示词。没有它们，模型只能按四个小数臆想一个军官；
有了它们，它才会自己想到"这是临时编成的师团，装备最差，不能当消耗品用"。

```json
"title": "第106师团长",
"persona": "熊本人，做过第6师团旅团长，熟悉山地战。性好冒险，常说“山地里人少反而好办事”。",
"agenda": "第106是新编师团，他急于一战立威，敢把部队往深山里带——这是他的贪，也是他的死穴。"
```

写法建议：`persona` 写"他是怎么打仗的人"，`agenda` 写"他为什么可能不听你的"。
两条都要具体到能拿来权衡，不要写成性格形容词的堆砌。

**三条硬约束**（保证不会离谱）：

1. 将领只能改方向，不能改阵营——永远不会投敌。
2. 抗命只会朝"更保守"或"更激进"偏，不会产生无意义动作。
3. 抗命概率受 `obedience` 硬约束，服从度 1.0 的将领永不抗命。

参考值：薛岳 / 波田重一这类攻击性将领，`aggression` 0.8+、`obedience` 0.6-0.7；张自忠这类执行型，`obedience` 0.9+；杂牌军将领 `obedience` 0.5 左右。

### 关于 effectiveness

这是最重要的平衡参数。它是**玩法参数，不是历史换算**。

建议起步值：

| 部队 | 建议 |
|---|---|
| 日军常备师团 | 2.3-2.8 |
| 日军支队 | 2.0-2.4 |
| 国军中央军 | 1.0-1.2 |
| 国军地方部队 | 0.6-0.8 |

警告：**这个参数有阈值效应**。调到某个点以下，进攻方一定全灭；调上去，进攻方碾压。中间几乎没有过渡。这是当前战斗模型太浅导致的，不要指望靠调参做出拉锯——那需要后续的指挥层。

---


## scale 与 domains（声明这是什么尺度的战役）

`module.json` 里声明，校验器据此决定严格程度。**引擎代码里不区分尺度**，只认省、阵地、单位。

```json
"scale": "siege",
"domains": ["land"]
```

| `scale` | 建议省数 | 额外要求 |
|---|---|---|
| `theater`（默认） | ≥12 | — |
| `battle` | 2-12 | 每省都要拆阵地 |
| `siege` | 1-6 | 每省都要拆阵地 |
| `naval` | ≥2 | 建议声明 `naval` 域 |

`domains`：`land` / `naval` / `air`。声明了 `naval` 却没有海军单位或没有水域省份 → **加载报错**；
声明了 `air` 却没有空军单位 → 警告。这样"模块说错了"在加载时就暴露，而不是玩到一半才发现。

三个现成的例子：`nanxun_1938`（theater，陆海空）、`demo_1805`（battle，陆）、
`siege_tianjiazhen`（siege，陆，要塞工事五级）。
## 校验规则

加载时会自动检查。**Error 级问题会拒绝加载**，Warning 只提示。

| 代号 | 级别 | 含义 |
|---|---|---|
| `map.id_out_of_range` | Error | 省份 id 必须 0..n-1 |
| `map.duplicate_id` | Error | id 重复 |
| `map.neighbor_out_of_range` | Error | 邻接指向不存在的省 |
| `map.asymmetric_neighbor` | Error | 邻接必须双向 |
| `map.self_loop` | Error | 省不能邻接自己 |
| `map.no_supply_source` | Error | 每方至少一个补给源 |
| `map.unowned_province` | Error | 每个省必须有 owner |
| `map.fort_range` | Error | fort 超出 0-5 |
| `map.throughput_range` | Error | supply_throughput 超出 0-1 |
| `map.owner_orphans` | Warning | 有己方省份从补给源不可达 |
| `oob.position_out_of_range` | Error | 单位位置不存在 |
| `oob.side_without_units` | Error | 某一方没有单位 |
| `oob.strength_nonpositive` | Error | 兵力必须为正 |
| `oob.unit_unreachable` | Warning | 单位开局即断补给 |
| `engine.too_old` | Error | 引擎版本不足 |
| `engine.too_new` | Warning | 引擎版本超出模块声明 |

---

## 数据从哪来

**别编。** 找不到就写 `"estimated": true` 并在 `note` 里说明推断依据。

参考 `docs/03-史料核对-战斗序列.md`：武汉会战的中方编制已经落到师级+师长姓名，两源交叉，冲突处并列。但**几乎所有师的实有兵力都没有记载**——这不是你没找到，是史料就缺。

---

## 常见错误

**省份 id 不连续**：写了 0,1,2,5。引擎按索引访问，必须 0..n-1 连续。

**邻接忘了一边**：A 里写了 B，B 里忘了 A。这是最常见的错误，校验器会明确指出来。

**兵力写成人数**：`"strength": 16000` 表示 1600 万人。单位是千人，写 `16`。

**没有补给源**：忘了给某一方标 `supply_source`，该方开局即断补给。

**双方实力悬殊且没有过渡**：见上面 effectiveness 的说明。

---

## scenario.json 加通信配置

不同时代的战役，命令传回来的速度完全不同。这是**跨时代兼容**的核心配置：

```json
{
  "id": "scen.mymod",
  "name": "我的战役",
  "start_date": "1805-10-21",
  "days": 40,
  "seed": 1,
  "comms": { "preset": "courier" }
}
```

| preset | 适用 | local | adjacent | air |
|---|---|---|---|---|
| `courier` | 前电报时代（骑兵传令） | 1 | 3 | — |
| `wire_telegraph` | 有线电报（默认） | 0 | 1 | 1 |
| `radio` | 无线电时代 | 0 | 1 | 0 |

也可以直接写数字覆盖：

```json
"comms": { "local_delay": 0, "adjacent_delay": 2, "air_delay": 3 }
```

**含义**：情报被发现后，要经过这么久才能进入你的视野。在此之前，你（和你的将领）什么都不知道。

**不要用年代名当配置。** 写延迟数字，不写 `"era": "ww2"`。引擎不认识年代，只认识天数。

---

## 侦察相关字段（单位）

| 字段 | 说明 |
|---|---|
| `kind` | `infantry` / `armor` / `air` / `naval` |
| `recon_radius` | 航空侦察半径（跳数）。仅 `air` 有效 |
| `strike_power` | 空中打击强度。仅 `air` 有效 |

```json
{
  "id": "jp.air3", "name": "第3飞行团", "side": "jp",
  "kind": "air", "strength": 1.5, "position": 0,
  "recon_radius": 2, "strike_power": 0.35
}
```

空军**不占省、不参与地面战斗**，只做侦察。所以不要给它填很大的兵力——它只是个代表单位规模的数字。

---

## 自定义规则（让模块有引擎没有的机制）

这是模块能力最强的一块：你可以在 `scenario.json` 里写规则，引擎会按固定时机执行。

**不需要写代码，不需要改引擎。**

### 基本结构

```json
{
  "rules": [
    {
      "id": "mountain_attrition",
      "phase": "post_movement",
      "priority": 0,
      "when": { "terrain": "mountain" },
      "then": [
        { "modify_strength_pct": -0.004 },
        { "log": "山地行军，部队出现非战斗减员" }
      ]
    }
  ]
}
```

这条规则的意思是：**每次移动结束后，凡是站在山地省的部队，损失 0.4% 兵力，并写一条战报。**

### 阶段（phase）

规则绑在哪个时间点执行。这个必须选对，否则行为不可预测。

| phase | 时机 |
|---|---|
| `pre_supply` | 补给计算之前 |
| `post_supply` | 补给计算之后 |
| `post_movement` | 移动之后、战斗之前 |
| `pre_combat` | 战斗解算之前 |
| `post_combat` | 战斗之后 |
| `post_tick` | 整个 tick 结束 |

### 条件（when）

| 写法 | 含义 |
|---|---|
| `{"always": true}` | 总是 |
| `{"terrain": "mountain"}` | 所在省是山地 |
| `{"unit_kind": "armor"}` | 单位是装甲 |
| `{"strength_below": 0.35}` | 兵力低于编制的 35% |
| `{"province_owner": "cn"}` | 所在省归中方 |
| `{"all": [...]}` | 全部满足 |
| `{"any": [...]}` | 任一满足 |
| `{"not": {...}}` | 取反 |

地形可选：`plain` / `hill` / `mountain` / `marsh` / `water` / `urban`
兵种可选：`infantry` / `armor` / `air` / `naval`
阵营可选：`cn` / `jp`

### 动作（then）

| 写法 | 含义 |
|---|---|
| `{"modify_strength_pct": -0.02}` | 按当前兵力百分比增减 |
| `{"modify_morale": 0.05}` | 士气增减（0-1） |
| `{"modify_supply": -0.1}` | 补给增减（0-1） |
| `{"log": "文本"}` | 写一条战报 |

### 三条约束

1. **规则不能有随机**。想随机只能靠引擎的确定性随机源——否则回放会崩。
2. **执行顺序固定**：先按 phase，再按 priority（小的先），最后按 id 字典序。**不依赖你写的顺序。**
3. **规则只能调用上面的动作**，不能直接改世界状态。

### 一个完整的例子

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
  },
  {
    "id": "last_stand_bonus",
    "phase": "pre_combat",
    "priority": 0,
    "when": { "all": [ { "strength_below": 0.3 }, { "province_owner": "cn" } ] },
    "then": [ { "modify_morale": 0.15 } ]
  }
]
```

第一条：装甲进沼泽，掉兵掉士气。
第二条：中方残部在自己地盘上，士气回升（"哀兵必胜"）。

**这两条机制引擎里都没有**，是模块自己定义的。
