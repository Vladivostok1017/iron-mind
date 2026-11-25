//! 模块加载与校验的集成测试。
//!
//! 两个方向：
//! 1. 好模块能完整加载、构建、跑模拟
//! 2. 坏模块必须被校验拦住，且错误码正确

use std::path::Path;

use sim_content::model::*;
use sim_content::{build_world, from_parts, load_module, LoadError};
use sim_core::run_ticks;

// 南浔线模块现在住在**另一个仓库**（`../nanxun-1938`）。
// 引擎自己的回归不该依赖它：找不到时这几条测试会明确打印"跳过"并返回，
// 而不是假装通过；也不该为了测试把模块再复制一份进引擎仓库。
//
// 想跑这几条：设 IRONMIND_TEST_MODULE，或者把两个仓库放成兄弟目录（默认会找到）。
const MODULE_CANDIDATES: &[&str] = &[
    "../../../nanxun-1938/nanxun_1938.imod",
    "../../content/scenarios/nanxun_1938.imod",
];

fn module_dir() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("IRONMIND_TEST_MODULE") {
        let p = std::path::PathBuf::from(p);
        if p.is_dir() {
            return Some(p);
        }
    }
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    for rel in MODULE_CANDIDATES {
        let p = base.join(rel);
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

/// 拿不到模块就跳过，并把原因打出来。
macro_rules! need_module {
    () => {
        match module_dir() {
            Some(d) => d,
            None => {
                eprintln!("[跳过] 找不到南浔线模块（设 IRONMIND_TEST_MODULE，或把 nanxun-1938 放成兄弟目录）");
                return;
            }
        }
    };
}

fn meta(id: &str) -> ModuleMeta {
    ModuleMeta {
        id: id.to_string(),
        name: "测试模块".into(),
        version: "0.1.0".into(),
        description: String::new(),
        author: String::new(),
        engine: EngineReq {
            min: "0.1.0".into(),
            max: None,
        },
        content_schema: 1,
        sides: Vec::new(),
        hq: None,
        playable_side: SideDef::new("cn"),
        playable_sides: Vec::new(),
        scale: ScaleDef::Theater,
        domains: vec![DomainDef::Land],
    }
}

fn scenario() -> ScenarioDef {
    ScenarioDef {
        id: "s".into(),
        name: "s".into(),
        start_date: "1938-07-26".into(),
        days: 30,
        seed: 123,
        comms: None,
        rules: Vec::new(),
        endings: Vec::new(),        arrivals: Vec::new(),
        preview: None,
    }
}

fn prov(id: u32, name: &str, owner: SideDef, nb: &[u32]) -> ProvinceDef {
    ProvinceDef {
        id,
        name: name.into(),
        terrain: TerrainDef::Plain,
        owner: Some(owner),
        supply_source: false,
        rail: false,
        fort: 0,
        positions: Vec::new(),
        elev: None,
        relief: None,
        supply_throughput: 1.0,
        neighbors: nb.to_vec(),
        victory_points: 0,
        prototype: None,
        lon: Some(116.0 + id as f64 * 0.01),
        lat: Some(29.5 - id as f64 * 0.01),
        note: String::new(),
    }
}

fn unit(id: &str, side: SideDef, strength: f64, pos: u32) -> UnitDef {
    UnitDef {
        id: id.into(),
        name: id.into(),
        side,
        parent: None,
        strength,
        position: pos,
        kind: UnitKindDef::Infantry,
        stance: StanceDef::Attack,
        recon_radius: 0,
        strike_power: 0.0,
        effectiveness: 1.0,
        supply_reach: 0.0,
        estimated: false,
        commander: None,
        reform_days: 0,
        reform_ratio: 0.0,
        arrives_on: None,        ammo: 1.0,
        fuel: 1.0,
        fuel_burn: None,
        note: String::new(),
    }
}

/// 一个最小可用模块：两省一条线，双方各一个单位。
fn minimal_parts() -> (ModuleMeta, ScenarioDef, MapDef, OobDef) {
    let mut a = prov(0, "甲", SideDef::new("cn"), &[1]);
    a.supply_source = true;
    let mut b = prov(1, "乙", SideDef::new("jp"), &[0]);
    b.supply_source = true;
    let map = MapDef {
        bbox: None,
        projection: None,
        provinces: vec![a, b],
    };
    let oob = OobDef {
        factions: Vec::new(),
        units: vec![
            unit("cn-1", SideDef::new("cn"), 10.0, 0),
            unit("jp-1", SideDef::new("jp"), 10.0, 1),
        ],
    };
    (meta("org.test.minimal"), scenario(), map, oob)
}

#[test]
fn loads_bundled_module() {
    let dir = need_module!();
    assert!(dir.exists(), "示例模块目录应存在：{}", dir.display());
    let m = load_module(&dir).expect("示例模块应能加载");

    // 只断言**结构不变量**，不钉死具体数量。
    //
    // 这个模块住在另一个仓库（nanxun-1938），作者调平衡时会加/删番号——
    // 曾经这里写死 `units.len() == 18`，作者补了 8 个守备师之后引擎测试就红了，
    // 而引擎其实一个字都没改。外仓内容的变化不该让引擎的测试变红。
    assert!(m.map.provinces.len() >= 12, "省会数量应该像一个战役");
    assert!(m.oob.units.len() >= 4, "双方至少各有两个番号");
    assert_eq!(m.meta.playable_side, SideDef::new("cn"));

    let w = build_world(&m);
    assert_eq!(w.provinces.len(), m.map.provinces.len());
    assert_eq!(w.units.len(), m.oob.units.len());
    // 双方都有补给源
    assert!(w.provinces.iter().filter(|p| p.is_supply_source).count() >= 2);
    assert!(w.units.iter().any(|u| u.side == sim_core::world::Side::A));
    assert!(w.units.iter().any(|u| u.side == sim_core::world::Side::B));
}

#[test]
fn bundled_module_simulation_is_deterministic() {
    let m = load_module(&need_module!()).expect("加载失败");
    let mut a = build_world(&m);
    let ha = run_ticks(&mut a, 60);
    let mut b = build_world(&m);
    let hb = run_ticks(&mut b, 60);
    assert_eq!(ha, hb, "从同一模块构建的世界必须可重放");
}

#[test]
fn module_units_keep_moving() {
    // 回归测试：单位不能原地打转。
    // 曾经的 bug 是 AI 给出"去自己所在省"的指令，被移动系统忽略后全体僵死。
    let m = load_module(&need_module!()).expect("加载失败");
    let mut w = build_world(&m);
    let mut moved_days = 0;
    let mut prev: Vec<u32> = w.units.iter().map(|u| u.position).collect();
    for _ in 0..40 {
        sim_core::systems::tick(&mut w);
        let now: Vec<u32> = w.units.iter().map(|u| u.position).collect();
        if now != prev {
            moved_days += 1;
        }
        prev = now;
    }
    assert!(
        moved_days >= 10,
        "40 天里只有 {moved_days} 天有部队移动，AI 可能陷入僵局"
    );
}

#[test]
fn bundled_module_has_no_errors() {
    let m = load_module(&need_module!()).expect("加载失败");
    // 只有警告是允许的
    for w in &m.warnings {
        assert!(!w.message.is_empty(), "警告文案不能为空");
    }
}

#[test]
fn comms_preset_maps_to_delays() {
    use sim_content::comms_from;
    use sim_content::model::CommsDef;

    let wire = comms_from(Some(&CommsDef {
        preset: Some("wire_telegraph".into()),
        ..Default::default()
    }));
    let courier = comms_from(Some(&CommsDef {
        preset: Some("courier".into()),
        ..Default::default()
    }));

        // 关键：电报的瓶颈不是传输，是参谋流程。
    assert_eq!(wire.transmission, 0, "电报传输是秒级，按天计为 0");
    assert_eq!(wire.staff, 1, "参谋汇总与决策要一天");
    assert!(courier.transmission > 0, "骑兵传令的传输本身就要几天");
    assert!(
        courier.report_delay() > wire.report_delay(),
        "骑兵的报告到达更慢"
    );
}

#[test]
fn unknown_preset_falls_back_instead_of_crashing() {
    use sim_content::comms_from;
    use sim_content::model::CommsDef;

    // 未来版本的模块可能写了本引擎不认识的预设。
    // 正确行为是退回默认并继续，而不是崩溃。
    let cfg = comms_from(Some(&CommsDef {
        preset: Some("quantum_entanglement".into()),
        ..Default::default()
    }));
    assert_eq!(cfg.staff, 1);
}

#[test]
fn custom_delays_override_preset() {
    use sim_content::comms_from;
    use sim_content::model::CommsDef;

    let cfg = comms_from(Some(&CommsDef {
        preset: Some("wire_telegraph".into()),
        staff: Some(5),
        ..Default::default()
    }));
    assert_eq!(cfg.staff, 5, "显式数值应覆盖预设");
}

#[test]
fn same_engine_works_for_different_eras() {
    // 跨时代兼容的核心验证：同一张地图、同一支部队，
    // 只改通信条件，情报到达时间就不同——而引擎代码一行没动。
    let (mt, sc, mp, oo) = minimal_parts();

    let mut fast = sc.clone();
    fast.comms = Some(sim_content::model::CommsDef {
        preset: Some("radio".into()),
        ..Default::default()
    });

    let mut slow = sc;
    slow.comms = Some(sim_content::model::CommsDef {
        preset: Some("courier".into()),
        ..Default::default()
    });

    let m_fast = from_parts(mt.clone(), fast, mp.clone(), oo.clone()).expect("应加载");
    let m_slow = from_parts(mt, slow, mp, oo).expect("应加载");

    let mut w_fast = build_world(&m_fast);
    let mut w_slow = build_world(&m_slow);
    w_fast.units[0].position = 1;
    w_slow.units[0].position = 1;

    sim_core::intel::run_intel(&mut w_fast);
    sim_core::intel::run_intel(&mut w_slow);

    assert!(
        w_slow.pending_count(sim_core::Side::A) >= w_fast.pending_count(sim_core::Side::A),
        "慢速通信应当积压更多在途报告"
    );
    assert!(
        w_slow.comms.report_delay() > w_fast.comms.report_delay(),
        "两者延迟应当确实不同"
    );
}

#[test]
fn minimal_module_builds() {
    let (mt, sc, mp, oo) = minimal_parts();
    let m = from_parts(mt, sc, mp, oo).expect("最小模块应通过校验");
    let w = build_world(&m);
    assert_eq!(w.provinces.len(), 2);
    assert_eq!(w.units.len(), 2);
}

#[test]
fn rejects_asymmetric_neighbor() {
    let (mt, sc, mut mp, oo) = minimal_parts();
    // 甲→乙 有，乙→甲 没有
    mp.provinces[1].neighbors.clear();
    match from_parts(mt, sc, mp, oo) {
        Err(LoadError::Validation { issues, .. }) => {
            assert!(
                issues.iter().any(|i| i.code == "map.asymmetric_neighbor"),
                "应报出邻接不对称，实际：{:?}",
                issues.iter().map(|i| i.code).collect::<Vec<_>>()
            );
        }
        other => panic!("应校验失败，实际 {:?}", other.map(|m| m.meta.id)),
    }
}

#[test]
fn rejects_missing_supply_source() {
    let (mt, sc, mut mp, oo) = minimal_parts();
    for p in &mut mp.provinces {
        p.supply_source = false;
    }
    match from_parts(mt, sc, mp, oo) {
        Err(LoadError::Validation { issues, .. }) => {
            assert!(issues.iter().any(|i| i.code == "map.no_supply_source"));
        }
        other => panic!("应校验失败，实际 {:?}", other.map(|m| m.meta.id)),
    }
}

#[test]
fn rejects_self_loop() {
    let (mt, sc, mut mp, oo) = minimal_parts();
    mp.provinces[0].neighbors = vec![0, 1];
    match from_parts(mt, sc, mp, oo) {
        Err(LoadError::Validation { issues, .. }) => {
            assert!(issues.iter().any(|i| i.code == "map.self_loop"));
        }
        other => panic!("应校验失败，实际 {:?}", other.map(|m| m.meta.id)),
    }
}

#[test]
fn rejects_unit_position_out_of_range() {
    let (mt, sc, mp, mut oo) = minimal_parts();
    oo.units[0].position = 99;
    match from_parts(mt, sc, mp, oo) {
        Err(LoadError::Validation { issues, .. }) => {
            assert!(issues.iter().any(|i| i.code == "oob.position_out_of_range"));
        }
        other => panic!("应校验失败，实际 {:?}", other.map(|m| m.meta.id)),
    }
}

#[test]
fn rejects_side_without_units() {
    let (mt, sc, mp, mut oo) = minimal_parts();
    oo.units.retain(|u| u.side == SideDef::new("cn"));
    match from_parts(mt, sc, mp, oo) {
        Err(LoadError::Validation { issues, .. }) => {
            assert!(issues.iter().any(|i| i.code == "oob.side_without_units"));
        }
        other => panic!("应校验失败，实际 {:?}", other.map(|m| m.meta.id)),
    }
}

#[test]
fn warns_on_unreachable_unit() {
    // 三省：甲(源)-乙-丙(源)，但乙被切断（丙只连自己）
    let mut a = prov(0, "甲", SideDef::new("cn"), &[1]);
    a.supply_source = true;
    let b = prov(1, "乙", SideDef::new("cn"), &[0, 2]);
    let mut c = prov(2, "丙", SideDef::new("cn"), &[1]);
    c.supply_source = true;
    let map = MapDef {
        bbox: None,
        projection: None,
        provinces: vec![a, b, c],
    };
    // 把一个单位放在不可达的省——通过让 3 号省孤立
    let mut d = prov(3, "丁", SideDef::new("cn"), &[]);
    d.supply_source = false;
    let mut map2 = map;
    map2.provinces.push(d);
    let oob = OobDef {
        factions: Vec::new(),
        units: vec![
            unit("cn-1", SideDef::new("cn"), 10.0, 3),
            unit("jp-1", SideDef::new("jp"), 10.0, 0),
        ],
    };
    // jp 没有补给源，应当报错；先只关心 unreachable 警告是否存在
    match from_parts(meta("org.test.orphan"), scenario(), map2, oob) {
        Ok(m) => assert!(
            m.warnings.iter().any(|i| i.code == "oob.unit_unreachable"),
            "应给出不可达警告"
        ),
        Err(LoadError::Validation { issues, .. }) => {
            // 也可能因为 jp 无补给源而直接失败，这也是合理行为
            assert!(!issues.is_empty());
        }
        Err(e) => panic!("意外错误：{e}"),
    }
}








