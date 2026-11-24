//! 无头测试台。
//!
//! 四件事：
//! 1. 加载模块（或内置场景），打印校验问题
//! 2. 跑两遍同种子，比对逐 tick 的状态哈希 —— 确定性验证
//! 3. 换种子，确认结果确实不同 —— 随机流生效验证
//! 4. 打印战局演化 —— 人眼确认这个模拟"看起来像回事"

use std::path::Path;

use sim_core::world::{Side, World};
use sim_core::{scenario, systems};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ticks: u32 = arg_or(&args, "--ticks", 120);
    let seed_override: Option<u64> = arg_or_opt(&args, "--seed");
    let verbose = args.iter().any(|a| a == "--verbose");
    let module_dir = arg_str(&args, "--module");
    // --trace 第101师团：逐 tick 打某个单位的位置序列。
    //
    // 为什么要这个：快照会骗人。"一个师团突然出现在后方"到底是一天走一省
    // 渗透过来的，还是被某段代码瞬移过去的，只有**时序**能回答。
    let trace = arg_str(&args, "--trace");

    println!("== Iron Mind · 无头测试台 ==\n");

    // ---- 加载 ----
    let (world, label) = match module_dir {
        Some(dir) => {
            let path = Path::new(&dir);
            println!("模块目录：{}", path.display());
            match sim_content::load_module(path) {
                Ok(m) => {
                    println!(
                        "  已加载 {} v{}（schema {}，引擎 >= {}）",
                        m.meta.name, m.meta.version, m.meta.content_schema, m.meta.engine.min
                    );
                    println!(
                        "  省份 {} · 单位 {} · 战役 {} 天 · 可玩方 {}",
                        m.map.provinces.len(),
                        m.oob.units.len(),
                        m.scenario.days,
                        m.meta.playable_side.as_str()
                    );
                    for w in &m.warnings {
                        println!("  [警告] {}: {}", w.code, w.message);
                    }
                    let mut w = sim_content::build_world(&m);
                    if let Some(s) = seed_override {
                        w.seed = s;
                        println!("  覆盖种子：{s}");
                    }
                    println!("  种子 {}", w.seed);
                    (w, m.meta.name.clone())
                }
                Err(e) => {
                    eprintln!("\n加载失败：{e}");
                    std::process::exit(2);
                }
            }
        }
        None => {
            let s = seed_override.unwrap_or(20250924);
            println!("使用内置演示场景（不是史实数据）· 种子 {s}");
            (scenario::nanxun_line(s), "内置演示场景".to_string())
        }
    };

    let seed = world.seed;
    println!("\n【{label}】种子 {seed} · {ticks} tick\n");

    // ---- 确定性：同输入两次 ----
    let (world_a, hashes_a) = simulate(world.clone(), ticks, verbose, trace.as_deref());
    let (_world_b, hashes_b) = simulate(world.clone(), ticks, false, None);

    if hashes_a == hashes_b {
        println!("[OK] 确定性通过：{ticks} 个 tick 的哈希序列完全一致");
        println!(
            "     末 tick 哈希 = {:#018x}",
            hashes_a.last().copied().unwrap_or(0)
        );
    } else {
        let at = hashes_a
            .iter()
            .zip(hashes_b.iter())
            .position(|(x, y)| x != y)
            .unwrap_or(0);
        println!("[FAIL] 确定性被破坏！首个分歧在 tick {at}");
        std::process::exit(1);
    }

    // ---- 换种子应产生不同演化 ----
    let mut other = world.clone();
    other.seed = seed.wrapping_add(1);
    let (_, hashes_c) = simulate(other, ticks, false, None);
    if hashes_c.last() != hashes_a.last() {
        println!("[OK] 随机流生效：不同种子得到不同结局");
    } else {
        println!("[WARN] 不同种子得到相同结局，随机扰动可能未生效");
    }

    // ---- 终局 ----
    println!("\n-- 终局 --");
    report(&world_a);
    if !world_a.log.is_empty() {
        println!("\n-- 战报 --");
        for e in world_a.log.iter().take(10) {
            println!("  {}", e.line());
        }
        if world_a.log.len() > 10 {
            println!("  ... 共 {} 条", world_a.log.len());
        }
    }

    println!("\n-- 各省归属 --");
    print_map(&world_a);
}

fn arg_str(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn arg_or<T: std::str::FromStr>(args: &[String], key: &str, default: T) -> T {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn arg_or_opt<T: std::str::FromStr>(args: &[String], key: &str) -> Option<T> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
}

fn simulate(mut world: World, ticks: u32, verbose: bool, trace: Option<&str>) -> (World, Vec<u64>) {
    let mut hashes = Vec::with_capacity(ticks as usize);
    // 被追踪单位的索引（按名字找一次）
    let traced = trace.and_then(|name| {
        world
            .units
            .iter()
            .position(|u| u.name.contains(name))
            .map(|i| (i, name.to_string()))
    });
    if let Some((_, name)) = &traced {
        println!("-- 追踪 {name}（逐 tick 位置）--");
    }
    for t in 0..ticks {
        systems::tick(&mut world);
        hashes.push(world.state_hash());
        if let Some((i, _)) = traced {
            if i < world.units.len() {
                let u = &world.units[i];
                let prov = &world.provinces[u.position as usize];
                let target = u
                    .order
                    .target()
                    .and_then(|t| world.provinces.get(t as usize))
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "-".to_string());
                println!(
                    "  tick {:3}  {:8} 兵{:5.1} 补{:.2} 归{} →{}",
                    world.tick,
                    prov.name,
                    u.strength.to_display_f64(),
                    u.supply.to_display_f64(),
                    match prov.owner {
                        Side::Cn => "国",
                        Side::Jp => "日",
                    },
                    target
                );
            }
        }
        if verbose && t % 15 == 0 {
            println!("-- tick {} --", world.tick);
            report(&world);
        }
    }
    (world, hashes)
}

fn report(w: &World) {
    let cn = w.total_strength(Side::Cn);
    let jp = w.total_strength(Side::Jp);
    let cn_prov = w.provinces.iter().filter(|p| p.owner == Side::Cn).count();
    let jp_prov = w.provinces.iter().filter(|p| p.owner == Side::Jp).count();
    println!(
        "  tick {:3} | 国军 {:5.1} 千 守 {:2} 省 | 日军 {:5.1} 千 占 {:2} 省 | 存活 {}",
        w.tick,
        cn.to_display_f64(),
        cn_prov,
        jp.to_display_f64(),
        jp_prov,
        w.units.iter().filter(|u| u.is_alive()).count()
    );
    if std::env::args().any(|a| a == "--units") {
        for u in w.units.iter().filter(|u| u.is_alive()) {
            let tgt = u
                .order
                .target()
                .map(|t| w.provinces[t as usize].name.as_str())
                .unwrap_or("-");
            println!(
                "      {:<8} {:<5} 兵{:5.1} 补{:.2} 士{:.2} → {} {}{}",
                u.name,
                u.commander.name,
                u.strength.to_display_f64(),
                u.supply.to_display_f64(),
                u.morale.to_display_f64(),
                u.order.label(),
                tgt,
                if u.disobeyed {
                    format!(
                        "   ⚠ 抗命：{}",
                        u.dissent_reason.as_deref().unwrap_or("")
                    )
                } else {
                    String::new()
                }
            );
        }
    }
}

fn print_map(w: &World) {
    for (i, p) in w.provinces.iter().enumerate() {
        let mark = match p.owner {
            Side::Cn => "中",
            Side::Jp => "日",
        };
        let unit_here: Vec<&str> = w
            .units
            .iter()
            .filter(|u| u.is_alive() && u.position as usize == i)
            .map(|u| u.name.as_str())
            .collect();
        println!(
            "  [{mark}] {:<6} {:<9} 工事{} {:>5} {}",
            p.name,
            format!("{:?}", p.terrain),
            p.fort,
            if p.is_supply_source { "补给源" } else { "" },
            if unit_here.is_empty() {
                String::new()
            } else {
                format!("← {}", unit_here.join("、"))
            }
        );
    }
}


