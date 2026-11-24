//! 模块数据模型。
//!
//! 这是**引擎与内容之间的契约**。改动这里的字段语义等于改契约，
//! 必须遵守"只增不改"，并带上迁移器。
//!
//! 关键设计：**输入用浮点，模拟用定点。**
//! 作者写 `"supply_throughput": 0.75` 很自然，但模拟核心只吃 `Fixed`。
//! 转换在加载时一次性完成，之后全程定点，确定性不受影响。

use serde::{Deserialize, Serialize};

pub const CURRENT_CONTENT_SCHEMA: u32 = 1;

fn default_one_f64() -> f64 {
    1.0
}
fn default_schema() -> u32 {
    CURRENT_CONTENT_SCHEMA
}

/// 参战方。模块里用小写字串（"cn" / "jp"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SideDef {
    Cn,
    Jp,
}

impl SideDef {
    pub fn as_str(self) -> &'static str {
        match self {
            SideDef::Cn => "cn",
            SideDef::Jp => "jp",
        }
    }
}

/// `module.json`
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct ModuleMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    pub engine: EngineReq,
    #[serde(default = "default_schema")]
    pub content_schema: u32,
    /// 玩家可操作的一方。当前引擎只支持单方可玩。
    #[serde(default = "default_playable")]
    pub playable_side: SideDef,
    /// 非玩家阵营的统帅部（AI 玩家）。
    ///
    /// 这是**敌方那一局的总指挥**：他有自己的战役目标、自己的性格，
    /// 会对自己的师团下达具体命令（走的是和玩家一样的"电报 → 将领判断"通道）。
    /// 内容层决定他是谁、想要什么；引擎不认识人名。
    #[serde(default)]
    pub hq: Option<HqDef>,
    /// 战役尺度。决定"这张图是什么"，也决定校验规则。
    ///
    /// - `theater` 战役级：几十到上百个省，师级单位，数天一级的时间颗粒
    /// - `battle` 战斗级：几个省，每个省拆阵地，逐点争夺
    /// - `siege` 攻坚：一到两个省，工事与突破口是主角
    /// - `naval` 海战/江防：水域省份占大头，舰炮与登陆
    ///
    /// 引擎代码里**不区分**这些尺度（它只认省、阵地、单位），
    /// 尺度只影响校验的严格程度与默认值的取舍。
    #[serde(default)]
    pub scale: ScaleDef,
    /// 本战役涉及哪些作战域。声明了却没单位、或有单位却没声明，都会告警。
    #[serde(default)]
    pub domains: Vec<DomainDef>,
}

fn default_playable() -> SideDef {
    SideDef::Cn
}

/// 战役尺度。见 `ModuleMeta::scale`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScaleDef {
    /// 战役级（默认）
    #[default]
    Theater,
    /// 战斗级
    Battle,
    /// 攻坚
    Siege,
    /// 海战/江防
    Naval,
}

impl ScaleDef {
    pub fn as_str(self) -> &'static str {
        match self {
            ScaleDef::Theater => "theater",
            ScaleDef::Battle => "battle",
            ScaleDef::Siege => "siege",
            ScaleDef::Naval => "naval",
        }
    }
}

/// 作战域。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DomainDef {
    Land,
    Naval,
    Air,
}

impl DomainDef {
    pub fn as_str(self) -> &'static str {
        match self {
            DomainDef::Land => "land",
            DomainDef::Naval => "naval",
            DomainDef::Air => "air",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct EngineReq {
    pub min: String,
    #[serde(default)]
    pub max: Option<String>,
}

/// `scenario.json`
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct ScenarioDef {
    pub id: String,
    pub name: String,
    /// 起始日期，ISO 格式。当前引擎只做展示，不影响模拟。
    #[serde(default)]
    pub start_date: String,
    /// 战役长度（天）。
    pub days: u32,
    /// 随机种子。留空时由引擎按模块 id 派生。
    #[serde(default)]
    pub seed: u64,
    /// 通信条件。留空则用有线电报时代默认值。
    #[serde(default)]
    pub comms: Option<CommsDef>,
    /// 模块声明的结局。引擎只提供句式，内容决定"什么算苦撑"。
    /// 不写就只有一个默认结局（战役期满）。
    #[serde(default)]
    pub endings: Vec<EndingDef>,
    /// 模块自定义规则。这是模块"扩展玩法"的入口。
    #[serde(default)]
    pub rules: Vec<RuleDef>,
    /// **预览版功能开关**。不写 = 全关 = 第一版发行版的定稿行为。
    ///
    /// 为什么放在内容里而不是命令行开关：同一台引擎要能同时跑
    /// 定稿战役和实验战役，而且回放必须靠**模块自己**说明它想开哪些机制，
    /// 否则同一个模块在两个人的机器上会跑出两种结果。
    #[serde(default)]
    pub preview: Option<PreviewDef>,
}

/// 预览版功能的开关表。
///
/// 这些机制**不进第一版发行版的平衡**：它们会改变战斗、补给与将领行为，
/// 所以只在模块显式打开时才生效。老模块不写这一段，跑出来的每个字节都和
/// 定稿版本一致（状态哈希也不变）。
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct PreviewDef {
    /// 弹药与油料：打光了真的打不动，缺油的装甲走不动。
    #[serde(default)]
    pub ammo_fuel: bool,
    /// 铁路可被炸断（3 级损坏），也会被就地抢修。
    #[serde(default)]
    pub rail_damage: bool,
    /// 派系与人事：将领有靠山，靠山影响服从，可以撤换。
    #[serde(default)]
    pub politics: bool,
}

/// 规则的阶段。
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseDef {
    PreSupply,
    PostSupply,
    PostMovement,
    PreCombat,
    PostCombat,
    PostTick,
}

/// 条件表达式。
///
/// ```json
/// "when": { "all": [ { "terrain": "mountain" }, { "unit_kind": "armor" } ] }
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionDef {
    Always,
    Never,
    All(Vec<ConditionDef>),
    Any(Vec<ConditionDef>),
    Not(Box<ConditionDef>),
    /// 地形：plain / hill / mountain / marsh / water / urban
    Terrain(String),
    /// 单位类型：infantry / armor / air / naval
    UnitKind(String),
    /// 兵力低于编制的比例
    StrengthBelow(f64),
    /// 所在省归属：cn / jp
    ProvinceOwner(String),
}

/// 原语（动作）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimitiveDef {
    /// 按当前兵力百分比增减
    ModifyStrengthPct(f64),
    ModifyMorale(f64),
    ModifySupply(f64),
    Log(String),
}

/// 一条规则。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct RuleDef {
    pub id: String,
    pub phase: PhaseDef,
    #[serde(default)]
    pub priority: i32,
    pub when: ConditionDef,
    pub then: Vec<PrimitiveDef>,
}

/// 通信配置。
///
/// 引擎只认延迟天数，不认年代。不同时代的模块配不同数值：
///
/// | 时代 | 预设 | 建议延迟 |
/// |---|---|---|
/// | 1805 | `courier` | local 1 / adjacent 3 |
/// | 1916 | `wire_telegraph` | local 0 / adjacent 2 |
/// | 1938 | `wire_telegraph` | local 0 / adjacent 1 |
/// | 1944 | `radio` | local 0 / adjacent 1 |
///
/// 也可以不用预设，直接写三个延迟。
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct CommsDef {
    /// 预设：`courier` / `wire_telegraph` / `radio`。
    #[serde(default)]
    pub preset: Option<String>,
    /// 技术传输延迟（天）。
    #[serde(default)]
    pub transmission: Option<u32>,
    /// 参谋汇总与决策延迟（天）。
    #[serde(default)]
    pub staff: Option<u32>,
    /// 命令下达延迟（天）。
    #[serde(default)]
    pub order_delay: Option<u32>,
}

/// `map.json`
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct MapDef {
    /// 西 / 南 / 东 / 北。
    #[serde(default)]
    pub bbox: Option<[f64; 4]>,
    #[serde(default)]
    pub projection: Option<String>,
    pub provinces: Vec<ProvinceDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerrainDef {
    Plain,
    Hill,
    Mountain,
    Marsh,
    Water,
    Urban,
}

/// 省内阵地（战术节点）。
///
/// 省是作战尺度（约 35 公里），阵地是战术尺度（2-5 公里）。
/// 万家岭不是"一个省"，而是张古山、扁担山、哔叽岭三个阵地。
///
/// 不写 `positions` 的省份自动得到一个阵地（省名 + 省地形，宽度全开），
/// 所以老模块不用改。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct PositionDef {
    /// 阵地名，出现在单位牌与战报里，例如「张古山」。
    pub name: String,
    /// 阵地地形。不写就沿用省的地形。
    #[serde(default)]
    pub terrain: Option<TerrainDef>,
    /// 展开正面 0.2-1.0。越窄，能同时投入的兵力越少（多的只能当预备队）。
    #[serde(default = "default_one_f64")]
    pub width: f64,
    /// 工事等级 0-5，由「筑工事」命令提升。
    #[serde(default)]
    pub fort: u8,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct ProvinceDef {
    /// 本地索引。引擎内部按此排序，要求从 0 连续。
    pub id: u32,
    pub name: String,
    pub terrain: TerrainDef,
    #[serde(default)]
    pub owner: Option<SideDef>,
    /// 是否补给源。
    #[serde(default)]
    pub supply_source: bool,
    /// 有没有铁路/公路干线经过（补给沿这条线几乎不掉）。
    #[serde(default)]
    pub rail: bool,
    /// 工事等级 0-5。
    #[serde(default)]
    pub fort: u8,
    #[serde(default = "default_one_f64")]
    pub supply_throughput: f64,
    #[serde(default)]
    pub neighbors: Vec<u32>,
    #[serde(default)]
    pub victory_points: u32,
    /// 地形原型，见 `docs/04` 附录 A.2。
    #[serde(default)]
    pub prototype: Option<String>,
    /// 经度（东经为正）。用于把省份排成真实地图。
    /// 现在的模拟不读它，但前端靠它生成多边形边界。
    #[serde(default)]
    pub lon: Option<f64>,
    /// 纬度（北纬为正）。
    #[serde(default)]
    pub lat: Option<f64>,
    ///
    /// 省内阵地。不写就自动生成一个（省名 + 省地形，宽度全开）。
    /// 写了之后，战斗、工事、调兵都在阵地粒度上进行。
    #[serde(default)]
    pub positions: Vec<PositionDef>,
    /// 平均海拔（米）。由 `tools/mapgen/terrain.js` 从真实高程（DEM）采样得来，
    /// 和 `terrain` 出自同一份数据。模拟**不读**它，只是"为什么这里是山地"的说明。
    #[serde(default)]
    pub elev: Option<i32>,
    /// 省内相对高差（米，p95−p5）。同上，只用来说明。
    #[serde(default)]
    pub relief: Option<i32>,
    /// 备注，供作者记录出处。
    #[serde(default)]
    pub note: String,
}

/// 非玩家阵营的统帅部（"AI 玩家"）。
///
/// 这是敌方那一局的总指挥：有自己的战役目标与性格，会对自己麾下的师团
/// 下具体命令——走的是**和玩家一样的通道**（电报 → 师团长判断 → 执行），
/// 所以师团长照样可以抗命。
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct HqDef {
    /// 司令官姓名（岡村寧次）。
    pub name: String,
    /// 职务（第11军司令官）。
    #[serde(default)]
    pub title: String,
    /// 人物志：他是什么人、怎么打仗。
    #[serde(default)]
    pub persona: String,
    /// 他要什么、怕什么。
    #[serde(default)]
    pub agenda: String,
    /// 战役目标，按优先级。写给模型看的，不是给引擎算的。
    #[serde(default)]
    pub objectives: Vec<String>,
}

/// 模块声明的一条结局。
///
/// 引擎不认识"天炉""苦撑"这些词，它只认三件事：指标、比较、优先级。
/// 所以结局是**内容**：同一套引擎可以描述 1938 年的南浔线，也可以描述别的战役。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct EndingDef {
    pub id: String,
    pub name: String,
    /// 一段话，给玩家看的。
    #[serde(default)]
    pub text: String,
    /// 越小越先判。
    #[serde(default)]
    pub priority: u32,
    /// true（默认）= 只在战役期满时判；false = 每天都可以提前触发（崩盘、速溃）。
    #[serde(default = "default_true")]
    pub at_end_only: bool,
    /// 全部满足才算命中。
    #[serde(default)]
    pub require: Vec<ClauseDef>,
}

/// 一条判定子句：`指标 [某方] 比较 阈值`。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct ClauseDef {
    /// day / provinces / strength / losses / units_alive / commanders_lost / holds_province
    pub metric: String,
    /// cn / jp。`holds_province` 必须写。
    #[serde(default)]
    pub side: Option<SideDef>,
    /// ge / gt / le / lt。默认 ge。
    #[serde(default)]
    pub cmp: Option<String>,
    #[serde(default)]
    pub value: f64,
    /// `holds_province` 用的省名。
    #[serde(default)]
    pub province: Option<String>,
}

/// `oob.json`（order of battle，战斗序列）
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct OobDef {
    pub units: Vec<UnitDef>,
    /// 派系（预览版）。不写就是没有派系政治。
    #[serde(default)]
    pub factions: Vec<FactionDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UnitKindDef {
    #[default]
    Infantry,
    Armor,
    Air,
    Naval,
}

/// 作战姿态。见 `UnitDef::stance`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StanceDef {
    /// 主动进攻：没有邻接敌人就朝最近敌人推进。
    #[default]
    Attack,
    /// 就地防守：**不主动离开**，只在邻接敌省变空时占回来。
    Defend,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct UnitDef {
    pub id: String,
    pub name: String,
    pub side: SideDef,
    /// 上级番号，纯展示与将来编制树用。
    #[serde(default)]
    pub parent: Option<String>,
    /// 单位类型。
    #[serde(default)]
    pub kind: UnitKindDef,
    /// 兵力（千人）。
    pub strength: f64,
    pub position: u32,
    /// 航空侦察半径（跳数）。仅 `kind: "air"` 有效。
    #[serde(default)]
    pub recon_radius: u32,
    /// 空中打击强度（0 表示仅侦察）。仅 `kind: "air"` 有效。
    #[serde(default)]
    pub strike_power: f64,
    /// 战斗力系数（装备/训练/组织）。
    ///
    /// 史实参考：日军常备师团约相当于国军 3-5 个师，
    /// 但这个系数不要照搬——它是**玩法参数**，不是历史换算。
    /// 建议起步：日军 1.5-1.8，国军中央军 1.0-1.2，地方部队 0.6-0.8。
    #[serde(default = "default_one_f64")]
    pub effectiveness: f64,
    /// 补给延伸能力 0-1。
    ///
    /// 表示"把补给不足的缺口补上多少"。机械化、汽车运输、铁路网越强，值越高。
    /// 建议起步：日军 0.4-0.5（有汽车与铁路），国军 0.1-0.2（人力挑运）。
    /// 这是武汉会战最关键的后勤不对称，别让双方共用一套衰减。
    #[serde(default)]
    pub supply_reach: f64,
    /// 作战姿态。
    ///
    /// 引擎不认番号也不认国家，只认姿态：
    /// - `attack`：没有邻接敌人时向敌人方向推进（默认，适合进攻方）。
    /// - `defend`：守在原地，只有邻接出现**空的**敌省才去占回来。
    ///
    /// 这一条是"前线"的最小实现。没有它，防守方会因为"邻接看不到敌人"
    /// 而一路朝最近的敌人行军，主动放弃阵地——兵力再多也没用。
    #[serde(default)]
    pub stance: StanceDef,
    /// 是否史实数据。false 表示推断，界面上要说明。
    #[serde(default = "default_true")]
    pub estimated: bool,
    /// 将领。留空则用一个完全服从的默认将领。
    #[serde(default)]
    pub commander: Option<CommanderDef>,
    /// 二次整编：覆灭后隔多少天可以在后方重新成军。0 = 不可重建。
    ///
    /// 由模块决定谁重建、谁不重建——1938 年国军有些师几个月里重建过两次，
    /// 有些部队打光就再没有了。这是内容，不是引擎该猜的事。
    #[serde(default)]
    pub reform_days: u32,
    /// 整编后补到满编的几成（0.4 = 四成）。
    #[serde(default)]
    pub reform_ratio: f64,
    /// 开局弹药基数 0-1（预览版 `preview.ammo_fuel` 才有意义，默认满）。
    #[serde(default = "default_one_f64")]
    pub ammo: f64,
    /// 开局油料基数 0-1。同上。
    #[serde(default = "default_one_f64")]
    pub fuel: f64,
    /// 每天每动一步的油料消耗 0-1（默认按兵种给：步兵 0，装甲 0.12，海空军 0.15）。
    ///
    /// 写 0 表示"这支部队不吃油"（马拉炮兵、徒步步兵）。
    #[serde(default)]
    pub fuel_burn: Option<f64>,
    #[serde(default)]
    pub note: String,
}

/// 将领性格。
///
/// 四项都是 0-1。它们不直接决定胜负，而是决定"上级意图被执行成什么样"。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct CommanderDef {
    pub name: String,
    /// 服从度：越低越可能抗命、打折扣。
    #[serde(default = "default_half")]
    pub obedience: f64,
    /// 主动性：越高越可能自行改变目标。
    #[serde(default = "default_half")]
    pub initiative: f64,
    /// 进攻倾向：高的人愿意强攻，低的人偏好稳守。
    #[serde(default = "default_half")]
    pub aggression: f64,
    /// 谨慎：高的人在劣势时倾向保存实力。
    #[serde(default = "default_half")]
    pub caution: f64,
    /// 这个人是谁：出身、经历、用兵习惯。**只给 LLM 看，不进模拟**。
    ///
    /// 四个数字决定"他会不会抗命"，这一段决定"他抗命时会怎么说、
    /// 争的是什么"。没有它，模型只能按四个小数臆想一个人。
    #[serde(default)]
    pub persona: String,
    /// 他图什么、怕什么。给模型一个可以拿来权衡私心的抓手。
    #[serde(default)]
    pub agenda: String,
    /// 职务（第几军司令官、师长……），纯展示与提示词用。
    #[serde(default)]
    pub title: String,
    /// 所属派系 id（预览版 `preview.politics`）。留空 = 无派系（技术军官、中央直系）。
    #[serde(default)]
    pub faction: Option<String>,
}

/// 一个派系（预览版 `preview.politics`）。
///
/// 派系是**人**的关系，所以写在 `oob.json` 里、和将领表放一起。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
pub struct FactionDef {
    /// 稳定 id，将领用 `faction` 引用它。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 归属阵营。派系只在本方内部竞争。
    pub side: SideDef,
    /// 初始朝中分量 0-1。引擎会围绕它缓慢漂移，战功与败绩再推动它。
    #[serde(default = "default_half")]
    pub weight: f64,
    #[serde(default)]
    pub note: String,
}

fn default_half() -> f64 {
    0.5
}

fn default_true() -> bool {
    true
}


