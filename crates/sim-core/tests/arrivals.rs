//! 迟到番号（上游报告 §6）。
//!
//! 没有哪一场战役是第一天全员到齐的：淞沪第10军第 84 天才在金山卫登陆，
//! 而那正是战役的转折点。以前只能把部队事先摆在滩头。

use sim_core::world::{Order, Province, Terrain, Unit};
use sim_core::{systems, Features, Side, World};

fn stage() -> World {
    let a = Province::new("A", Terrain::Plain, Side::Cn)
        .with_neighbors(&[1])
        .as_source();
    let b = Province::new("B", Terrain::Plain, Side::Cn).with_neighbors(&[0]);
    let mut early = Unit::new("early", Side::Cn, 0, 5);
    early.manual = true;
    early.order = Order::Hold;
    let mut late = Unit::new("late", Side::Cn, 1, 5);
    late.manual = true;
    late.order = Order::Hold;
    late.arrives_on = Some(2);
    late.arrive_at = Some(1);
    late.arrived = false;
    let mut w = World::new(23, vec![a, b], vec![early, late]).with_features(Features::default());
    w.has_arrivals = true;
    w
}

#[test]
fn unit_is_invisible_until_its_day() {
    let mut w = stage();
    assert!(w.units[0].is_alive(), "开局的部队当场就在");
    assert!(!w.units[1].is_alive(), "第 2 日才到的部队现在不算存在");

    // world.tick = 已经过去的天数；`arrives_on = 2` 意思是"第 2 日那一 tick 开始在场"。
    systems::tick(&mut w); // 走完第 0 日，tick=1
    systems::tick(&mut w); // 走完第 1 日，tick=2 —— 第 2 日还没开始
    assert!(!w.units[1].arrived, "第 2 日还没开始");
    systems::tick(&mut w); // 第 2 日：开场就到位
    assert!(w.units[1].arrived, "第 2 日应该到场");
    assert!(w.units[1].is_alive());
    assert_eq!(w.units[1].position, 1, "到场位置就是声明的那一省");
}

#[test]
fn arrival_is_deferred_when_the_beachhead_is_lost() {
    let mut w = stage();
    // 登陆场落到敌人手里：部队在海上等，不顺移、不瞬移。
    w.provinces[1].owner = Side::Jp;
    for _ in 0..5 {
        systems::tick(&mut w);
    }
    assert!(!w.units[1].arrived, "登陆场不在手里就不能上岸");
    assert_eq!(w.units[1].position, 1, "位置也不该被改到别处");
}

#[test]
fn arrived_flag_participates_in_the_hash() {
    let a = stage();
    let mut b = stage();
    let h0 = a.state_hash();
    b.units[1].arrived = true;
    assert_ne!(h0, b.state_hash(), "到场没有是世界状态，回放必须能复现");
}

#[test]
fn modules_without_arrivals_are_unaffected() {
    // 老模块（has_arrivals = false）：arrived 不参与哈希，行为也不变。
    let mut w = stage();
    w.has_arrivals = false;
    let h0 = w.state_hash();
    w.units[1].arrived = true;
    assert_eq!(h0, w.state_hash(), "没有迟到番号的模块一个字节都不该变");
}

#[test]
fn arrival_report_is_written() {
    let mut w = stage();
    for _ in 0..3 {
        systems::tick(&mut w);
    }
    let hit = w
        .log
        .iter()
        .any(|e| e.text.contains("抵达") && e.text.contains("late"));
    assert!(hit, "到场要有一条战报，否则玩家不知道援军到了");
}
