//! 预览版·弹药与油料。
//!
//! 这一组测试的立场：开关关着的时候，这些东西**完全不存在**；
//! 开关打开的时候，它们必须真的改变结果（否则玩家点了"开"却什么都没发生）。

use sim_core::world::{Order, Province, Terrain, Unit, UnitKind};
use sim_core::{systems, Features, Fixed, Side, World};

fn fx_ammo() -> Features {
    Features {
        ammo_fuel: true,
        ..Features::none()
    }
}

/// 一条三省的战线：A（我方、补给源）— B（我方）— C（敌方守军）。
///
/// 为什么不是两省：两省图上，双方各自的邻省全是敌人，`is_encircled` 会把
/// 两边都判成被围，补给同时归零——那样测的就不是弹药而是断粮了。
fn line(fx: Features) -> World {
    let a = Province::new("A", Terrain::Plain, Side::A)
        .with_neighbors(&[1])
        .as_source();
    let b = Province::new("B", Terrain::Plain, Side::A).with_neighbors(&[0, 2]);
    let c = Province::new("C", Terrain::Plain, Side::B).with_neighbors(&[1]);
    let mut cn = Unit::new("1st", Side::A, 1, 10);
    cn.order = Order::Attack(2);
    cn.manual = true; // AI 不接管：这里测的是玩家下的命令
    let mut jp = Unit::new("2nd", Side::B, 2, 10);
    jp.order = Order::Hold;
    jp.manual = true;
    World::new(11, vec![a, b, c], vec![cn, jp]).with_features(fx)
}

#[test]
fn off_by_default_nothing_changes() {
    let mut w = line(Features::none());
    for _ in 0..10 {
        systems::tick(&mut w);
    }
    for u in &w.units {
        assert_eq!(u.ammo, Fixed::ONE, "开关关着时弹药不许动");
        assert_eq!(u.fuel, Fixed::ONE, "开关关着时油料不许动");
    }
}

#[test]
fn fighting_burns_ammo() {
    let mut w = line(fx_ammo());
    for _ in 0..3 {
        systems::tick(&mut w);
    }
    // 打起来之后每天烧 8%，补给每天补 34%×补给水平，所以稳态在七成到满之间。
    assert!(
        w.units[0].ammo < Fixed::ONE,
        "打了三天凭什么还是满弹？实测 {}",
        w.units[0].ammo.to_display_f64()
    );
    assert!(
        w.units[0].ammo > Fixed::ratio(5, 10),
        "有补给时不该打空，实测 {}",
        w.units[0].ammo.to_display_f64()
    );
}

#[test]
fn cut_off_units_run_out_of_ammo() {
    // 补给断绝的部队补不上弹药：存量打完就打不动了。
    let mut w = line(fx_ammo());
    for _ in 0..10 {
        systems::tick(&mut w);
    }
    let starving = w.units[1].ammo; // 敌方那支孤军：这张图上没有它的补给源
    assert!(
        starving < w.units[0].ammo,
        "断补给的一方应该比有补给的一方弹更少（{} vs {}）",
        starving.to_display_f64(),
        w.units[0].ammo.to_display_f64()
    );
}

#[test]
fn ammo_floor_still_fights_but_weaker() {
    let mut full = line(fx_ammo());
    full.units[0].ammo = Fixed::ONE;
    let mut empty = line(fx_ammo());
    empty.units[0].ammo = Fixed::ZERO;
    assert!(empty.ammo_factor(&empty.units[0]) < full.ammo_factor(&full.units[0]));
    // 打光弹药不等于没有火力：地板 35%。
    assert!(empty.ammo_factor(&empty.units[0]) >= Fixed::ratio(35, 100));
    // 关着的时候恒为 1，老战役因此一丝一毫都不受影响。
    let off = line(Features::none());
    for u in &off.units {
        assert_eq!(off.ammo_factor(u), Fixed::ONE);
    }
}

#[test]
fn armored_units_burn_fuel_when_they_move() {
    let a = Province::new("A", Terrain::Plain, Side::A)
        .with_neighbors(&[1])
        .as_source();
    let b = Province::new("B", Terrain::Plain, Side::A).with_neighbors(&[0, 2]);
    let c = Province::new("C", Terrain::Plain, Side::A).with_neighbors(&[1]);
    let mut tank = Unit::new("tank", Side::A, 0, 5);
    tank.kind = UnitKind::Armor;
    tank.fuel_burn = Fixed::ratio(15, 100);
    tank.order = Order::Move(2);
    tank.manual = true;
    let mut w = World::new(3, vec![a, b, c], vec![tank]).with_features(fx_ammo());

    systems::tick(&mut w);
    assert!(w.units[0].fuel < Fixed::ONE, "装甲动一步就该烧油");
    // 走到目标后原地固守，不再烧油。
    for _ in 0..5 {
        systems::tick(&mut w);
    }
    assert_eq!(w.units[0].position, 2, "应该已经走到 C");
}

#[test]
fn no_fuel_no_movement_when_cut_off() {
    // 断补给的装甲：燃料补不上来，烧干之后就动不了。
    let a = Province::new("A", Terrain::Plain, Side::B)
        .with_neighbors(&[1])
        .as_source();
    let b = Province::new("B", Terrain::Plain, Side::B).with_neighbors(&[0, 2]);
    let c = Province::new("C", Terrain::Plain, Side::B).with_neighbors(&[1]);
    let mut tank = Unit::new("tank", Side::A, 0, 5);
    tank.kind = UnitKind::Armor;
    tank.fuel_burn = Fixed::ratio(15, 100);
    tank.fuel = Fixed::ZERO;
    tank.supply = Fixed::ZERO;
    tank.order = Order::Move(2);
    tank.manual = true;
    let mut w = World::new(3, vec![a, b, c], vec![tank]).with_features(fx_ammo());

    // 我方（Cn）在这张图上没有任何省，补给恒为 0 → 补不上油。
    for _ in 0..6 {
        systems::tick(&mut w);
    }
    assert_eq!(w.units[0].position, 0, "没油就该原地不动");
}
