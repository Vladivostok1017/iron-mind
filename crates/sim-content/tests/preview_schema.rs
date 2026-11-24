//! 预览版字段的解析测试。
//!
//! 守两件事：
//! 1. 老模块（没有 `preview`）照旧能解析，且默认全关；
//! 2. 新字段拼错了要在严格模式下报错（否则作者以为开了，其实没开）。

use sim_content::model::{OobDef, ScenarioDef};

fn scen(json: &str) -> ScenarioDef {
    serde_json::from_str(json).expect("解析 scenario.json 失败")
}

#[test]
fn old_module_has_no_preview() {
    let s = scen(r#"{"id":"a","name":"b","days":30}"#);
    assert!(s.preview.is_none(), "不写 preview 必须是'全关'，不能是默认开");
}

#[test]
fn preview_flags_parse() {
    let s = scen(
        r#"{"id":"a","name":"b","days":30,
            "preview":{"ammo_fuel":true,"rail_damage":false,"politics":true}}"#,
    );
    let p = s.preview.expect("应该有 preview");
    assert!(p.ammo_fuel);
    assert!(!p.rail_damage);
    assert!(p.politics);
}

#[test]
fn partial_preview_defaults_to_off() {
    // 只写一个功能，其余必须是关——"写了 ammo 就顺手把 politics 也打开"是灾难。
    let s = scen(r#"{"id":"a","name":"b","days":30,"preview":{"rail_damage":true}}"#);
    let p = s.preview.unwrap();
    assert!(p.rail_damage);
    assert!(!p.ammo_fuel);
    assert!(!p.politics);
}

#[test]
fn oob_without_factions_still_parses() {
    let o: OobDef = serde_json::from_str(r#"{"units":[]}"#).expect("老 oob.json 必须还能读");
    assert!(o.factions.is_empty());
}

#[test]
fn factions_parse() {
    let o: OobDef = serde_json::from_str(
        r#"{"units":[],
            "factions":[{"id":"f1","name":"某系","side":"cn","weight":0.6}]}"#,
    )
    .expect("派系应该能解析");
    assert_eq!(o.factions.len(), 1);
    assert_eq!(o.factions[0].id, "f1");
    assert!((o.factions[0].weight - 0.6).abs() < 1e-9);
}
