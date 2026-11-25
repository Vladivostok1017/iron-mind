//! 极简 HTTP 服务：把模拟核心暴露给浏览器。
//!
//! 为什么不用 axum/actix：这个服务的全部职责是"收几个请求、回几个 JSON"，
//! 手写 200 行比引入一个异步框架更可控，也更容易看清数据流。
//!
//! 端点：
//! - `GET  /`            前端页面
//! - `GET  /api/state`   当前状态（**已按情报过滤**，不是真值）
//! - `POST /api/tick`    推进 N 天
//! - `POST /api/order`   给某个单位下达意图
//! - `POST /api/reset`   重开
//!
//! 安全边界：只监听 127.0.0.1，不处理任何外部输入之外的副作用。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

use sim_core::world::{Order, Side, Terrain, World};
use sim_core::Accuracy;
use sim_content::{build_world, load_module, LoadedModule};

mod llm;

const DEFAULT_PORT: u16 = 8899;
/// 每推进多少天自动存一次档。
const AUTOSAVE_EVERY: u32 = 5;
/// 自动存档的槽位名。固定一个，不挤占玩家手工存档。
const AUTOSAVE_SLOT: &str = "auto";

fn main() {
    // 先把 settings.env / .env 读进环境，再决定用哪个模块、开哪些目录。
    // 顺序很重要：模块选择比 LLM 配置更早发生。
    llm::load_env_files();
    let args: Vec<String> = std::env::args().collect();
    let module_path = args
        .iter()
        .position(|a| a == "--module")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .or_else(|| std::env::var("IRONMIND_MODULE").ok())
        .or_else(|| discover_modules().into_iter().next().map(|p| p.to_string_lossy().to_string()))
        .unwrap_or_else(|| "content/scenarios/demo_1805.imod".to_string());
    println!("模块根目录：{:?}", module_roots());
    println!("可用模块：{} 个", discover_modules().len());

    let dir = PathBuf::from(&module_path);
    let module = match load_module(&dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("加载模块失败：{e}");
            std::process::exit(2);
        }
    };

    println!("== Iron Mind · 本地服务 ==");
    println!("模块：{} v{}", module.meta.name, module.meta.version);
    println!(
        "省份 {} · 单位 {} · 战役 {} 天",
        module.map.provinces.len(),
        module.oob.units.len(),
        module.scenario.days
    );
    for w in &module.warnings {
        println!("  [警告] {}: {}", w.code, w.message);
    }
    // 预览版开关：开着才打这一行。作者/玩家看到它就知道"现在的规则不是定稿那套"。
    {
        let fx = sim_content::features_from(module.scenario.preview.as_ref(), module.meta.scale);
        if fx.any() {
            println!("  预览版：{}", fx.names().join("、"));
        }
    }

    // 模块可以自带真实省界；没有就让前端用 Voronoi 兜底（保持向后兼容）。
    // 从 `module.geo` 拿而不是自己拼路径：模块有两种形态（目录 / .imod 压缩包），
    // 一旦模块是单个文件，"exe 旁边的 provinces_geo.json" 就不存在了。
    let provinces_geo = module.geo.clone();
    if provinces_geo.is_some() {
        println!("  省界：使用模块自带的真实边界（provinces_geo.json）");
    }

    // 配置是**共享可改**的：玩家可以在首次设置里现填 base/model/key，
    // 不用重启游戏（工作线程每一单都重新取一次）。
    let llm_cfg = Arc::new(std::sync::Mutex::new(llm::Config::from_env()));
    {
        let c = llm_cfg.lock().unwrap();
        println!("  将领大脑：{}", c.describe());
        drop(c);
    }

    let state = Arc::new(ServerState {
        module: std::sync::Mutex::new(module),
        module_path: std::sync::Mutex::new(module_path.clone()),
        world: std::sync::Mutex::new(None),
        commands: std::sync::Mutex::new(Vec::new()),
        llm: Arc::clone(&llm_cfg),
        brain: llm::Broker::new(Arc::clone(&llm_cfg)),
        hq: llm::HqDriver::new(Arc::clone(&llm_cfg)),
        hq_plan: std::sync::Mutex::new(String::new()),
        talk_log: std::sync::Mutex::new(std::collections::HashMap::new()),
        hq_last_day: std::sync::Mutex::new(0),
        reports: std::sync::Mutex::new(Vec::new()),
        next_report_id: std::sync::atomic::AtomicU32::new(1),
        provinces_geo: std::sync::Mutex::new(provinces_geo),
        telegrams: std::sync::Mutex::new(Vec::new()),
        next_telegram_id: std::sync::atomic::AtomicU32::new(1),
        generation: std::sync::atomic::AtomicU64::new(1),
    });
    {
        let mut w = state.world.lock().unwrap();
        let mut world = build_world(&state.module.lock().unwrap());
        // 开局先做一次侦察，否则玩家看到的是一片全黑
        sim_core::intel::run_intel(&mut world);
        *w = Some(world);
    }

    let port: u16 = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    let listen = format!("127.0.0.1:{port}");

    let listener = match TcpListener::bind(&listen) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("无法监听 {listen}: {e}");
            std::process::exit(3);
        }
    };
    println!("\n打开浏览器访问：http://{listen}/");
    println!("（Ctrl+C 退出）\n");

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                // 每条连接一个线程。
                //
                // 原来是单线程顺序处理：浏览器只要开一条"预连接"（连上了但还没发请求），
                // 服务器就会在 read_line 上死等，整个服务卡住——前端所有请求
                // （包括状态刷新）一起挂起，表现就是"点什么都没反应"。
                // 世界状态本来就在 Mutex 后面，多线程不会破坏确定性。
                let st = Arc::clone(&state);
                std::thread::spawn(move || {
                    if let Err(e) = handle(s, &st) {
                        eprintln!("请求处理出错：{e}");
                    }
                });
            }
            Err(e) => eprintln!("连接错误：{e}"),
        }
    }
}

struct ServerState {
    /// 当前模块。切模块时会整体替换，所以放在 Mutex 后面。
    module: std::sync::Mutex<LoadedModule>,
    /// 模块目录（切换时用）。
    module_path: std::sync::Mutex<String>,
    world: std::sync::Mutex<Option<World>>,
    /// 玩家命令流。`(模块, 种子, 命令流)` 就是一份完整存档。
    commands: std::sync::Mutex<Vec<CmdRecord>>,
    /// 将领的 LLM 配置与后台问询。
    /// 配置放在锁后面：首次设置里可以现填，不用重启。
    llm: Arc<std::sync::Mutex<llm::Config>>,
    brain: llm::Broker,
    /// 敌方统帅部（AI 玩家）：独立线程 + 上一次的方针。
    hq: llm::HqDriver,
    hq_plan: std::sync::Mutex<String>,
    /// 每次通话的问答（单位索引 → 最近几句）。
    ///
    /// 进将领的提示词，但不进世界状态：它影响**判断**，不影响回放。
    talk_log: std::sync::Mutex<std::collections::HashMap<usize, Vec<String>>>,
    hq_last_day: std::sync::Mutex<u32>,
    /// 公文堆（请示 / 抗命 / 通报）
    reports: std::sync::Mutex<Vec<Report>>,
    next_report_id: std::sync::atomic::AtomicU32,
    /// 模块自带的真实省界（provinces_geo.json）。没有就退回前端 Voronoi。
    provinces_geo: std::sync::Mutex<Option<String>>,
    /// 在途与已送达的电报
    telegrams: std::sync::Mutex<Vec<Telegram>>,
    next_telegram_id: std::sync::atomic::AtomicU32,
    /// 世界代号。每次模块被替换（切模块、开发工具热重载）+1，前端据此重新拉省界。
    /// 引擎自己不做热重载——开发循环在 SDK 仓库，它只是调 `POST /api/module`；
    /// 这里留的是那个接口本来就该有的副作用（换世界 = 换代号）。
    generation: std::sync::atomic::AtomicU64,
}

fn handle(mut stream: TcpStream, state: &Arc<ServerState>) -> std::io::Result<()> {
    // 读写都要有超时。浏览器会开预连接、也会中途放弃请求；
    // 没有超时的话这条连接会一直占着（单线程时就是整台服务器）。
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    match reader.read_line(&mut request_line) {
        Ok(0) => return Ok(()),
        Ok(_) => {}
        // 超时/对端断开都当正常结束，不当错误刷屏
        Err(_) => return Ok(()),
    }
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return respond(&mut stream, 400, "text/plain", "bad request");
    }
    let method = parts[0];
    let path = parts[1].split('?').next().unwrap_or("/");

    // 读头，取 Content-Length
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            break;
        }
        if line.is_empty() {
            break;
        }
        let t = line.trim();
        if t.is_empty() {
            break;
        }
        let lower = t.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }
    let body_str = String::from_utf8_lossy(&body).to_string();

    // 请求日志。目的很具体：玩家报"点了没反应"时，要能查到那一刻到底发生了什么——
    // 是点击没发出去、被服务端拒了、还是模拟里被别的东西覆盖了。
    let brief: String = body_str.chars().take(200).collect();
    trace(format!(
        "{method} {path}{}",
        if brief.is_empty() { String::new() } else { format!(" body={brief}") }
    ));

    match (method, path) {
        ("GET", "/") => respond(&mut stream, 200, "text/html; charset=utf-8", PAGE),
        ("GET", "/api/log") => {
            // 把日志尾部当纯文本吐出来，方便直接贴给人看
            let all = std::fs::read_to_string("logs/server.log")
                .unwrap_or_else(|_| "(还没有日志)".to_string());
            let tail: Vec<&str> = all.lines().rev().take(200).collect();
            let text: Vec<String> = tail.into_iter().rev().map(|s| s.to_string()).collect();
            respond(&mut stream, 200, "text/plain; charset=utf-8", &text.join("\n"))
        }
        ("GET", "/provinces_geo.json") => match &*state.provinces_geo.lock().unwrap() {
            Some(s) => respond(&mut stream, 200, "application/json", s),
            None => respond(&mut stream, 404, "text/plain", "this module has no provinces_geo.json"),
        },
        ("GET", "/basemap.json") => {
            respond(&mut stream, 200, "application/json", BASEMAP)
        }
        ("GET", "/api/state") => {
            let json = {
                let w = state.world.lock().unwrap();
                snapshot(w.as_ref().unwrap())
            };
            respond(&mut stream, 200, "application/json", &json)
        }
        ("POST", "/api/tick") | ("POST", "/api/advance") => {
            // `advance` = 一直推到"有事发生"为止（新战报 / 新公文 / 战役结束）。
            // 玩家真正想要的是"跳过没事的日子"，不是"再推一天"。
            let advance = method == "POST" && path == "/api/advance";
            let days: u32 = serde_json::from_str::<serde_json::Value>(&body_str)
                .ok()
                .and_then(|v| v.get("days").and_then(|d| d.as_u64()))
                .unwrap_or(1) as u32;
            let json = {
                let mut guard = state.world.lock().unwrap();
                let w = guard.as_mut().unwrap();
                let cap = if advance { 30 } else { days.min(30) };
                let mut done = 0u32;
                for _ in 0..cap {
                    if w.outcome.is_some() {
                        break;
                    }
                    let day = w.tick;
                    // 只数"要紧的"战报：规则类（山地行军减员）每 tick 每个单位一条，
                    // 把它也算成"有事发生"，advance 就等于逐日推进了。
                    let key_before = w
                        .log
                        .iter()
                        .filter(|e| e.kind != sim_core::world::EventKind::Rule)
                        .count();
                    let dossiers_before = w
                        .units
                        .iter()
                        .filter(|u| u.ended_day.is_none())
                        .count();
                    for n in expire_requests(&state, w, day) {
                        w.report(sim_core::world::EventKind::Dissent, n);
                    }
                    // 将领的判断在 tick 边界生效：先落命令，再推演这一天。
                    let notes = drain_ready(&state, w, day);
                    for n in deliver_telegrams(&state, w, day) {
                        w.report(sim_core::world::EventKind::Command, n);
                    }
                    request_due(&state, w);
                    // 敌方统帅部：收电报、开会。放在 tick 之前，
                    // 这样他这一天的判断当天就能落到师团长头上。
                    let hq_notes = run_hq(&state, w, day);
                    for (_dissent, text, side) in hq_notes {
                        w.report_for(sim_core::world::EventKind::Command, text, side);
                    }
                    sim_core::systems::tick(w);
                    for (dissent, text, side) in notes {
                        let kind = if dissent {
                            sim_core::world::EventKind::Dissent
                        } else {
                            sim_core::world::EventKind::Command
                        };
                        // 将领的话是内部文件：按方标记，服务端只把本方那些发给前端
                        w.report_for(kind, text, side);
                    }
                    done += 1;
                    // 自动存档：每几天一次，**在循环里**判。
                    //
                    // 以前放在循环外，于是"一次推进 6 天"只会检查最后一天（第 6 日，
                    // 不是 5 的倍数），整局都不会自动存档。按天检查才对。
                    if w.tick % AUTOSAVE_EVERY == 0 && w.outcome.is_none() {
                        match write_save(&state, w, Some(AUTOSAVE_SLOT.to_string())) {
                            Ok((name, day, n)) => {
                                trace(format!("   自动存档 {name}（第 {day} 日，{n} 条命令）"))
                            }
                            Err(e) => trace(format!("   自动存档失败：{e}")),
                        }
                    }
                    // 有事发生就停：新战报到、番号有增减、战役判出结局。
                    // "没事"的日子（只有山地行军减员那种）继续跳过去。
                    if advance {
                        let new_reports = w
                            .log
                            .iter()
                            .filter(|e| e.kind != sim_core::world::EventKind::Rule)
                            .count()
                            > key_before;
                        let oob_changed = w
                            .units
                            .iter()
                            .filter(|u| u.ended_day.is_none())
                            .count()
                            != dossiers_before;
                        if new_reports || oob_changed || w.outcome.is_some() {
                            break;
                        }
                    }
                }
                trace(format!(
                    "   tick → {} (第 {} 日){}",
                    w.tick,
                    w.tick,
                    if advance {
                        format!("，推进了 {done} 日")
                    } else {
                        String::new()
                    }
                ));
                snapshot(w)
            };
            respond(&mut stream, 200, "application/json", &json)
        }
        ("POST", "/api/order") => {
            // 玩家不再能直接调兵。指挥只有一个入口：发电报（/api/dispatch）。
            // 直接改 order 等于绕过指挥链——那是棋子游戏，不是这个。
            let json = {
                let guard = state.world.lock().unwrap();
                let snap = snapshot(guard.as_ref().unwrap());
                format!(
                    "{{\"error\":\"不能直接调兵。请向将领发电报，由他决定怎么执行\",\"state\":{snap}}}"
                )
            };
            respond(&mut stream, 200, "application/json", &json)
        }
        ("POST", "/api/order_disabled") => {
            let json = {
                let mut guard = state.world.lock().unwrap();
                let w = guard.as_mut().unwrap();
                // 记录命令：day 取应用时的天数，回放时按同样顺序重放
                let day = w.tick;
                let parsed = serde_json::from_str::<serde_json::Value>(&body_str).ok();
                let unit = parsed
                    .as_ref()
                    .and_then(|v| v.get("unit").and_then(|x| x.as_u64()))
                    .unwrap_or(0) as usize;
                let kind = parsed
                    .as_ref()
                    .and_then(|v| v.get("kind").and_then(|x| x.as_str()))
                    .unwrap_or("hold")
                    .to_string();
                let target = parsed
                    .as_ref()
                    .and_then(|v| v.get("target").and_then(|x| x.as_u64()))
                    .map(|x| x as u32);

                let result = apply_order(w, &body_str);
                state.commands.lock().unwrap().push(CmdRecord {
                    day,
                    unit,
                    kind,
                    target,
                    reject: result.clone(),
                    source: "player".into(),
                    reason: String::new(),
                });
                match &result {
                    Some(err) => trace(format!("   指令被拒：{err}")),
                    None => trace("   指令已写入".to_string()),
                }
                let mut v = snapshot(w);
                if let Some(err) = result {
                    v = format!("{{\"error\":\"{err}\",\"state\":{v}}}");
                }
                v
            };
            respond(&mut stream, 200, "application/json", &json)
        }
        // 预览版：人事（撤换主官）。和命令一样记进命令流——
        // 世界必须是 (模块, 种子, 命令流) 的函数，人事也算命令。
        ("POST", "/api/personnel") => {
            use serde_json::json;
            let v: serde_json::Value = serde_json::from_str(&body_str).unwrap_or(json!({}));
            let unit = v.get("unit").and_then(|x| x.as_u64()).unwrap_or(u64::MAX) as usize;
            let action = v
                .get("action")
                .and_then(|x| x.as_str())
                .unwrap_or("relieve")
                .to_string();
            if action != "relieve" {
                let msg = json!({ "error": format!("不认识的人事动作：{action}") });
                return respond(&mut stream, 400, "application/json", &msg.to_string());
            }
            let mut g = state.world.lock().unwrap();
            let Some(w) = g.as_mut() else {
                let msg = json!({ "error": "还没有开始战役" });
                return respond(&mut stream, 400, "application/json", &msg.to_string());
            };
            // 先在这个世界上试一次（拒绝就不落命令），再照命令流重放一次。
            match sim_core::politics::relieve(w, unit) {
                Ok((old, new_chief)) => {
                    let rec = CmdRecord {
                        day: w.tick,
                        unit,
                        kind: "relieve".into(),
                        target: None,
                        reject: None,
                        source: "player".into(),
                        reason: String::new(),
                    };
                    state.commands.lock().unwrap().push(rec);
                    let body = json!({
                        "ok": true,
                        "old": old,
                        "new": new_chief,
                        "state": serde_json::from_str::<serde_json::Value>(&snapshot(w)).unwrap(),
                    })
                    .to_string();
                    respond(&mut stream, 200, "application/json", &body)
                }
                Err(e) => {
                    let msg = json!({ "error": e });
                    respond(&mut stream, 400, "application/json", &msg.to_string())
                }
            }
        }
        // 预览版：回放浏览。**只读**——它按命令流重算一个世界出来给前端看，
        // 不碰正在跑的那一局。世界本来就是 (模块, 种子, 命令流) 的确定性函数，
        // 所以"看第 30 天"不需要录像文件，重算一次就是那天的样子。
        ("POST", "/api/replay") => {
            use serde_json::json;
            let v: serde_json::Value = serde_json::from_str(&body_str).unwrap_or(json!({}));
            let live_day = {
                let g = state.world.lock().unwrap();
                g.as_ref().map(|w| w.tick).unwrap_or(0)
            };
            let day = v
                .get("day")
                .and_then(|x| x.as_u64())
                .unwrap_or(live_day as u64) as u32;
            let day = day.min(live_day);
            let cmds = state.commands.lock().unwrap().clone();
            let module = state.module.lock().unwrap().clone();
            let w = rebuild(&module, &cmds, day);
            // 回放里的"今天发生了什么"：只给这一天的战报，否则右栏会被整局刷屏。
            let events: Vec<serde_json::Value> = w
                .log
                .iter()
                .filter(|e| e.tick == day)
                .rev()
                .take(40)
                .map(|e| {
                    json!({
                        "kind": e.kind.code(),
                        "text": e.text,
                        "side": e.side.map(|s| if s == Side::Cn { "cn" } else { "jp" }),
                    })
                })
                .collect();
            let body = json!({
                "state": serde_json::from_str::<serde_json::Value>(&snapshot(&w)).unwrap(),
                "day": day,
                "live_day": live_day,
                "hash": format!("{:016x}", w.state_hash()),
                "events": events,
                "commands": cmds.iter().filter(|c| c.day <= day && c.reject.is_none()).count(),
            })
            .to_string();
            respond(&mut stream, 200, "application/json", &body)
        }
        ("GET", "/api/replay-check") => {
            let (live_hash, live_tick) = {
                let g = state.world.lock().unwrap();
                let w = g.as_ref().unwrap();
                (w.state_hash(), w.tick)
            };
            let cmds = state.commands.lock().unwrap().clone();
            let json = replay_check(&state.module.lock().unwrap(), live_hash, live_tick, &cmds);
            respond(&mut stream, 200, "application/json", &json)
        }
        // 模块列表：`content/scenarios/*.imod`，读每个的 module.json 取名字。
        //
        // 这个游戏只提供框架，模块由玩家自己开发——那就不该让他们改命令行重启。
        ("GET", "/api/modules") => {
            use serde_json::json;
            // 路径比较要抹平分隔符：命令行给的是 `/`，read_dir 在 Windows 上给的是 `\`
            let cur = state.module_path.lock().unwrap().replace('\\', "/");
            let mut list = Vec::new();
            // 引擎自带的示例 + 外部模块仓库（IRONMIND_MODULES）
            for p in discover_modules() {
                // 目录与 .imod 压缩包都要能读标题——统一走 peek_meta_json。
                let meta = sim_content::peek_meta_json(&p);
                let path = p.to_string_lossy().replace('\\', "/");
                // 首页要拿这些字段把"战役卡"填起来：名字、规模、这仗在讲什么、
                // 你替哪一边打。模块写什么就显示什么，引擎不替它编。
                let g = |k: &str| {
                    meta.as_ref()
                        .and_then(|m| m.get(k))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                };
                list.push(json!({
                    "path": path,
                    "name": meta.as_ref().and_then(|m| m.get("name")).and_then(|v| v.as_str()).unwrap_or("?"),
                    "scale": meta.as_ref().and_then(|m| m.get("scale")).and_then(|v| v.as_str()).unwrap_or(""),
                    "current": path == cur,
                    "description": g("description"),
                    "playable_side": g("playable_side"),
                    "version": g("version"),
                    "domains": meta.as_ref().and_then(|m| m.get("domains")).cloned().unwrap_or(json!([])),
                }));
            }
            let roots: Vec<String> = module_roots()
                .iter()
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .collect();
            respond(
                &mut stream,
                200,
                "application/json",
                &json!({ "modules": list, "roots": roots }).to_string(),
            )
        }
        // 切换模块：整体替换世界，清空命令流/电报/公文。
        // 导入模块：把玩家选中的一堆文本文件写进 modules/<名字>/。
        //
        // 为什么要有：装一个战役以前要"把文件夹拷到 modules/ 再重启"——
        // 对能开资源管理器的人不算事，但那是安装文档的事，不是游戏的事。
        // 界面上「新的战役 → 导入模块」选一个文件夹就完事。
        //
        // 收的是 JSON 而不是 multipart：模块文件全是 UTF-8 文本
        // （module.json / map.json / oob.json / scenario.json / provinces_geo.json），
        // 手写 HTTP 服务器解析 multipart 不值得。
        ("POST", "/api/modules/import") => {
            use serde_json::json;
            let v: serde_json::Value = match serde_json::from_str(&body_str) {
                Ok(v) => v,
                Err(e) => {
                    let msg = json!({ "ok": false, "error": format!("请求不是 JSON：{e}") });
                    return respond(&mut stream, 400, "application/json", &msg.to_string());
                }
            };
            // ② 单个 `.imod` 压缩包：浏览器选一个文件，前端把它 base64 过来。
            //    这是发布之后的正常路径——玩家从 Releases 下到一个 .imod，直接导入。
            if let (Some(name), Some(b64)) = (
                v.get("file").and_then(|x| x.as_str()),
                v.get("base64").and_then(|x| x.as_str()),
            ) {
                let bytes = match decode_base64(b64) {
                    Some(b) => b,
                    None => {
                        let msg = json!({ "ok": false, "error": "base64 解不开" });
                        return respond(&mut stream, 400, "application/json", &msg.to_string());
                    }
                };
                // 先写到临时文件、用真正的加载器验一遍，再落到模块根。
                // 不能直接落盘：一个坏包/伪装成 .imod 的随便什么文件不该进模块目录。
                let tmp = std::env::temp_dir().join(format!(
                    "imod-{}-{}.imod",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0)
                ));
                if let Err(e) = std::fs::write(&tmp, &bytes) {
                    let msg = json!({ "ok": false, "error": format!("写临时文件失败：{e}") });
                    return respond(&mut stream, 500, "application/json", &msg.to_string());
                }
                let loaded = sim_content::load_module(&tmp);
                let _ = std::fs::remove_file(&tmp);
                let module = match loaded {
                    Ok(m) => m,
                    Err(e) => {
                        let msg = json!({ "ok": false, "error": format!("这不是一个可用的模块：{e}") });
                        return respond(&mut stream, 400, "application/json", &msg.to_string());
                    }
                };
                // 落盘名用模块 id 的尾段（和文件夹导入一致），不信客户端给的文件名。
                let dir = sanitize_slot(module.dir_name());
                if dir.is_empty() {
                    let msg = json!({ "ok": false, "error": "module.json 里的 id 不可用" });
                    return respond(&mut stream, 400, "application/json", &msg.to_string());
                }
                let root = import_root();
                let target = root.join(format!("{dir}.imod"));
                if target.exists() {
                    let msg = json!({ "ok": false, "error": format!("已经有一个同名模块：{dir}") });
                    return respond(&mut stream, 409, "application/json", &msg.to_string());
                }
                if let Err(e) = std::fs::write(&target, &bytes) {
                    let msg = json!({ "ok": false, "error": format!("写入失败：{e}") });
                    return respond(&mut stream, 500, "application/json", &msg.to_string());
                }
                trace(format!(
                    "   导入模块包 {name} → {}（{} KB）",
                    target.display(),
                    bytes.len() / 1024
                ));
                return respond(
                    &mut stream,
                    200,
                    "application/json",
                    &json!({
                        "ok": true,
                        "name": dir,
                        "path": target.to_string_lossy().replace('\\', "/"),
                        "files": 1,
                        "kind": "package",
                    })
                    .to_string(),
                );
            }
            let Some(files) = v.get("files").and_then(|f| f.as_array()) else {
                let msg = json!({ "ok": false, "error": "没有收到文件" });
                return respond(&mut stream, 400, "application/json", &msg.to_string());
            };
            // 目录名：优先用 module.json 里的 id 尾段，否则用请求里的 name
            let mut dir_name: Option<String> = None;
            let mut module_json: Option<serde_json::Value> = None;
            for f in files {
                let path = f.get("path").and_then(|p| p.as_str()).unwrap_or("");
                if path == "module.json" || path.ends_with("/module.json") {
                    if let Some(text) = f.get("text").and_then(|t| t.as_str()) {
                        module_json = serde_json::from_str::<serde_json::Value>(text).ok();
                    }
                }
            }
            match &module_json {
                None => {
                    let msg = json!({ "ok": false, "error": "这不是一个模块：顶层缺少可读的 module.json" });
                    return respond(&mut stream, 400, "application/json", &msg.to_string());
                }
                Some(m) => {
                    let id = m.get("id").and_then(|x| x.as_str()).unwrap_or("");
                    let name = m.get("name").and_then(|x| x.as_str()).unwrap_or("imported");
                    let raw = if id.contains('.') {
                        id.rsplit('.').next().unwrap_or(id)
                    } else if id.is_empty() {
                        name
                    } else {
                        id
                    };
                    dir_name = Some(sanitize_slot(raw));
                }
            }
            let Some(dir) = dir_name.filter(|d| !d.is_empty()) else {
                let msg = json!({ "ok": false, "error": "module.json 里的 id/name 不可用作目录名" });
                return respond(&mut stream, 400, "application/json", &msg.to_string());
            };
            // 落到哪个根：第一个"可写且不是引擎自带"的模块根
            let root = import_root();
            let target = root.join(format!("{dir}.imod"));
            if target.exists() {
                let msg = json!({ "ok": false, "error": format!("已经有一个同名模块：{}", dir) });
                return respond(&mut stream, 409, "application/json", &msg.to_string());
            }
            if let Err(e) = std::fs::create_dir_all(&target) {
                let msg = json!({ "ok": false, "error": format!("建目录失败：{e}") });
                return respond(&mut stream, 500, "application/json", &msg.to_string());
            }
            let mut written = 0usize;
            for f in files {
                let Some(path) = f.get("path").and_then(|p| p.as_str()) else { continue };
                // 只要文件名安全的：不许绝对路径、不许 ..、只收文本类
                let rel = path.trim_start_matches('/');
                if rel.contains("..") || rel.contains(':') || rel.contains('\\') {
                    continue;
                }
                let ext = rel.rsplit('.').next().unwrap_or("").to_lowercase();
                if !matches!(ext.as_str(), "json" | "md" | "txt") {
                    continue;
                }
                let Some(text) = f.get("text").and_then(|t| t.as_str()) else { continue };
                let dest = target.join(rel);
                if let Some(parent) = dest.parent() {
                    if std::fs::create_dir_all(parent).is_err() {
                        continue;
                    }
                }
                if std::fs::write(&dest, text).is_ok() {
                    written += 1;
                }
            }
            trace(format!("   导入模块 → {}（{written} 个文件）", target.display()));
            respond(
                &mut stream,
                200,
                "application/json",
                &json!({
                    "ok": true,
                    "name": dir,
                    "path": target.to_string_lossy().replace('\\', "/"),
                    "files": written,
                })
                .to_string(),
            )
        }
        // 切换模块：整体替换世界，清空命令流/电报/公文，并给世界换代号。
        ("POST", "/api/module") => {
            use serde_json::json;
            let v: serde_json::Value = serde_json::from_str(&body_str).unwrap_or(json!({}));
            let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("").to_string();
            // 只允许"某个模块根目录下的 .imod"：根 = IRONMIND_MODULES 里列的 + content/scenarios。
            // 别让一个网页把服务器指向任意目录。
            let p = PathBuf::from(&path);
            let canon = std::fs::canonicalize(&p).ok();
            let ok_path = path.ends_with(".imod")
                && !path.contains("..")
                && canon.as_ref().map_or(false, |c| {
                    module_roots().iter().any(|r| {
                        std::fs::canonicalize(r)
                            .map(|rc| c.starts_with(&rc))
                            .unwrap_or(false)
                    })
                });
            if !ok_path {
                let _ = respond(&mut stream, 400, "application/json", "{\"error\":\"模块路径不合法\"}");
                return Ok(());
            }
            if let Err(e) = activate_module(state, &path) {
                let msg = json!({ "error": format!("加载失败：{e}") });
                let _ = respond(&mut stream, 400, "application/json", &msg.to_string());
                return Ok(());
            }
            trace(format!("   切换模块 → {path}"));
            let snap = snapshot(state.world.lock().unwrap().as_ref().unwrap());
            respond(&mut stream, 200, "application/json", &snap)
        }
        // 首页要显示"引擎是什么版本"。玩家的 bug 报告里有没有这一行，差别很大。
        ("GET", "/api/version") => {
            use serde_json::json;
            // `built` = 内核 exe 的修改时间（秒）。这才是"你到底在跑哪一版"：
            // 同一版本号编译很多次之后，7 天前的 exe 和今天的看起来一模一样。
            let built = std::env::current_exe()
                .ok()
                .and_then(|p| p.metadata().ok())
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let body = json!({
                "engine": env!("CARGO_PKG_VERSION"),
                "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
                "built": built,
                // 这个引擎**会**哪些预览版机制。模块声明了但引擎不支持时，
                // 前端要明说"你的引擎太旧"，而不是默默少几个面板。
                "preview": ["ammo_fuel", "rail_damage", "politics", "front_line", "replay", "arrivals"],
            })
            .to_string();
            respond(&mut stream, 200, "application/json", &body)
        }        ("GET", "/api/llm") => {
            use serde_json::json;
            let c = cfg(&state);
            let body = json!({
                "enabled": c.enabled,
                // 这一次判断到底会不会真的调模型。前端据此区分"接了模型"和
                // "本地规则顶上"——两者都能玩，但不是一回事。
                "uses_model": c.uses_model(),
                "generation": state.generation.load(std::sync::atomic::Ordering::SeqCst),
                "mode": c.describe(),
                "model": c.model,
                "base": c.base,
                // 密钥绝不回传，只告诉前端"填没填"
                "has_key": !c.key.trim().is_empty(),
                "local_only": c.local_only,
                // 首次设置做过没有：做过就不再自动弹出来
                "setup_done": std::env::var("IRONMIND_SETUP").map(|v| v == "done").unwrap_or(false),
                "interval_days": c.interval_days,
                "calls": *state.brain.calls.lock().unwrap(),
                "ready": state.brain.ready.lock().unwrap().len(),
                "errors": state.brain.errors.lock().unwrap().clone(),
            })
            .to_string();
            respond(&mut stream, 200, "application/json", &body)
        }
        // 首次设置 / 设置面板：写 settings.env + 立刻生效（不重启）。
        //
        // 为什么要有这个接口：装完软件的人不应该被要求"去改一个文本文件再重启"。
        // 键值照旧写进 settings.env——文件仍然是唯一事实，接口只是它的编辑器。
        ("POST", "/api/settings") => {
            use serde_json::json;
            let v: serde_json::Value = serde_json::from_str(&body_str).unwrap_or(json!({}));
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|x| x.trim().to_string());
            let mode = s("mode").unwrap_or_default(); // "model" | "local"
            let base = s("base");
            let model = s("model");
            let key = s("api_key");

            let mut pairs: Vec<(&str, Option<String>)> = Vec::new();
            match mode.as_str() {
                "model" => {
                    pairs.push(("IRONMIND_LLM", Some("auto".into())));
                    if let Some(b) = base.filter(|b| !b.is_empty()) {
                        if !b.starts_with("http") {
                            let msg = json!({ "ok": false, "error": "API 地址要以 http(s):// 开头" });
                            return respond(&mut stream, 400, "application/json", &msg.to_string());
                        }
                        pairs.push(("IRONMIND_API_BASE", Some(b)));
                    }
                    if let Some(m) = model.filter(|m| !m.is_empty()) {
                        pairs.push(("IRONMIND_MODEL", Some(m)));
                    }
                    if let Some(k) = key.filter(|k| !k.is_empty()) {
                        pairs.push(("IRONMIND_API_KEY", Some(k)));
                    }
                }
                "local" => {
                    // 明确的"先不接模型"：有 key 也不用
                    pairs.push(("IRONMIND_LLM", Some("local".into())));
                }
                _ => {
                    let msg = json!({ "ok": false, "error": "mode 必须是 model 或 local" });
                    return respond(&mut stream, 400, "application/json", &msg.to_string());
                }
            }
            pairs.push(("IRONMIND_SETUP", Some("done".into())));

            if let Err(e) = merge_settings_env(&pairs) {
                let msg = json!({ "ok": false, "error": e });
                return respond(&mut stream, 500, "application/json", &msg.to_string());
            }
            // 写回进程环境，再按环境重算一份配置——文件和内存不会各说一套。
            for (k, val) in &pairs {
                match val {
                    Some(x) => std::env::set_var(k, x),
                    None => std::env::remove_var(k),
                }
            }
            let fresh = llm::Config::from_env();
            let mode_text = fresh.describe();
            let uses = fresh.uses_model();
            *state.llm.lock().unwrap() = fresh;
            trace(format!("   配置已更新：{mode_text}"));
            respond(
                &mut stream,
                200,
                "application/json",
                &json!({ "ok": true, "mode": mode_text, "uses_model": uses }).to_string(),
            )
        }
        // 测试连接：拿还没保存的 base/model/key 真的打一次模型。
        ("POST", "/api/settings/test") => {
            use serde_json::json;
            let v: serde_json::Value = serde_json::from_str(&body_str).unwrap_or(json!({}));
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|x| x.trim().to_string());
            let mut c = cfg(&state);
            if let Some(b) = s("base").filter(|b| !b.is_empty()) {
                c.base = b;
            }
            if let Some(m) = s("model").filter(|m| !m.is_empty()) {
                c.model = m;
            }
            if let Some(k) = s("api_key").filter(|k| !k.is_empty()) {
                c.key = k;
            }
            let out = match llm::ping(&c) {
                Ok(t) => json!({ "ok": true, "reply": t }),
                Err(e) => json!({ "ok": false, "error": e }),
            };
            respond(&mut stream, 200, "application/json", &out.to_string())
        }
        ("GET", "/api/reports") => {
            use serde_json::json;
            let list = state.reports.lock().unwrap();
            let arr: Vec<_> = list
                .iter()
                .rev()
                .take(40)
                .map(|r| {
                    json!({
                        "id": r.id,
                        "day": r.day,
                        "kind": r.kind,
                        "unit": r.unit,
                        "unit_name": r.unit_name,
                        "general": r.general,
                        "title": r.title,
                        "text": r.text,
                        "options": r.options,
                        "deadline": r.deadline,
                        "status": r.status, "from_telegram": r.from_telegram,
                    })
                })
                .collect();
            drop(list);
            respond(&mut stream, 200, "application/json", &json!({ "reports": arr }).to_string())
        }
        ("POST", "/api/report") => {
            use serde_json::json;
            let v: serde_json::Value = serde_json::from_str(&body_str).unwrap_or(json!({}));
            let id = v.get("id").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
            // 前端发 {id, choice:"yes"|"no"}；也接受 approve: true/false（脚本方便）。
            // 两个都没有就**明确报错**，不要悄悄当成"驳回"——
            // 悄悄反驳会把玩家的一次"准他改令"变成"维持原令"，而且完全看不出错在哪。
            let approve = match (
                v.get("choice").and_then(|x| x.as_str()),
                v.get("approve").and_then(|x| x.as_bool()),
            ) {
                (Some("yes"), _) => Some(true),
                (Some("no"), _) => Some(false),
                (_, Some(b)) => Some(b),
                _ => None,
            };
            let Some(approve) = approve else {
                let msg = json!({ "ok": false, "error": "缺少 choice（yes/no）或 approve" });
                return respond(&mut stream, 400, "application/json", &msg.to_string());
            };
            let msg = {
                let mut guard = state.world.lock().unwrap();
                let w = guard.as_mut().unwrap();
                answer_report(&state, w, id, approve)
            };
            trace(format!("   批复公文 #{id}：{msg}"));
            let json = {
                let guard = state.world.lock().unwrap();
                let snap = snapshot(guard.as_ref().unwrap());
                json!({ "ok": true, "msg": msg, "state": serde_json::from_str::<serde_json::Value>(&snap).unwrap_or(json!({})) })
            };
            respond(&mut stream, 200, "application/json", &json.to_string())
        }
        ("POST", "/api/dispatch") => {
            use serde_json::json;
            let v: serde_json::Value = serde_json::from_str(&body_str).unwrap_or(json!({}));
            let to = v.get("to").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            let out = {
                let g = state.world.lock().unwrap();
                let w = g.as_ref().unwrap();
                if text.is_empty() {
                    json!({ "ok": false, "error": "电报内容不能为空" })
                } else if text.chars().count() > 300 {
                    json!({ "ok": false, "error": "电报太长了，控制在 300 字以内" })
                } else {
                    // 通信延迟：电报按线路传到兵团部，再由参谋分下去
                    // （电报本身是秒级的，这里的延迟是参谋作业与下达，见 docs/10）
                    let delay = w.comms.command_delay();
                    let arrive = w.tick + delay;
                    let id = state
                        .next_telegram_id
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    state.telegrams.lock().unwrap().push(Telegram {
                        id,
                        day_sent: w.tick,
                        arrive_day: arrive,
                        to: to.clone(),
                        text: text.split("　回电：").next().unwrap_or(&text).to_string(),
                        status: "enroute",
                        reply: String::new(),
                        traces: Vec::new(),
                    });
                    // 打印的是**实际要等几天**，不是 comms 里的原始参数。
                    // 以前打 delay=0，读日志的人会以为电报当天就到了。
                    trace(format!(
                        "   发电报 #{id} → {to}，第 {arrive} 日送达（{} 日后）",
                        arrive.saturating_sub(w.tick)
                    ));
                    json!({ "ok": true, "id": id, "arrive_day": arrive })
                }
            };
            respond(&mut stream, 200, "application/json", &out.to_string())
        }
        // 通话：拿起电话问将领一句话。**不下命令**，也不进命令流。
        //
        // 为什么要有：电报只有"下达"这一个功能，玩家的好奇没法落地——
        // 想知道"你那儿还剩多少粮""当面敌军到底几个师"，只能靠猜。
        // 而真实指挥员最常用的动作恰恰是问一句。
        //
        // 回答是**他的话**，不是上帝视角：他只知道自己的侦察、部下报上来的、
        // 以及他自己愿意说的。所以这里不能拿真值直接回给玩家。
        ("POST", "/api/ask") => {
            use serde_json::json;
            let v: serde_json::Value = serde_json::from_str(&body_str).unwrap_or(json!({}));
            let idx = v.get("unit").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            let text = v
                .get("text")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if text.is_empty() {
                let msg = json!({ "ok": false, "error": "问点什么？" });
                return respond(&mut stream, 400, "application/json", &msg.to_string());
            }
            // 先把上下文取出来再放锁：模型要几秒，不能把整个世界锁住
            let ask = {
                let g = state.world.lock().unwrap();
                g.as_ref().and_then(|w| build_ask_kind(&state, w, idx, "", false))
            };
            let Some(ask) = ask else {
                let msg = json!({ "ok": false, "error": "这个番号联系不上" });
                return respond(&mut stream, 404, "application/json", &msg.to_string());
            };
            let who = ask.general.clone();
            let reply = match llm::talk(&cfg(&state), &ask, &text) {
                Ok(t) => t,
                Err(e) => format!("（联络不上：{e}）"),
            };
            trace(format!("   通话 {who}：{text} → {reply}"));
            {
                // 他说过的话要记住：下一封电报的判断该受影响。
                let mut g = state.talk_log.lock().unwrap();
                let e = g.entry(idx).or_default();
                e.push(format!("（通话）上峰问：{text}"));
                e.push(format!("（通话）{who}答：{reply}"));
                let n = e.len();
                if n > 12 { e.drain(0..n - 12); }
            }
            respond(
                &mut stream,
                200,
                "application/json",
                &json!({ "ok": true, "general": who, "reply": reply }).to_string(),
            )
        }
        ("GET", "/api/telegrams") => {
            use serde_json::json;
            let list = state.telegrams.lock().unwrap();
            let arr: Vec<_> = list
                .iter()
                .rev()
                .take(20)
                .map(|t| {
                    json!({
                        "id": t.id, "day_sent": t.day_sent, "arrive_day": t.arrive_day,
                        "to": t.to, "text": t.text, "status": t.status, "reply": t.reply,
                        "traces": t.traces.iter().map(|x| json!({
                            "day": x.day, "unit": x.unit, "unit_name": x.unit_name,
                            "general": x.general, "kind": x.kind, "target": x.target,
                            "same": x.same, "confidence": round2(x.confidence),
                            "reason": x.reason,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            drop(list);
            respond(&mut stream, 200, "application/json", &json!({ "telegrams": arr }).to_string())
        }
        // 存档槽位列表：名字、模块、天数、命令条数、修改时间。
        ("GET", "/api/saves") => {
            use serde_json::json;
            let mut arr = Vec::new();
            let mut files: Vec<_> = std::fs::read_dir("saves")
                .map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>())
                .unwrap_or_default();
            files.sort();
            for p in files {
                if p.extension().and_then(|x| x.to_str()) != Some("json") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&p) else { continue };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
                let slot = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let modified = p
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let mid = v
                    .get("module_id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                // 存档属于哪场战役，要能看见。读档现在会自动切模块，
                // 但玩家在"读取存档"列表里还是得知道这一条是哪一仗的。
                let mname = mid_display_name(&mid);
                arr.push(json!({
                    "slot": slot,
                    "module_id": mid,
                    "module_name": mname,
                    "day": v.get("day").cloned().unwrap_or(json!(0)),
                    "commands": v.get("commands").and_then(|c| c.as_array()).map(|a| a.len()).unwrap_or(0),
                    "modified": modified,
                }));
            }
            // 最新的排最前：首页的"继续"直接取第一条，
            // 以前按路径排，于是"auto"永远排在"第20日"前面，读到的不一定是最新的。
            arr.sort_by_key(|v| {
                std::cmp::Reverse(v.get("modified").and_then(|m| m.as_u64()).unwrap_or(0))
            });
            respond(&mut stream, 200, "application/json", &json!({ "saves": arr }).to_string())
        }        ("POST", "/api/save") => {
            use serde_json::json;
            let out = {
                // 槽位：给了名字就用名字，没给就按时间戳（向后兼容）。
                // 名字要消毒——它会被拼进文件路径，不能让它带 ../ 或分隔符。
                let slot = serde_json::from_str::<serde_json::Value>(&body_str)
                    .ok()
                    .and_then(|v| v.get("slot").and_then(|x| x.as_str()).map(|s| s.to_string()))
                    .map(|s| sanitize_slot(&s))
                    .filter(|s| !s.is_empty());
                let g = state.world.lock().unwrap();
                let w = g.as_ref().unwrap();
                match write_save(&state, w, slot) {
                    Ok((name, day, n)) => {
                        trace(format!("   存档 {name} （第 {day} 日，{n} 条命令）"));
                        json!({ "ok": true, "path": name, "day": day, "commands": n })
                    }
                    Err(e) => json!({ "ok": false, "error": e }),
                }
            };
            respond(&mut stream, 200, "application/json", &out.to_string())
        }
        ("POST", "/api/load") => {
            use serde_json::json;
            // 三种写法：`slot`（槽位名）、`path`（完整路径）、都不给（载入最新）
            let body: serde_json::Value =
                serde_json::from_str(&body_str).unwrap_or_else(|_| json!({}));
            let want: Option<String> = body
                .get("slot")
                .and_then(|x| x.as_str())
                .map(|s| format!("saves/{}.json", sanitize_slot(s)))
                .or_else(|| {
                    body.get("path")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string())
                });
            let path = match want.or_else(latest_save) {
                Some(p) => p,
                None => {
                    let msg = json!({ "ok": false, "error": "没有存档" }).to_string();
                    return respond(&mut stream, 200, "application/json", &msg);
                }
            };
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let parsed: Result<SaveFile, _> = serde_json::from_str(&text);
            match parsed {
                Ok(sv) => {
                    // 存档属于**另一场战役**怎么办？
                    //
                    // 以前这里是直接报错返回："存档属于模块 A，当前是 B"。玩家看到的是
                    // 首页"继续（第 10 日）"点下去**毫无反应**——只有一条一闪而过的 toast，
                    // 而且不知道该怎么办（去设置里换战役？换哪个？）。
                    //
                    // 现在：自动找到那个模块并切过去，再读档。首页的"继续"就该是
                    // "接着上次那一仗打"，不该要求玩家先记住上次玩的是哪个模块。
                    let mut switched: Option<String> = None;
                    {
                        let current_id = state.module.lock().unwrap().meta.id.clone();
                        if sv.module_id != current_id {
                            let found = discover_modules().into_iter().find(|p| {
                                sim_content::peek_meta_json(p)
                                    .and_then(|m| {
                                        m.get("id")
                                            .and_then(|v| v.as_str())
                                            .map(|s| s.to_string())
                                    })
                                    == Some(sv.module_id.clone())
                            });
                            match found {
                                Some(p) => {
                                    let path = p.to_string_lossy().to_string();
                                    match activate_module(state, &path) {
                                        Ok(m) => switched = Some(m.meta.name.clone()),
                                        Err(e) => {
                                            let msg = json!({
                                                "ok": false,
                                                "error": format!("这个存档属于「{}」，但加载它失败了：{e}", sv.module_id),
                                            })
                                            .to_string();
                                            return respond(&mut stream, 200, "application/json", &msg);
                                        }
                                    }
                                }
                                None => {
                                    let msg = json!({
                                        "ok": false,
                                        "error": format!(
                                            "这个存档属于「{}」，但这台机器上已经找不到那个模块了（可能被删了或挪走了）。",
                                            sv.module_id
                                        ),
                                    })
                                    .to_string();
                                    return respond(&mut stream, 200, "application/json", &msg);
                                }
                            }
                        }
                    }
                    let world = rebuild(&state.module.lock().unwrap(), &sv.commands, sv.day);
                    let hash = world.state_hash();
                    let expect = u64::from_str_radix(sv.state_hash.trim(), 16).unwrap_or(0);
                    let matched = hash == expect;
                    {
                        let mut g = state.world.lock().unwrap();
                        *g = Some(world);
                        *state.commands.lock().unwrap() = sv.commands.clone();
                    }
                    trace(format!(
                        "   读档 {} （第 {} 日，{} 条命令，哈希{}）",
                        path,
                        sv.day,
                        sv.commands.len(),
                        if matched { "一致" } else { "不一致！" }
                    ));
                    let g = state.world.lock().unwrap();
                    let snap = snapshot(g.as_ref().unwrap());
                    drop(g);
                    let msg = json!({
                        "ok": true,
                        "path": path,
                        "day": sv.day,
                        "hash_matched": matched,
                        // 顺带换了战役的话要告诉前端：地图换了，得重拉省界。
                        "switched_to": switched,
                        "state": serde_json::from_str::<serde_json::Value>(&snap).unwrap_or(json!({})),
                    })
                    .to_string();
                    respond(&mut stream, 200, "application/json", &msg)
                }
                Err(e) => {
                    let msg = json!({ "ok": false, "error": format!("存档解析失败：{e}") }).to_string();
                    respond(&mut stream, 200, "application/json", &msg)
                }
            }
        }
        ("POST", "/api/reset") => {
            let json = {
                let mut guard = state.world.lock().unwrap();
                let mut nw = build_world(&state.module.lock().unwrap());
                sim_core::intel::run_intel(&mut nw);
                *guard = Some(nw);
                // 重开 = 清空命令流，否则回放会带着上一局的命令
                state.commands.lock().unwrap().clear();
                // 电报和公文也是上一局的。不清的话"重开"之后电报台上
                // 还挂着上一局的电报、公文堆还挂着上一局的请示——
                // 玩家会以为重开没生效（实际发生过）。
                state.telegrams.lock().unwrap().clear();
                state.reports.lock().unwrap().clear();
                snapshot(guard.as_ref().unwrap())
            };
            respond(&mut stream, 200, "application/json", &json)
        }
        _ => respond(&mut stream, 404, "text/plain", "not found"),
    }
}

/// 处理一条意图指令。
///
/// 请求体：`{"unit": 3, "kind": "attack", "target": 5}`
/// `kind` 可以是 hold / attack / move / retreat。
/// 玩家的意图会写进单位的 `order`，下一 tick 由将领解释（可能抗命）。
fn apply_order(w: &mut World, body: &str) -> Option<String> {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return Some(format!("JSON 解析失败：{e}")),
    };
    let unit_idx = v.get("unit").and_then(|x| x.as_u64())? as usize;
    let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("hold");
    let target = v.get("target").and_then(|x| x.as_u64()).map(|x| x as u32);
    apply_typed(w, unit_idx, kind, target)
}

/// 真正执行一条命令。**回放用的也是这个函数**，所以校验与副作用必须全在这里。
///
/// 这就是 OpenRA `ResolveOrder` 的角色：命令是数据，执行是纯函数式的
/// `(world, command) -> Option<拒绝原因>`，回放时喂同一串命令就能重建世界。
fn apply_typed(w: &mut World, unit_idx: usize, kind: &str, target: Option<u32>) -> Option<String> {
    let u = w.units.get(unit_idx)?;
    if !u.is_alive() {
        return Some("该单位已不存在".into());
    }
    let from = u.position;
    let side = u.side;
    let is_ground = u.kind.is_ground();

    // 语义校正：前端只发"我点了哪个省"，进攻还是增援由目标归属决定。
    //
    // 之前的写法把 `kind` 原样写进 order，于是点击一个**自己控制的**省
    // 也会变成 `Order::Attack(自己的省)`，界面还显示"已下令：进攻 沙河"。
    // 同阵营并不会真的打起来，但这是明显的语义错误；再叠上堆叠上限，
    // 表现就是"点了没反应"。
    let (kind, target) = match (kind, target) {
        ("fortify", _) => ("fortify", None),
        ("rest", _) => ("rest", None),
        ("shift", Some(n)) => ("shift", Some(n)),
        // 预览版：炸路 / 抢修。没开这个开关的战役里，这两条命令必须**明确拒绝**，
        // 而不是静默变成"固守"——否则玩家会以为飞机出去了，其实什么都没发生。
        ("bomb_rail" | "repair_rail", Some(t)) => {
            if !w.fx.rail_damage {
                return Some(
                    "这个战役没有开「铁路被炸」（scenario.json 的 preview.rail_damage），\
                     炸路与抢修都无效"
                        .into(),
                );
            }
            if t as usize >= w.provinces.len() {
                return Some("目标省份不存在".into());
            }
            if !w.provinces[t as usize].rail {
                return Some(format!("{}没有铁路线", w.provinces[t as usize].name));
            }
            if kind == "bomb_rail" {
                if u.kind != sim_core::UnitKind::Air {
                    return Some(format!("只有空军能炸铁路，{}是{}", u.name, u.kind.label()));
                }
                if t == from {
                    return Some("不能炸自己脚下的铁路".into());
                }
            } else {
                if !u.kind.is_ground() {
                    return Some("抢修要地面部队就近动手".into());
                }
                if t != from {
                    return Some(format!(
                        "抢修只能修脚下这一段：{}不在这里",
                        w.provinces[t as usize].name
                    ));
                }
            }
            (kind, Some(t))
        }
        ("attack" | "move" | "retreat", Some(t)) => {
            if t as usize >= w.provinces.len() {
                return Some("目标省份不存在".into());
            }
            if t == from {
                ("hold", None) // 点自己所在的省 = 固守
            } else if is_ground
                && sim_core::systems::hops_to(w, from, t, side, u.kind).is_none()
            {
                // 不再要求相邻：**千里奔袭是常态**（见 sim-core `walk`）。
                // 只拦真正走不通的：地面部队要跨水，或根本没有陆路。
                return Some(format!(
                    "{}走不到{}：陆路上不通（要跨水，得走水路或换个目标）",
                    w.units[unit_idx].name, w.provinces[t as usize].name
                ));
            } else if !is_ground
                && u.kind == sim_core::UnitKind::Naval
                && w.provinces[t as usize].terrain != Terrain::Water
            {
                // 舰队只在江面上活动。让它"开进城里"是毫无意义的命令，
                // 静默忽略比报错更糟：玩家会以为命令生效了。
                return Some(format!(
                    "舰队只能沿水道移动，{}是陆地",
                    w.provinces[t as usize].name
                ));
            } else {
                let dest = &w.provinces[t as usize];
                if is_ground && dest.terrain == Terrain::Water {
                    return Some("地面部队不能进入水域".into());
                }
                // 敌方空省：走进去就是占领，不该报"进攻"
                let empty_enemy = dest.owner != side
                    && w.units_in(t).iter().all(|&i| {
                        let x = &w.units[i as usize];
                        x.side == side || !x.kind.is_ground()
                    });
                let kind = match kind {
                    "attack" if dest.owner == side => "move", // 我方省 = 移防/增援
                    "attack" if empty_enemy => "move",
                    other => other,
                };
                (kind, Some(t))
            }
        }
        _ => ("hold", None),
    };

    let final_kind = kind;
    let u = w.units.get_mut(unit_idx)?;
    // 只有 "auto" 交还控制权，其余都算玩家亲自下的命令。
    u.manual = final_kind != "auto";
    u.order = match (final_kind, target) {
        ("attack", Some(t)) => Order::Attack(t),
        ("move", Some(t)) => Order::Move(t),
        ("retreat", Some(t)) => Order::Retreat(t),
        ("shift", Some(n)) => Order::Shift(n as u8),
        ("fortify", _) => Order::Fortify,
        ("rest", _) => Order::Rest,
        ("bomb_rail", Some(t)) => Order::BombRail(t),
        ("repair_rail", Some(t)) => Order::RepairRail(t),
        _ => Order::Hold,
    };
    None
}

/// 一条玩家命令的记录。`(模块, 种子, 命令流)` 就是一份完整存档。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CmdRecord {
    day: u32,
    unit: usize,
    kind: String,
    target: Option<u32>,
    #[serde(default)]
    reject: Option<String>,
    /// 谁下的：player / llm / auto
    #[serde(default)]
    source: String,
    /// 将领的解释（llm 命令才有）
    #[serde(default)]
    reason: String,
}

/// 按"命令记录"重放一条命令。**实时与回放共用这一段**。
///
/// 为什么不能只调 `apply_typed`：LLM 命令除了写 order，还会改动
/// `manual / disobeyed / dissent_count`，而这些都进状态哈希。
/// 两条路径只要有一处不一致，回放校验就会挂——这正是在替我抓这类不一致。
fn apply_record(w: &mut World, c: &CmdRecord) {
    // 预览版人事：撤换不是"命令"，但它同样改变状态，所以必须走命令流，
    // 否则回放算出来的世界和实时世界对不上（回放校验会当场抓住）。
    if c.kind == "relieve" {
        let _ = sim_core::politics::relieve(w, c.unit);
        return;
    }
    if apply_typed(w, c.unit, &c.kind, c.target).is_some() {
        return; // 被拒的命令不产生状态变化
    }
    if c.source == "llm" {
        if let Some(u) = w.units.get_mut(c.unit) {
            u.manual = true; // 将领自己拿的主意，本 tick 规则 AI 不许覆盖
            if !c.reason.is_empty() {
                u.disobeyed = true;
                u.dissent_reason = Some(c.reason.clone());
                u.dissent_count += 1;
            }
        }
    }
}

/// 用命令流重建世界（回放、读档共用同一段代码）。
///
/// 这是整套架构的地基：世界 = `(模块, 种子, 命令流)` 的确定性函数。
/// 只要这条成立，存档就只是把命令流写下来，回放就是再跑一遍。
fn rebuild(module: &sim_content::LoadedModule, cmds: &[CmdRecord], until_day: u32) -> World {
    let mut w = build_world(module);
    sim_core::intel::run_intel(&mut w);
    for day in 0..=until_day {
        for c in cmds.iter().filter(|c| c.day == day && c.reject.is_none()) {
            apply_record(&mut w, c);
        }
        if day == until_day {
            break;
        }
        sim_core::systems::tick(&mut w);
    }
    w
}

/// 用命令流重建世界，和实时世界比对哈希。
///
/// 通过 = 这一局是 `(模块, 种子, 命令流)` 的确定性函数，回放/存档/复现 bug 都成立。
/// 不通过 = 有状态没被命令流覆盖（漏记命令、或者模拟里用了隐藏的全局状态）。
fn replay_check(
    module: &sim_content::LoadedModule,
    live_hash: u64,
    live_tick: u32,
    cmds: &[CmdRecord],
) -> String {
    use serde_json::json;
    let fresh = rebuild(module, cmds, live_tick);
    let applied = cmds.iter().filter(|c| c.day <= live_tick && c.reject.is_none()).count();
    let fh = fresh.state_hash();
    json!({
        "ok": live_hash == fh,
        "day": live_tick,
        "live_hash": format!("{live_hash:016x}"),
        "replay_hash": format!("{fh:016x}"),
        "commands": cmds.len(),
        "applied": applied,
    })
    .to_string()
}

/// 存档内容。命令流 + 元信息，不含世界快照。
#[derive(serde::Serialize, serde::Deserialize)]
struct SaveFile {
    v: u32,
    module_id: String,
    module_version: String,
    seed: u64,
    day: u32,
    state_hash: String,
    commands: Vec<CmdRecord>,
}

/// 存档槽位名消毒。
///
/// 这个名字会被拼进文件路径，所以只留字母数字、`-`、`_`、下划线以外的
/// 一概换成 `-`，并砍到 40 字符。空名 = 不指定槽位（走时间戳）。
/// 模块 id → 显示名（存档列表里用）。找不到就返回 id 本身，不编造。
fn mid_display_name(module_id: &str) -> String {
    if module_id.is_empty() {
        return String::new();
    }
    for p in discover_modules() {
        if let Some(m) = sim_content::peek_meta_json(&p) {
            if m.get("id").and_then(|v| v.as_str()) == Some(module_id) {
                if let Some(name) = m.get("name").and_then(|v| v.as_str()) {
                    return name.to_string();
                }
            }
        }
    }
    module_id.to_string()
}

/// 把一个模块装进正在跑的服务：切模块、以及"读档时顺带换战役"都走这里。
///
/// 为什么抽出来：这两条路径要做的事情一字不差（换模块、换省界、重建世界、
/// 清空命令/电报/公文、世界代号 +1）。分开写迟早会漏一条——
/// 漏掉 `telegrams.clear()` 的那次表现是"重开之后电报台还挂着上一局的电报"。
fn activate_module(
    state: &Arc<ServerState>,
    path: &str,
) -> Result<sim_content::LoadedModule, String> {
    let loaded = sim_content::load_module(&std::path::PathBuf::from(path))
        .map_err(|e| format!("{e}"))?;
    let mut world = build_world(&loaded);
    sim_core::intel::run_intel(&mut world);
    *state.module.lock().unwrap() = loaded.clone();
    *state.module_path.lock().unwrap() = path.to_string();
    *state.provinces_geo.lock().unwrap() = loaded.geo.clone();
    *state.world.lock().unwrap() = Some(world);
    state.commands.lock().unwrap().clear();
    state.reports.lock().unwrap().clear();
    state.telegrams.lock().unwrap().clear();
    state
        .generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Ok(loaded)
}

/// 解 base64（标准字母表，容忍换行与 `=` 补齐）。
///
/// 为什么手写而不是拉一个 crate：只需要"把浏览器 `btoa` 出来的东西还原成字节"
/// 这一件事，20 行能写完，还有单测盯着；多一个依赖反而要多维护一条供应链。
fn decode_base64(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        if c == b'=' || c == b'\n' || c == b'\r' || c == b' ' || c == b'\t' {
            continue;
        }
        let v = val(c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn sanitize_slot(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    cleaned.trim_matches('-').chars().take(40).collect()
}

/// 找最新的一份存档。
///
/// 以前只认 `save-<时间戳>.json`（"不指定槽位"时自动起的名字），
/// 于是玩家起了名字的存档、以及自动存档，都不算"最新"——
/// 首页那个"继续"就会读到一个旧的，或者干脆说"没有存档"。
/// 现在按**文件修改时间**取最新，名字随便起。
fn latest_save() -> Option<String> {
    let dir = std::fs::read_dir("saves").ok()?;
    let mut best: Option<(std::time::SystemTime, String)> = None;
    for e in dir.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        let Ok(t) = meta.modified() else { continue };
        if best.as_ref().map_or(true, |(b, _)| t > *b) {
            best = Some((t, path.to_string_lossy().replace('\\', "/")));
        }
    }
    best.map(|(_, p)| p)
}

/// 写一份存档：世界快照不要，存的是**命令流**（模块 + 种子 + 命令）。
///
/// 抽成函数是因为它现在有两个调用者：玩家点"存档"，和每几天的自动存档。
/// 调用者必须已经持有 `world` 锁——这里会再锁 `commands` 与 `module`，
/// 顺序必须和 /api/save 一致（world → commands → module），否则会自死锁。
fn write_save(
    state: &Arc<ServerState>,
    w: &World,
    slot: Option<String>,
) -> Result<(String, u32, usize), String> {
    let cmds: Vec<CmdRecord> = state
        .commands
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.reject.is_none())
        .cloned()
        .collect();
    let (mid, mver) = {
        let m = state.module.lock().unwrap();
        (m.meta.id.clone(), m.meta.version.clone())
    };
    let payload = SaveFile {
        v: 1,
        module_id: mid,
        module_version: mver,
        seed: w.seed,
        day: w.tick,
        state_hash: format!("{:016x}", w.state_hash()),
        commands: cmds,
    };
    let name = match slot.filter(|s| !s.is_empty()) {
        Some(s) => format!("saves/{s}.json"),
        None => format!(
            "saves/save-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        ),
    };
    std::fs::create_dir_all("saves").map_err(|e| format!("建目录失败：{e}"))?;
    let text = serde_json::to_string_pretty(&payload).map_err(|e| format!("序列化失败：{e}"))?;
    std::fs::write(&name, text).map_err(|e| format!("写文件失败：{e}"))?;
    Ok((name, payload.day, payload.commands.len()))
}

/// 取一份当前配置的快照。配置会被首次设置改写，所以不能缓存。
fn cfg(state: &ServerState) -> llm::Config {
    state.llm.lock().unwrap().clone()
}

/// 把几项配置合并进 `settings.env`，**保留文件里其它内容**（注释、别的键）。
///
/// 为什么不是整个重写：玩家可能自己在里头写了 IRONMIND_MODULES、
/// IRONMIND_HQ_INTERVAL 之类，界面上改个 key 不该把它们抹掉。
/// `Some(x)` = 写入/覆盖，`None` = 删掉这一行。
fn merge_settings_env(pairs: &[(&str, Option<String>)]) -> Result<(), String> {
    let path = llm::settings_path();
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = old.lines().map(|s| s.to_string()).collect();

    for (key, val) in pairs {
        let pos = lines.iter().position(|l| {
            let t = l.trim_start();
            !t.starts_with('#')
                && t.split_once('=')
                    .map(|(k, _)| k.trim() == *key)
                    .unwrap_or(false)
        });
        match (val, pos) {
            (Some(v), Some(i)) => lines[i] = format!("{key}={v}"),
            (Some(v), None) => lines.push(format!("{key}={v}")),
            (None, Some(i)) => {
                lines.remove(i);
            }
            (None, None) => {}
        }
    }
    let mut text = lines.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    std::fs::write(&path, text).map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
    Ok(())
}

/// 一条公文。
///
/// 参照 HOI4 的布局思路：右侧栏是"部队与军情"，不是流水账。
/// 将领的话必须分三类，各自有不同处理方式：
/// - `request` 请示：要玩家点头，**会过期**
/// - `dissent` 抗命：已经发生，要玩家追认或追责
/// - `notice` 通报：只读
#[derive(Clone)]
struct Report {
    id: u32,
    day: u32,
    kind: &'static str,
    unit: Option<usize>,
    unit_name: String,
    general: String,
    title: String,
    text: String,
    /// 请示里他打算干的事
    proposed: Option<(String, Option<u32>)>,
    /// 请示的选项文案
    options: Vec<String>,
    deadline: u32,
    status: &'static str,
    /// 这件公文是不是由某封电报引出来的。没有原令时界面措辞要换（准许执行/不许），
    /// 不能叫"准他改令"——玩家根本没下过令。
    from_telegram: bool,
}

/// 追加一条公文，返回 id。
fn push_report(state: &Arc<ServerState>, mut r: Report) -> u32 {
    let id = state
        .next_report_id
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    r.id = id;
    let mut list = state.reports.lock().unwrap();
    list.push(r);
    // 只留最近 60 条，避免无限增长
    if list.len() > 60 {
        let drop = list.len() - 60;
        list.drain(0..drop);
    }
    id
}

/// 到期的请示：**不回答本身就是一个决定**。
///
/// 过期后将领按自己的判断办，并记一条"上意未回"。
/// 这是整套上报机制里最要紧的一条：玩家可以不回话，但代价是失控。
fn expire_requests(state: &Arc<ServerState>, w: &mut World, day: u32) -> Vec<String> {
    let due: Vec<(u32, usize, String, Option<u32>, String, String)> = {
        let list = state.reports.lock().unwrap();
        list.iter()
            .filter(|r| r.status == "pending" && r.kind == "request" && day > r.deadline)
            .filter_map(|r| {
                let (kind, target) = r.proposed.clone()?;
                Some((
                    r.id,
                    r.unit?,
                    kind,
                    target,
                    r.unit_name.clone(),
                    r.general.clone(),
                ))
            })
            .collect()
    };
    let mut notes = Vec::new();
    for (id, unit, kind, target, unit_name, general) in due {
        if unit >= w.units.len() || !w.units[unit].is_alive() {
            continue;
        }
        let rec = CmdRecord {
            day,
            unit,
            kind: kind.clone(),
            target,
            reject: None,
            source: "llm".into(),
            reason: "上意未回，本官自决".into(),
        };
        state.commands.lock().unwrap().push(rec.clone());
        apply_record(w, &rec);
        {
            let mut list = state.reports.lock().unwrap();
            if let Some(r) = list.iter_mut().find(|r| r.id == id) {
                r.status = "expired";
            }
        }
        // 玩家看到的是"改成了什么动作"，不是内部命令码（bomb_rail 这种
        // 英文标识符一旦漏进战报，读起来就像 bug）。
        let kind_cn = match kind.as_str() {
            "attack" => "进攻",
            "move" => "推进",
            "retreat" => "撤退",
            "fortify" => "筑工事",
            "rest" => "整补",
            "shift" => "调兵",
            "hold" => "固守",
            "bomb_rail" => "炸路",
            "repair_rail" => "抢修",
            _ => "改令",
        };
        notes.push(format!(
            "{unit_name}（{general}）：请示无人答复，已自决{kind_cn}"
        ));
    }
    notes
}

/// 一封上级电报。玩家唯一的指挥手段。
///
/// 玩家**不能直接调兵**：他把话发给某个番号，电报按通信延迟送到，
/// 收到之后由那个番号下的每个将领自己解释、自己决定做到什么程度。
#[derive(Clone)]
struct Telegram {
    id: u32,
    day_sent: u32,
    arrive_day: u32,
    /// 收件番号（第1兵团 / 第11军…）
    to: String,
    text: String,
    /// enroute / delivered
    status: &'static str,
    /// 回电（将领的话，拼起来）
    reply: String,
    /// 这封电报落到了谁头上、被理解成什么（指挥链视图用）。
    ///
    /// 只在内存里：不进世界状态、不进存档、不影响回放。
    /// 它回答的是玩家最想问的那个问题——"我这封电报到底改变了什么"。
    traces: Vec<Trace>,
}

/// 一封电报的一次落地：某位将领把它理解成了什么。
#[derive(Clone)]
struct Trace {
    day: u32,
    unit: usize,
    unit_name: String,
    general: String,
    /// 中文命令名（进攻/推进/撤退/筑工事…）
    kind: String,
    target: String,
    /// 同意上级（照办 / 有异议照办） vs 请示改令
    same: bool,
    confidence: f64,
    reason: String,
}

/// 一封电报到底发给谁。
///
/// 玩家是"第九战区长官部"，可以直接给兵团下令，也可以**越过兵团直接
/// 给某个军、某个师**下令——史实里这很常见（薛岳越级调 74 军守万家岭）。
/// 之前只认 `formation`，所以下拉框里只有三个兵团，18 个师里有 12 个
/// 根本无从下令。现在三种写法都认：
///
/// - 兵团番号（`第1兵团`）→ 该番号下所有部队
/// - 部队名（`第74军`）→ 就这一个单位（它本身就是那一级指挥部）
/// - `全体` / `全军` / `第九战区` → 本战区所有部队
fn resolve_recipients(w: &World, side: Side, to: &str) -> Vec<usize> {
    let everyone = matches!(to, "全体" | "全军" | "第九战区" | "战区");
    (0..w.units.len())
        .filter(|&i| {
            let u = &w.units[i];
            if !u.is_alive() || u.side != side {
                return false;
            }
            if everyone {
                return true;
            }
            // 空军也听命令（只是它的"执行"不是走路，而是出击）
            u.name == to || u.formation == to
        })
        .collect()
}

/// 到期电报：把原文交给该番号下的将领，让他自己判断。
///
/// 注意这不是"把命令写进 order"。写进去的是**电报原文**，
/// 将领看它、理解它、可能打折扣——和真正的指挥链一样。
fn deliver_telegrams(state: &Arc<ServerState>, w: &World, day: u32) -> Vec<String> {
    if !cfg(state).enabled {
        return Vec::new();
    }
    let due: Vec<(u32, String, String)> = {
        let list = state.telegrams.lock().unwrap();
        list.iter()
            .filter(|t| t.status == "enroute" && t.arrive_day <= day)
            .map(|t| (t.id, t.to.clone(), t.text.clone()))
            .collect()
    };
    let mut notes = Vec::new();
    for (id, to, text) in due {
        // 收到电报的将领各自独立判断。上限是**成本**上限，不是逻辑上限：
        // 每多一个将领就多一次模型调用。
        const MAX_RECIPIENTS: usize = 8;
        let mut targets = resolve_recipients(w, Side::Cn, &to);
        targets.truncate(MAX_RECIPIENTS);
        for idx in &targets {
            // 空军平时不"执行命令"（第一版没有转场），所以它的问询一直被挡掉；
            // 但预览版开了铁路被炸之后，空军是**唯一**能炸路的兵种——
            // 再挡掉就等于这个机制永远只能靠模组作者手改数据来触发。
            let ground_only = !w.fx.rail_damage;
            if let Some(ask) = build_ask_kind(state, w, *idx, &text, ground_only) {
                state.brain.submit(llm::Job { unit: *idx, day, ask, telegram: Some(id) });
            }
        }
        {
            let mut list = state.telegrams.lock().unwrap();
            if let Some(t) = list.iter_mut().find(|t| t.id == id) {
                t.status = "delivered";
            }
        }
        trace(format!("   电报 #{id} 送达 {to}（{} 个单位收到）", targets.len()));
        notes.push(format!(
            "电报送达 {to}（{} 个将领收到），等待回电",
            targets.len()
        ));
    }
    notes
}
fn terrain_cn(t: Terrain) -> &'static str {
    match t {
        Terrain::Plain => "平原",
        Terrain::Hill => "丘陵",
        Terrain::Mountain => "山地",
        Terrain::Marsh => "沼泽",
        Terrain::Water => "水域",
        Terrain::Urban => "城镇",
    }
}

fn side_cn(s: Side) -> &'static str {
    match s {
        Side::Cn => "国军",
        Side::Jp => "日军",
    }
}

/// 一个将领的"近事"：他自己下过的判断 + 与他有关的战报。
///
/// 为什么要给他记忆：无状态的问询会让同一个将领每天重新掷一次骰子，前言不搭后语。
/// 有了这几条，他才会说"我前天主张守城垣，今天城垣还没破"。
/// 这些内容**不进世界状态**，只进提示词——回放靠命令流，不靠记忆。
fn memory_for(state: &Arc<ServerState>, w: &World, idx: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if idx >= w.units.len() {
        return out;
    }
    let day = w.tick;
    // 通话里说过的话也算记忆。
    //
    // "务必坚守，党国看好你"这种嘱咐，下一封电报的判断就该受影响——
    // 否则通话只是一次性闲聊，玩家不会认真对待它。
    // 只进提示词，不进世界状态：它改变**判断**，不改变回放。
    {
        let log = state.talk_log.lock().unwrap();
        if let Some(lines) = log.get(&idx) {
            for l in lines.iter().rev().take(6) {
                out.push(l.clone());
            }
        }
    }
    {
        let cmds = state.commands.lock().unwrap();
        for c in cmds
            .iter()
            .rev()
            .filter(|c| c.unit == idx && c.source == "llm")
            .take(3)
        {
            let why: String = c.reason.chars().take(60).collect();
            let tail = if why.is_empty() {
                String::new()
            } else {
                format!("，理由是「{why}」")
            };
            out.push(format!("第{}日 我主张 {}（已执行）{}", c.day, c.kind, tail));
        }
    }
    let name = w.units[idx].name.clone();
    for e in w
        .log
        .iter()
        .rev()
        .filter(|e| e.tick + 6 >= day && e.text.contains(&name))
        .take(3)
    {
        let t: String = e.text.chars().take(60).collect();
        out.push(format!("第{}日 {t}", e.tick));
    }
    out.reverse(); // 旧的在前，读起来是时间顺序
    out
}

/// 给一个单位准备"问将领"的材料。**只放他自己知道的东西**：
/// 邻接敌情用的是本方情报估计，不是真值。
fn build_ask(state: &Arc<ServerState>, w: &World, idx: usize, incoming: &str) -> Option<llm::Ask> {
    build_ask_kind(state, w, idx, incoming, true)
}

/// require_ground = false 时允许空军/海军也生成问询（通话要用）。
fn build_ask_kind(state: &Arc<ServerState>, w: &World, idx: usize, incoming: &str, require_ground: bool) -> Option<llm::Ask> {
    let u = w.units.get(idx)?;
    if !u.is_alive() || (require_ground && !u.kind.is_ground()) {
        return None;
    }
    let p = w.provinces.get(u.position as usize)?;
    let node = p.position_at(u.node);
    let mut neighbors = Vec::new();
    for &nb in &p.neighbors {
        let q = &w.provinces[nb as usize];
        neighbors.push((
            q.name.clone(),
            if q.owner == u.side { "我方控制" } else { "敌方控制" }.to_string(),
            w.enemy_estimate(u.side, nb).to_display_f64(),
            terrain_cn(q.terrain).to_string(),
        ));
    }
    // 远处的地：将领不该只看得见邻省。
    //
    // 千里奔袭是这个战场上的常态（74 军从赣北调万家岭、106 师团从九江走过来），
    // 只给他邻省，他的判断就只能是"打隔壁"，玩家的"抽两个师去守万家岭"
    // 也就永远落不了地。这里给 2-7 日行程内的目标，按天数排序。
    let mut far = Vec::new();
    // 空军没有"行程"（它不占省、也不行军），`reachable` 对它是空的。
    // 但开着铁路被炸时，空军需要知道"我能炸到哪里"——否则它的判断里
    // 永远不会出现目标省，炸路这条命令也就永远发不出来。
    let hops_of = |pid: u32| -> Option<u32> {
        if u.kind.is_ground() || u.kind == sim_core::UnitKind::Naval {
            sim_core::systems::hops_to(w, u.position, pid, u.side, u.kind)
        } else {
            w.hops_between(u.position, pid).ok()
        }
    };
    let candidate: Vec<(u32, u32)> = if u.kind.is_ground() || u.kind == sim_core::UnitKind::Naval {
        sim_core::systems::reachable(w, u.position, u.side, u.kind)
    } else {
        let mut v: Vec<(u32, u32)> = Vec::new();
        for pid in 0..w.provinces.len() as u32 {
            if let Some(d) = hops_of(pid) {
                v.push((pid, d));
            }
        }
        v
    };
    for (pid, hops) in candidate {
        if hops < 2 || hops > 7 {
            continue;
        }
        let q = &w.provinces[pid as usize];
        far.push((
            q.name.clone(),
            hops,
            if q.owner == u.side { "我方控制" } else { "敌方控制" }.to_string(),
            w.enemy_estimate(u.side, pid).to_display_f64(),
            terrain_cn(q.terrain).to_string(),
        ));
        if far.len() >= 10 {
            break;
        }
    }
    Some(llm::Ask {
        day: w.tick,
        general: u.commander.name.clone(),
        unit: u.name.clone(),
        side: side_cn(u.side).to_string(),
        // 兵团部是否还在，会改实际服从度（兵团部被打掉，师长就更听自己的）
        obedience: w.effective_obedience(idx).to_display_f64(),
        initiative: u.commander.initiative.to_display_f64(),
        aggression: u.commander.aggression.to_display_f64(),
        caution: u.commander.caution.to_display_f64(),
        strength: u.strength.to_display_f64(),
        max_strength: u.max_strength.to_display_f64(),
        morale: u.morale.to_display_f64(),
        supply: u.supply.to_display_f64(),
        province: p.name.clone(),
        node: node.map(|n| n.name.clone()).unwrap_or_else(|| "主阵地".into()),
        terrain: terrain_cn(p.terrain).to_string(),
        owner: side_cn(p.owner).to_string(),
        node_fort: node.map(|n| n.fort).unwrap_or(0),
        width: p.node_width(u.node).to_display_f64(),
        neighbors,
        far,
        order_label: u.order.label().to_string(),
        order_from: if u.manual { "玩家亲自" } else { "统帅部" }.to_string(),
        nodes: p.positions.iter().map(|n| n.name.clone()).collect(),
        persona: u.commander.persona.clone(),
        agenda: u.commander.agenda.clone(),
        title: u.commander.title.clone(),
        memory: memory_for(state, w, idx),
        incoming: incoming.to_string(),
        rail_damage: w.fx.rail_damage,
        hq_plan: sim_core::ai::theater_plan(w, u.side).summary,
    })
}

/// 按名字找省（模型只看得到名字，看不到 id）。
fn province_by_name(w: &World, name: &str) -> Option<u32> {
    let name = name.trim();
    w.provinces
        .iter()
        .position(|p| p.name == name)
        .map(|i| i as u32)
        .or_else(|| {
            // 容忍"德安省""德安（城垣）"这类写法
            w.provinces
                .iter()
                .position(|p| p.name.contains(name) || name.contains(&p.name))
                .map(|i| i as u32)
        })
}

fn node_by_name(w: &World, prov: u32, name: &str) -> Option<u8> {
    w.provinces[prov as usize]
        .positions
        .iter()
        .position(|n| n.name == name.trim())
        .map(|i| i as u8)
}

/// 取走本日已就绪的判断，转成命令写进世界与命令流。
///
/// 规则：
/// - 判断与现有命令**一致**（`dissent=false`）→ 不改命令，只记一条战报（他表了态）。
/// - 判断不同 → 覆盖命令，记一条抗命战报。**这就是"将领向你上报"的落点。**
fn drain_ready(state: &Arc<ServerState>, w: &mut World, day: u32) -> Vec<(bool, String, Side)> {
    let taken: Vec<(usize, llm::Ready)> = {
        let mut ready = state.brain.ready.lock().unwrap();
        // 允许"迟到"：模型要几秒，判断几乎总在当日 tick 之后才回来。
        // 所以取 `day <= 现在` 的，太旧的（超过 3 天）就丢掉，别用过时的判断。
        let ids: Vec<usize> = ready
            .iter()
            .filter(|(_, r)| r.day <= day && day.saturating_sub(r.day) <= 3)
            .map(|(k, _)| *k)
            .collect();
        ids.into_iter()
            .filter_map(|k| ready.remove(&k).map(|r| (k, r)))
            .collect()
    };

    let mut notes: Vec<(bool, String, Side)> = Vec::new();
    for (idx, ready) in taken {
        if idx >= w.units.len() || !w.units[idx].is_alive() {
            continue;
        }
        let j = ready.judgement;
        let general = w.units[idx].commander.name.clone();
        let unit_name = w.units[idx].name.clone();
        let cur = w.units[idx].order;
        let side = w.units[idx].side;

        // 把模型的输出翻译成引擎命令
        let kind = j.order.as_str();
        let target: Option<u32> = match kind {
            "attack" | "move" | "retreat" | "bomb_rail" | "repair_rail" => j
                .target
                .as_deref()
                .and_then(|n| province_by_name(w, n)),
            "shift" => j
                .target
                .as_deref()
                .and_then(|n| node_by_name(w, w.units[idx].position, n))
                .map(|n| n as u32),
            _ => None,
        };
        // 目标解析不出来就当成"按兵不动"，不要瞎打
        let kind = if matches!(
            kind,
            "attack" | "move" | "retreat" | "bomb_rail" | "repair_rail"
        ) && target.is_none() {
            "hold"
        } else {
            kind
        };
        // "和上级一致"要同时比**动作**和**目标**。
        //
        // 只比动作会出大错：部队已经在"推进"某省，模型改主意要"推进"另一个
        // 更远的省，动作标签一样，于是被判成"同意上级、不改命令"，
        // 新目标被静静丢掉——玩家看到的回电说"照办"，部队却往老地方走。
        let kind_label = match kind {
            "attack" => "进攻",
            "move" => "推进",
            "retreat" => "撤退",
            "fortify" => "筑工事",
            "rest" => "整补",
            "shift" => "调兵",
            _ => "固守",
        };
        let same_target = match kind {
            "attack" | "move" | "retreat" => cur.target() == target,
            "bomb_rail" | "repair_rail" => cur.place() == target,
            "shift" => matches!(*&cur, Order::Shift(n) if Some(n as u32) == target),
            _ => true,
        };
        let same = cur.label() == kind_label && same_target;

        // 报告里带上他自己的把握程度：玩家看到的是"他有多大信心"，不是一个已定事实
        let grip = format!("（把握 {:.0}%）", j.confidence * 100.0);
        let reply = if j.reply.is_empty() {
            String::new()
        } else {
            format!("　回电：{}", j.reply)
        };
        let text = if j.reason.is_empty() {
            format!("{unit_name}（{general}）{kind}{grip}{reply}")
        } else if j.cost.is_empty() {
            format!("{unit_name}（{general}）：{}{grip}{reply}", j.reason)
        } else {
            format!("{unit_name}（{general}）：{}（代价：{}）{grip}{reply}", j.reason, j.cost)
        };
        // 模型自己也认为与命令不一致时，即使结果同向也标出来：那是"有异议但照办了"
        let misgiving = j.dissent && same;

        // 回电挂到**被问的那封**电报上（照办与抗命都要挂）。
        // 以前按"该番号最近一封"找，连发两封电报时回电会串到错的那封上。
        if !j.reply.is_empty() {
            let who = w.units[idx].commander.name.clone();
            let mut list = state.telegrams.lock().unwrap();
            let target = match ready.telegram {
                Some(id) => list.iter_mut().find(|t| t.id == id),
                None => None,
            };
            if let Some(t) = target {
                if !t.reply.is_empty() {
                    t.reply.push('\n');
                }
                t.reply.push_str(&format!("{who}：{}", j.reply));
            }
        }
        // 指挥链：记下"这封电报在他这里变成了什么"。
        // 纯内存、只给界面看——玩家据此能看出命令在哪一环被改掉或被请示掉。
        if let Some(id) = ready.telegram {
            let mut list = state.telegrams.lock().unwrap();
            if let Some(t) = list.iter_mut().find(|t| t.id == id) {
                t.traces.push(Trace {
                    day,
                    unit: idx,
                    unit_name: unit_name.clone(),
                    general: general.clone(),
                    kind: kind_label.to_string(),
                    target: j.target.clone().unwrap_or_default(),
                    same,
                    confidence: j.confidence,
                    reason: j.reason.clone(),
                });
            }
        }
        if same {
            // 同意上级：不改命令，只表态
            let t = if misgiving { format!("{text}〔有异议，仍照办〕") } else { text };
            notes.push((false, t, side));
            continue;
        }

        // 先写记录，再**用记录本身**应用命令：实时与回放走同一段代码。
        let rec = CmdRecord {
            day,
            unit: idx,
            kind: kind.to_string(),
            target,
            reject: None,
            source: "llm".into(),
            reason: j.reason.clone(),
        };

        // 中方的将领要向上报：抗命不是立刻执行，而是**请你点头**。
        // 日方由 AI 全权指挥，没有上级可问，直接执行。
        // 什么才值得"请示"？
        //
        // 只有两种情况要玩家点头：
        //   · 他在回应你**明确下过的令**（电报）——他要么照办，要么请你改；
        //   · 改动代价很大：撤退，或者放弃工事/驻地开拔。
        //
        // 其余（自己微调目标、闲着去挖工事、换个方向推进）一律**直接执行**
        // 并进战报。以前只要"和他的现行命令不一致"就开请示，于是玩家
        // 每三天被敲一次门，而请示本来该是"这事要你担责任"。
        let high_cost = matches!(kind, "retreat")
            || (matches!(cur, Order::Fortify | Order::Hold)
                && matches!(kind, "move" | "attack"));
        let needs_blessing = ready.telegram.is_some() || high_cost;
        if w.units[idx].side == Side::Cn && needs_blessing {
            // 公文标题用中文命令名。以前这里直接拼 `kind`（引擎内部代号），
            // 公文堆里于是出现"请示：attack 万家岭 准不准？"——半句英文。
            let title = format!(
                "{} {}",
                kind_label,
                j.target.clone().unwrap_or_default()
            );
            let rid = push_report(
                state,
                Report {
                    id: 0,
                    day,
                    kind: "request",
                    unit: Some(idx),
                    unit_name: unit_name.clone(),
                    general: general.clone(),
                    title: title.trim().to_string(),
                    text: text.split("　回电：").next().unwrap_or(&text).to_string(),
                    proposed: Some((kind.to_string(), target)),
                    options: vec!["准他改令".into(), "驳回，维持原令".into()],
                    deadline: day + 2,
                    status: "pending",
                    from_telegram: ready.telegram.is_some(),
                },
            );
            notes.push((
                true,
                format!("{text}〔请示 #{rid}：请裁示是否准他改令〕"),
                side,
            ));
            continue;
        }

        // 日方：直接执行，并记一条抗命通报
        let mut trial = w.clone();
        if apply_typed(&mut trial, idx, kind, target).is_some() {
            let tgt = j.target.clone().unwrap_or_else(|| "无".into());
            notes.push((
                false,
                format!("{unit_name}（{general}）提出的「{kind} {tgt}」不合法，已驳回"),
                side,
            ));
            continue;
        }
        state.commands.lock().unwrap().push(rec.clone());
        apply_record(w, &rec);
        notes.push((true, format!("{text}（擅自改为 {kind}）"), side));
    }
    notes
}

/// 玩家批复一条请示。
fn answer_report(state: &Arc<ServerState>, w: &mut World, id: u32, approve: bool) -> String {
    let target = {
        let list = state.reports.lock().unwrap();
        list.iter()
            .find(|r| r.id == id)
            .map(|r| (r.status.to_string(), r.proposed.clone(), r.unit_name.clone(), r.general.clone()))
    };
    let Some((status, proposed, unit_name, general)) = target else {
        return format!("找不到公文 #{id}");
    };
    if status != "pending" {
        return format!("这份公文已经处理过了（{status}）");
    }

    if !approve {
        let mut list = state.reports.lock().unwrap();
        if let Some(r) = list.iter_mut().find(|r| r.id == id) {
            r.status = "denied";
        }
        return format!("已驳回 {unit_name}（{general}）的请示，维持原令");
    }

    let Some((kind, tgt)) = proposed else {
        let mut list = state.reports.lock().unwrap();
        if let Some(r) = list.iter_mut().find(|r| r.id == id) {
            r.status = "approved";
        }
        return format!("已批复 {unit_name} 的请示");
    };

    // 找到是哪个单位提的（按公文里的 unit 索引）
    let unit = {
        let list = state.reports.lock().unwrap();
        list.iter().find(|r| r.id == id).and_then(|r| r.unit)
    };
    let Some(unit) = unit else {
        return "公文里没有对应单位".into();
    };

    let rec = CmdRecord {
        day: w.tick,
        unit,
        kind: kind.clone(),
        target: tgt,
        reject: None,
        source: "llm".into(),
        reason: "经上峰核准".into(),
    };
    state.commands.lock().unwrap().push(rec.clone());
    apply_record(w, &rec);
    {
        let mut list = state.reports.lock().unwrap();
        if let Some(r) = list.iter_mut().find(|r| r.id == id) {
            r.status = "approved";
        }
    }
    format!("已准 {unit_name}（{general}）改令为 {kind}")
}

/// 提交到期的问询。同一个单位 `interval_days` 内只问一次，每 tick 最多几单。
fn request_due(state: &Arc<ServerState>, w: &World) {
    if !cfg(state).enabled {
        return;
    }
    let day = w.tick;
    let interval = cfg(state).interval_days.max(1);

    // 先挑出"到期该问"的单位，再按**最久没问过的优先**排队。
    //
    // 不这样排会饿死后面的部队：预算只有 3 个，按索引顺序取的话，
    // 0/1/2 号单位每 tick 都到期，永远占着三个名额，中方部队一次都轮不到。
    let mut cands: Vec<(u32, usize)> = Vec::new();
    {
        let asked = state.brain.asked.lock().unwrap();
        // 已经有判断躺在池子里、还没被取走的单位，不要再问一遍：
        // 白花钱，而且会跟电报问询抢同一个 ready 槽位。
        let ready = state.brain.ready.lock().unwrap();
        for idx in 0..w.units.len() {
            let u = &w.units[idx];
            if !u.is_alive() || !u.kind.is_ground() {
                continue;
            }
            if ready.contains_key(&idx) {
                continue;
            }
            let last = asked.get(&idx).copied();
            if let Some(d) = last {
                if day.saturating_sub(d) < interval {
                    continue;
                }
            }
            cands.push((last.unwrap_or(0), idx));
        }
    }
    cands.sort();

    let mut budget = 3usize;
    for (_, idx) in cands {
        if budget == 0 {
            break;
        }
        if let Some(ask) = build_ask(state, w, idx, "") {
            state.brain.submit(llm::Job { unit: idx, day, ask, telegram: None });
            trace(format!("   问询 unit#{idx}（第{day}日）"));
            budget -= 1;
        }
    }
}

/// 生成给前端的快照。
///
/// **关键：只输出中方的情报视图，不输出真值。**
/// 这也是"玩家不是上帝"在接口层的落地。
fn snapshot(w: &World) -> String {
    use serde_json::json;

    let viewer = Side::Cn;
    let intel = w.intel(viewer);

    // 编制簿：已经覆灭的番号（我方）。
    //
    // 不能塞进 `provinces[].units`——地图上会凭空多出一个"0 千的部队方块"。
    // 但也不能不告诉玩家：番号没了、军长殉国、还有多少天重建，
    // 这些恰恰是"编制改变"这件事的全部内容。
    let gone: Vec<serde_json::Value> = (0..w.units.len())
        .filter(|&i| {
            let u = &w.units[i];
            u.side == viewer && u.ended_day.is_some()
        })
        .map(|i| {
            let u = &w.units[i];
            json!({
                "index": i,
                "name": u.name,
                "formation": u.formation,
                "commander": u.commander.name,
                "fate": u.commander.fate.label(),
                "fate_code": u.commander.fate.code(),
                "ended_day": u.ended_day,
                "reform_day": u.reform_day,
                "reform_ratio": round2(u.reform_ratio.to_display_f64()),
                "province": w.provinces.get(u.position as usize).map(|p| p.name.clone()),
                "strength": round1(u.strength.to_display_f64()),
            })
        })
        .collect();

    let mut provinces = Vec::new();
    for (i, p) in w.provinces.iter().enumerate() {
        let i_intel = intel.get(i).copied().unwrap_or_else(|| {
            sim_core::ProvinceIntel::unknown()
        });

        let units_here: Vec<_> = w
            .units_in(i as u32)
            .into_iter()
            .filter_map(|ui| {
                let u = &w.units[ui as usize];
                if !u.is_alive() {
                    return None;
                }
                let mine = u.side == viewer;
                if !mine && i_intel.accuracy == Accuracy::Unknown {
                    return None; // 没侦察到就不显示
                }
                Some(json!({
                    "index": ui,
                    "name": u.name,
                    "commander": u.commander.name,
                    "commander_title": u.commander.title,
                    "commander_persona": u.commander.persona,
                    "kind": u.kind.label(),
                    "kind_code": u.kind.code(),
                    "side": if mine { "cn" } else { "jp" },
                    "mine": mine,
                    "strength": round1(u.strength.to_display_f64()),
                    "morale": round2(u.morale.to_display_f64()),
                    "supply": round2(u.supply.to_display_f64()),
                    "encircled": u.encircled,
                    "order": u.order.label(),
                    "order_code": u.order.code().1,
                    "node": u.node,
                    "formation": u.formation,
                    // 编制状态：覆灭日、将领结局、整编日。
                    // 前端要能说清"这支部队已经没了 / 主官殉国 / 还有 12 天重建"。
                    "ended_day": u.ended_day,
                    "fate": u.commander.fate.label(),
                    "fate_code": u.commander.fate.code(),
                    "reform_day": u.reform_day,
                    // 距命令目标还有几天（每 tick 走一省）。远距离调动全靠它，
                    // 否则玩家看不出"这支部队正在路上"还是"根本没动"。
                    "march_hops": u.order.target().map(|t| {
                        if t == u.position {
                            0
                        } else {
                            sim_core::systems::hops_to(w, u.position, t, u.side, u.kind)
                                .unwrap_or(0)
                        }
                    }),
                    "march_to": u.order.target().map(|t| w.provinces[t as usize].name.clone()),
                    // 省 id：地图上要画一条从当前省到目标的虚线
                    "march_target": u.order.target(),
                    "disobeyed": u.disobeyed,
                    "dissent": u.dissent_reason,
                    // 预览版：弹药/油料（开关关着时恒为 1.0，前端不显示）。
                    "ammo": round2(u.ammo.to_display_f64()),
                    "fuel": round2(u.fuel.to_display_f64()),
                    "faction": u.commander.faction,
                }))
            })
            .collect();

        provinces.push(json!({
            "id": i,
            "name": p.name,
            "terrain": format!("{:?}", p.terrain).to_lowercase(),
            // 敌方控制省的归属本身也是情报：没侦察到就显示 unknown。
            // 这是"非上帝视角"的最后一道口子——如果归属全暴露，
            // 玩家就知道整条战线在哪，侦察就没意义了。
            "owner": if p.owner == viewer {
                "cn"
            } else if i_intel.is_known() {
                match p.owner { Side::Cn => "cn", Side::Jp => "jp" }
            } else {
                "unknown"
            },
            "owner_known": p.owner == viewer || i_intel.is_known(),
            "mine": p.owner == viewer,
            // 铁路：地形视图里要画出来。
            // 之前只有引擎知道（`Province.rail` 参与补给计算），界面拿不到，
            // 于是"守住南浔铁路"这条最重要的战略提示在图上完全看不见。
            "rail": p.rail,
            // 预览版：铁路损坏 0-3（0 = 通）。开关关着时恒为 0。
            "rail_damage": p.rail_damage,
            "neighbors": p.neighbors,
            // 真实高程（米）：地形是怎么判出来的，给玩家一个可核对的数字。
            // 模块没写就是 null，前端据此决定要不要显示。
            "elev": p.elev,
            "relief": p.relief,
            // 舰炮支援倍率。江面上的舰船能让沿岸陆战好打，但玩家看不见这件事——
            // 看不到就以为舰队是摆设，所以直接给前端一个数。
            // 我方没有海军时是 1.00；敌方那一项只在侦察到该省时给出
            // （"江上有日本军舰"这是看得见的情报，不是上帝视角）。
            "naval_bonus": round2(
                sim_core::systems::naval_support(w, i as u32, viewer).to_display_f64()
            ),
            "naval_bonus_enemy": if i_intel.is_known() {
                round2(sim_core::systems::naval_support(w, i as u32, viewer.opponent()).to_display_f64())
            } else {
                1.0
            },
            "positions": p.positions.iter().map(|x| json!({
                "name": x.name,
                "terrain": format!("{:?}", x.terrain).to_lowercase(),
                "width": round2(x.width.to_display_f64()),
                "fort": x.fort,
            })).collect::<Vec<_>>(),
            "fort": p.fort,
            "supply_source": p.is_supply_source,
            "neighbors": p.neighbors,
            "lon": round2(p.lon.to_display_f64()),
            "lat": round2(p.lat.to_display_f64()),
            "intel": {
                "accuracy": i_intel.accuracy.label(),
                "accuracy_code": i_intel.accuracy.code(),
                "strength": round1(i_intel.enemy_strength.to_display_f64()),
                "stale": i_intel.is_known() && w.tick > i_intel.seen_tick + 3,
                "known": i_intel.is_known(),
            },
            "units": units_here,
        }));
    }

    let edges: Vec<(u32, u32)> = {
        let mut out = Vec::new();
        for (i, p) in w.provinces.iter().enumerate() {
            for &nb in &p.neighbors {
                if (nb as usize) > i {
                    out.push((i as u32, nb));
                }
            }
        }
        out
    };

    let cn_strength = w.total_strength(Side::Cn).to_display_f64();
    let jp_provinces = w.provinces.iter().filter(|p| p.owner == Side::Jp).count();
    let cn_provinces = w.provinces.iter().filter(|p| p.owner == Side::Cn).count();
    let jp_estimate: f64 = intel
        .iter()
        .filter(|x| x.enemy_strength.raw() > 0)
        .map(|x| x.enemy_strength.to_display_f64())
        .sum();

    // 结局：一旦有值，这一局就结束了。前端据此弹面板并锁住推进按钮。
    let outcome = w.outcome.as_ref().map(|o| {
        json!({
            "id": o.id,
            "name": o.name,
            "text": o.text,
            "day": o.day,
            "metrics": o.metrics.iter().map(|(k, v)| json!({ "label": k, "value": round1(*v) })).collect::<Vec<_>>(),
        })
    });
    // 本方司令部的作战方针：这是给玩家的（自己的统帅部怎么想，他应该知道），
    // 敌方的方针属于情报，不给。
    let own_plan = sim_core::ai::theater_plan(w, viewer);
    let plan = json!({
        "posture": own_plan.posture.label(),
        "focus": own_plan.focus.iter().map(|&p| w.provinces[p as usize].name.clone()).collect::<Vec<_>>(),
        "summary": own_plan.summary,
    });
    json!({
        "tick": w.tick,
        "campaign_days": w.campaign_days,
        // 预览版功能（模块显式声明才有）。前端据此显示"预览"角标与对应面板，
        // 没开就完全当作不存在——第一版发行版的界面不该多出用不了的按钮。
        "features": w.fx.names(),
        // 显式关掉的机制（1.0 起默认全开）：界面据此挂"经典模式"角标。
        "features_off": w.fx.disabled(),
        // 派系（预览版 politics）：只有模块声明了才有内容。
        // 影响力是**公开的**（朝中谁说得上话，军中都看得见），
        // 将领个人的服从度则永远不给玩家——那是模型用的内部量。
        "factions": w
            .factions
            .iter()
            .map(|f| {
                json!({
                    "id": f.id,
                    "name": f.name,
                    "cn": f.side == Side::Cn,
                    "influence": round2(f.influence.to_display_f64()),
                    "generals": f
                        .units
                        .iter()
                        .filter_map(|&i| w.units.get(i).map(|u| u.commander.name.clone()))
                        .collect::<Vec<_>>(),
                    "units": f.units,
                })
            })
            .collect::<Vec<_>>(),
        // 已去职的将领（预览版 politics）：人事不是没有代价的事，
        // 撤过谁得能查——否则玩家会以为"撤了就是没了"，不知道该不该撤。
        "dismissed": w
            .dismissed
            .iter()
            .map(|(unit, who, day)| json!({ "unit": unit, "who": who, "day": day }))
            .collect::<Vec<_>>(),
        "plan": plan,
        "outcome": outcome,
        "player": "cn",
        // 已覆灭的番号（含殉国、整编倒计时）
        "gone": gone,
        "side": {
            "strength": round1(cn_strength),
            "provinces": cn_provinces,
            "pending_reports": w.pending_count(Side::Cn),
        },
        "enemy": {
            "provinces": jp_provinces,
            "estimated_strength": round1(jp_estimate),
            // 玩家已经侦察到的敌方省份数——用来衡量情报覆盖
            "known_provinces": intel.iter().filter(|x| x.is_known()).count(),
        },
        "comms": {
            "kind": w.comms.kind.label(),
            "report_delay": w.comms.report_delay(),
            "staff": w.comms.staff,
            "transmission": w.comms.transmission,
        },
        "provinces": provinces,
        "edges": edges,
        // 战报改成类型化事件：tick + kind + text。
        // 前端按 kind 上色，不再用正则去匹配中文措辞。
        // 只把"客观事实"和"本方内部文件"发给玩家。
        // 敌方将领的判断、回电、抗命理由属于内部消息——玩家不该看到敌人的心里话。
        "log": w
            .log
            .iter()
            .rev()
            .filter(|e| e.side.is_none() || e.side == Some(viewer))
            .take(40)
            .map(|e| json!({ "tick": e.tick, "kind": e.kind.code(), "text": e.text }))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

fn round1(v: f64) -> f64 {
    let r = (v * 10.0).round() / 10.0;
    // 避免 JSON 里出现 -0
    if r == 0.0 { 0.0 } else { r }
}
fn round2(v: f64) -> f64 {
    let r = (v * 100.0).round() / 100.0;
    if r == 0.0 { 0.0 } else { r }
}

/// 写一行操作日志（同时打到 stderr）。
///
/// 追加到 `logs/server.log`，并可由 `GET /api/log` 取回尾部。
/// 时间戳是 UTC，够用来对齐"我几点点的"。
fn trace(msg: String) {
    use std::io::Write;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (hh, mm, ss) = ((now / 3600) % 24, (now / 60) % 60, now % 60);
    let line = format!("[{hh:02}:{mm:02}:{ss:02}Z] {msg}\n");
    eprintln!("{}", line.trim_end());
    let _ = std::fs::create_dir_all("logs");
    let path = "logs/server.log";
    // 超过 1MB 就重开一个，免得长跑把盘写满
    if std::fs::metadata(path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        let _ = std::fs::remove_file(path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

fn respond(
    stream: &mut TcpStream,
    code: u16,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let status = match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        404 => "404 Not Found",
        _ => "500 Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.as_bytes().len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// 前端页面（内嵌，避免额外文件依赖）。
const PAGE: &str = include_str!("../web/index.html");

/// 真实地理底图（海岸线/河流/湖泊）。按 docs/04 的分层，G0 全球底图归引擎，
/// 所以它属于服务端，不属于任何模块——换战役不用重画。
const BASEMAP: &str = include_str!("../web/basemap.json");







#[cfg(test)]
mod tests {
    use super::sanitize_slot;

    #[test]
    fn slot_names_cannot_escape_the_saves_directory() {
        assert_eq!(sanitize_slot("../../../etc/passwd"), "etc-passwd");
        assert_eq!(sanitize_slot("..\\..\\x"), "x");
        assert_eq!(sanitize_slot("a/b"), "a-b");
        assert_eq!(sanitize_slot("  "), "");
    }

    #[test]
    fn slot_names_keep_ascii_and_cjk() {
        assert_eq!(sanitize_slot("nanxun-01"), "nanxun-01");
        assert_eq!(sanitize_slot("第一局"), "第一局");
        // 40 字符上限
        let long: String = "a".repeat(80);
        assert_eq!(sanitize_slot(&long).len(), 40);
    }
}// AI 玩家（敌方统帅部）的接入。放在 main.rs 里，紧贴服务端状态。

/// 距上次叫司令部几天了（用来控制频率：司令部不该每天开会）。
fn hq_due(state: &Arc<ServerState>, w: &World) -> bool {
    if !cfg(state).enabled {
        return false;
    }
    // 模块没写统帅部就不做——不是每个模块都想给敌国一个"人"。
    let Some(_) = state.module.lock().unwrap().meta.hq.clone() else {
        return false;
    };
    let interval = std::env::var("IRONMIND_HQ_INTERVAL")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(2);
    let last = *state.hq_last_day.lock().unwrap();
    w.tick >= last + interval
}

/// 组装给敌国司令部的问询。
fn build_hq_ask(state: &Arc<ServerState>, w: &World, side: Side) -> Option<llm::HqAsk> {
    let hq = state.module.lock().unwrap().meta.hq.clone()?;
    let enemy = side.opponent();

    let mut units = Vec::new();
    for u in w.units.iter() {
        if u.side != side || !u.kind.is_ground() || !u.is_alive() {
            continue;
        }
        let prov = w
            .provinces
            .get(u.position as usize)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "?".into());
        units.push((
            u.name.clone(),
            prov,
            u.strength.to_display_f64(),
            u.supply.to_display_f64(),
            u.order.label().to_string(),
            u.encircled,
        ));
    }

    // 已知敌情（本方视角的情报，不是真值）
    let intel = w.intel(side);
    let mut known = Vec::new();
    for (i, p) in w.provinces.iter().enumerate() {
        let it = intel.get(i).copied().unwrap_or_else(sim_core::ProvinceIntel::unknown);
        if it.accuracy == sim_core::Accuracy::Unknown || p.owner == side {
            continue;
        }
        known.push((
            p.name.clone(),
            it.enemy_strength.to_display_f64(),
            if p.owner == enemy { "敌方控制".to_string() } else { "归属不明".to_string() },
        ));
    }
    known.truncate(18);

    Some(llm::HqAsk {
        day: w.tick,
        side: side_cn(side).to_string(),
        title: hq.title.clone(),
        general: hq.name.clone(),
        persona: hq.persona.clone(),
        agenda: hq.agenda.clone(),
        objectives: hq.objectives.clone(),
        units,
        enemy: known,
        last_plan: state.hq_plan.lock().unwrap().clone(),
    })
}

/// 每 tick 边界：该开会就开会；会开完了就把电报发给各师团。
fn run_hq(state: &Arc<ServerState>, w: &mut World, day: u32) -> Vec<(bool, String, Side)> {
    let mut notes = Vec::new();

    // ---- 收：司令部的判断到了 ----
    if let Some((hq_day, j)) = state.hq.take() {
        if day.saturating_sub(hq_day) <= 3 {
            *state.hq_plan.lock().unwrap() = j.plan.clone();
            trace(format!(
                "   司令部（第{hq_day}日）：{} —— {} 条电报",
                j.plan,
                j.orders.len()
            ));
            // 他的命令走**和玩家一样的路**：先变成电报原文，交给师团长判断。
            // 师团长可以抗命——这正是"敌方也有指挥链"的意思。
            let title = state
                .module
                .lock()
                .unwrap()
                .meta
                .hq
                .clone()
                .map(|h| format!("{}{}", h.title, h.name))
                .unwrap_or_else(|| "司令部".into());
            for (to, text) in j.orders {
                let idx = w
                    .units
                    .iter()
                    .position(|u| u.name == to && u.side == Side::Jp && u.is_alive());
                let Some(idx) = idx else {
                    trace(format!("   司令部电报找不到收件人：{to}"));
                    continue;
                };
                let incoming = format!("{title}：{text}");
                if let Some(ask) = build_ask(state, w, idx, &incoming) {
                    state.brain.submit(llm::Job { unit: idx, day, ask, telegram: None });
                    trace(format!("   司令部 → {to}：{text}"));
                    notes.push((false, format!("敌司令部电令 {}", to), Side::Jp));
                }
            }
            if !j.reason.is_empty() {
                trace(format!("   司令部理由：{}", j.reason));
            }
        }
    }

    // ---- 发：到日子了就叫一次 ----
    if hq_due(state, w) {
        if let Some(ask) = build_hq_ask(state, w, Side::Jp) {
            *state.hq_last_day.lock().unwrap() = w.tick;
            state.hq.submit(w.tick, ask);
            trace(format!("   问询敌司令部（第{}日）", w.tick));
        }
    }

    notes
}

// 模块根目录：外部模块仓库优先（IRONMIND_MODULES，分隔符 ; 或 ,），
// 然后是引擎自带的 content/scenarios。
//
// 为什么要有它：模块现在住在**别的仓库**里（例如 F:\dsh\nanxun-1938）。
// 引擎不该假设"模块一定在 content/scenarios 下"——那等于把两个项目又绑回去。
fn module_roots() -> Vec<std::path::PathBuf> {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(v) = std::env::var("IRONMIND_MODULES") {
        for part in v.split([';', ',']) {
            let p = part.trim();
            if !p.is_empty() && !roots.iter().any(|r| r == std::path::Path::new(p)) {
                roots.push(std::path::PathBuf::from(p));
            }
        }
    }
    // 安装布局：`modules/` 就在 exe 旁边。
    //
    // 桌面外壳会主动把这个目录塞进 IRONMIND_MODULES，但那只在上壳正常时会做。
    // 直接跑 sim-server.exe（或上壳的 env 逻辑哪天坏了）就会变成"可用模块 0 个"——
    // 游戏照常启动，只是没战役可开，而且没有任何报错。这里按布局兜底一次。
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        for cand in [dir.join("modules"), std::path::PathBuf::from("modules")] {
            if !cand.is_dir() {
                continue;
            }
            // 用 canonicalize 比，否则 `<exe>/modules` 和相对路径 `modules`
            // 会同时进列表，模块在界面上被列两遍。
            let key = std::fs::canonicalize(&cand).unwrap_or_else(|_| cand.clone());
            let dup = roots.iter().any(|r| {
                std::fs::canonicalize(r).unwrap_or_else(|_| r.clone()) == key
            });
            if !dup {
                roots.push(cand);
            }
        }
    }
    let builtin = std::path::PathBuf::from("content/scenarios");
    if !roots.contains(&builtin) {
        roots.push(builtin);
    }
    roots
}

/// 导入的模块往哪写：第一个"可写、且不是引擎自带"的模块根。
///
/// 装好的布局是 exe 旁边的 `modules/`；开发时是 IRONMIND_MODULES 指的那个仓库。
/// 两个都没有就现造一个 `modules/`——总比报错说"请先配置环境变量"强。
fn import_root() -> std::path::PathBuf {
    for r in module_roots() {
        if r.ends_with("content/scenarios") || r.ends_with("content\\scenarios") {
            continue;
        }
        if r.is_dir() {
            return r;
        }
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        let m = dir.join("modules");
        let _ = std::fs::create_dir_all(&m);
        if m.is_dir() {
            return m;
        }
    }
    let m = std::path::PathBuf::from("modules");
    let _ = std::fs::create_dir_all(&m);
    m
}

/// 扫出所有模块目录（`*.imod`），按根目录顺序、路径排序，保证确定性。
fn discover_modules() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for root in module_roots() {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        let mut dirs: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for p in dirs {
            // 模块有两种形态：目录（开发）与 .imod 压缩包（发布），都要能列出来。
            let imod = p.extension().and_then(|x| x.to_str()) == Some("imod");
            if imod && (p.is_dir() || p.is_file()) {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}
