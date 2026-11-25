//! 兵力对比的体检（`balance.lopsided`）。
//!
//! 这条不是引擎的平衡表，是**内容层**的提醒：1938 年国军人数远多于日军，
//! 靠装备劣势才打成苦战。数据写成"可玩方比敌人还少"，那这一局多半没法玩——
//! 引擎会照规则跑，但跑出来的是一个不可能赢的局面。
//!
//! 踩过的坑：本模块最初把国军一个军写成 8–15 千（比日军师团还少），
//! 于是"人多却打得苦"这个基本事实在数据里是反的，玩家一上手就发现打不了。

use sim_content::{from_parts, LoadError};
use sim_content::model::*;

fn meta(playable: SideDef) -> ModuleMeta {
    ModuleMeta {
        id: "org.test.balance".into(),
        name: "兵力对比测试".into(),
        version: "0.1.0".into(),
        author: "test".into(),
        description: String::new(),
        engine: EngineReq { min: "0.1.0".into(), max: None },
        content_schema: 1,
        playable_side: playable,
        // 兵力对比与敌方有没有统帅部无关，这里不声明
        hq: None,
        scale: ScaleDef::Theater,
        domains: vec![DomainDef::Land],
    }
}

fn scenario() -> ScenarioDef {
    ScenarioDef {
        id: "scen.test".into(),
        name: "test".into(),
        start_date: "1938-07-26".into(),
        days: 30,
        seed: 1,
        comms: None,
        rules: Vec::<RuleDef>::new(),
        endings: Vec::new(),        arrivals: Vec::new(),
        preview: None,
    }
}

fn prov(id: u32, name: &str, owner: SideDef, nb: &[u32], supply: bool) -> ProvinceDef {
    ProvinceDef {
        id,
        name: name.into(),
        terrain: TerrainDef::Plain,
        owner: Some(owner),
        supply_source: supply,
        rail: false,
        fort: 0,
        supply_throughput: 1.0,
        neighbors: nb.to_vec(),
        victory_points: 0,
        prototype: None,
        lon: Some(116.0 + id as f64 * 0.1),
        lat: Some(29.0 - id as f64 * 0.1),
        positions: Vec::new(),
        elev: None,
        relief: None,
        note: String::new(),
    }
}

fn unit(id: &str, side: SideDef, strength: f64, pos: u32) -> UnitDef {
    UnitDef {
        id: id.into(),
        name: id.into(),
        side,
        parent: None,
        kind: UnitKindDef::Infantry,
        strength,
        position: pos,
        recon_radius: 0,
        strike_power: 0.0,
        effectiveness: 1.0,
        supply_reach: 0.0,
        stance: StanceDef::Attack,
        estimated: true,
        commander: None,
        reform_days: 0,
        reform_ratio: 0.0,
        arrives_on: None,        ammo: 1.0,
        fuel: 1.0,
        fuel_burn: None,
        note: String::new(),
    }
}

fn build(cn: f64, jp: f64) -> Vec<String> {
    let map = MapDef {
        bbox: None,
        projection: None,
        provinces: vec![
            prov(0, "甲", SideDef::Cn, &[1], true),
            prov(1, "乙", SideDef::Jp, &[0], true),
        ],
    };
    let oob = OobDef {
        factions: Vec::new(),
        units: vec![
            unit("cn-1", SideDef::Cn, cn, 0),
            unit("jp-1", SideDef::Jp, jp, 1),
        ],
    };
    match from_parts(meta(SideDef::Cn), scenario(), map, oob) {
        Ok(m) => m.warnings.iter().map(|i| i.code.to_string()).collect(),
        Err(LoadError::Validation { issues, .. }) => {
            issues.iter().map(|i| i.code.to_string()).collect()
        }
        Err(e) => panic!("意外错误：{e}"),
    }
}

#[test]
fn outnumbered_playable_side_warns() {
    // 可玩方比敌人少：正是本模块最初写错的样子
    let codes = build(9.0, 24.0);
    assert!(
        codes.iter().any(|c| c == "balance.lopsided"),
        "可玩方兵力少于敌方时应当警告：{codes:?}"
    );
}

#[test]
fn outnumbering_playable_side_is_quiet() {
    // 史实结构：国军人数远多于日军
    let codes = build(27.0, 24.0);
    assert!(
        !codes.iter().any(|c| c == "balance.lopsided"),
        "可玩方人数占优时不该报警：{codes:?}"
    );
}
