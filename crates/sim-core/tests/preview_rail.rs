//! 预览版·铁路被炸与抢修。
//!
//! 守三件事：
//! 1. 开关关着的时候，轰炸命令什么也做不了（定稿版本没有这条命令）；
//! 2. 打开之后炸弹真的会落到铁路线上，而且炸断有上限；
//! 3. 有人驻守就修得回来，工兵命令修得更快。

use sim_core::rail::{MAX_DAMAGE, REPAIR_DAYS};
use sim_core::world::{Order, Province, Terrain, Unit, UnitKind};
use sim_core::{rail, systems, Features, Side, World};

fn fx_rail() -> Features {
    Features {
        rail_damage: true,
        ..Features::none()
    }
}

/// A(我方·铁路) — B(敌方·铁路) — C(敌方·铁路)，外加一支我方空军在 A。
///
/// 空军侦察半径 2，所以 B、C 都在作战半径内。
fn line(fx: Features) -> World {
    let a = Province::new("A", Terrain::Plain, Side::Cn)
        .with_neighbors(&[1])
        .as_source();
    let mut b = Province::new("B", Terrain::Plain, Side::Jp).with_neighbors(&[0, 2]);
    b.rail = true;
    let mut c = Province::new("C", Terrain::Plain, Side::Jp).with_neighbors(&[1]);
    c.rail = true;
    let mut air = Unit::new("air", Side::Cn, 0, 1);
    air.kind = UnitKind::Air;
    air.recon_radius = 2;
    air.manual = true;
    let mut garrison = Unit::new("garrison", Side::Jp, 1, 5);
    garrison.manual = true;
    World::new(5, vec![a, b, c], vec![air, garrison]).with_features(fx)
}

#[test]
fn bombing_does_nothing_when_off() {
    let mut w = line(Features::none());
    w.units[0].order = Order::BombRail(1);
    for _ in 0..5 {
        systems::tick(&mut w);
    }
    assert_eq!(
        w.provinces[1].rail_damage, 0,
        "开关关着时炸路命令必须是空转，不能偷偷改状态"
    );
}

#[test]
fn bombing_damages_rail_up_to_the_cap() {
    let mut w = line(fx_rail());
    w.units[0].order = Order::BombRail(1);
    systems::tick(&mut w);
    assert_eq!(w.provinces[1].rail_damage, 1, "一次空袭炸坏一级");
    for _ in 0..6 {
        systems::tick(&mut w);
    }
    assert_eq!(
        w.provinces[1].rail_damage, MAX_DAMAGE,
        "炸到底就停在上限，不能无限叠加"
    );
    assert!(rail::is_cut(&w, 1), "3 级就是断");
}

#[test]
fn bombing_out_of_range_does_nothing() {
    let mut w = line(fx_rail());
    w.units[0].recon_radius = 1; // 够不到 C（隔两省）
    w.units[0].order = Order::BombRail(2);
    for _ in 0..3 {
        systems::tick(&mut w);
    }
    assert_eq!(w.provinces[2].rail_damage, 0, "作战半径外的目标炸不到");
}

#[test]
fn garrison_repairs_the_line() {
    let mut w = line(fx_rail());
    w.provinces[1].rail_damage = 2;
    // 守军原地不动，按每天 1 点进度攒，两天修好一级。
    for _ in 0..REPAIR_DAYS as u32 {
        systems::tick(&mut w);
    }
    assert_eq!(w.provinces[1].rail_damage, 1, "驻军应该修好一级");
}

#[test]
fn repair_order_is_faster() {
    let mut w = line(fx_rail());
    w.provinces[1].rail_damage = 2;
    w.units[1].order = Order::RepairRail(1);
    systems::tick(&mut w);
    assert_eq!(
        w.provinces[1].rail_damage, 1,
        "专门的抢修命令一天就该修好一级"
    );
}

#[test]
fn captured_railway_is_not_repaired_by_the_enemy() {
    // 我方部队占住敌方铁路省：归属没变之前，这条线不该被"修好还要通车"。
    let mut w = line(fx_rail());
    w.provinces[1].rail_damage = 2;
    w.units[1].position = 2; // 守军走了
    w.units[1].order = Order::Hold;
    for _ in 0..4 {
        systems::tick(&mut w);
    }
    assert_eq!(
        w.provinces[1].rail_damage, 2,
        "没人守的铁路不该自己长好"
    );
}

#[test]
fn damaged_rail_carries_less_supply() {
    // 通路损耗：完好 0.95、坏一级 0.77、断线退回普通陆路 0.75。
    let mut w = line(fx_rail());
    let good = rail::rail_throughput(&w, 1).to_display_f64();
    w.provinces[1].rail_damage = 1;
    let hurt = rail::rail_throughput(&w, 1).to_display_f64();
    w.provinces[1].rail_damage = MAX_DAMAGE;
    let cut = rail::rail_throughput(&w, 1).to_display_f64();
    assert!(good > hurt && hurt > cut, "{good} > {hurt} > {cut}");
    // Q16.16 的十进制显示会有一点舍入，比较用 1% 的容差。
    assert!((good - 0.95).abs() < 0.01, "完好应该是 0.95，实测 {good}");
    assert!((cut - 0.75).abs() < 0.01, "断线应该退回 0.75，实测 {cut}");
}
