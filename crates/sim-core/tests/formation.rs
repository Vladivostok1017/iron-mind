//! 兵团是实体，不只是字符串。
//!
//! 以前 `formation` 只是一个名字：兵团部存不存在、在不在，都不影响任何事。
//! "打掉敌人的兵团部"在引擎里毫无意义——因为它根本没有实体。
//!
//! 现在兵团有 `hq`（兵团部单位），并且**兵团部覆灭会压低下属的服从度**。
//! `obedience` 是抗命的唯一硬约束（规则 AI 与 LLM 提示词都读它），
//! 所以这一条不需要新机制，只是让已有的数字反映编制状态。

use sim_core::world::{Commander, Formation, Province, Side, Terrain, Unit, World};
use sim_core::Fixed;

fn world() -> World {
    let p = Province::new("战场", Terrain::Plain, Side::A);
    let mut hq = Unit::new("第29军团", Side::A, 0, 8);
    hq.formation = "第1兵团".into();
    hq.commander = Commander::new("李汉魂", 0.8, 0.5, 0.6, 0.5);
    let mut sub = Unit::new("第187师", Side::A, 0, 6);
    sub.formation = "第29军团".into();
    sub.commander = Commander::new("孔可权", 0.8, 0.5, 0.4, 0.6);
    let mut w = World::new(1, vec![p], vec![hq, sub]);
    w.formations = vec![
        Formation {
            name: "第1兵团".into(),
            side: Side::A,
            hq: None, // 纸面番号：没有兵团部这个单位
            units: vec![0],
        },
        Formation {
            name: "第29军团".into(),
            side: Side::A,
            hq: Some(0), // 兵团部就是 0 号单位自己
            units: vec![0, 1],
        },
    ];
    w
}

#[test]
fn paper_formation_changes_nothing() {
    let w = world();
    assert_eq!(w.hq_alive(0), None, "纸面番号没有兵团部");
    assert_eq!(
        (w.effective_obedience(0).to_display_f64() - 0.8).abs() < 1e-3,
        true,
        "没有兵团部就不该有加成或惩罚"
    );
}

#[test]
fn living_hq_helps_obedience() {
    let w = world();
    assert_eq!(w.hq_alive(1), Some(true));
    let e = w.effective_obedience(1).to_display_f64();
    assert!((e - 0.85).abs() < 1e-3, "兵团部在，服从度 +0.05（实际 {e}）");
}

#[test]
fn destroyed_hq_hurts_obedience() {
    let mut w = world();
    // 兵团部被打掉
    w.units[0].strength = Fixed::ZERO;
    w.units[0].ended_day = Some(3);
    assert_eq!(w.hq_alive(1), Some(false));
    let e = w.effective_obedience(1).to_display_f64();
    assert!(
        (e - 0.65).abs() < 1e-3,
        "兵团部没了，服从度 -0.15（实际 {e}）"
    );
}

#[test]
fn obedience_is_clamped() {
    let mut w = world();
    w.units[1].commander.obedience = Fixed::from_f64_input(0.05);
    w.units[0].strength = Fixed::ZERO;
    w.units[0].ended_day = Some(1);
    assert_eq!(
        w.effective_obedience(1).to_display_f64(),
        0.0,
        "再低也不能变成负数"
    );
}

#[test]
fn formation_lookup_covers_its_units() {
    let w = world();
    let f = w.formation_of(1).expect("187 师属于第29军团");
    assert_eq!(f.name, "第29军团");
    assert!(f.units.contains(&1));
    assert!(w.formation_of(0).is_some(), "兵团部自己也属于一个番号");
}
