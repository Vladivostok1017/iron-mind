//! `preview` 段的语义（1.0 起）：**默认全开，写 false 才关**。
//!
//! 历史：0.12 时这里是"只有显式 true 才开"的预览开关，为的是不动已定稿的
//! 第一版发行版。1.0 把这几个机制并回主线，于是语义翻了过来：
//! 不写 = 用正式基线；想退回旧行为必须显式关掉（而且关掉会在界面上挂牌子）。

use sim_content::model::{OobDef, ScenarioDef};

fn scen(json: &str) -> ScenarioDef {
    serde_json::from_str(json).expect("解析 scenario.json 失败")
}

#[test]
fn no_preview_block_means_everything_on() {
    let s = scen(r#"{"id":"a","name":"b","days":30}"#);
    assert!(s.preview.is_none(), "不写 preview 段");
    let fx = sim_content::features_from(s.preview.as_ref(), sim_content::model::ScaleDef::Front);
    assert!(fx.ammo_fuel && fx.rail_damage && fx.politics && fx.front_line);
    assert!(fx.disabled().is_empty(), "没有关掉的机制");
}

#[test]
fn explicit_false_turns_one_mechanism_off() {
    let s = scen(r#"{"id":"a","name":"b","days":30,"preview":{"front_line":false}}"#);
    let fx = sim_content::features_from(s.preview.as_ref(), sim_content::model::ScaleDef::Front);
    assert!(!fx.front_line, "显式关掉");
    assert!(fx.ammo_fuel, "没提到的保持默认开");
    assert_eq!(fx.disabled(), vec!["front_line"]);
}

#[test]
fn explicit_true_is_same_as_default() {
    let s = scen(r#"{"id":"a","name":"b","days":30,"preview":{"ammo_fuel":true}}"#);
    let fx = sim_content::features_from(s.preview.as_ref(), sim_content::model::ScaleDef::Front);
    assert_eq!(fx, sim_core::Features::default());
}

#[test]
fn all_off_is_expressible_for_legacy_reproduction() {
    let s = scen(
        r#"{"id":"a","name":"b","days":30,
            "preview":{"ammo_fuel":false,"rail_damage":false,"politics":false,"front_line":false}}"#,
    );
    let fx = sim_content::features_from(s.preview.as_ref(), sim_content::model::ScaleDef::Front);
    assert_eq!(fx, sim_core::Features::none(), "四个都关 = 0.12 的旧语义");
    assert_eq!(fx.disabled().len(), 4);
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
