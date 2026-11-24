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
fn load_dotenv() {
    let Ok(text) = std::fs::read_to_string(".env") else {
        return;
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

/// LLM 配置。全部来自环境变量。
#[derive(Clone, Debug)]
pub struct Config {
    pub enabled: bool,
    /// 用内置假模型（不联网，用来验证整条链路）
    pub mock: bool,
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
        Self {
            enabled: !off && (mock || !key.trim().is_empty()),
            mock,
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

    pub fn describe(&self) -> String {
        if !self.enabled {
            "规则 AI（未配置 IRONMIND_API_KEY）".into()
        } else if self.mock {
            "模拟模型（IRONMIND_LLM=mock）".into()
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
    s.push_str(&format!(
        "\n上级给你的命令：{}（来自：{}）\n",
        a.order_label, a.order_from
    ));
    s.push_str("\n可选行动：attack（攻邻接省）/move（去邻接省）/retreat（撤向邻接己方省）/hold/fortify/rest/shift（省内换阵地）\n");
    s.push_str("输出 JSON：{\"order\":\"...\",\"target\":\"省名或阵地名\",\"dissent\":true,\"confidence\":0.6,\"reason\":\"不超过80字，你自己的判断\",\"cost\":\"照办时你接受的代价，不超过40字\"}\n");
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
    if cfg.mock {
        return Ok(mock_judgement(a));
    }
    if cfg.key.trim().is_empty() {
        return Err("没有 IRONMIND_API_KEY".into());
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
        .find(|n| n.1 != a.side && n.2 > 0.0)
        .map(|n| n.0.clone());
    let own = a.neighbors.iter().find(|n| n.1 == a.side).map(|n| n.0.clone());

    let weak = a.strength < a.max_strength * 0.45;
    let has_enemy = enemy.is_some();
    let (order, target, dissent) = if weak && own.is_some() {
        ("retreat", own, a.order_label != "撤退")
    } else if let Some(e) = enemy {
        if a.aggression > 0.6 || r % 3 != 0 {
            ("attack", Some(e), a.order_label != "进攻")
        } else {
            ("fortify", None, a.order_label != "筑工事")
        }
    } else if r % 2 == 0 {
        ("fortify", None, false)
    } else {
        ("hold", None, false)
    };

    Judgement {
        order: order.into(),
        target,
        dissent,
        confidence: 0.55 + (r % 40) as f64 / 100.0,
        reason: if weak {
            format!("{}兵已不足半数，再拼下去是白送，先保退路。", a.general)
        } else if has_enemy {
            "当面之敌未展开，趁其立足未稳先动手，比等它压上来划算。".to_string()
        } else {
            "当面无敌情，抢时间把工事做厚，等敌人来碰。".to_string()
        },
        cost: if dissent {
            "抗命要担责任，这次认了。".to_string()
        } else {
            "照办，接受这段间隙可能错失战机。".to_string()
        },
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
}

pub struct Ready {
    pub day: u32,
    pub judgement: Judgement,
}

impl Broker {
    pub fn new(cfg: Config) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        let ready = Arc::new(Mutex::new(HashMap::new()));
        let asked = Arc::new(Mutex::new(HashMap::new()));
        let calls = Arc::new(Mutex::new(0u32));
        let errors = Arc::new(Mutex::new(Vec::<String>::new()));

        let r2 = Arc::clone(&ready);
        let c2 = Arc::clone(&calls);
        let e2 = Arc::clone(&errors);
        std::thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                match ask(&cfg, &job.ask) {
                    Ok(j) => {
                        *c2.lock().unwrap() += 1;
                        r2.lock()
                            .unwrap()
                            .insert(job.unit, Ready { day: job.day, judgement: j });
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
