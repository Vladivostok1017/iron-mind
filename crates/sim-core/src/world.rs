//! 世界状态：省份、单位、参战方。
//!
//! 设计约束：
//! - 所有集合都是 `Vec` + 稳定索引，遍历顺序即定义顺序，**不用 HashMap**。
//! - 所有数值是 `Fixed`，无浮点。
//! - 每个字段都参与 `state_hash`，任何状态漂移都能被检测到。

use crate::fixed::Fixed;
use crate::hash::Hasher;

/// 参战方。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Cn,
    Jp,
}

impl Side {
    pub fn opponent(self) -> Self {
        match self {
            Side::Cn => Side::Jp,
            Side::Jp => Side::Cn,
        }
    }

    pub fn index(self) -> u32 {
        match self {
            Side::Cn => 0,
            Side::Jp => 1,
        }
    }

    pub fn from_index(i: usize) -> Self {
        if i == 0 {
            Side::Cn
        } else {
            Side::Jp
        }
    }
}

/// 情报精度。
///
/// 这是"非上帝视角"的核心：玩家与将领看到的都是这里的值，
/// 而不是真实兵力。等级越高越接近真值。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Accuracy {
    /// 完全不知道。
    Unknown = 0,
    /// 传闻：可能有敌人，规模不明。
    Rumor = 1,
    /// 近似：知道大概多少，误差可达 ±40%。
    Approximate = 2,
    /// 精确：战斗接触或航空侦察所得。
    Exact = 3,
}

impl Accuracy {
    pub fn code(self) -> u32 {
        self as u32
    }

    /// 误差倍率：用于给估计值加偏差。
    pub fn error_ratio(self) -> Fixed {
        match self {
            Accuracy::Exact => Fixed::ZERO,
            Accuracy::Approximate => Fixed::ratio(4, 10),
            Accuracy::Rumor => Fixed::ratio(8, 10),
            Accuracy::Unknown => Fixed::ONE,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Accuracy::Exact => "精确",
            Accuracy::Approximate => "近似",
            Accuracy::Rumor => "传闻",
            Accuracy::Unknown => "未知",
        }
    }

    fn decay(self) -> Self {
        match self {
            Accuracy::Exact => Accuracy::Approximate,
            Accuracy::Approximate => Accuracy::Rumor,
            Accuracy::Rumor => Accuracy::Unknown,
            Accuracy::Unknown => Accuracy::Unknown,
        }
    }
}

/// 通信手段。
///
/// 这是"跨时代兼容"的核心抽象：引擎只关心"报告多久能到"，
/// 不关心"这是骑兵还是电台"。不同年代的模块声明不同的手段，
/// 引擎行为由参数驱动，代码里没有任何年代判断。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommsKind {
    /// 骑兵/步卒传令。前电报时代。
    Courier,
    /// 有线电报。需要固定线路，移动部队难用。
    WireTelegraph,
    /// 无线电。机动性好，但可能被截获。
    Radio,
    /// 野战电话。战术级实时，战略级有限。
    FieldTelephone,
}

impl CommsKind {
    pub fn label(self) -> &'static str {
        match self {
            CommsKind::Courier => "骑兵传令",
            CommsKind::WireTelegraph => "有线电报",
            CommsKind::Radio => "无线电",
            CommsKind::FieldTelephone => "野战电话",
        }
    }

    pub fn code(self) -> u32 {
        match self {
            CommsKind::Courier => 0,
            CommsKind::WireTelegraph => 1,
            CommsKind::Radio => 2,
            CommsKind::FieldTelephone => 3,
        }
    }
}

/// 通信能力配置。
///
/// **延迟不是一个数，是三段。** 这一条之前做错过，值得写下来：
/// 有线电报与无线电的传输是秒级的，战场上情报慢两三天，
/// 慢的不是"电报"，是"找电台、写报告、逐级核实、参谋汇总、指挥员决策、
/// 命令再逐级下达"这套组织流程。
///
/// 所以：
/// - `transmission`：技术传输。电报/无线电为 0，骑兵 2-3 天。
/// - `staff`：汇总与决策。跟通信手段关系不大，**这才是主要瓶颈**。
/// - `order_delay`：命令下达。再走一遍链路。
///
/// 修正带来的结论：从电报升级到无线电，总延迟几乎不变。
/// 这与史实一致——通信技术飞跃，指挥周期仍是小时到天。
#[derive(Clone, Copy, Debug)]
pub struct CommsConfig {
    /// 通信手段（用于叙事与界面，不影响模拟计算）。
    pub kind: CommsKind,
    /// 技术传输延迟（天）。电报/无线电为 0，骑兵 2-3。
    pub transmission: u32,
    /// 参谋汇总、核实与决策延迟（天）。与通信手段弱相关。
    pub staff: u32,
    /// 命令逐级下达的额外延迟（天）。
    pub order_delay: u32,
}

impl CommsConfig {
    /// 报告从"被发现"到"进入指挥员视野"的总延迟。
    pub fn report_delay(&self) -> u32 {
        self.transmission + self.staff
    }

    /// 命令从"指挥员决定"到"执行单位收到"的总延迟。
    pub fn command_delay(&self) -> u32 {
        self.transmission + self.order_delay
    }

    /// 有线电报（1930 年代）：传输秒级，瓶颈在参谋流程。
    pub const fn telegraph_era() -> Self {
        Self {
            kind: CommsKind::WireTelegraph,
            transmission: 0,
            staff: 1,
            order_delay: 0,
        }
    }

    /// 前电报时代：骑兵传令，传输本身就要几天。
    pub const fn courier_era() -> Self {
        Self {
            kind: CommsKind::Courier,
            transmission: 2,
            staff: 1,
            order_delay: 1,
        }
    }

    /// 无线电时代：传输秒级，参谋流程仍是瓶颈。
    pub const fn radio_era() -> Self {
        Self {
            kind: CommsKind::Radio,
            transmission: 0,
            staff: 1,
            order_delay: 0,
        }
    }

    /// 即时通信。仅用于测试，用来隔离"延迟"这个变量。
    pub const fn instant() -> Self {
        Self {
            kind: CommsKind::FieldTelephone,
            transmission: 0,
            staff: 0,
            order_delay: 0,
        }
    }
}

impl Default for CommsConfig {
    fn default() -> Self {
        Self::telegraph_era()
    }
}

/// 一份"还在路上"的情报报告。
///
/// 这是"玩家不是上帝"的第二层：情报不仅要被侦察到，
/// 还要经过通信链路传回来。传递期间上级什么都不知道。
#[derive(Clone, Copy, Debug)]
pub struct PendingReport {
    /// 预计送达的 tick。
    pub arrive_tick: u32,
    pub province: u32,
    pub enemy_strength: Fixed,
    pub accuracy: Accuracy,
    pub identity_known: bool,
}

/// 一个省的情报。
#[derive(Clone, Copy, Debug)]
pub struct ProvinceIntel {
    /// 对敌方兵力的估计（不是真值）。
    pub enemy_strength: Fixed,
    /// 估计的精度。
    pub accuracy: Accuracy,
    /// 最后一次获取情报的 tick。
    pub seen_tick: u32,
    /// 是否识别出敌方番号。
    pub identity_known: bool,
}

impl ProvinceIntel {
    pub fn unknown() -> Self {
        Self {
            enemy_strength: Fixed::ZERO,
            accuracy: Accuracy::Unknown,
            seen_tick: 0,
            identity_known: false,
        }
    }

    pub fn is_known(&self) -> bool {
        self.accuracy > Accuracy::Unknown
    }

    /// 每 tick 的时效衰减：超过 3 天没有新情报，精度降一级。
    pub fn decay(&mut self, now: u32) {
        if now > self.seen_tick.saturating_add(3) && self.accuracy > Accuracy::Unknown {
            self.accuracy = self.accuracy.decay();
            self.seen_tick = now;
            if self.accuracy == Accuracy::Unknown {
                self.enemy_strength = Fixed::ZERO;
            }
        }
    }
}

/// 地形类型。每种带一个防御加成。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Terrain {
    Plain,
    Hill,
    Mountain,
    Marsh,
    Water,
    Urban,
}

impl Terrain {
    /// 防御方战斗力乘数（1.0 + 加成）。
    pub fn defense_multiplier(self) -> Fixed {
        match self {
            Terrain::Plain => Fixed::ONE,
            Terrain::Hill => Fixed::ratio(12, 10),
            Terrain::Mountain => Fixed::ratio(14, 10),
            Terrain::Marsh => Fixed::ratio(11, 10),
            Terrain::Water => Fixed::ratio(16, 10),
            Terrain::Urban => Fixed::ratio(13, 10),
        }
    }

    /// 通行消耗（点数越高越难走）。
    pub fn move_cost(self) -> Fixed {
        match self {
            Terrain::Plain => Fixed::ONE,
            Terrain::Hill => Fixed::ratio(15, 10),
            Terrain::Mountain => Fixed::ratio(25, 10),
            Terrain::Marsh => Fixed::ratio(30, 10),
            Terrain::Water => Fixed::ratio(40, 10),
            Terrain::Urban => Fixed::ratio(12, 10),
        }
    }

    pub fn code(self) -> u32 {
        match self {
            Terrain::Plain => 0,
            Terrain::Hill => 1,
            Terrain::Mountain => 2,
            Terrain::Marsh => 3,
            Terrain::Water => 4,
            Terrain::Urban => 5,
        }
    }
}

/// 阵地：省内的战术节点。
///
/// 省是**作战**尺度（约 35 公里），阵地是**战术**尺度（2-5 公里）。
/// 万家岭不是"一个省"，而是张古山、扁担山、哔叽岭三个阵地。
///
/// 三件事靠它实现：
/// 1. **战场宽度**：`width` 限制同一阵地能投入的兵力比例，突破阈值效应；
/// 2. **调兵**：部队可以在省内阵地间转移（不消耗天数），这就是预备队；
/// 3. **工事**：工事做在阵地上，不是做在省上，所以"守住哪个山头"才有意义。
#[derive(Clone, Debug)]
pub struct Position {
    pub name: String,
    /// 阵地地形（决定防御乘数与通行性）。
    pub terrain: Terrain,
    /// 展开正面 0.2-1.0。同一阵地里，超出这个比例的部分只能当预备队。
    pub width: Fixed,
    /// 本阵地工事等级 0-5。
    pub fort: u8,
}

impl Position {
    pub fn new(name: &str, terrain: Terrain) -> Self {
        Self {
            name: name.to_string(),
            terrain,
            width: Fixed::ONE,
            fort: 0,
        }
    }
}

/// 战报事件的类别。
///
/// 为什么要有类别：以前战报是 `Vec<String>`，前端靠正则匹配"抗命/退/失守"来决定颜色。
/// 那是把业务语义塞进字符串——加一种事件就要改一次前端正则，而且中文措辞一变就全错。
/// 现在引擎产出类别，前端只按类别上色。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventKind {
    /// 模块规则触发（`rules.rs` 的 Log 原语）。
    Rule,
    /// 玩家下达的命令。
    Command,
    /// 交战结果（双方伤亡）。
    Combat,
    /// 阵地或省份易手。
    Occupy,
    /// 撤退/溃退。
    Retreat,
    /// 抗命与改令。
    Dissent,
    /// 情报：首次接触、侦察回报。
    Intel,
    /// 补给与工事。
    Supply,
    /// 编制变动：全军覆没、将领殉国/被俘/去职、二次整编。
    ///
    /// 单独一类，是因为它和"今天打死了多少人"不是一回事：
    /// 兵力可以补，番号没了就是没了。
    Oob,
}

impl EventKind {
    /// 前端用来上色的短标识。改了要同步改 `web/index.html`。
    pub fn code(self) -> &'static str {
        match self {
            EventKind::Rule => "rule",
            EventKind::Command => "cmd",
            EventKind::Combat => "combat",
            EventKind::Occupy => "occupy",
            EventKind::Retreat => "retreat",
            EventKind::Dissent => "dissent",
            EventKind::Intel => "intel",
            EventKind::Supply => "supply",
            EventKind::Oob => "oob",
        }
    }
}

/// 一条战报。
#[derive(Clone, Debug)]
pub struct Event {
    pub tick: u32,
    pub kind: EventKind,
    pub text: String,
    /// 这条消息属于哪一方。`None` = 双方都看得到的客观事实（交火、易手）。
    ///
    /// 为什么要有这一位：将领的判断与回电是**内部文件**。
    /// 不加这一位，日军将领的"我不服命令"会直接出现在我方战报里——
    /// 玩家不该知道敌人心里怎么想，那是上帝视角。
    pub side: Option<Side>,
}

impl Event {
    /// 客观事实：双方都能看到。
    pub fn new(tick: u32, kind: EventKind, text: impl Into<String>) -> Self {
        Self { tick, kind, text: text.into(), side: None }
    }

    /// 某一方的内部消息：只有这一方能看见。
    pub fn sided(tick: u32, kind: EventKind, text: impl Into<String>, side: Side) -> Self {
        Self { tick, kind, text: text.into(), side: Some(side) }
    }

    /// 纯文本（无头测试台、控制台用）。
    pub fn line(&self) -> String {
        format!("[{}] {}", self.tick, self.text)
    }
}

impl World {
    /// 追加一条公开战报（双方可见）。
    ///
    /// 上限 400 条：模拟会跑上千 tick，不封顶会把内存和前端都拖垮。
    pub fn report(&mut self, kind: EventKind, text: impl Into<String>) {
        if self.log.len() >= 400 {
            return;
        }
        let tick = self.tick;
        self.log.push(Event::new(tick, kind, text));
    }

    /// 追加一条**某一方内部**的战报（将领的判断、回电、抗命理由）。
    pub fn report_for(&mut self, kind: EventKind, text: impl Into<String>, side: Side) {
        // 内部消息：只有本方看得到（服务端按视角过滤）
        if self.log.len() >= 400 {
            return;
        }
        let tick = self.tick;
        self.log.push(Event::sided(tick, kind, text, side));
    }

    /// 一封要在路上走 `delay` 天的战报（编制变动、断粮、被围都用它）。
    ///
    /// 事件的 `tick` 记的是**发生日**，不是送达日——玩家看到的日期应当是事发那天，
    /// 只是他今天才收到。
    pub fn report_delayed(
        &mut self,
        delay: u32,
        kind: EventKind,
        text: impl Into<String>,
        side: Option<Side>,
    ) {
        if self.log.len() >= 400 {
            return;
        }
        let tick = self.tick;
        let ev = match side {
            Some(s) => Event::sided(tick, kind, text, s),
            None => Event::new(tick, kind, text),
        };
        if delay == 0 {
            self.log.push(ev);
        } else {
            self.in_transit.push((tick + delay, ev));
        }
    }

    /// 把今天该到的战报放进日志。顺序固定（按送达 tick、再按入队顺序）。
    pub fn flush_in_transit(&mut self) {
        // +1：这一天还没走完（`tick` 要到 tick 末尾才自增）。
        // 不 +1 的话 delay=2 的电报要等到第三天早上才到，等于永远慢一天。
        let now = self.tick + 1;
        let mut due: Vec<Event> = Vec::new();
        let mut keep: Vec<(u32, Event)> = Vec::with_capacity(self.in_transit.len());
        for (arrive, ev) in self.in_transit.drain(..) {
            if arrive <= now {
                due.push(ev);
            } else {
                keep.push((arrive, ev));
            }
        }
        self.in_transit = keep;
        for ev in due {
            if self.log.len() >= 400 {
                break;
            }
            self.log.push(ev);
        }
    }
}

/// 一个兵团/军团（编制实体）。
///
/// 以前 `formation` 只是一个字符串：兵团部存不存在、在不在，都不影响任何事。
/// 于是"打掉敌人的兵团部"这种战法在引擎里毫无意义。
///
/// 现在兵团是一个实体，`hq` 指向自己的**兵团部单位**（例如 `第29军团`
/// 既是部队、也是 `第187师` 的上级）。数据里没有兵团部单位的
/// （`第1兵团`）就是纸面番号：`hq = None`，不加成也不惩罚。
#[derive(Clone, Debug)]
pub struct Formation {
    pub name: String,
    pub side: Side,
    /// 兵团部所在单位的索引。
    pub hq: Option<usize>,
    /// 下属单位索引（含兵团部自己）。
    pub units: Vec<usize>,
}

/// 一个省份。
#[derive(Clone, Debug)]
pub struct Province {
    pub name: String,
    pub terrain: Terrain,
    /// 工事等级 0-5。**省级的**，用于整省防御；阵地另有自己的工事。
    pub fort: u8,
    /// 补给通过能力 0-1。
    pub supply_throughput: Fixed,
    /// 邻接省份索引，必须双向一致（校验器检查）。
    pub neighbors: Vec<u32>,
    /// 控制方。
    pub owner: Side,
    /// 是否补给源。
    pub is_supply_source: bool,
    /// 有没有铁路/公路干线经过。
    ///
    /// 这条线才是后勤的命脉：南浔铁路在，一个军就能从南昌拿到补给；
    /// 铁路断了（省丢了），同一支部队的补给立刻塌。
    /// 以前 supply_throughput 只是一个省上的常数，看不出"干线"这件事。
    pub rail: bool,
    /// 铁路损坏程度 0-3（0 = 通，3 = 断）。**只有 `Features::rail_damage` 开启时才变**；
    /// 老模块永远是 0，也不进哈希——定稿的行为不受影响。
    pub rail_damage: u8,
    /// 抢修进度：攒够天数就减一级损坏。同上，只在预览版里动。
    pub rail_repair: u8,
    /// 经度。**不参与模拟计算**，只用于渲染时把省份摆到真实位置。
    pub lon: Fixed,
    /// 纬度。同上。
    pub lat: Fixed,
    /// 省内阵地。至少一个；模块不写就自动生成一个（向后兼容）。
    pub positions: Vec<Position>,
    /// 平均海拔（米）。**不参与模拟**，只是给玩家看的"为什么这里是山地"。
    pub elev: Option<i32>,
    /// 省内相对高差（米）。同上。
    pub relief: Option<i32>,
}

impl Province {
    pub fn new(name: &str, terrain: Terrain, owner: Side) -> Self {
        Self {
            name: name.to_string(),
            terrain,
            fort: 0,
            supply_throughput: Fixed::ONE,
            neighbors: Vec::new(),
            owner,
            is_supply_source: false,
            rail: false,
            rail_damage: 0,
            rail_repair: 0,
            lon: Fixed::ZERO,
            lat: Fixed::ZERO,
            positions: Vec::new(),
            elev: None,
            relief: None,
        }
    }

    /// 记下真实高程（米）。由内容层从 DEM 采样后填，模拟不读。
    pub fn with_elevation(mut self, elev: Option<i32>, relief: Option<i32>) -> Self {
        self.elev = elev;
        self.relief = relief;
        self
    }

    /// 阵地数（至少 1）。
    pub fn node_count(&self) -> u8 {
        self.positions.len().max(1) as u8
    }

    /// 取阵地；越界时退化到 0 号（内容层永远可能写错，不 panic）。
    pub fn position_at(&self, node: u8) -> Option<&Position> {
        self.positions.get(node as usize).or_else(|| self.positions.first())
    }

    /// 该阵地的防御乘数：阵地地形 × 阵地工事。
    pub fn node_defense(&self, node: u8) -> Fixed {
        let p = match self.position_at(node) {
            Some(p) => p,
            None => return self.terrain.defense_multiplier(),
        };
        let base = p.terrain.defense_multiplier();
        // 每级工事 +15%，省级工事叠加一半。上限约 2.5 倍。
        let fort_bonus = Fixed::ratio(15, 100) * Fixed::from_int(p.fort as i32)
            + Fixed::ratio(8, 100) * Fixed::from_int(self.fort as i32);
        base * (Fixed::ONE + fort_bonus)
    }

    /// 该阵地的展开宽度（0.2-1.0）。
    pub fn node_width(&self, node: u8) -> Fixed {
        self.position_at(node)
            .map(|p| p.width)
            .unwrap_or(Fixed::ONE)
    }

    pub fn with_neighbors(mut self, n: &[u32]) -> Self {
        self.neighbors = n.to_vec();
        self
    }

    pub fn with_fort(mut self, level: u8) -> Self {
        self.fort = level.min(5);
        self
    }

    pub fn as_source(mut self) -> Self {
        self.is_supply_source = true;
        self
    }

    /// 有效防御乘数：地形 × 工事。
    pub fn defense_multiplier(&self) -> Fixed {
        let fort_bonus = Fixed::from_int(self.fort as i32) * Fixed::ratio(15, 100);
        self.terrain.defense_multiplier() + fort_bonus
    }
}

/// 单位类型。
///
/// 一个师、一个飞行团、一支舰队，在模拟里的行为完全不同：
/// 空中单位不占省、不参与地面消耗，只提供侦察与火力支援；
/// 海军单位沿水域行动，提供舰炮支援。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnitKind {
    /// 步兵。地面战斗与占领的主力。
    Infantry,
    /// 装甲。地面战斗，机动强。
    Armor,
    /// 空军。不占省，提供侦察与轰炸。
    Air,
    /// 海军。提供舰炮支援与封锁。
    Naval,
}

/// 作战姿态。由模块声明，引擎只实现语义。
///
/// 引擎里刻意**没有**"哪一方该守"这种判断——那是史实知识和玩法设计，
/// 属于模块。引擎只提供两种姿态的机械差别。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Stance {
    /// 主动进攻：没有邻接敌人时朝最近敌人推进。
    #[default]
    Attack,
    /// 就地防守：不主动离开所在省；只在邻接敌省变空时占回来。
    Defend,
}

impl UnitKind {
    pub fn is_ground(self) -> bool {
        matches!(self, UnitKind::Infantry | UnitKind::Armor)
    }

    pub fn code(self) -> u32 {
        match self {
            UnitKind::Infantry => 0,
            UnitKind::Armor => 1,
            UnitKind::Air => 2,
            UnitKind::Naval => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            UnitKind::Infantry => "步兵",
            UnitKind::Armor => "装甲",
            UnitKind::Air => "空军",
            UnitKind::Naval => "海军",
        }
    }
}

/// 将领。
///
/// 这是"意图式指挥"的核心：玩家下达的是意图，将领做的是**解释**。
/// 同一个"进攻"命令，不同的人执行出来完全不一样。
///
/// 四个维度：
/// - `obedience` 服从度：越低越可能抗命、打折扣
/// - `initiative` 主动性：越高越可能自行改变目标（"将在外军令有所不受"）
/// - `aggression` 进攻倾向：高的人愿意强攻，低的人偏好稳守
/// - `caution` 谨慎：高的人在劣势时倾向保存实力
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commander {
    pub name: String,
    pub obedience: Fixed,
    pub initiative: Fixed,
    pub aggression: Fixed,
    pub caution: Fixed,
    /// 人物志：出身、经历、用兵习惯。只给 LLM 读，不参与模拟，也不进状态哈希。
    pub persona: String,
    /// 他图什么、怕什么。
    pub agenda: String,
    /// 职务。
    pub title: String,
    /// 他的结局。殉国不是一句战报——它会改变这支部队还能不能重建。
    pub fate: Fate,
    /// 所属派系 id（预览版 `Features::politics`）。None = 无派系。
    ///
    /// 为什么不进哈希：它是**内容常量**，同一模块每次加载都一样；
    /// 会变的是派系影响力，那个已经单独进哈希了。
    pub faction: Option<String>,
}

/// 将领的结局。
///
/// 为什么要有它：部队被全歼时，军长可能阵亡、被俘、去职，也可能突围出来。
/// 后果完全不同——**没有将领就没有人能把溃兵重新编起来**，
/// 所以"二次整编"不是时间一到自动发生，它取决于那个人还在不在。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fate {
    /// 在任（活着，也可能只是突围生还）
    Active,
    /// 阵亡 / 殉国
    Killed,
    /// 被俘
    Captured,
    /// 去职（撤职、调任、自请辞职）
    Relieved,
}

impl Fate {
    pub fn label(self) -> &'static str {
        match self {
            Fate::Active => "在任",
            Fate::Killed => "殉国",
            Fate::Captured => "被俘",
            Fate::Relieved => "去职",
        }
    }

    pub fn code(self) -> u32 {
        match self {
            Fate::Active => 0,
            Fate::Killed => 1,
            Fate::Captured => 2,
            Fate::Relieved => 3,
        }
    }

    /// 还有人带得动这支部队吗？
    pub fn can_lead(self) -> bool {
        self == Fate::Active
    }
}

impl Commander {
    /// 默认将领：完全服从、没有个性。用于占位与测试。
    pub fn default_for(name: &str) -> Self {
        Self {
            name: name.to_string(),
            obedience: Fixed::ONE,
            initiative: Fixed::ZERO,
            aggression: Fixed::ratio(5, 10),
            caution: Fixed::ratio(5, 10),
            persona: String::new(),
            agenda: String::new(),
            title: String::new(),
            fate: Fate::Active,
            faction: None,
        }
    }

    pub fn new(name: &str, obedience: f64, initiative: f64, aggression: f64, caution: f64) -> Self {
        Self {
            name: name.to_string(),
            obedience: Fixed::from_f64_input(obedience).clamp(Fixed::ZERO, Fixed::ONE),
            initiative: Fixed::from_f64_input(initiative).clamp(Fixed::ZERO, Fixed::ONE),
            aggression: Fixed::from_f64_input(aggression).clamp(Fixed::ZERO, Fixed::ONE),
            caution: Fixed::from_f64_input(caution).clamp(Fixed::ZERO, Fixed::ONE),
            persona: String::new(),
            agenda: String::new(),
            title: String::new(),
            fate: Fate::Active,
            faction: None,
        }
    }

    /// 挂上派系（预览版 `Features::politics`）。
    pub fn with_faction(mut self, faction: Option<&str>) -> Self {
        self.faction = faction.map(|f| f.to_string());
        self
    }

    /// 挂上人物志（只给 LLM 用）。
    pub fn with_persona(mut self, title: &str, persona: &str, agenda: &str) -> Self {
        self.title = title.to_string();
        self.persona = persona.to_string();
        self.agenda = agenda.to_string();
        self
    }

    /// 这是不是个"听话"的将领（用于战报分级）。
    pub fn is_obedient(&self) -> bool {
        self.obedience > Fixed::ratio(7, 10)
    }
}

/// 单位本 tick 的意图。
///
/// 这是"意图式指挥"在模拟层的最小表达：单位不直接改位置，
/// 而是持有意图，由 AI（或玩家）设置、由移动系统执行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// 原地固守。
    Hold,
    /// 进攻相邻省份。
    Attack(u32),
    /// 移动到相邻省份。
    Move(u32),
    /// 撤退到相邻省份。
    Retreat(u32),
    /// 省内调兵：转到本省的第 n 个阵地。**不消耗天数**——这是预备队的用法。
    Shift(u8),
    /// 修筑工事：留在原地把当前阵地挖深。遇敌自动中止。
    Fortify,
    /// 整补：留在原地恢复补给与士气。
    Rest,
}

impl Order {
    pub fn code(&self) -> (u32, u32) {
        match self {
            Order::Hold => (0, 0),
            Order::Attack(p) => (1, *p),
            Order::Move(p) => (2, *p),
            Order::Retreat(p) => (3, *p),
            Order::Shift(n) => (4, *n as u32),
            Order::Fortify => (5, 0),
            Order::Rest => (6, 0),
        }
    }

    /// 意图指向的目标省（若有）。
    pub fn target(&self) -> Option<u32> {
        match self {
            Order::Hold | Order::Shift(_) | Order::Fortify | Order::Rest => None,
            Order::Attack(p) | Order::Move(p) | Order::Retreat(p) => Some(*p),
        }
    }

    /// 这条命令要不要动身子。筑工事、整补、省内调兵都不动（调兵另算）。
    pub fn is_stationary(&self) -> bool {
        matches!(self, Order::Hold | Order::Fortify | Order::Rest)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Order::Hold => "固守",
            Order::Attack(_) => "进攻",
            Order::Move(_) => "推进",
            Order::Retreat(_) => "撤退",
            Order::Shift(_) => "调兵",
            Order::Fortify => "筑工事",
            Order::Rest => "整补",
        }
    }
}

/// 预览版功能开关。
///
/// 为什么要有它：第一版发行版已经定稿，**默认行为必须逐位不变**。
/// 新系统（弹药油料、铁路被炸、派系人事）只有在模块显式声明时才参与模拟，
/// 而且只有开启时才写进状态哈希——所以老模块的哈希序列一个字节都不变。
///
/// 这不是"难度选项"：它是**内容格式的扩展**。同一个引擎既跑定稿的旧战役，
/// 也跑用新机制写的新战役，两者的确定性互不干扰。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features {
    /// 弹药与油料：打光了真的打不动。
    pub ammo_fuel: bool,
    /// 铁路可被炸断、也会被抢修。
    pub rail_damage: bool,
    /// 派系与人事：将领的靠山会变，靠山影响他的服从。
    pub politics: bool,
}

impl Features {
    pub fn any(&self) -> bool {
        self.ammo_fuel || self.rail_damage || self.politics
    }

    /// 界面与 SDK 用的能力名（顺序稳定，别随便改）。
    pub fn names(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.ammo_fuel {
            v.push("ammo_fuel");
        }
        if self.rail_damage {
            v.push("rail_damage");
        }
        if self.politics {
            v.push("politics");
        }
        v
    }
}

/// 一个单位（师级）。
///
/// **兵力单位是千人**（`strength = 16` 表示 16000 人）。
/// 原因是 Q16.16 定点数的 i32 表示上限约 32767，直接存"人"会在
/// 多个单位求和时溢出；用千人后，即使 1000 个满编师求和也只有 20000。
/// 显示层乘 1000 即可还原。
#[derive(Clone, Debug)]
pub struct Unit {
    pub name: String,
    pub side: Side,
    /// 当前兵力（千人）。
    pub strength: Fixed,
    pub max_strength: Fixed,
    /// 所在省份。
    pub position: u32,
    /// 所在省份内的阵地索引。见 [`Position`]。
    pub node: u8,
    /// 上级番号（第1兵团 / 第11军…）。玩家发电报时的收件人。
    /// 只做展示与路由，不进状态哈希。
    pub formation: String,
    /// 补给 0-1。
    pub supply: Fixed,
    /// 士气 0-1。
    pub morale: Fixed,
    /// 本 tick 打算移动到的省份。
    pub moving_to: Option<u32>,
    /// 累计伤亡（千人，统计用，不参与战斗）。
    pub losses: Fixed,
    /// 战斗力系数。装备、训练、组织的综合体现。
    ///
    /// 这一项是必须的：没有它，战斗退化成纯兵力比拼，
    /// 而武汉会战的核心不对称恰恰是"日军少而精，国军多而杂"。
    /// 史实参考：一个日军常备师团的战斗力约相当于国军 3-5 个师。
    pub effectiveness: Fixed,
    /// 补给延伸能力 0-1。
    ///
    /// 含义：把"补给不足的差距"缩小多少。
    /// `0.0` 表示完全依赖后方补给线（纯人力挑夫运输）；
    /// `0.5` 表示能把补给缺口补上一半（汽车、铁路、就地征发）。
    ///
    /// 这是武汉会战最容易被做错的一点：日军有大规模机械化与汽车运输，
    /// 补给线拉长后的衰减远小于靠人力肩挑的国军。
    /// 用同一套衰减等于抹掉了这场战争最核心的后勤不对称。
    pub supply_reach: Fixed,
    /// 本 tick 的意图。由 AI 或玩家设置，由移动系统执行。
    pub order: Order,
    /// 将领。意图会经由他解释后才落到 `order`。
    pub commander: Commander,
    /// 单位类型。
    pub kind: UnitKind,
    /// 作战姿态。见 [`Stance`]。
    pub stance: Stance,
    /// 空军侦察半径（跳数）。仅 `UnitKind::Air` 有效。
    pub recon_radius: u32,
    /// 空军的轰炸强度（0 表示只侦察）。
    pub strike_power: Fixed,
    /// 上一 tick 是否发生了抗命/改令，供战报使用。
    pub disobeyed: bool,
    /// 命令来源：玩家亲自下的就是 true，AI 下的是 false。
    /// 没有这一位，AI 每 tick 都会覆盖玩家刚下的命令——玩家等于没在指挥。
    pub manual: bool,
    /// 抗命理由（进战报）。
    pub dissent_reason: Option<String>,
    /// 累计抗命次数。太频繁会被撤换（后续实现）。
    pub dissent_count: u32,
    /// 这支部队哪天没的（全军覆没）。`None` = 还在。
    ///
    /// 为什么要留一个"日期"而不是直接把单位从 `Vec` 里删掉：
    /// 命令流、存档、前端快照全都按**单位下标**引用部队。删一个，
    /// 后面所有下标都要挪一位，历史命令会指向别的部队，回放就废了。
    /// 所以死亡是"状态"，不是"删除"。
    pub ended_day: Option<u32>,
    /// 预定整编完成日。只有模块声明了 `reform` 的部队才会有。
    pub reform_day: Option<u32>,
    /// 上一 tick 是否处于合围。只用于"被围"战报的去重，不参与模拟。
    pub encircled: bool,
    /// 模块声明的整编天数（覆灭后隔多久能重建）。0 = 不可重建。
    pub reform_days: u32,
    /// 二次整编后的兵力比例（相对满编）。
    pub reform_ratio: Fixed,
    /// 弹药存量 0-1（1 = 一个基数满）。**只有 `Features::ammo_fuel` 开启时才变**。
    /// 老模块永远是 1.0，也不进哈希——所以定稿的行为不受影响。
    pub ammo: Fixed,
    /// 油料存量 0-1。同上。
    pub fuel: Fixed,
    /// 每动一步消耗的油料 0-1。0 = 不吃油（徒步步兵、马拉炮兵）。
    pub fuel_burn: Fixed,
}

impl Unit {
    /// 还在编成内：兵没打光，也没被判定覆灭。
    pub fn in_being(&self) -> bool {
        self.strength.raw() > 0 && self.ended_day.is_none()
    }

    /// `strength_k` 是千人。`Unit::new(.., 16)` 表示 16000 人。
    pub fn new(name: &str, side: Side, position: u32, strength_k: i32) -> Self {
        let s = Fixed::from_int(strength_k);
        Self {
            name: name.to_string(),
            side,
            strength: s,
            max_strength: s,
            position,
            node: 0,
            formation: String::new(),
            supply: Fixed::ONE,
            morale: Fixed::ONE,
            moving_to: None,
            losses: Fixed::ZERO,
            effectiveness: Fixed::ONE,
            supply_reach: Fixed::ZERO,
            order: Order::Hold,
            commander: Commander::default_for(name),
            kind: UnitKind::Infantry,
            stance: Stance::Attack,
            recon_radius: 0,
            strike_power: Fixed::ZERO,
            disobeyed: false,
            dissent_reason: None,
            manual: false,
            dissent_count: 0,
            ended_day: None,
            encircled: false,
            reform_day: None,
            reform_days: 0,
            reform_ratio: Fixed::ZERO,
            ammo: Fixed::ONE,
            fuel: Fixed::ONE,
            fuel_burn: Fixed::ZERO,
        }
    }

    /// 挂上将领。
    pub fn with_commander(mut self, c: Commander) -> Self {
        self.commander = c;
        self
    }

    /// 设置战斗力系数。
    pub fn with_effectiveness(mut self, k: f64) -> Self {
        self.effectiveness = Fixed::from_f64_input(k);
        self
    }

    /// 设置补给延伸能力（0-1）。
    pub fn with_supply_reach(mut self, k: f64) -> Self {
        self.supply_reach = Fixed::from_f64_input(k).clamp(Fixed::ZERO, Fixed::ONE);
        self
    }

    /// 兵力的"人"数，仅供显示。
    pub fn strength_men(&self) -> i32 {
        (self.strength.to_display_f64() * 1000.0) as i32
    }

    pub fn is_alive(&self) -> bool {
        self.strength.raw() > 0
    }

    /// 战斗力 = 兵力 × 士气 × 补给 × 战斗力系数。
    pub fn combat_power(&self) -> Fixed {
        self.strength * self.morale * self.supply * self.effectiveness
    }
}

/// 世界状态。
#[derive(Clone, Debug)]
pub struct World {
    pub tick: u32,
    pub seed: u64,
    pub provinces: Vec<Province>,
    pub units: Vec<Unit>,
    /// 中方对各省的情报视图。**不是真实状态**，由侦察生成。
    pub intel_cn: Vec<ProvinceIntel>,
    /// 日方对各省的情报视图。
    pub intel_jp: Vec<ProvinceIntel>,
    /// 中方的"在路上"报告。
    pub pending_cn: Vec<PendingReport>,
    /// 日方的"在路上"报告。
    pub pending_jp: Vec<PendingReport>,
    /// 通信能力配置。由模块决定，引擎不认识年代。
    pub comms: CommsConfig,
    /// 模块声明的规则集。用 Arc 避免每次 clone 世界时深拷贝。
    pub rules: std::sync::Arc<crate::rules::RuleSet>,
    /// 战报。只进人类可读日志，不进状态哈希。
    pub log: Vec<Event>,
    /// 模块声明的编制：兵团（实体，不只是字符串）。
    pub formations: Vec<Formation>,
    /// 预览版功能开关。全部关闭 = 第一版发行版的定稿行为。
    pub fx: Features,
    /// 派系（预览版）。模块不声明就永远是空的，也不进哈希。
    pub factions: Vec<crate::politics::Faction>,
    /// 战役长度（天）。到日子就判结局。
    pub campaign_days: u32,
    /// 模块声明的结局规则，按 priority 判。
    pub endings: Vec<crate::endings::Ending>,
    /// 已经判出的结局。一旦有值，这一局就结束了。
    pub outcome: Option<crate::endings::Outcome>,
    /// 还在路上的战报：(送达 tick, 事件)。
    ///
    /// 为什么要有它：指挥链是有距离的。一支部队在几百公里外全军覆没，
    /// 长官部**不可能当天就知道**——电报要走、参谋要核对、再往上呈。
    /// 之前所有战报都是即时进日志，等于把电台架在了每个连部。
    /// 进状态哈希没有意义（它只影响"什么时候看到"，不影响世界），
    /// 但它决定了玩家的时间感，所以必须存在。
    pub in_transit: Vec<(u32, Event)>,
}

impl World {
    pub fn new(seed: u64, provinces: Vec<Province>, units: Vec<Unit>) -> Self {
        let n = provinces.len();
        Self {
            tick: 0,
            seed,
            provinces,
            units,
            intel_cn: vec![ProvinceIntel::unknown(); n],
            intel_jp: vec![ProvinceIntel::unknown(); n],
            pending_cn: Vec::new(),
            pending_jp: Vec::new(),
            comms: CommsConfig::default(),
            rules: std::sync::Arc::new(crate::rules::RuleSet::default()),
            log: Vec::new(),
            in_transit: Vec::new(),
            formations: Vec::new(),
            fx: Features::default(),
            factions: Vec::new(),
            campaign_days: u32::MAX,
            endings: Vec::new(),
            outcome: None,
        }
    }

    /// 打开模块声明的预览版功能。**默认全关**，关着就是定稿行为。
    pub fn with_features(mut self, fx: Features) -> Self {
        self.fx = fx;
        self
    }

    /// 某方"在路上"的报告队列。
    pub fn pending_mut(&mut self, side: Side) -> &mut Vec<PendingReport> {        match side {
            Side::Cn => &mut self.pending_cn,
            Side::Jp => &mut self.pending_jp,
        }
    }

    /// 某方"在路上"的报告数。用于界面提示"有 N 份战报未到"。
    pub fn pending_count(&self, side: Side) -> usize {
        match side {
            Side::Cn => self.pending_cn.len(),
            Side::Jp => self.pending_jp.len(),
        }
    }

    /// 某方的情报视图（只读）。
    pub fn intel(&self, side: Side) -> &[ProvinceIntel] {
        match side {
            Side::Cn => &self.intel_cn,
            Side::Jp => &self.intel_jp,
        }
    }

    /// 某方的情报视图（可写）。
    pub fn intel_mut(&mut self, side: Side) -> &mut [ProvinceIntel] {
        match side {
            Side::Cn => &mut self.intel_cn,
            Side::Jp => &mut self.intel_jp,
        }
    }

    /// 某方对某省的敌方兵力**估计**（不是真值）。
    ///
    /// 这是 AI 与玩家应该使用的接口。真实兵力只有战斗接触后才知道。
    pub fn enemy_estimate(&self, side: Side, province: u32) -> Fixed {
        match self.intel(side).get(province as usize) {
            Some(i) => i.enemy_strength,
            None => Fixed::ZERO,
        }
    }

    /// 某省内的单位索引（有序）。
    pub fn units_in(&self, province: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for (i, u) in self.units.iter().enumerate() {
            if u.position == province && u.is_alive() {
                out.push(i as u32);
            }
        }
        out
    }

    /// 某省某方的总战斗力。
    pub fn power_at(&self, province: u32, side: Side) -> Fixed {
        let mut total = Fixed::ZERO;
        for &i in &self.units_in(province) {
            let u = &self.units[i as usize];
            if u.side == side {
                total += u.combat_power();
            }
        }
        total
    }

    /// 某省某方的总兵力（不含士气、补给、战斗力系数）。
    ///
    /// 情报里存的应该是这个，不是 `power_at`。否则玩家看到的是
    /// "战斗力 70 千"，而实际上日军只有 37 千兵——这会让估算系统
    /// 变得没法理解。
    pub fn strength_at(&self, province: u32, side: Side) -> Fixed {
        let mut total = Fixed::ZERO;
        for &i in &self.units_in(province) {
            let u = &self.units[i as usize];
            if u.side == side && u.is_alive() {
                total += u.strength;
            }
        }
        total
    }

    /// 某方总兵力。
    pub fn total_strength(&self, side: Side) -> Fixed {
        let mut total = Fixed::ZERO;
        for u in &self.units {
            if u.side == side && u.is_alive() {
                total += u.strength;
            }
        }
        total
    }

    /// 这支部队所属的兵团（若模块声明了编制）。
    pub fn formation_of(&self, idx: usize) -> Option<&Formation> {
        self.formations.iter().find(|f| f.units.contains(&idx))
    }

    /// 兵团部还在不在？
    ///
    /// `None` = 纸面番号（没有兵团部这个单位），既不加成也不惩罚。
    pub fn hq_alive(&self, idx: usize) -> Option<bool> {
        let f = self.formation_of(idx)?;
        let hq = f.hq?;
        Some(self.units.get(hq).map(|u| u.in_being()).unwrap_or(false))
    }

    /// 实际服从度：编制完整时略高，兵团部被打掉后明显更低。
    ///
    /// 为什么要动它：`obedience` 是抗命的唯一硬约束（规则 AI 与 LLM 提示词都用它）。
    /// 兵团部在，参谋部就压得住下面的师长；兵团部没了，师长就只听自己的。
    /// 这条不需要新增机制，只是让已有的 `obedience` 反映编制状态。
    pub fn effective_obedience(&self, idx: usize) -> Fixed {
        let base = self
            .units
            .get(idx)
            .map(|u| u.commander.obedience)
            .unwrap_or(Fixed::ONE);
        match self.hq_alive(idx) {
            Some(true) => (base + Fixed::ratio(5, 100)).clamp(Fixed::ZERO, Fixed::ONE),
            Some(false) => (base - Fixed::ratio(15, 100)).clamp(Fixed::ZERO, Fixed::ONE),
            None => base,
        }
    }

    /// 世界状态哈希。**字段顺序即协议**，改动要慎重（会让旧回放失效）。
    pub fn state_hash(&self) -> u64 {
        let mut h = Hasher::new();

        h.write_u32(self.tick);
        h.write_u64(self.seed);

        h.write_u32(self.provinces.len() as u32);
        for p in &self.provinces {
            h.write_str(&p.name);
            h.write_u32(p.terrain.code());
            h.write_u32(p.fort as u32);
            h.write_i32(p.supply_throughput.raw());
            h.write_u32(p.owner.index());
            h.write_u32(p.is_supply_source as u32);
            h.write_u32(p.neighbors.len() as u32);
            for &n in &p.neighbors {
                h.write_u32(n);
            }
        }

        h.write_u32(self.units.len() as u32);
        for u in &self.units {
            h.write_str(&u.name);
            h.write_u32(u.side.index());
            h.write_i32(u.strength.raw());
            h.write_i32(u.max_strength.raw());
            h.write_u32(u.position);
            h.write_i32(u.supply.raw());
            h.write_i32(u.morale.raw());
            h.write_u32(u.moving_to.map(|v| v + 1).unwrap_or(0));
            h.write_i32(u.losses.raw());
            h.write_i32(u.effectiveness.raw());
            h.write_i32(u.supply_reach.raw());
            let (oc, ot) = u.order.code();
            h.write_u32(oc);
            h.write_u32(ot);
            h.write_str(&u.commander.name);
            h.write_i32(u.commander.obedience.raw());
            h.write_i32(u.commander.initiative.raw());
            h.write_i32(u.commander.aggression.raw());
            h.write_i32(u.commander.caution.raw());
            h.write_u32(u.kind.code());
            h.write_u32(u.recon_radius);
            h.write_i32(u.strike_power.raw());
            h.write_u32(u.disobeyed as u32);
            h.write_u32(u.dissent_count);
            // 阵地与命令归属也改变结果（阵地决定战斗宽度，manual 决定谁在下令），
            // 不进哈希的话两个不同状态会算出同一个哈希，回放校验就形同虚设。
            h.write_u32(u.node as u32);
            h.write_u32(u.manual as u32);
            // 编制变动也要进哈希：覆灭日决定"还能不能被问策、能不能整编"，
            // 将领结局决定"这支部队还重建得起来吗"。不同结局必须算出不同哈希。
            h.write_u32(u.ended_day.map(|d| d + 1).unwrap_or(0));
            h.write_u32(u.reform_day.map(|d| d + 1).unwrap_or(0));
            h.write_i32(u.reform_ratio.raw());
            h.write_u32(u.commander.fate.code());
        }

        // 情报也该进哈希：它会影响 AI 决策，不哈希就抓不到确定性漂移。
        for intel in [&self.intel_cn, &self.intel_jp] {
            h.write_u32(intel.len() as u32);
            for i in intel {
                h.write_u32(i.accuracy.code());
                h.write_i32(i.enemy_strength.raw());
                h.write_u32(i.seen_tick);
                h.write_u32(i.identity_known as u32);
            }
        }
        for pending in [&self.pending_cn, &self.pending_jp] {
            h.write_u32(pending.len() as u32);
            for r in pending {
                h.write_u32(r.arrive_tick);
                h.write_u32(r.province);
                h.write_i32(r.enemy_strength.raw());
                h.write_u32(r.accuracy.code());
            }
        }

        // ---- 预览版状态：只在模块开启对应功能时才写 ----
        //
        // 这是"定稿的第一版不受影响"的关键：老模块（fx 全关）走不到下面任何一行，
        // 哈希字节流与 0.11.0 完全一致，旧回放、旧存档、黄金哈希都还成立。
        if self.fx.ammo_fuel {
            for u in &self.units {
                h.write_i32(u.ammo.raw());
                h.write_i32(u.fuel.raw());
            }
        }
        if self.fx.rail_damage {
            for p in &self.provinces {
                h.write_u8(p.rail_damage);
                h.write_u8(p.rail_repair);
            }
        }
        if self.fx.politics {
            crate::politics::hash_into(&mut h, self);
        }
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_world() -> World {
        let provinces = vec![
            Province::new("A", Terrain::Plain, Side::Cn)
                .with_neighbors(&[1])
                .as_source(),
            Province::new("B", Terrain::Hill, Side::Cn).with_neighbors(&[0]),
        ];
        let units = vec![Unit::new("1st", Side::Cn, 0, 1000)];
        World::new(7, provinces, units)
    }

    #[test]
    fn hash_stable() {
        let w = tiny_world();
        assert_eq!(w.state_hash(), w.state_hash());
    }

    #[test]
    fn hash_changes_with_state() {
        let mut w = tiny_world();
        let before = w.state_hash();
        w.units[0].strength -= Fixed::from_int(1);
        assert_ne!(before, w.state_hash());
    }

    #[test]
    fn units_in_filters_by_position() {
        let w = tiny_world();
        assert_eq!(w.units_in(0).len(), 1);
        assert_eq!(w.units_in(1).len(), 0);
    }

    #[test]
    fn defense_scales_with_terrain_and_fort() {
        let plain = Province::new("p", Terrain::Plain, Side::Cn);
        let mountain = Province::new("m", Terrain::Mountain, Side::Cn);
        assert!(mountain.defense_multiplier() > plain.defense_multiplier());

        let fortified = Province::new("f", Terrain::Mountain, Side::Cn).with_fort(5);
        assert!(fortified.defense_multiplier() > mountain.defense_multiplier());
    }
}









