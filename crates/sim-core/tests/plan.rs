//! 统帅部的作战方针（给非玩家阵营的"假指挥官"）。
//!
//! 之前非玩家阵营没有指挥部：八个师团各自看邻省、各自决定，像八个独立军阀。
//! 现在有一份确定性的方针（不额外花钱）：姿态由兵力比定，
//! 主攻方向由"够得着的敌方省"里价值最高的两个定。

use sim_core::ai::{theater_plan, Posture};
use sim_core::world::{Province, Side, Terrain, Unit, World};

/// 0 我方(源) - 1 中间 - 2 敌方(源) - 3 敌后方(源, 铁路)
fn map() -> World {
    let mut a = Province::new("九江", Terrain::Plain, Side::A);
    a.is_supply_source = true;
    a.neighbors = vec![1];
    let mut b = Province::new("沙河", Terrain::Plain, Side::B);
    b.neighbors = vec![0, 2];
    let mut c = Province::new("德安", Terrain::Plain, Side::B);
    c.is_supply_source = true;
    c.rail = true;
    c.neighbors = vec![1, 3];
    let mut d = Province::new("南昌", Terrain::Plain, Side::B);
    d.is_supply_source = true;
    d.neighbors = vec![2];

    let units = vec![
        Unit::new("第9战区", Side::A, 0, 40),
        Unit::new("第106师团", Side::B, 1, 10),
    ];
    let mut w = World::new(1, vec![a, b, c, d], units);
    w.campaign_days = u32::MAX;
    w
}

#[test]
fn plan_is_deterministic() {
    let w = map();
    let a = theater_plan(&w, Side::A);
    let b = theater_plan(&w, Side::A);
    assert_eq!(a, b, "同一态势必须给出同一方针");
}

#[test]
fn only_reachable_enemy_provinces_are_targets() {
    let w = map();
    let p = theater_plan(&w, Side::A);
    assert!(!p.focus.is_empty(), "应当有主攻方向：{:?}", p.summary);
    // 够得着的只有"沙河"（与九江相邻）；德安、南昌都在后面
    assert_eq!(p.focus[0], 1, "主攻方向应当是相邻的沙河");
    // 沙河不是补给源也不是铁路，价值 0；但它够得着，所以就是目标
    assert!(!p.summary.is_empty());
}

#[test]
fn posture_follows_the_balance() {
    let mut w = map();
    // 我方 40 千 vs 敌方 10 千 → 推进
    assert_eq!(theater_plan(&w, Side::A).posture, Posture::Advance);
    // 把敌方抬到 100 千 → 收缩
    w.units[1].strength = sim_core::Fixed::from_int(100);
    w.units[1].max_strength = sim_core::Fixed::from_int(100);
    // 我方 40 vs 敌方 100：连一半都不到 → 收缩
    assert_eq!(theater_plan(&w, Side::A).posture, Posture::Withdraw);
}

#[test]
fn summary_mentions_direction_and_ratio() {
    let w = map();
    let s = theater_plan(&w, Side::A).summary;
    assert!(s.contains("沙河"), "方针里要有方向：{s}");
    assert!(s.contains("兵力比"), "方针里要有兵力比：{s}");
}

#[test]
fn no_enemy_in_reach_falls_back_to_holding() {
    let mut w = map();
    // 把所有敌方省都变成我方的
    for p in w.provinces.iter_mut() {
        p.owner = Side::A;
    }
    let plan = theater_plan(&w, Side::A);
    assert!(plan.focus.is_empty());
    assert!(plan.summary.contains("没有可攻目标"), "{}", plan.summary);
}
