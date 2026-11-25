//! 规则条件：`side` 与 `in_combat`（上游报告 §7）。
//!
//! 没有这两条的时候：想写"国军弹药不足"写不出来（规则会同时作用在双方），
//! 想写"巷战绞肉"只能按地形按天收税——对所有驻军一起收，91 天固定 −32%，
//! 把胜负彻底搅浑。这两个谓词就是为这两类需求加的。
//!
//! 测试用"对照组"比较：开规则的世界 vs 同一初始状态的对照世界，
//! 这样断粮、战斗结算等无关因素不会混进来。

use sim_core::rules::{Condition, Phase, Primitive, Rule, RuleSet};
use sim_core::world::{Order, Province, Terrain, Unit};
use sim_core::{systems, Fixed, Side, World};

fn setup() -> World {
    let a = Province::new("A", Terrain::Urban, Side::Cn)
        .with_neighbors(&[1])
        .as_source();
    let b = Province::new("B", Terrain::Urban, Side::Jp).with_neighbors(&[0]);
    let mut cn = Unit::new("cn", Side::Cn, 0, 10);
    cn.manual = true;
    cn.order = Order::Hold;
    let mut jp = Unit::new("jp", Side::Jp, 0, 10);
    jp.manual = true;
    jp.order = Order::Hold;
    World::new(5, vec![a, b], vec![cn, jp])
}

fn with_rule(w: &mut World, when: Condition) {
    w.rules = std::sync::Arc::new(RuleSet::new(vec![Rule {
        id: "r".into(),
        phase: Phase::PostTick,
        priority: 0,
        when,
        // 负值 = 掉兵（和模块里写 -0.1 的意思一样）
        then: vec![Primitive::ModifyStrengthPct(Fixed::ratio(-1, 10))],
    }]));
}

/// 跑一天，返回 (国军兵力, 日军兵力)。
fn one_day(mut w: World) -> (f64, f64) {
    systems::tick(&mut w);
    (
        w.units[0].strength.to_display_f64(),
        w.units[1].strength.to_display_f64(),
    )
}

#[test]
fn side_condition_only_hits_one_side() {
    let control = one_day(setup());
    let mut ruled = setup();
    with_rule(&mut ruled, Condition::UnitSide(Side::Cn));
    let got = one_day(ruled);
    assert!(got.0 < control.0 - 1e-6, "国军应该掉兵：{} vs 对照 {}", got.0, control.0);
    assert!((got.1 - control.1).abs() < 1e-6, "日军不该被这条规则影响");
}

#[test]
fn in_combat_only_hits_units_that_are_actually_fighting() {
    // 两个单位分省驻守 = 不在交战 → 规则不起作用。
    let apart = || {
        let mut w = setup();
        w.units[1].position = 1;
        w
    };
    let control = one_day(apart());
    let mut ruled = apart();
    with_rule(&mut ruled, Condition::InCombat);
    let got = one_day(ruled);
    assert!(
        (got.0 - control.0).abs() < 1e-6 && (got.1 - control.1).abs() < 1e-6,
        "没有交战就不该收税：{got:?} vs 对照 {control:?}"
    );
}

#[test]
fn in_combat_fires_when_both_sides_share_a_province() {
    let control = one_day(setup());
    let mut ruled = setup();
    with_rule(&mut ruled, Condition::InCombat);
    let got = one_day(ruled);
    assert!(got.0 < control.0 - 1e-6 && got.1 < control.1 - 1e-6, "两边都在挨打才算绞肉");
}

#[test]
fn combinations_work() {
    // "只对正在交战的国军收税"——作者真正想写的那条规则。
    let control = one_day(setup());
    let mut ruled = setup();
    with_rule(
        &mut ruled,
        Condition::All(vec![Condition::UnitSide(Side::Cn), Condition::InCombat]),
    );
    let got = one_day(ruled);
    assert!(got.0 < control.0 - 1e-6, "国军应该掉兵");
    assert!((got.1 - control.1).abs() < 1e-6, "日军不该掉");
}
