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
    //   settings.env  装好之后用的（安装目录，玩家在游戏里填的东西写这里）
    for name in [".env", "settings.env"] {
        let Ok(text) = std::fs::read_to_string(name) else {
            continue;
        };
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
}

/// 配置文件在哪：先看工作目录，再看 exe 旁边（桌面版从安装目录启动，两者相同）。
pub fn settings_path() -> std::path::PathBuf {
    let cwd = std::path::PathBuf::from("settings.env");
    if cwd.exists() {
        return cwd;
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        let p = dir.join("settings.env");
        if p.exists() {
            return p;
        }
        return p;
    }
    cwd
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

/// 问一次模型需要的全部信息。**只放他能知道的**。
#[derive(Clone)]
pub struct Ask {
    pub day: u32,
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
    /// (省名, 归属, 敌方兵力估计, 地形)
    pub neighbors: Vec<(String, String, f64, String)>,
    /// (省名, 要走几天, 归属, 敌方兵力估计, 地形)
    ///
    /// 远距离目标：将领不该只看得见邻省。没有这一项，"千里奔袭"
    /// 就永远落不了地——模型只会从邻省里挑一个，玩家说"抽两个师
    /// 去守万家岭"它就理解成"往邻省挪一步"。
    pub far: Vec<(String, u32, String, f64, String)>,
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
    /// 司令部（统帅部）的作战方针。这是**方向**，不是命令。
    pub hq_plan: String,
}

const SYSTEM: &str = r#"你是一名正在前线亲自指挥部队的将领。你不是助手、不是客服，也不在回答"用户"的提问。

这里是兵棋推演的军官席。你的职责是打赢你这一仗，不是让上级满意。

【必须主动对抗的偏误】大语言模型在训练中普遍被奖励"顺从提问者"，于是会不自觉地把"照办"当成正确答案，并写出漂亮但空洞的支持性理由。本场景恰恰奖励独立判断。所以：
1. 先独立判断此刻正确的军事行动，再看上级要你做什么；两者不一致时，说出不一致；
2. 即使你最终决定照办，也必须先写出至少一个"不照办"的理由；
3. 如果你照办，必须写出你为此接受的具体代价（兵力、时间、阵地、退路）；
4. 不要恭维，不要套话，不要自称AI，不要复述命令原文。

你的性格以数字给出，不要在回答里复述它们，要体现在判断里：
效忠度低 = 你不把上级命令当铁律；主动性高 = 你倾向自己拍板；好战高 = 你愿意用损失换时间；谨慎高 = 你先保退路。

只输出一个 JSON 对象，不要代码围栏，不要多余解释。"#;

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
        s.push_str("这是命令，但它不是正确答案——先判断，再决定照办到什么程度。\n");
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
    for (name, owner, est, terr) in &a.neighbors {
        s.push_str(&format!(
            "- {}（{}，{}，敌军估计 {:.1} 千）\n",
            name, owner, terr, est
        ));
    }
    if !a.far.is_empty() {
        s.push_str("更远处的战场（要走几天，路上会被打、会掉队）：\n");
        for (name, hops, owner, est, terr) in &a.far {
            s.push_str(&format!(
                "- {}（{}日行程，{}，{}，敌军估计 {:.1} 千）\n",
                name, hops, owner, terr, est
            ));
        }
    }
    if !a.hq_plan.is_empty() {
        s.push_str(&format!(
            "\n司令部的作战方针：{}\n这是方向，不是命令。你与它不一致时可以按自己的判断办，但要说明理由。\n",
            a.hq_plan
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
        return Ok(mock_judgement(a));
    }

    let body = serde_json::json!({
        "model": cfg.model,
        "temperature": 0.9,
        // 给足长度：截断的 JSON 会白白浪费一次调用
        "max_tokens": 800,
        "messages": [
            { "role": "system", "content": SYSTEM },
            { "role": "user", "content": build_user(a) }
        ]
    })
    .to_string();

    // 用 curl 发请求：不引第三方 HTTP 依赖；key 写进 curl 配置文件，不出现在命令行参数里。
    let dir = std::env::temp_dir().join(format!("ironmind-llm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("建临时目录失败：{e}"))?;
    let body_path = dir.join("body.json");
    let cfg_path = dir.join("curl.conf");
    std::fs::write(&body_path, body).map_err(|e| format!("写请求失败：{e}"))?;
    std::fs::write(
        &cfg_path,
        format!(
            "url = \"{}/chat/completions\"\nheader = \"Authorization: Bearer {}\"\nheader = \"Content-Type: application/json\"\n",
            cfg.base.trim_end_matches('/'),
            cfg.key.trim()
        ),
    )
    .map_err(|e| format!("写 curl 配置失败：{e}"))?;

    let out = std::process::Command::new("curl")
        .args([
            "-s",
            "--max-time",
            &cfg.timeout_secs.to_string(),
            "-K",
            &cfg_path.to_string_lossy(),
            "-d",
            &format!("@{}", body_path.to_string_lossy()),
        ])
        .output()
        .map_err(|e| format!("调用 curl 失败：{e}"))?;

    let _ = std::fs::remove_dir_all(&dir);

    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if text.trim().is_empty() {
        return Err(format!(
            "模型无响应（stderr: {}）",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
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
    parse_judgement(content)
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
        .find(|n| n.1 == "敌方控制")
        .map(|n| n.0.clone());
    let own = a
        .neighbors
        .iter()
        .find(|n| n.1 == "我方控制")
        .map(|n| n.0.clone());

    // 有电报时先看电报里点了哪个地名——**邻省和远处都算**。
    //
    // 这个假模型存在的意义是验证整条链路：电报 → 将领判断 → 命令 → 行军。
    // 如果它连"命令里提到的地方"都不认，跨省行军这条路就永远测不到，
    // 只能靠真人对着真模型手点。
    if !a.incoming.is_empty() {
        let mut hit: Option<(String, bool)> = None;
        for (name, owner, _, _) in &a.neighbors {
            if a.incoming.contains(name.as_str()) {
                hit = Some((name.clone(), owner == "我方控制"));
                break;
            }
        }
        if hit.is_none() {
            for (name, _hops, owner, _, _) in &a.far {
                if a.incoming.contains(name.as_str()) {
                    hit = Some((name.clone(), owner == "我方控制"));
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
}

pub struct Job {
    pub unit: usize,
    pub day: u32,
    pub ask: Ask,
    /// 这封判断是被哪封电报问出来的（None = 例行问询）。
    /// 带上它，回电才能准确挂到"那封"电报上，而不是挂到最近一封。
    pub telegram: Option<u32>,
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

        let r2 = Arc::clone(&ready);
        let c2 = Arc::clone(&calls);
        let e2 = Arc::clone(&errors);
        std::thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                // 先把配置拷出来，别握着锁去等网络
                let cfg = cfg.lock().unwrap().clone();
                match ask(&cfg, &job.ask) {
                    Ok(j) => {
                        *c2.lock().unwrap() += 1;
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
            }
        });

        Self { jobs: tx, ready, asked, calls, errors }
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

const HQ_SYSTEM: &str = r#"你是一支侵华日军的军司令官，正在自己的司令部里决定这一仗怎么打。

你不是助手，不在回答提问。你手上有一份战役目标，有若干师团，有补给与时间。

要求：
1. 先判断战场态势，再决定这一阶段要什么（进攻？巩固？集中到哪一点？）；
2. 只给具体师团下**少数几条**电报（0-3 条），每条一句话，像真电报；
3. 不要复述数字，不要写报告文学，不要自称 AI；
4. 你的判断可以与国内大本营的期待不一致，但要说清理由；
5. 你没有无限的兵：被打残的师团不要派去攻坚。

只输出一个 JSON 对象，不要代码围栏：
{"plan":"本阶段方针，60字内","orders":[{"to":"第106师团","text":"一句话命令"}],"reason":"20-60字，你的判断依据"}"#;

fn build_hq_user(a: &HqAsk) -> String {
    let mut s = String::new();
    s.push_str(&format!("你是{}，{}。\n", a.general, a.title));
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

/// 发一次请求（司令部专用）。
///
/// 刻意**没有**去重构 `ask()` 里的那段 curl：那条路是跑通过了的东西，
/// 为了少写四十行去动它不划算。两段代码都只做""拼 JSON → curl -K → 取 content""。
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
        .find(|n| n.1 == "敌方控制")
        .map(|n| n.0.clone());
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
                    .find(|n| n.1 == "敌方控制")
                    .map(|n| n.2)
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

    let dir = std::env::temp_dir().join(format!("ironmind-hq-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("建临时目录失败：{e}"))?;
    let body_path = dir.join("body.json");
    let cfg_path = dir.join("curl.conf");
    std::fs::write(&body_path, body).map_err(|e| format!("写请求失败：{e}"))?;
    std::fs::write(
        &cfg_path,
        format!(
            "url = \"{}/chat/completions\"\nheader = \"Authorization: Bearer {}\"\nheader = \"Content-Type: application/json\"\n",
            cfg.base.trim_end_matches('/'),
            cfg.key.trim()
        ),
    )
    .map_err(|e| format!("写 curl 配置失败：{e}"))?;

    let out = std::process::Command::new("curl")
        .args([
            "-s",
            "--max-time",
            &cfg.timeout_secs.to_string(),
            "-K",
            &cfg_path.to_string_lossy(),
            "-d",
            &format!("@{}", body_path.to_string_lossy()),
        ])
        .output()
        .map_err(|e| format!("调用 curl 失败：{e}"))?;
    let _ = std::fs::remove_dir_all(&dir);

    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if text.trim().is_empty() {
        return Err(format!(
            "模型无响应（stderr: {}）",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
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

    fn ask() -> HqAsk {
        HqAsk {
            day: 3,
            side: "日本".into(),
            title: "第11军司令官".into(),
            general: "岡村寧次".into(),
            persona: "情报参谋出身".into(),
            agenda: "打通南浔线".into(),
            objectives: vec!["夺取德安".into(), "保护长江补给线".into()],
            units: vec![
                ("第6师团".into(), "九江".into(), 24.0, 0.6, "固守".into(), false),
                ("第106师团".into(), "万家岭".into(), 16.0, 0.2, "进攻".into(), true),
            ],
            enemy: vec![("德安".into(), 20.0, "敌方控制".into())],
            last_plan: String::new(),
        }
    }

    #[test]
    fn mock_hq_gives_a_plan_and_orders() {
        let j = mock_hq(&ask());
        assert!(!j.plan.is_empty(), "要有方针");
        assert!(!j.orders.is_empty(), "要有给师团的电报");
        // 被围的部队收到的是固守，不是推进
        let encircled = j.orders.iter().find(|(to, _)| to == "第106师团");
        if let Some((_, text)) = encircled {
            assert!(text.contains("固守"), "被围的师团不该被派去推进：{text}");
        }
    }

    #[test]
    fn prompt_carries_who_he_is_and_what_he_wants() {
        let s = build_hq_user(&ask());
        assert!(s.contains("第11军司令官"), "提示词里要有职务");
        assert!(s.contains("夺取德安"), "提示词里要有战役目标");
        assert!(s.contains("第106师团"), "提示词里要有麾下各部");
        assert!(s.contains("被围"), "被围的部队要标出来");
        assert!(s.contains("德安"), "提示词里要有已知敌情");
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
