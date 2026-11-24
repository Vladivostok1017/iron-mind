//! 结局与整补。
//!
//! 这一组测的是"这一局怎么收场"，以及"打残了还能不能缓过来"。
//! 两条都是 77 天战役里绕不过去的东西：
//! - 没有结局，打到第 78 天游戏就静静卡住；
//! - `整补` 只回士气不回兵，于是打到后期所有部队都是空架子，永远恢复不了。

use sim_core::endings::{Clause, Cmp, Ending, Metric};
use sim_core::world::{Order, Province, Side, Terrain, Unit, World};
use sim_core::Fixed;

fn two_province() -> World {
    let mut a = Province::new("南昌", Terrain::Urban, Side::Cn);
    a.is_supply_source = true;
    a.neighbors = vec![1];
    let mut b = Province::new("九江", Terrain::Plain, Side::Jp);
    b.is_supply_source = true;
    b.neighbors = vec![0];
    // 后方：不让南昌变成"四面皆敌"的孤城（合围会把补给打成 0，整补就测不出来了）
    let mut rear = Province::new("吉安", Terrain::Plain, Side::Cn);
    rear.is_supply_source = true;
    rear.neighbors = vec![0];
    a.neighbors = vec![1, 2];

    let mut u = Unit::new("第74军", Side::Cn, 0, 10);
    u.manual = true;
    u.order = Order::Hold;
    let mut w = World::new(1, vec![a, b, rear], vec![u]);
    w.comms.transmission = 0;
    w.comms.staff = 0;
    w.campaign_days = 5;
    w
}

fn keep_provinces(value: f64) -> Ending {
    Ending {
        id: "E3".into(),
        name: "苦撑".into(),
        text: "守住了。".into(),
        priority: 10,
        at_end_only: true,
        clauses: vec![Clause {
            metric: Metric::Provinces,
            side: Some(Side::Cn),
            cmp: Cmp::Ge,
            value,
            province: None,
        }],
    }
}

#[test]
fn campaign_ends_on_the_declared_day_with_a_named_ending() {
    let mut w = two_province();
    w.endings = vec![keep_provinces(1.0)];
    sim_core::run_ticks(&mut w, 4);
    assert!(w.outcome.is_none(), "还没到日子，不该有结局");
    sim_core::run_ticks(&mut w, 1);
    let o = w.outcome.clone().expect("第 5 日应当判出结局");
    assert_eq!(o.id, "E3");
    assert_eq!(o.day, 5);
    assert!(!o.metrics.is_empty(), "结局要带关键数字给玩家对账");
}

#[test]
fn fallback_ending_when_nothing_matches() {
    let mut w = two_province();
    w.endings = vec![keep_provinces(99.0)]; // 不可能满足
    sim_core::run_ticks(&mut w, 6);
    let o = w.outcome.clone().expect("战役期满必须有结局");
    assert_eq!(o.id, "E0", "没有命中的规则也要给一个，别让战役无声卡住");
}

#[test]
fn early_ending_can_fire_before_the_end() {
    let mut w = two_province();
    w.endings = vec![Ending {
        id: "E7".into(),
        name: "速溃".into(),
        text: "崩了。".into(),
        priority: 50,
        at_end_only: false, // 允许提前
        clauses: vec![Clause {
            metric: Metric::Provinces,
            side: Some(Side::Cn),
            cmp: Cmp::Le,
            value: 0.0,
            province: None,
        }],
    }];
    // 把国军那个省给日军，并在邻省留一支敌军。
    // 不留的话我方部队会在下个 tick 把它重新占回来（"邻接无敌军才占领"）。
    w.provinces[0].owner = Side::Jp;
    w.provinces[2].owner = Side::Jp;
    let mut jp = Unit::new("敌一部", Side::Jp, 1, 3);
    jp.manual = true;
    jp.order = Order::Hold;
    w.units.push(jp);
    sim_core::run_ticks(&mut w, 1);
    assert_eq!(
        w.outcome.as_ref().map(|o| o.id.clone()),
        Some("E7".to_string()),
        "提前触发的结局应当当天生效"
    );
}

#[test]
fn end_only_ending_does_not_fire_early() {
    let mut w = two_province();
    w.endings = vec![keep_provinces(1.0)]; // at_end_only = true
    w.provinces[0].owner = Side::Jp;
    sim_core::run_ticks(&mut w, 2);
    assert!(w.outcome.is_none(), "只判期末的结局，中途条件满足也不该触发");
}

#[test]
fn rest_makes_replacements_only_when_supplied() {
    let mut w = two_province();
    w.campaign_days = u32::MAX;
    w.units[0].strength = Fixed::from_int(5);
    w.units[0].max_strength = Fixed::from_int(10);
    w.units[0].supply = Fixed::from_int(1);
    w.units[0].order = Order::Rest;
    sim_core::run_ticks(&mut w, 10);
    let s = w.units[0].strength.to_display_f64();
    assert!(s > 5.0, "整补十天应当补进新兵（5.0 → {s}）");
    assert!(s < 6.0, "补给到位也不该十天补满（{s}）");
}

#[test]
fn rest_without_supply_brings_no_replacements() {
    let mut p = Province::new("绝地", Terrain::Plain, Side::Cn);
    p.is_supply_source = false; // 没有补给源 → 补给恒 0
    let mut u = Unit::new("残部", Side::Cn, 0, 10);
    u.strength = Fixed::from_int(4);
    u.max_strength = Fixed::from_int(10);
    u.manual = true;
    u.order = Order::Rest;
    let mut w = World::new(1, vec![p], vec![u]);
    w.campaign_days = u32::MAX;
    sim_core::run_ticks(&mut w, 10);
    // 补给上不去 → 补兵也上不去（士气会涨，但兵不会）
    assert!(
        w.units[0].strength.to_display_f64() <= 4.05,
        "没有补给就补不进新兵（实际 {}）",
        w.units[0].strength.to_display_f64()
    );
}
