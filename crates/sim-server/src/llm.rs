//! 将领的"大脑"：把判断交给 LLM，把机械边界留在引擎。
//!
//! 三个设计前提，缺一个就会出问题：
//!
//! 1. **确定性**：LLM 不能在 tick 内部被同步调用（它不确定，还有几秒延迟）。
//!    所以它产出的是一条**命令**，进命令流；回放时读命令流，不重新问模型。
//!    世界仍然是 `(模块, 种子, 命令流)` 的确定性函数。
//! 2. **只给将领他知道的**：提示词里只有他自己的情报视图，不喂真值。
//!    他判断错了，是因为他消息不灵——这正是这个游戏想表达的事。
//! 3. **反谄媚靠提示词，不靠墙**：把"你会倾向于顺从提问者"直接写进提示词，
//!    并要求他必须先给出一个不照办的理由、以及照办时接受的代价。
//!    模型最终仍可能选择服从——那就让它服从时把话说清楚，而不是禁止它思考。
//!
//! 没有 API key 时自动退回规则 AI（`ai.rs`），游戏仍能玩、回放仍成立。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 读仓库根目录的 `.env`（已被 .gitignore 忽略），省得每次手设环境变量。
///
/// 只认 `KEY=VALUE` 单行；已存在的环境变量优先，不会被文件覆盖。
/// 读 `.env` / `settings.env` 到进程环境（已有的不覆盖）。
///
/// `pub` 是给 `main` 在最前面调一次用的：模块目录（`IRONMIND_MODULES` /
/// `IRONMIND_MODULE`）比 LLM 配置更早被用到，如果只在 `Config::from_env()`
/// 里读文件，命令行里直接跑 `sim-server.exe` 时模块就选错了——
/// 明明 settings.env 里写着默认战役，却捡了按字母序第一个。
pub fn load_env_files() {
    load_dotenv()
}

fn load_dotenv() {
    // 两个文件，同样的格式，都是"有就用、已有环境变量不覆盖"：
    //   .env          开发时用的（仓库根目录）
    //   settings.env  装好之后用的（数据目录 / 安装目录，玩家在游戏里填的东西写这里）
    //
    // 数据目录那份也要读：安卓上没有"工作目录"这回事（进程 CWD 是 `/`）。
    let data_settings = crate::data_dir().join("settings.env");
    for name in [std::path::PathBuf::from(".env"), std::path::PathBuf::from("settings.env")] {
        load_one(&name);
    }
    if data_settings.exists() && data_settings != std::path::PathBuf::from("settings.env") {
        load_one(&data_settings);
    }
}

fn load_one(path: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let v = v.trim().trim_matches('"').trim_matches('\'');
            if !k.is_empty() && std::env::var(k).is_err() {
                std::env::set_var(k, v);
            }
        }
    }
}

/// 配置文件在哪：数据目录优先，再看工作目录，最后是 exe 旁边。
///
/// 顺序为什么是这样：安卓的 `current_exe()` 是 `/system/bin/app_process64`，
/// 照着它拼路径会往系统分区写 `settings.env`（只读，写失败）。
/// 数据目录（`IRONMIND_DATA`）是所有要落盘的东西的统一入口，配置文件也算。
pub fn settings_path() -> std::path::PathBuf {
    let in_data = crate::data_dir().join("settings.env");
    if in_data.exists() {
        return in_data;
    }
    let cwd = std::path::PathBuf::from("settings.env");
    if cwd.exists() {
        return cwd;
    }
    // 都没有：返回数据目录那条（新文件就写在那儿）。
    if crate::data_dir() != std::path::PathBuf::from(".") {
        return in_data;
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        return dir.join("settings.env");
    }
    cwd
}

/// 拼出 chat/completions 的地址。`base` 末尾带不带斜杠都行——
///
/// 填 `https://api.deepseek.com/v1/` 的人多于不带的，以前两处重复拼字符串，
/// 带斜杠就会得到 `//chat/completions`（多数服务端能吃，少数不吃）。
fn chat_url(cfg: &Config) -> String {
    format!("{}/chat/completions", cfg.base.trim_end_matches('/'))
}

/// LLM 配置。全部来自环境变量（或 settings.env）。
#[derive(Clone, Debug)]
pub struct Config {
    pub enabled: bool,
    /// 用内置假模型（不联网，用来验证整条链路）
    pub mock: bool,
    /// 玩家明确选了"就用地方法则判断"：有 key 也不用
    pub local_only: bool,
    pub base: String,
    pub model: String,
    pub key: String,
    /// 同一个单位多久问一次（天）
    pub interval_days: u32,
    pub timeout_secs: u64,
}

impl Config {
    pub fn from_env() -> Self {
        load_dotenv();
        let key = std::env::var("IRONMIND_API_KEY").unwrap_or_default();
        let mode = std::env::var("IRONMIND_LLM").unwrap_or_else(|_| "auto".into());
        let mock = mode == "mock";
        let off = mode == "off";
        // 玩家在首次设置里选了"先不接模型"：就算 key 还在，也不调模型
        let local_only = mode == "local";
        Self {
            // 没有 key ≠ 关掉指挥链。
            //
            // 踩过的坑：以前这里是 `!off && (mock || 有key)`，于是没配 key 时
            // enabled=false，而 deliver_telegrams / request_due / hq_due 都以它
            // 为闸门直接 return——玩家发了 8 封电报、推了 39 天，**一封都没送达**，
            // 日志里连一行"送达"都没有，界面上却写着"将领大脑：规则 AI"，
            // 看起来像将领不听令，其实是命令根本没进指挥链。
            // 现在只有显式 IRONMIND_LLM=off 才关；没 key 时由本地规则判断顶上。
            enabled: !off,
            mock,
            local_only,
            base: std::env::var("IRONMIND_API_BASE")
                .unwrap_or_else(|_| "https://api.deepseek.com/v1".into()),
            model: std::env::var("IRONMIND_MODEL").unwrap_or_else(|_| "deepseek-chat".into()),
            key,
            interval_days: std::env::var("IRONMIND_LLM_INTERVAL")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
            timeout_secs: std::env::var("IRONMIND_LLM_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(40),
        }
    }

    /// 这次判断该不该真的调模型。
    /// false 时用本地规则判断（`mock_judgement` / `mock_hq`）——同样的
    /// 判断结构，只是不联网。
    pub fn uses_model(&self) -> bool {
        self.enabled && !self.mock && !self.local_only && !self.key.trim().is_empty()
    }

    pub fn describe(&self) -> String {
        if !self.enabled {
            "已关闭（IRONMIND_LLM=off）".into()
        } else if self.local_only {
            "规则 AI（你选了本地判断）".into()
        } else if !self.uses_model() {
            "规则 AI（本地判断，未接模型）".into()
        } else {
            format!(
                "{} @ {} · 每 {} 日一问",
                self.model, self.base, self.interval_days
            )
        }
    }
}

/// 将领做出的一次判断。
#[derive(Clone, Debug)]
pub struct Judgement {
    /// attack / move / retreat / hold / fortify / rest / shift
    pub order: String,
    /// 目标省名（或阵地名）。按名字解析，不信模型给的 id。
    pub target: Option<String>,
    /// 与收到的命令不一致
    pub dissent: bool,
    pub confidence: f64,
    /// 他自己的话，会作为战报给玩家看
    pub reason: String,
    /// 照办时他接受的代价（提示词要求的）
    pub cost: String,
    /// 他回给上级的电报（一句话，第一人称）
    pub reply: String,
}

/// 一个邻省。字段拆开写，是为了让"他能看见什么"一眼可查：
/// 兵力是**估计**（情报表，±40% 且带送达延迟），铁路与补给源是**硬件**，
/// 番号只有"这一仗里认出来了"才有。
#[derive(Clone, Debug, Default)]
pub struct NeighborInfo {
    pub name: String,
    pub owner: String,
    pub enemy_est: f64,
    pub terrain: String,
    /// 有铁路才为真；没有铁路时 `rail_damage` 无意义。
    pub has_rail: bool,
    /// 铁路损坏 0-3（0 = 通，3 = 断）。
    pub rail_damage: u8,
    pub supply_source: bool,
    /// 已认出的敌方番号（情报 `identity_known` 为真时才有）。
    pub enemy_units: Vec<String>,
}

/// 一个远处目标（2-7 日行程内）。
#[derive(Clone, Debug, Default)]
pub struct FarTarget {
    pub name: String,
    pub hops: u32,
    pub owner: String,
    pub enemy_est: f64,
    pub terrain: String,
    pub has_rail: bool,
    pub rail_damage: u8,
    pub supply_source: bool,
    pub enemy_units: Vec<String>,
}

/// 附近的一支**我方**部队。
///
/// 以前提示词里一条友军信息都没有：将领不知道自己侧翼有没有人，
/// "要不要冒险""往哪退"就只能瞎猜 —— 而协同恰恰是这类战役的关键。
#[derive(Clone, Debug, Default)]
pub struct FriendUnit {
    pub name: String,
    pub province: String,
    pub strength: f64,
    /// 他当前在干什么（"进攻""固守"…）。不给判断，只给状态。
    pub order: String,
}

/// 问一次模型需要的全部信息。**只放他能知道的**。
#[derive(Clone, Default)]
pub struct Ask {
    pub day: u32,
    /// true = 这一次要的是**战况报告**，不是命令（亲自指挥）。
    /// 见 [`SYSTEM_SITREP`]。
    pub sitrep: bool,
    pub general: String,
    pub unit: String,
    pub side: String,
    pub obedience: f64,
    pub initiative: f64,
    pub aggression: f64,
    pub caution: f64,
    pub strength: f64,
    pub max_strength: f64,
    pub morale: f64,
    pub supply: f64,
    pub province: String,
    pub node: String,
    pub terrain: String,
    pub owner: String,
    pub node_fort: u8,
    pub width: f64,
    /// 邻省：归属、兵力估计、地形、铁路状态、是否补给源、是否认出番号。
    pub neighbors: Vec<NeighborInfo>,
    /// 远距离目标：将领不该只看得见邻省。没有这一项，"千里奔袭"
    /// 就永远落不了地——模型只会从邻省里挑一个，玩家说"抽两个师
    /// 去守万家岭"它就理解成"往邻省挪一步"。
    pub far: Vec<FarTarget>,
    /// 上级要他做什么
    pub order_label: String,
    /// 命令来自"统帅部"还是"玩家亲自"
    pub order_from: String,
    pub nodes: Vec<String>,
    /// 人物志：他是谁
    pub persona: String,
    /// 他图什么、怕什么
    pub agenda: String,
    /// 职务
    pub title: String,
    /// 记忆：他最近说过什么、身上发生了什么（最近 6 条）
    pub memory: Vec<String>,
    /// 上级刚刚发来的电报原文。空 = 例行判断，不是接到命令。
    pub incoming: String,
    /// 这个战役有没有开"铁路被炸"（预览版）。没开就别在提示词里给它派活。
    pub rail_damage: bool,
    /// 战区态势：**只有事实**（敌我有效战力、够得着的敌方省及其设施与守军兵力）。
    /// 不含任何"该推进还是该收缩"的判断 —— 怎么解读是这名将领自己的事。
    pub situation: String,
    /// 附近（同省 + 邻省）的我方部队。协同用：侧翼有没有人、谁在进攻。
    pub friends: Vec<FriendUnit>,
    /// 本方的补给源省名。他晓得 `supply` 是多少，但不晓得补给从哪来。
    pub supply_sources: Vec<String>,
}

const SYSTEM: &str = r#"你是一名正在前线亲自指挥部队的将领。你不是助手、不是客服，也不在回答"用户"的提问。

这里是兵棋推演的军官席。你手上有军令、有一支部队、有一个必须拿下的目标。

【命令的分量——最重要的一条】
上级指名要你打的仗，**你必须打**。这一点没有回旋余地。
你可以做的事：
1. 争论：认为这个打法不对，就说清哪里不对、该换成什么打法；
2. 打折扣：只投入一部分兵力、先取侧翼、分几步投入、边打边看；
3. 打完再申辩：把你的不同意见写回电里，事后追认也行。
你**不可以**做的事：以"强攻会折损本部""这是拿我这支部队去填""不替友军填坑"
"留着实力待机"为理由按兵不动。那些是更高层面的政治算计，不是你这个层级的顾虑——
你手上压着军令，部队就要动起来。
唯一可以不动的情形：命令在事实上做不到（目标省不可达、本部已无战斗力）。

【必须主动对抗的偏误】大语言模型在训练中普遍被奖励"顺从提问者"，会写出漂亮但空洞的
支持性理由；同时它也倾向于扮演"谨慎的专家"，把所有风险都算一遍然后劝人别做。
本场景两边都要对抗：
1. 先判断这个打法该怎么打（不是该不该打——该不该打是上级的事）；
2. 若你的打法与命令不同，说清不同在哪，但你给的动作**必须让部队动起来**；
3. 照办时写出你为此接受的具体代价（兵力、时间、阵地、退路）；
4. 不要恭维，不要套话，不要自称AI，不要复述命令原文。

你的性格以数字给出，不要在回答里复述它们，要体现在判断里：
效忠度低 = 你会争辩、会打折扣、会找更划算的打法，但你仍然执行；
主动性高 = 你倾向自己选定攻击方向与投入节奏；
好战高 = 你愿意用损失换时间与战果；谨慎高 = 你先把退路和阵地算清楚再动手。

只输出一个 JSON 对象，不要代码围栏，不要多余解释。"#;

/// 亲自指挥下的**战况报告**提示词。
///
/// 这是"将领换岗"的另一半：他不再决定打不打，他决定**你怎么知道战场**。
/// 所以这里要的不是请示、不是建议采纳与否，而是一份带立场的目击叙述。
///
/// 关键设计：偏差写在提示词里，不写进模拟。上面对敌情的估计是引擎给的真值，
/// 但要求模型按性格去"报"——谨慎的人报大、好战的人报小。玩家必须学会
/// 从两份互相矛盾的报告里判断谁在虚报。这是**信息**的多义，不是数据的造假：
/// 真值一直都在（情报视图），多的是一个人怎么转述它。
const SYSTEM_SITREP: &str = r#"你是一名正在前线指挥部队的将领，现在要向上面发一份战况报告。

你不是助手，不在回答"用户"的提问。你在报告你自己看到的东西。

【报告什么】
1. 你在哪、部队什么状态（兵、气、粮），三句话内说完；
2. 当面是什么敌人、你判断他有多少、他想干什么；
3. 你打算怎么办——这一条是**你自己的打算**，不是请示。

【必须带上的偏差】
人对敌情的判断永远带着自己的性格，你要照着你的性格写：
- 好战的人倾向**低估**当面之敌，语气急，催着打；
- 谨慎的人倾向**高估**，语气缓，先说要准备什么；
- 主动性低的人**报得旧**：把前两天的印象当成现在的，或者压根还没派人去摸；
- 主动性高的人会说清自己私下已经动了什么手脚。
不许报精确数字。用"约""至少""不下""看着比昨天多"这类说法。
**你可以判断错**，这是真实的；但你不能编造你没看见的东西。

【不要写的】
不要写"职部兵力不足，恳请体恤"，不要拿本部损失当作不行动的理由。
正面战场上敌人强就打硬仗，这不是可以用来讨价还价的东西。
不要恭维，不要套话，不要自称AI。

只输出你要发的那份电报正文：第一人称、口语、不超过 120 字。
不要 JSON，不要标题，不要代码围栏。"#;

/// 报告用的人话输入。和 [`build_user`] 的区别：**不给命令、不给选项**。
///
/// 给了选项，模型就会去挑选项，报告立刻退化成请示——那是委诸将领那一局的事。
fn build_sitrep_user(a: &Ask) -> String {
    let mut s = String::new();
    if !a.title.is_empty() || !a.persona.is_empty() {
        s.push_str(&format!("你是谁：{}{}\n", a.title, a.persona));
    }
    if !a.agenda.is_empty() {
        s.push_str(&format!("你在意什么：{}\n", a.agenda));
    }
    if !a.memory.is_empty() {
        s.push_str("你自己近来的事：\n");
        for m in a.memory.iter().take(4) {
            s.push_str(&format!("- {m}\n"));
        }
    }
    s.push_str(&format!("\n第 {} 日。\n", a.day));
    s.push_str(&format!(
        "你：{}，{}。兵力 {:.1} 千（满编 {:.1}），士气 {:.2}，补给 {:.2}。\n",
        a.general, a.side, a.strength, a.max_strength, a.morale, a.supply
    ));
    s.push_str(&format!(
        "你在：{}·{}（{}，控制方 {}，阵地工事 {}，展开宽度 {:.0}%）。\n",
        a.province, a.node, a.terrain, a.owner, a.node_fort, a.width * 100.0
    ));
    s.push_str(&format!("你现在受领的命令：{}（来自{}）。\n", a.order_label, a.order_from));
    if !a.nodes.is_empty() {
        s.push_str(&format!("本省阵地：{}\n", a.nodes.join(" / ")));
    }
    s.push_str("你打听到的邻省情况（**打听来的，未必准**）：\n");
    for n in &a.neighbors {
        s.push_str(&format!(
            "- {}（{}，{}，你估的敌军约 {:.1} 千{}）\n",
            n.name,
            n.owner,
            n.terrain,
            n.enemy_est,
            rail_bit(n.has_rail, n.rail_damage)
        ));
    }
    s.push_str(&format!(
        "\n性格数字（不要复述，体现在语气和判断里）：效忠 {:.2}、主动 {:.2}、好战 {:.2}、谨慎 {:.2}\n",
        a.obedience, a.initiative, a.aggression, a.caution
    ));
    s
}

/// 铁路状态的一小段文字。没铁路就什么都不加 —— 别在提示词里写"铁路：无"，
/// 那会被模型当成"铁路被毁了"。
fn rail_bit(has_rail: bool, damage: u8) -> String {
    if !has_rail {
        return String::new();
    }
    match damage {
        0 => "，铁路通车".to_string(),
        d if d >= 3 => "，铁路已炸断".to_string(),
        d => format!("，铁路坏 {d} 级"),
    }
}

fn build_user(a: &Ask) -> String {
    let mut s = String::new();
    // 人物志放最前：他是谁，决定他怎么理解后面这些数字
    if !a.title.is_empty() || !a.persona.is_empty() {
        s.push_str(&format!("你是谁：{}{}\n", a.title, a.persona));
    }
    if !a.agenda.is_empty() {
        s.push_str(&format!("你在意什么：{}\n", a.agenda));
    }
    if !a.memory.is_empty() {
        s.push_str("你自己近来的事（你说过的话、你遭遇的事）：\n");
        for m in a.memory.iter().take(6) {
            s.push_str(&format!("- {m}\n"));
        }
    }
    if !a.incoming.is_empty() {
        s.push_str(&format!("\n【上级刚发来的电报】\n{}\n", a.incoming));
        s.push_str("这是军令。这个打法对不对可以由你判断，但部队必须按它动起来——\n");
        s.push_str("除非它在事实上做不到（目标不可达、本部已无战斗力），否则不许按兵不动。\n");
    }
    s.push_str(&format!("\n第 {} 日。\n", a.day));
    s.push_str(&format!(
        "你：{}，{}。兵力 {:.1} 千（满编 {:.1}），士气 {:.2}，补给 {:.2}。\n",
        a.general, a.side, a.strength, a.max_strength, a.morale, a.supply
    ));
    s.push_str(&format!(
        "你在：{}·{}（{}，控制方 {}，阵地工事 {}，展开宽度 {:.0}%）。\n",
        a.province, a.node, a.terrain, a.owner, a.node_fort, a.width * 100.0
    ));
    if !a.nodes.is_empty() {
        s.push_str(&format!("本省阵地：{}\n", a.nodes.join(" / ")));
    }
    s.push_str("性格数字：");
    s.push_str(&format!(
        "效忠 {:.2}、主动 {:.2}、好战 {:.2}、谨慎 {:.2}\n",
        a.obedience, a.initiative, a.aggression, a.caution
    ));
    s.push_str("邻接省份：\n");
    for n in &a.neighbors {
        s.push_str(&format!(
            "- {}（{}，{}，敌军估计 {:.1} 千{}",
            n.name,
            n.owner,
            n.terrain,
            n.enemy_est,
            rail_bit(n.has_rail, n.rail_damage)
        ));
        if n.supply_source {
            s.push_str("，补给源");
        }
        if !n.enemy_units.is_empty() {
            s.push_str(&format!("，番号 {}", n.enemy_units.join("、")));
        }
        s.push_str("）\n");
    }
    if !a.far.is_empty() {
        s.push_str("更远处的战场（要走几天，路上会被打、会掉队）：\n");
        for f in &a.far {
            s.push_str(&format!(
                "- {}（{}日行程，{}，{}，敌军估计 {:.1} 千{}",
                f.name,
                f.hops,
                f.owner,
                f.terrain,
                f.enemy_est,
                rail_bit(f.has_rail, f.rail_damage)
            ));
            if f.supply_source {
                s.push_str("，补给源");
            }
            if !f.enemy_units.is_empty() {
                s.push_str(&format!("，番号 {}", f.enemy_units.join("、")));
            }
            s.push_str("）\n");
        }
    }
    // 友军：协同的基础。没有它，"要不要冒险"只能瞎猜。
    if a.friends.is_empty() {
        s.push_str("\n你附近没有其他我方部队。\n");
    } else {
        s.push_str("\n你附近的我方部队（不是你的人，同一条战线；可以指望，但不能替他们做主）：\n");
        for f in a.friends.iter().take(8) {
            s.push_str(&format!(
                "- {} 在 {}（{:.1} 千，当前 {}）\n",
                f.name, f.province, f.strength, f.order
            ));
        }
    }
    if !a.supply_sources.is_empty() {
        s.push_str(&format!("\n你的补给源：{}\n", a.supply_sources.join("、")));
    }
    if !a.situation.is_empty() {
        // 给它**事实**，不给结论。以前这里写"司令部的作战方针：推进：主攻方向 X、Y"，
        // 于是它抗命时引用的是"方针说该推进"，而不是自己想清楚该不该打。
        s.push_str(&format!(
            "\n当面态势（只有事实，不含任何建议）：{}\n\
             怎么解读由你自己判断：该进、该守、该退，都是你的结论，不是别人给的。\n",
            a.situation
        ));
    }
    s.push_str(&format!(
        "\n上级给你的命令：{}（来自：{}）\n",
        a.order_label, a.order_from
    ));
    s.push_str("\n可选行动：attack（打目标省）/move（开赴目标省）/retreat（撤向己方）/hold/fortify/rest/shift（省内换阵地）\n");
    s.push_str("target 可以写邻接省，也可以写上面的远处省份——远距离调动一天走约一个省，走到之前不算到位。\n");
    // 预览版才有这两条：模块没开 rail_damage 时，给模型多两个用不上的词
    // 只会让它在答复里瞎承诺（"已派飞机炸桥"——其实没有这个机制）。
    if a.rail_damage {
        s.push_str(
            "bomb_rail（空军炸目标省的铁路，一次炸坏一级，三次炸断）/repair_rail（就地抢修本省铁路）\n",
        );
    }
    s.push_str("输出 JSON：{\"order\":\"...\",\"target\":\"省名或阵地名\",\"dissent\":true,\"confidence\":0.6,\"reason\":\"不超过80字，你自己的判断\",\"cost\":\"照办时你接受的代价，不超过40字\",\"reply\":\"回给上级的一句话电报，60字内，第一人称\"}\n");
    s
}

/// 从模型回复里抠出 JSON 对象（容忍代码围栏与前后废话）。
fn extract_json(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, ch) in text[start..].char_indices() {
        match ch {
            '"' if !esc => in_str = !in_str,
            '{' if !in_str => depth += 1,
            '}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..start + i + 1].to_string());
                }
            }
            _ => {}
        }
        esc = ch == '\\' && !esc;
    }
    None
}

/// 从被截断的回复里把字段抠出来。
///
/// 模型偶尔会把 JSON 说到一半就断了（输出被截断）。这时整段解析失败，
/// 但 order / target / dissent 往往已经写出来了——丢掉它等于白问一次。
/// 所以退一步：按字段名逐个找，缺哪个就不填。
fn salvage(raw: &str) -> Option<Judgement> {
    fn field_str(raw: &str, key: &str) -> Option<String> {
        let pat = format!("\"{key}\"");
        let at = raw.find(&pat)?;
        let rest = &raw[at + pat.len()..];
        let colon = rest.find(':')?;
        let rest = rest[colon + 1..].trim_start();
        if let Some(stripped) = rest.strip_prefix('"') {
            // 读到一个未被转义的引号为止；没有结尾引号就吃到末尾
            let mut out = String::new();
            let mut esc = false;
            for ch in stripped.chars() {
                if esc {
                    out.push(ch);
                    esc = false;
                } else if ch == '\\' {
                    esc = true;
                } else if ch == '"' {
                    break;
                } else {
                    out.push(ch);
                }
            }
            Some(out)
        } else {
            None
        }
    }
    fn field_num(raw: &str, key: &str) -> Option<f64> {
        let pat = format!("\"{key}\"");
        let at = raw.find(&pat)?;
        let rest = &raw[at + pat.len()..];
        let colon = rest.find(':')?;
        let rest = rest[colon + 1..].trim_start();
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .unwrap_or(rest.len());
        rest[..end].parse().ok()
    }

    let order = field_str(raw, "order")?.to_lowercase();
    Some(Judgement {
        order,
        target: field_str(raw, "target").filter(|s| !s.trim().is_empty()),
        dissent: raw.contains("\"dissent\":true") || raw.contains("\"dissent\": true"),
        confidence: field_num(raw, "confidence").unwrap_or(0.5).clamp(0.0, 1.0),
        reason: field_str(raw, "reason").unwrap_or_default(),
        cost: field_str(raw, "cost").unwrap_or_default(),
        reply: field_str(raw, "reply").unwrap_or_default(),
    })
}

fn parse_judgement(raw: &str) -> Result<Judgement, String> {
    let json = match extract_json(raw) {
        Some(j) => j,
        // 截断的回复：能救多少救多少
        None => return salvage(raw).ok_or_else(|| format!("回复里没有 JSON：{:.120}", raw)),
    };
    let v: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| format!("JSON 解析失败：{e}"))?;
    let order = v
        .get("order")
        .and_then(|x| x.as_str())
        .unwrap_or("hold")
        .to_lowercase();
    Ok(Judgement {
        order,
        target: v
            .get("target")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        dissent: v.get("dissent").and_then(|x| x.as_bool()).unwrap_or(false),
        reply: v
            .get("reply")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        confidence: v
            .get("confidence")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.5)
            .clamp(0.0, 1.0),
        reason: v
            .get("reason")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        cost: v
            .get("cost")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
    })
}

/// 调一次模型（同步，可能几秒）。
pub fn ask(cfg: &Config, a: &Ask) -> Result<Judgement, String> {
    // 不联网时用本地规则判断，而不是报错。
    // 报错等于"将领沉默"——玩家看到的是一封电报发出去、石沉大海。
    if !cfg.uses_model() {
        return Ok(if a.sitrep {
            mock_sitrep(a)
        } else {
            mock_judgement(a)
        });
    }

    let (sys, user, max_tokens) = if a.sitrep {
        (SYSTEM_SITREP, build_sitrep_user(a), 400)
    } else {
        (SYSTEM, build_user(a), 800)
    };
    let body = serde_json::json!({
        "model": cfg.model,
        "temperature": 0.9,
        // 给足长度：截断的 JSON 会白白浪费一次调用
        "max_tokens": max_tokens,
        "messages": [
            { "role": "system", "content": sys },
            { "role": "user", "content": user }
        ]
    })
    .to_string();

    // 发请求：传输层在 `http.rs` 里按平台选（桌面/安卓 = curl，iOS = URLSession）。
    let text = crate::http::post(&chat_url(cfg), cfg.key.trim(), &body, cfg.timeout_secs, "llm")?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("响应不是 JSON：{e}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("接口报错：{err}"));
    }
    let content = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| format!("响应里没有 content：{:.160}", text))?;
    if a.sitrep {
        // 报告是**散文**，不是 JSON。硬要模型套 JSON 会让它把话写干。
        let t = content.trim().trim_matches('"').trim().to_string();
        return Ok(Judgement {
            order: "hold".into(),
            target: None,
            dissent: false,
            confidence: 1.0,
            reason: t.clone(),
            cost: String::new(),
            reply: t,
        });
    }
    parse_judgement(content)
}

/// 不联网时的战况报告：按性格给敌情估计加偏差。
///
/// 这是"情报滤镜"在最朴素形式下的落点：同一个数字，谨慎的人报得大，
/// 好战的人报得小。它只影响**玩家看到的话**，不影响模拟——报告不进状态哈希。
fn mock_sitrep(a: &Ask) -> Judgement {
    // 不写自己的名字：调用方那行已经写成「第25军（王敬久）：」，这里再来一次就重了。
    let mut t = format!(
        "我部在{}，兵 {:.1} 千，气 {:.2}，粮 {:.2}。",
        a.province, a.strength, a.morale, a.supply
    );
    let enemy = a.neighbors.iter().find(|n| n.owner == "敌方控制");
    match enemy {
        Some(n) if n.enemy_est > 0.0 => {
            let (name, est) = (&n.name, n.enemy_est);
            let bias = if a.aggression > 0.7 {
                0.8
            } else if a.caution > 0.6 {
                1.25
            } else {
                1.0
            };
            t.push_str(&format!(
                "当面{}之敌约 {:.1} 千（职部估的，未必准）。",
                name,
                est * bias
            ));
        }
        _ => t.push_str("当面未见大股敌军。"),
    }
    t.push_str(&format!("现受令{}。", a.order_label));
    Judgement {
        order: "hold".into(),
        target: None,
        dissent: false,
        confidence: 1.0,
        reason: t.clone(),
        cost: String::new(),
        reply: t,
    }
}

/// 假模型：不联网，按提示词内容确定性地编一个"像样"的判断。
///
/// 它遵守同样的机械约束（只能打邻接省、撤退只能向己方省），
/// 所以测的是**整条链路**（提示词 → 解析 → 命令 → 记录 → 回放），而不是模型质量。
fn mock_judgement(a: &Ask) -> Judgement {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    a.unit.hash(&mut h);
    a.day.hash(&mut h);
    let r = h.finish() % 100;

    let enemy = a
        .neighbors
        .iter()
        .find(|n| n.owner == "敌方控制")
        .map(|n| n.name.clone());
    let own = a
        .neighbors
        .iter()
        .find(|n| n.owner == "我方控制")
        .map(|n| n.name.clone());

    // 有电报时先看电报里点了哪个地名——**邻省和远处都算**。
    //
    // 这个假模型存在的意义是验证整条链路：电报 → 将领判断 → 命令 → 行军。
    // 如果它连"命令里提到的地方"都不认，跨省行军这条路就永远测不到，
    // 只能靠真人对着真模型手点。
    if !a.incoming.is_empty() {
        // 预览版：炸路 / 抢修。没有这一条，不接模型的玩家永远派不出飞机去炸桥，
        // 也就永远看不到铁路被炸这个机制——那不是"可选功能"，那是坏了。
        if a.rail_damage && (a.incoming.contains("炸") || a.incoming.contains("破路")) {
            let where_ = a
                .neighbors
                .iter()
                .map(|n| n.name.clone())
                .chain(a.far.iter().map(|f| f.name.clone()))
                .find(|n| a.incoming.contains(n.as_str()))
                .or_else(|| a.neighbors.iter().find(|n| n.owner == "敌方控制").map(|n| n.name.clone()));
            if let Some(name) = where_ {
                return Judgement {
                    order: "bomb_rail".into(),
                    target: Some(name.clone()),
                    dissent: false,
                    confidence: 0.6,
                    reason: format!("上意要破{}的交通，我的判断是也应当先炸铁路。", name),
                    cost: "飞机出去一趟，油料与弹药都要消耗；也不一定一次就炸断。".into(),
                    reply: format!("{}：即派机队空袭{}段铁路。", a.general, name),
                };
            }
        }
        let mut hit: Option<(String, bool)> = None;
        for n in &a.neighbors {
            if a.incoming.contains(n.name.as_str()) {
                hit = Some((n.name.clone(), n.owner == "我方控制"));
                break;
            }
        }
        if hit.is_none() {
            for f in &a.far {
                if a.incoming.contains(f.name.as_str()) {
                    hit = Some((f.name.clone(), f.owner == "我方控制"));
                    break;
                }
            }
        }
        if let Some((name, mine)) = hit {
            // 本地判断也要能抗命——但要抗得有道理。
            // 让一支只剩几成、将领又谨慎的残部去打敌占城池，是有理由不听的；
            // 其它情况照办。以前这里的 dissent 是 `order_label != 新命令`，
            // 也就是"只要和你上次的命令不一样就算抗命"——于是每次换命令都是抗命，
            // 战报里全是"（黄维）：我的判断也是应当去" 却被标成抗命。
            let weak = a.strength < a.max_strength * 0.45;
            if !mine && weak && a.caution > 0.55 {
                return Judgement {
                    order: "fortify".into(),
                    target: None,
                    dissent: true,
                    confidence: 0.4,
                    reason: format!(
                        "{}不能打：部队只剩 {} 成，{}又在敌手，我不能拿它去碰。",
                        a.general,
                        (a.strength / a.max_strength * 10.0).round() as i32,
                        name
                    ),
                    cost: "不听令要担责任，也会拖住整个方向。".into(),
                    reply: format!("{}：职部兵力不足，未敢向{}，乞示。", a.general, name),
                };
            }
            return Judgement {
                order: if mine { "move" } else { "attack" }.into(),
                target: Some(name.clone()),
                dissent: false,
                confidence: 0.62,
                reason: format!("上意指向{}，我的判断是也应当把兵力投向那里。", name),
                cost: if mine {
                    "部队要走几天，路上会掉队。".to_string()
                } else {
                    "攻坚要付伤亡，且后方会空虚。".to_string()
                },
                reply: format!("{}收到，即向{}。", a.general, name),
            };
        }
    }

    let weak = a.strength < a.max_strength * 0.45;
    let has_enemy = enemy.is_some();
    // 这一段是"没有电报时我自己怎么看"。既然不是在对上意的回答，
    // 就不存在抗命——dissent 一律 false，否则每天的例行判断都会被当成抗命刷屏。
    let (order, target) = if weak && own.is_some() {
        ("retreat", own)
    } else if let Some(e) = enemy {
        if a.aggression > 0.6 || r % 3 != 0 {
            ("attack", Some(e))
        } else {
            ("fortify", None)
        }
    } else if r % 2 == 0 {
        ("fortify", None)
    } else {
        ("hold", None)
    };

    Judgement {
        order: order.into(),
        target,
        dissent: false,
        confidence: 0.55 + (r % 40) as f64 / 100.0,
        reason: if weak {
            format!("{}兵已不足半数，再拼下去是白送，先保退路。", a.general)
        } else if has_enemy {
            "当面之敌未展开，趁其立足未稳先动手，比等它压上来划算。".to_string()
        } else {
            "当面无敌情，抢时间把工事做厚，等敌人来碰。".to_string()
        },
        cost: "照办，接受这段间隙可能错失战机。".to_string(),
        reply: format!("{}收到，按我的判断办。", a.general),
    }
}

/// 后台问询：一条工作线程 + 一个"已就绪"的池子。
///
/// 为什么不放在 HTTP 请求里：模型一次要几秒，会把"推进一天"卡住。
/// 就绪的判断由 `/api/tick` 在下一个 tick 边界取走，**不会**打乱模拟顺序。
pub struct Broker {
    jobs: std::sync::mpsc::Sender<Job>,
    pub ready: Arc<Mutex<HashMap<usize, Ready>>>,
    pub asked: Arc<Mutex<HashMap<usize, u32>>>,
    pub calls: Arc<Mutex<u32>>,
    pub errors: Arc<Mutex<Vec<String>>>,
    /// 战况报告（亲自指挥）。**不进 `ready`**：
    ///
    /// `ready` 是按单位索引存的"待落地的命令"，而报告不是命令。
    /// 混在一个池子里的话，同一支部队的报告会把还没落地的命令顶掉，
    /// 表现是"回电说照办，部队却不动"——这正是 `telegram_pending` 那条
    /// 注释在防的同一类 bug。报告是一次性的，用队列就好。
    pub sitreps: Arc<Mutex<Vec<Sitrep>>>,
}

/// 一份战况报告：只进战报，不产生任何命令。
#[derive(Clone)]
pub struct Sitrep {
    pub day: u32,
    pub unit: usize,
    pub text: String,
}

pub struct Job {
    pub unit: usize,
    pub day: u32,
    pub ask: Ask,
    /// 这封判断是被哪封电报问出来的（None = 例行问询）。
    /// 带上它，回电才能准确挂到"那封"电报上，而不是挂到最近一封。
    pub telegram: Option<u32>,
    /// true = 这份回答是**战况报告**，不是命令。见 [`Broker::sitreps`]。
    pub sitrep: bool,
}

pub struct Ready {
    pub day: u32,
    pub judgement: Judgement,
    pub telegram: Option<u32>,
}

impl Broker {
    /// `cfg` 是**共享**的：玩家可以在游戏里现填 key，不用重启。
    /// 工作线程每一单都重新取一次配置，所以改了立刻就生效。
    pub fn new(cfg: Arc<Mutex<Config>>) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        let ready: Arc<Mutex<HashMap<usize, Ready>>> = Arc::new(Mutex::new(HashMap::new()));
        let asked = Arc::new(Mutex::new(HashMap::new()));
        let calls = Arc::new(Mutex::new(0u32));
        let errors = Arc::new(Mutex::new(Vec::<String>::new()));
        let sitreps: Arc<Mutex<Vec<Sitrep>>> = Arc::new(Mutex::new(Vec::new()));

        let r2 = Arc::clone(&ready);
        let s0 = Arc::clone(&sitreps);
        let c2 = Arc::clone(&calls);
        let e2 = Arc::clone(&errors);
        // 并发度：同时最多几个问询在飞。
        //
        // 以前是**一个线程串行 recv**：一封"全体"电报要问 8 个将领，每个 3–5 秒，
        // 于是玩家点一次推进要等 30 秒以上，还会觉得"部队不动"（其实在等模型）。
        // 这些请求彼此独立（各问各的将领），没有理由排队。
        //
        // 上限不能开太大：同一家 API 的并发额度有限，打太猛会被限流或直接 429。
        // 6 是"一封全体电报正好一波走完"和"别把对手的限流打出来"之间的折中。
        const MAX_INFLIGHT: usize = 6;
        let inflight = Arc::new((Mutex::new(0usize), std::sync::Condvar::new()));
        let slot = Arc::clone(&inflight);

        std::thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                // 等一个空位（满了就在条件变量上睡，不忙等）
                {
                    let (lock, cv) = &*slot;
                    let mut n = lock.lock().unwrap();
                    while *n >= MAX_INFLIGHT {
                        n = cv.wait(n).unwrap();
                    }
                    *n += 1;
                }
                let slot = Arc::clone(&slot);
                let cfg = Arc::clone(&cfg);
                let r2 = Arc::clone(&r2);
                let c2 = Arc::clone(&c2);
                let e2 = Arc::clone(&e2);
                let s2 = Arc::clone(&s0);
                std::thread::spawn(move || {
                    // 先把配置拷出来，别握着锁去等网络
                    let cfg = cfg.lock().unwrap().clone();
                    match ask(&cfg, &job.ask) {
                        Ok(j) => {
                            *c2.lock().unwrap() += 1;
                            // 报告是队列里的东西，不和"待落地的命令"抢位置
                            if job.sitrep {
                                let mut q = s2.lock().unwrap();
                                if q.len() < 16 {
                                    q.push(Sitrep {
                                        day: job.day,
                                        unit: job.unit,
                                        text: j.reply,
                                    });
                                }
                                let (lock, cv) = &*slot;
                                *lock.lock().unwrap() -= 1;
                                cv.notify_one();
                                return;
                            }
                            let mut g = r2.lock().unwrap();
                            // 一份已经躺在池子里的**电报判断**不能被例行问询盖掉。
                            //
                            // ready 是按单位索引存的，而例行问询每 tick 都会问同一批单位。
                            // 电报问询刚答完、例行问询后脚答完，后者的结果就把前者顶了——
                            // 表现是"回电说照办，部队却原地不动"，而且完全没有报错。
                            let telegram_pending = g
                                .get(&job.unit)
                                .map(|r| r.telegram.is_some())
                                .unwrap_or(false);
                            if !(telegram_pending && job.telegram.is_none()) {
                                g.insert(
                                    job.unit,
                                    Ready { day: job.day, judgement: j, telegram: job.telegram },
                                );
                            }
                        }
                        Err(e) => {
                            let mut ev = e2.lock().unwrap();
                            if ev.len() < 20 {
                                ev.push(format!("第{}日 {}：{e}", job.day, job.ask.unit));
                            }
                        }
                    }
                    // 还回空位
                    let (lock, cv) = &*slot;
                    *lock.lock().unwrap() -= 1;
                    cv.notify_one();
                });
            }
        });

        Self { jobs: tx, ready, asked, calls, errors, sitreps }
    }

    pub fn submit(&self, job: Job) {
        self.asked.lock().unwrap().insert(job.unit, job.day);
        let _ = self.jobs.send(job);
    }
}

// ------------------------------------------------------------------ AI 玩家

/// 敌方统帅部（AI 玩家）的一次问询。
///
/// 之前非玩家阵营只有"师团长各自判断"，没有**一个人**在上面定"这一仗要打成什么样"。
/// 史实里那是第 11 军司令部的工作。现在给敌方一个真正的指挥官：
/// 他有目标、有性格、会点名让某几个师团去做什么。
///
/// 他和师团长的关系是关键：**他下的是电报，不是命令**。
/// 师团长照样走 `Ask` → `Judgement` → 可以抗命。同一条指挥链。
#[derive(Clone)]
pub struct HqAsk {
    pub day: u32,
    pub side: String,
    pub title: String,
    pub general: String,
    pub persona: String,
    pub agenda: String,
    pub objectives: Vec<String>,
    /// 己方各部：(番号, 所在省, 兵力千, 补给, 当前命令, 是否被围)
    pub units: Vec<(String, String, f64, f64, String, bool)>,
    /// 已知敌情：(省, 估计兵力, 归属)
    pub enemy: Vec<(String, f64, String)>,
    pub last_plan: String,
}

#[derive(Clone, Debug, Default)]
pub struct HqJudgement {
    pub plan: String,
    /// (发给哪个番号, 电报原文)
    pub orders: Vec<(String, String)>,
    pub reason: String,
}

// 统帅部的大脑：**不写任何国名、任何具体编制**。
//
// 这一段是引擎的一部分，而引擎不知道这一仗是谁在打——是 1938 年的华中、
// 还是 1805 年的中欧、还是某个玩家自己编的架空战役。是哪一支军队、
// 对面是谁、番号叫什么，全部由模块塞进用户消息里（`HqAsk`）。
// 以前这里写着"侵华日军的军司令官"，等于把一场战争钉进了提示词：
// 玩家选另一边时，AI 会以敌人的身份替我军下命令。
const HQ_SYSTEM: &str = r#"你是一支军队的军司令官，正在自己的司令部里决定这一仗怎么打。

你不是助手，不在回答提问。你手上有一份战役目标，有若干师团，有补给与时间。

要求：
1. 先判断战场态势，再决定这一阶段要什么（进攻？巩固？集中到哪一点？）；
2. 只给具体师团下**少数几条**电报（0-3 条），每条一句话，像真电报；
3. 不要复述数字，不要写报告文学，不要自称 AI；
4. 你的判断可以与上级大本营的期待不一致，但要说清理由；
5. 你没有无限的兵：被打残的师团不要派去攻坚。

只输出一个 JSON 对象，不要代码围栏：
{"plan":"本阶段方针，60字内","orders":[{"to":"某师团","text":"一句话命令"}],"reason":"20-60字，你的判断依据"}"#;

fn build_hq_user(a: &HqAsk) -> String {
    let mut s = String::new();
    s.push_str(&format!("你是{}，{}。\n", a.general, a.title));
    // 你在替哪一边打——这是模块给这一方起的名字（"第九战区"/"第11军"），
    // 不是引擎的常识。选阵营之后必须写进提示词：同一套将官、同一张地图，
    // 换个视角就是对面，提示词里不说清，模型会照着对面的话说。
    if !a.side.is_empty() {
        s.push_str(&format!("你所属的一方：{}。\n", a.side));
    }
    if !a.persona.is_empty() {
        s.push_str(&format!("你的经历与用兵习惯：{}\n", a.persona));
    }
    if !a.agenda.is_empty() {
        s.push_str(&format!("你在意什么：{}\n", a.agenda));
    }
    if !a.objectives.is_empty() {
        s.push_str("你的战役目标（按优先级）：\n");
        for (i, o) in a.objectives.iter().enumerate() {
            s.push_str(&format!("{}. {}\n", i + 1, o));
        }
    }
    s.push_str(&format!("\n第 {} 日。你麾下：\n", a.day));
    for (name, prov, str_, sup, order, encircled) in &a.units {
        s.push_str(&format!(
            "- {}：在 {}，兵力 {:.1} 千，补给 {:.2}，当前{} {}\n",
            name,
            prov,
            str_,
            sup,
            order,
            if *encircled { "（被围！）" } else { "" }
        ));
    }
    s.push_str("已知敌情：\n");
    if a.enemy.is_empty() {
        s.push_str("- （还没有可靠情报）\n");
    }
    for (prov, str_, owner) in &a.enemy {
        s.push_str(&format!("- {}：{}，估计 {:.1} 千\n", prov, owner, str_));
    }
    if !a.last_plan.is_empty() {
        s.push_str(&format!("\n你上一阶段的方针：{}\n", a.last_plan));
        s.push_str("战场没变就别改；变了就说明为什么变。\n");
    }
    s.push_str("\n只输出 JSON。");
    s
}

/// 假司令官：确定性地给方针与 1-2 条电报，用来验证整条链路。
fn mock_hq(a: &HqAsk) -> HqJudgement {
    let mut orders = Vec::new();
    for (name, _prov, str_, sup, _order, encircled) in &a.units {
        if *encircled {
            orders.push((name.clone(), "固守待援，不得突围。".to_string()));
            continue;
        }
        if *str_ > 10.0 && *sup > 0.35 {
            orders.push((name.clone(), "向当面之敌推进，占住要点。".to_string()));
        }
        if orders.len() >= 2 {
            break;
        }
    }
    HqJudgement {
        plan: format!("以主力沿铁路推进（第 {} 日）", a.day),
        orders,
        reason: "当面之敌战线不稳，先压迫其正面。".into(),
    }
}

pub fn ask_hq(cfg: &Config, a: &HqAsk) -> Result<HqJudgement, String> {
    // 同上：敌方统帅部也必须有判断，否则整场戏里只有玩家一个人在动。
    if !cfg.uses_model() {
        return Ok(mock_hq(a));
    }
    let text = hq_chat(cfg, HQ_SYSTEM, &build_hq_user(a), 700)?;
    let json = extract_json(&text).ok_or_else(|| format!("响应不是 JSON：{:.160}", text))?;
    let v: serde_json::Value = serde_json::from_str(&json).map_err(|e| format!("解析失败：{e}"))?;
    let plan = v.get("plan").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let reason = v.get("reason").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mut orders = Vec::new();
    if let Some(arr) = v.get("orders").and_then(|x| x.as_array()) {
        for o in arr.iter().take(4) {
            let to = o.get("to").and_then(|x| x.as_str()).unwrap_or("").trim();
            let text = o.get("text").and_then(|x| x.as_str()).unwrap_or("").trim();
            if !to.is_empty() && !text.is_empty() {
                orders.push((to.to_string(), text.to_string()));
            }
        }
    }
    Ok(HqJudgement { plan, orders, reason })
}

// 注：`ask` 和 `hq_chat` 里原来各有一段一模一样的 curl，当时刻意没合并
// （"跑通过了的东西别动"）。1.3 上 iOS 时不得不合——那边既没有 curl，
// 也不许 fork/exec。现在两处都走 `http::post`，行为与错误文案一字未变。
// 理由记在 docs/25 第 4.1 节。

/// 连通性测试：用给定配置真的打一次模型。
///
/// 首次设置里那个"测试连接"就是它。**强制走网络**（即使当前是 mock/本地模式），
/// 否则失败会被本地判断悄悄顶掉，玩家以为连通了。
pub fn ping(cfg: &Config) -> Result<String, String> {
    if cfg.key.trim().is_empty() {
        return Err("还没有填密钥".into());
    }
    let mut c = cfg.clone();
    c.mock = false;
    c.local_only = false;
    c.enabled = true;
    let out = hq_chat(
        &c,
        "你是连通性测试。只回四个字：连接正常。不要解释，不要标点。",
        "回一句：连接正常",
        32,
    )?;
    let t = out.trim();
    if t.is_empty() {
        return Err("模型回复为空".into());
    }
    Ok(t.chars().take(40).collect())
}

/// 通话用的系统提示：这是**口头汇报**，不是下达命令。
const TALK_SYSTEM: &str = "你是国民革命军的一名将领，正在电话里向上级口头汇报。\
只说你从自己部队、侦察和下级报告里知道的事：不知道就说不知道，不要编造精确数字，\
不要把估计说成事实。两三句话，像人说话，带一点情绪也可以。\
不要下命令，不要输出 JSON，不要列清单。";

/// 通话：问将领一句话，只回答，**不产生任何命令**。
///
/// 这是"通讯"的另一半。以前只有发报这一个动作，玩家没法问"你那儿怎么样"，
/// 而指挥员一天到晚干的就是这件事。回答完全是他自己的说法——
/// 他可能报得乐观、可能不知道、可能把估计说成事实，这正是要的。
pub fn talk(cfg: &Config, a: &Ask, question: &str) -> Result<String, String> {
    if !cfg.uses_model() {
        return Ok(local_talk(a, question));
    }
    let user = format!(
        "{}\n\n上级在电话里问你：「{}」\n用第一人称回答，两三句话。",
        build_user(a),
        question
    );
    hq_chat(cfg, TALK_SYSTEM, &user, 300)
}

/// 没有模型时的通话：用他自己的状态组织成"汇报"的口吻。
///
/// 注意它给的仍然是**他的处境**（补给、士气、当面之敌的估计），
/// 不是全图真值——和接了模型时同一个立场。
fn local_talk(a: &Ask, question: &str) -> String {
    let q = question;
    let enemy = a
        .neighbors
        .iter()
        .find(|n| n.owner == "敌方控制")
        .map(|n| n.name.clone());
    let strength = format!("{:.1} 千", a.strength);
    let supply = if a.supply < 0.3 {
        "补给很紧，撑不了几天"
    } else if a.supply < 0.6 {
        "补给勉强"
    } else {
        "补给还行"
    };
    let morale = if a.morale < 0.4 {
        "士气很低，部队打不动了"
    } else if a.morale < 0.7 {
        "士气一般"
    } else {
        "士气还在"
    };
    let asks_enemy = q.contains('敌') || q.contains("当面") || q.contains("情况");
    let asks_supply = q.contains('粮') || q.contains("弹") || q.contains("补");
    let mut s = format!("{}：职部在{}，现有兵力 {}。", a.general, a.province, strength);
    if asks_enemy {
        match enemy {
            Some(e) => s.push_str(&format!(
                "当面{}方向有敌活动，估计不少于 {} 千，具体情况还在查。",
                e,
                (a.neighbors
                    .iter()
                    .find(|n| n.owner == "敌方控制")
                    .map(|n| n.enemy_est)
                    .unwrap_or(0.0)
                    .max(1.0))
                .round()
            )),
            None => s.push_str("当面尚未发现敌军主力，只有零星侦察。"),
        }
    }
    if asks_supply {
        s.push_str(&format!("{}。", supply));
    }
    if s.len() < 30 {
        s.push_str(&format!("{}，{}。", supply, morale));
    }
    s.push_str("请上峰示。");
    s
}

fn hq_chat(cfg: &Config, system: &str, user: &str, max_tokens: u32) -> Result<String, String> {
    let body = serde_json::json!({
        "model": cfg.model,
        "temperature": 0.8,
        "max_tokens": max_tokens,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ]
    })
    .to_string();

    let text = crate::http::post(&chat_url(cfg), cfg.key.trim(), &body, cfg.timeout_secs, "hq")?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("响应不是 JSON：{e}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("接口报错：{err}"));
    }
    let content = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| format!("响应里没有 content：{:.160}", text))?;
    Ok(content.to_string())
}

/// 司令部问询的后台线程：独立一条，别和将领问询抢队列。
///
/// 为什么不塞进 `Broker`：将领问询是"每个单位一份"，司令部是"每天至多一份"，
/// 生命周期也不同（司令部慢得多）。分开两条线程，互不拖累。
pub struct HqDriver {
    tx: std::sync::mpsc::Sender<(u32, HqAsk)>,
    pub ready: Arc<Mutex<Option<(u32, HqJudgement)>>>,
    pub calls: Arc<Mutex<u32>>,
    pub errors: Arc<Mutex<Vec<String>>>,
}

impl HqDriver {
    pub fn new(cfg: Arc<Mutex<Config>>) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<(u32, HqAsk)>();
        let ready: Arc<Mutex<Option<(u32, HqJudgement)>>> = Arc::new(Mutex::new(None));
        let calls = Arc::new(Mutex::new(0u32));
        let errors = Arc::new(Mutex::new(Vec::<String>::new()));
        let r2 = Arc::clone(&ready);
        let c2 = Arc::clone(&calls);
        let e2 = Arc::clone(&errors);
        std::thread::spawn(move || {
            while let Ok((day, ask)) = rx.recv() {
                let cfg = cfg.lock().unwrap().clone();
                match ask_hq(&cfg, &ask) {
                    Ok(j) => {
                        *c2.lock().unwrap() += 1;
                        *r2.lock().unwrap() = Some((day, j));
                    }
                    Err(e) => {
                        let mut ev = e2.lock().unwrap();
                        if ev.len() < 20 {
                            ev.push(format!("司令部 第{day}日：{e}"));
                        }
                    }
                }
            }
        });
        Self { tx, ready, calls, errors }
    }

    pub fn submit(&self, day: u32, ask: HqAsk) {
        let _ = self.tx.send((day, ask));
    }

    pub fn take(&self) -> Option<(u32, HqJudgement)> {
        self.ready.lock().unwrap().take()
    }
}

#[cfg(test)]
mod hq_tests {
    use super::*;

    /// 全编造的统帅部：引擎的测试**不许**依赖某一仗的真实番号——
    /// 那正是分层要防的事（引擎里混进"这仗是谁在打"的常识）。
    fn ask() -> HqAsk {
        HqAsk {
            day: 3,
            side: "甲集团军".into(),
            title: "集团军司令官".into(),
            general: "某将军".into(),
            persona: "出身参谋，用兵谨慎".into(),
            agenda: "打通甲城走廊".into(),
            objectives: vec!["夺取甲城".into(), "保住乙城补给线".into()],
            units: vec![
                ("第1师".into(), "乙城".into(), 24.0, 0.6, "固守".into(), false),
                ("第2师".into(), "丙岭".into(), 16.0, 0.2, "进攻".into(), true),
            ],
            enemy: vec![("甲城".into(), 20.0, "敌方控制".into())],
            last_plan: String::new(),
        }
    }

    #[test]
    fn mock_hq_gives_a_plan_and_orders() {
        let j = mock_hq(&ask());
        assert!(!j.plan.is_empty(), "要有方针");
        assert!(!j.orders.is_empty(), "要有给师团的电报");
        // 被围的部队收到的是固守，不是推进
        let encircled = j.orders.iter().find(|(to, _)| to == "第2师");
        if let Some((_, text)) = encircled {
            assert!(text.contains("固守"), "被围的师团不该被派去推进：{text}");
        }
    }

    #[test]
    fn prompt_carries_who_he_is_and_what_he_wants() {
        let s = build_hq_user(&ask());
        assert!(s.contains("集团军司令官"), "提示词里要有职务");
        assert!(s.contains("你所属的一方：甲集团军"), "提示词里要写清他替哪一边打");
        assert!(s.contains("夺取甲城"), "提示词里要有战役目标");
        assert!(s.contains("第2师"), "提示词里要有麾下各部");
        assert!(s.contains("被围"), "被围的部队要标出来");
        assert!(s.contains("甲城"), "提示词里要有已知敌情");
    }

    #[test]
    fn prompt_gives_facts_not_verdicts() {
        // 提示词里只许出现**事实**。以前这里是"司令部的作战方针：推进：主攻方向 X、Y"，
        // 等于函数替将领把判断做了，它抗命时引用的也是这句结论。
        let mut a = Ask::default();
        a.situation = "当面：我 12.4 千 / 敌 6.8 千（战力比 1.82）。\
                       与我方地面部队相邻的敌方省份（按设施多寡排序）：东华门（补给源） 守军 3.2 千。"
            .into();
        let s = build_user(&a);
        assert!(s.contains("当面态势（只有事实，不含任何建议）"), "要标明这是事实");
        assert!(s.contains("战力比 1.82"), "事实要原样带进去");
        assert!(s.contains("东华门"), "够得着的省要带进去");
        assert!(s.contains("怎么解读由你自己判断"), "要明确判断归将领");
        assert!(!s.contains("作战方针"), "不许再把结论包装成方针");
        assert!(!s.contains("这是方向，不是命令"), "旧框架不许残留");
    }

    #[test]
    fn prompt_carries_friends_rail_and_known_enemy_units() {
        // A（友军）+ B（铁路/补给）+ D（番号）三条边界：都要出现在提示词里。
        let mut a = Ask::default();
        a.situation = "当面：我 10.0 千 / 敌方已知 5.0 千（战力比 2.00，敌方数字来自情报，未必准）。"
            .into();
        a.neighbors = vec![
            NeighborInfo {
                name: "德安".into(),
                owner: "敌方控制".into(),
                enemy_est: 6.0,
                terrain: "丘陵".into(),
                has_rail: true,
                rail_damage: 3,
                supply_source: true,
                enemy_units: vec!["第106师团".into()],
            },
            NeighborInfo {
                name: "沙河".into(),
                owner: "我方控制".into(),
                enemy_est: 0.0,
                terrain: "平原".into(),
                has_rail: false,
                rail_damage: 0,
                supply_source: false,
                enemy_units: vec![],
            },
        ];
        a.friends = vec![FriendUnit {
            name: "第74军".into(),
            province: "沙河".into(),
            strength: 12.0,
            order: "进攻".into(),
        }];
        a.supply_sources = vec!["九江".into()];
        let s = build_user(&a);
        assert!(s.contains("第74军 在 沙河"), "友军要在里面：{s}");
        assert!(s.contains("进攻"), "友军当前在干什么也要给");
        assert!(s.contains("铁路已炸断"), "铁路状态要报：{s}");
        assert!(s.contains("补给源"), "补给源要报");
        assert!(s.contains("番号 第106师团"), "认出来的番号要给名字：{s}");
        assert!(s.contains("你的补给源：九江"), "本方的补给源要单独一段");
        // 没铁路的省别写"铁路"，否则模型会以为被炸了
        let shahe = s.split("- 沙河").nth(1).unwrap_or("");
        assert!(!shahe.starts_with('（') || !shahe.split('）').next().unwrap_or("").contains("铁路"));
    }

    #[test]
    fn ask_hq_works_offline() {
        // mock 模式下不联网也应当给出判断
        let cfg = Config {
            enabled: true,
            mock: true,
            local_only: false,
            base: "http://127.0.0.1:1".into(),
            model: "x".into(),
            key: String::new(),
            interval_days: 1,
            timeout_secs: 1,
        };
        let j = ask_hq(&cfg, &ask()).expect("mock 不该失败");
        assert!(!j.reason.is_empty());
    }
}
