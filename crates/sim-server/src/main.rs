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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let module_path = args
        .iter()
        .position(|a| a == "--module")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "content/scenarios/nanxun_1938.imod".to_string());

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

    // 模块可以自带真实省界；没有就让前端用 Voronoi 兜底（保持向后兼容）
    let geo_path = std::path::Path::new(&module_path).join("provinces_geo.json");
    let provinces_geo = std::fs::read_to_string(&geo_path).ok();
    if provinces_geo.is_some() {
        println!("  省界：使用模块自带的真实边界（provinces_geo.json）");
    }

    let llm_cfg = llm::Config::from_env();
    println!("  将领大脑：{}", llm_cfg.describe());

    let state = Arc::new(ServerState {
        module,
        world: std::sync::Mutex::new(None),
        commands: std::sync::Mutex::new(Vec::new()),
        llm: llm_cfg.clone(),
        brain: llm::Broker::new(llm_cfg),
        reports: std::sync::Mutex::new(Vec::new()),
        next_report_id: std::sync::atomic::AtomicU32::new(1),
        provinces_geo,
        telegrams: std::sync::Mutex::new(Vec::new()),
        next_telegram_id: std::sync::atomic::AtomicU32::new(1),
    });
    {
        let mut w = state.world.lock().unwrap();
        let mut world = build_world(&state.module);
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
    module: LoadedModule,
    world: std::sync::Mutex<Option<World>>,
    /// 玩家命令流。`(模块, 种子, 命令流)` 就是一份完整存档。
    commands: std::sync::Mutex<Vec<CmdRecord>>,
    /// 将领的 LLM 配置与后台问询。
    llm: llm::Config,
    brain: llm::Broker,
    /// 公文堆（请示 / 抗命 / 通报）
    reports: std::sync::Mutex<Vec<Report>>,
    next_report_id: std::sync::atomic::AtomicU32,
    /// 模块自带的真实省界（provinces_geo.json）。没有就退回前端 Voronoi。
    provinces_geo: Option<String>,
    /// 在途与已送达的电报
    telegrams: std::sync::Mutex<Vec<Telegram>>,
    next_telegram_id: std::sync::atomic::AtomicU32,
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
        ("GET", "/provinces_geo.json") => match &state.provinces_geo {
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
        ("GET", "/api/replay-check") => {
            let (live_hash, live_tick) = {
                let g = state.world.lock().unwrap();
                let w = g.as_ref().unwrap();
                (w.state_hash(), w.tick)
            };
            let cmds = state.commands.lock().unwrap().clone();
            let json = replay_check(&state.module, live_hash, live_tick, &cmds);
            respond(&mut stream, 200, "application/json", &json)
        }
        ("GET", "/api/llm") => {
            use serde_json::json;
            let body = json!({
                "enabled": state.llm.enabled,
                "mode": state.llm.describe(),
                "model": state.llm.model,
                "interval_days": state.llm.interval_days,
                "calls": *state.brain.calls.lock().unwrap(),
                "ready": state.brain.ready.lock().unwrap().len(),
                "errors": state.brain.errors.lock().unwrap().clone(),
            })
            .to_string();
            respond(&mut stream, 200, "application/json", &body)
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
                        "status": r.status,
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
            let approve = v.get("choice").and_then(|x| x.as_str()) == Some("yes");
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
                    let delay = w.comms.transmission + w.comms.order_delay;
                    let arrive = w.tick + delay.max(1);
                    let id = state
                        .next_telegram_id
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    state.telegrams.lock().unwrap().push(Telegram {
                        id,
                        day_sent: w.tick,
                        arrive_day: arrive,
                        to: to.clone(),
                        text: text.clone(),
                        status: "enroute",
                        reply: String::new(),
                    });
                    trace(format!("   发电报 #{id} → {to}，{delay} 日后送达"));
                    json!({ "ok": true, "id": id, "arrive_day": arrive })
                }
            };
            respond(&mut stream, 200, "application/json", &out.to_string())
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
                    })
                })
                .collect();
            drop(list);
            respond(&mut stream, 200, "application/json", &json!({ "telegrams": arr }).to_string())
        }
        ("POST", "/api/save") => {
            use serde_json::json;
            let out = {
                let g = state.world.lock().unwrap();
                let w = g.as_ref().unwrap();
                let cmds: Vec<CmdRecord> = state
                    .commands
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|c| c.reject.is_none())
                    .cloned()
                    .collect();
                let payload = SaveFile {
                    v: 1,
                    module_id: state.module.meta.id.clone(),
                    module_version: state.module.meta.version.clone(),
                    seed: w.seed,
                    day: w.tick,
                    state_hash: format!("{:016x}", w.state_hash()),
                    commands: cmds,
                };
                let _ = std::fs::create_dir_all("saves");
                let name = format!(
                    "saves/save-{}.json",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                );
                match serde_json::to_string_pretty(&payload) {
                    Ok(s) => match std::fs::write(&name, s) {
                        Ok(_) => {
                            trace(format!(
                                "   存档 {} （第 {} 日，{} 条命令）",
                                name,
                                payload.day,
                                payload.commands.len()
                            ));
                            json!({
                                "ok": true, "path": name, "day": payload.day,
                                "commands": payload.commands.len(),
                            })
                        }
                        Err(e) => json!({ "ok": false, "error": format!("写文件失败：{e}") }),
                    },
                    Err(e) => json!({ "ok": false, "error": format!("序列化失败：{e}") }),
                }
            };
            respond(&mut stream, 200, "application/json", &out.to_string())
        }
        ("POST", "/api/load") => {
            use serde_json::json;
            // 不传 path 就载入最新的一份
            let want: Option<String> = serde_json::from_str::<serde_json::Value>(&body_str)
                .ok()
                .and_then(|v| v.get("path").and_then(|x| x.as_str()).map(|s| s.to_string()));
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
                    if sv.module_id != state.module.meta.id {
                        let msg = json!({
                            "ok": false,
                            "error": format!("存档属于模块 {}，当前是 {}", sv.module_id, state.module.meta.id),
                        })
                        .to_string();
                        return respond(&mut stream, 200, "application/json", &msg);
                    }
                    let world = rebuild(&state.module, &sv.commands, sv.day);
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
                let mut nw = build_world(&state.module);
                sim_core::intel::run_intel(&mut nw);
                *guard = Some(nw);
                // 重开 = 清空命令流，否则回放会带着上一局的命令
                state.commands.lock().unwrap().clear();
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

/// 找最新的存档（按文件名里的时间戳）。
fn latest_save() -> Option<String> {
    let dir = std::fs::read_dir("saves").ok()?;
    let mut best: Option<(u64, String)> = None;
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with("save-") || !name.ends_with(".json") {
            continue;
        }
        let secs: u64 = name
            .trim_start_matches("save-")
            .trim_end_matches(".json")
            .parse()
            .unwrap_or(0);
        if best.as_ref().map_or(true, |(b, _)| secs > *b) {
            best = Some((secs, format!("saves/{name}")));
        }
    }
    best.map(|(_, p)| p)
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
    status: &'static str, // pending / approved / denied / expired
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
        notes.push(format!(
            "{unit_name}（{general}）：请示无人答复，已自决改令 {kind}"
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
    if !state.llm.enabled {
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
            if let Some(ask) = build_ask(state, w, *idx, &text) {
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
    let u = w.units.get(idx)?;
    if !u.is_alive() || !u.kind.is_ground() {
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
    for (pid, hops) in sim_core::systems::reachable(w, u.position, u.side, u.kind) {
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
            "attack" | "move" | "retreat" => j
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
        let kind = if matches!(kind, "attack" | "move" | "retreat") && target.is_none() {
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
        if w.units[idx].side == Side::Cn {
            let title = format!("{} {}", kind, j.target.clone().unwrap_or_default());
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
                    text: text.clone(),
                    proposed: Some((kind.to_string(), target)),
                    options: vec!["准他改令".into(), "驳回，维持原令".into()],
                    deadline: day + 2,
                    status: "pending",
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
    if !state.llm.enabled {
        return;
    }
    let day = w.tick;
    let interval = state.llm.interval_days.max(1);

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
    json!({
        "tick": w.tick,
        "campaign_days": w.campaign_days,
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






