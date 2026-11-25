//! P2：海战、制空权、行军纵队被空袭、铁路与水路运力。
//!
//! 四件事以前都不成立：
//! 1. 两支舰队停在同一段江面上互相看不见（舰队只是"浮动加成器"）；
//! 2. 空军不管谁有制空权，投弹效率一样；
//! 3. 行军纵队和蹲在工事里的部队一样好打；
//! 4. 补给只认省上的一个常数，铁路不存在，水道直接跳过。

use sim_core::world::{Order, Province, Side, Terrain, Unit, UnitKind, World};
use sim_core::Fixed;

/// 一段水道 + 两岸，双方各一支舰队。
fn naval() -> World {
    let mut shore = Province::new("江岸", Terrain::Plain, Side::A);
    shore.is_supply_source = true;
    shore.neighbors = vec![1];
    let mut river = Province::new("长江·某段", Terrain::Water, Side::A);
    river.neighbors = vec![0, 2];
    river.is_supply_source = true;
    // 日军控制的下一段水道：给日军舰队一条补给线。
    // 没有它，日军舰队会被"合围"打成补给 0 → 战斗力 0 → 只能挨打不能还手。
    let mut lower = Province::new("长江·下游", Terrain::Water, Side::B);
    lower.neighbors = vec![1];
    lower.is_supply_source = true;

    let mut cn = Unit::new("海军炮艇队", Side::A, 1, 2);
    cn.kind = UnitKind::Naval;
    cn.manual = true;
    cn.order = Order::Hold;
    let mut jp = Unit::new("第11战队", Side::B, 1, 3);
    jp.kind = UnitKind::Naval;
    jp.manual = true;
    jp.order = Order::Hold;

    let mut w = World::new(3, vec![shore, river, lower], vec![cn, jp]);
    w.campaign_days = u32::MAX;
    w.comms.transmission = 0;
    w.comms.staff = 0;
    w
}

#[test]
fn two_fleets_in_one_reach_fight() {
    let mut w = naval();
    let (a0, b0) = (
        w.units[0].strength.to_display_f64(),
        w.units[1].strength.to_display_f64(),
    );
    sim_core::run_ticks(&mut w, 2);
    let (a1, b1) = (
        w.units[0].strength.to_display_f64(),
        w.units[1].strength.to_display_f64(),
    );
    assert!(a1 < a0 && b1 < b0, "同一段水道应当互相炮击（{a0}/{b0} → {a1}/{b1}）");
    assert!(
        w.log.iter().any(|e| e.text.contains("炮战")),
        "炮战要有战报：{:?}",
        w.log.iter().map(|e| &e.text).collect::<Vec<_>>()
    );
}

#[test]
fn fleets_in_different_reaches_do_not_fight() {
    let mut w = naval();
    // 把日军舰队挪到岸上省（不是水道）——它就不该参战
    w.units[1].position = 0;
    let before = w.units[1].strength;
    sim_core::run_ticks(&mut w, 2);
    assert_eq!(w.units[1].strength, before, "不在同一段水道就不该打起来");
}

/// 空军 + 一个敌军目标。
fn air_war(enemy_air: Option<f64>) -> World {
    let mut base = Province::new("机场", Terrain::Plain, Side::A);
    base.is_supply_source = true;
    base.neighbors = vec![1];
    let mut target = Province::new("敌阵地", Terrain::Plain, Side::B);
    target.is_supply_source = true;
    target.neighbors = vec![0, 2];
    // 再往南两跳：给"行军纵队"测试留出路程。
    // 否则一个 tick 就走到头，命令被清成固守，测不出"在路上"。
    let mut mid = Province::new("敌中途", Terrain::Plain, Side::B);
    mid.neighbors = vec![1, 3];
    let mut rear = Province::new("敌后方", Terrain::Plain, Side::B);
    rear.neighbors = vec![2];
    rear.is_supply_source = true;

    let mut air = Unit::new("第3飞行团", Side::A, 0, 1);
    air.kind = UnitKind::Air;
    air.recon_radius = 2;
    air.strike_power = Fixed::ratio(35, 100);
    // 制空权测试要让空军自己占优势方
    air.side = Side::A;

    let mut ground = Unit::new("敌守军", Side::B, 1, 10);
    ground.manual = true;
    ground.order = Order::Hold;

    let mut w = World::new(5, vec![base, target, mid, rear], vec![air, ground]);
    if let Some(power) = enemy_air {
        let mut foe = Unit::new("敌飞行团", Side::B, 1, 1);
        foe.kind = UnitKind::Air;
        foe.recon_radius = 1;
        foe.strike_power = Fixed::from_f64_input(power);
        w.units.push(foe);
    }
    w.campaign_days = u32::MAX;
    w.comms.transmission = 0;
    w.comms.staff = 0;
    w
}

#[test]
fn enemy_air_supremacy_reduces_bombing_effect() {
    let mut free = air_war(None);
    sim_core::run_ticks(&mut free, 1);
    let dmg_free = 10.0 - free.units[1].strength.to_display_f64();

    let mut contested = air_war(Some(2.0)); // 敌机远比我们强
    sim_core::run_ticks(&mut contested, 1);
    let dmg_contested = 10.0 - contested.units[1].strength.to_display_f64();

    assert!(dmg_free > 0.05, "没有敌机时应当正常投弹（{dmg_free}）");
    assert!(
        dmg_contested < dmg_free * 0.8,
        "敌方掌握制空权时投弹效率应当明显下降（{dmg_free} → {dmg_contested}）"
    );
}

#[test]
fn marching_columns_are_easier_to_bomb() {
    // 同一支部队，一版让它原地守、一版让它行军
    let mut dug = air_war(None);
    dug.units[1].order = Order::Hold;
    sim_core::run_ticks(&mut dug, 1);
    let dmg_dug = 10.0 - dug.units[1].strength.to_display_f64();

    let mut moving = air_war(None);
    // 目标省只有一个邻省，让它下令推进到那儿 —— 它就在"路上"
    moving.units[1].order = Order::Move(3);
    moving.units[1].manual = true;
    sim_core::run_ticks(&mut moving, 1);
    let dmg_moving = 10.0 - moving.units[1].strength.to_display_f64();

    assert!(
        dmg_moving > dmg_dug,
        "行军纵队应当比据守部队更好打（{dmg_dug} → {dmg_moving}）"
    );
}

#[test]
fn railway_carries_supply_further() {
    // 0(补给源) — 1(普通) — 2(远端)
    let build = |rail: bool| {
        let mut a = Province::new("后方", Terrain::Plain, Side::A);
        a.is_supply_source = true;
        a.neighbors = vec![1];
        let mut b = Province::new("中间", Terrain::Plain, Side::A);
        b.neighbors = vec![0, 2];
        b.rail = rail;
        let mut c = Province::new("前线", Terrain::Plain, Side::A);
        c.neighbors = vec![1];
        let mut u = Unit::new("前线师", Side::A, 2, 8);
        u.supply_reach = Fixed::ZERO;
        u.manual = true;
        u.order = Order::Hold;
        let mut w = World::new(1, vec![a, b, c], vec![u]);
        w.campaign_days = u32::MAX;
        sim_core::run_ticks(&mut w, 1);
        w.units[0].supply.to_display_f64()
    };
    let without = build(false);
    let with = build(true);
    assert!(
        with > without,
        "有铁路的补给线应当送得更远（{without} → {with}）"
    );
}

#[test]
fn river_carries_supply() {
    // 补给源在江对岸的水道省：以前水道被直接跳过，根本送不过去
    let mut a = Province::new("后方", Terrain::Plain, Side::A);
    a.is_supply_source = true;
    a.neighbors = vec![1];
    let mut river = Province::new("长江·某段", Terrain::Water, Side::A);
    river.neighbors = vec![0, 2];
    let mut c = Province::new("前线", Terrain::Plain, Side::A);
    c.neighbors = vec![1];
    let mut u = Unit::new("前线师", Side::A, 2, 8);
    u.supply_reach = Fixed::ZERO;
    u.manual = true;
    u.order = Order::Hold;
    let mut w = World::new(1, vec![a, river, c], vec![u]);
    w.campaign_days = u32::MAX;
    sim_core::run_ticks(&mut w, 1);
    assert!(
        w.units[0].supply.to_display_f64() > 0.3,
        "水道应当能运补给（实际 {}）",
        w.units[0].supply.to_display_f64()
    );
}
