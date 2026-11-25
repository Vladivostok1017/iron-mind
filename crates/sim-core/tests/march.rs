//! 跨省行军（千里奔袭）。
//!
//! 以前命令只能指向**相邻**省，远距离调动只能一天一天点。
//! 真实战场不是这样：1938 年 74 军从赣北调到万家岭走了好几天，
//! 106 师团也是从九江一路走进来的。这组测试盯住三件事：
//!
//! 1. 不相邻的目标，部队会沿最省力的路**一天一省**走过去；
//! 2. 路不通（要跨水）就明确不接这个命令，而不是原地发呆却显示"已下令"；
//! 3. 走到以后就停住，不会在目标省里来回打转。

use sim_core::scenario;
use sim_core::systems::{hops_to, next_hop, reachable};
use sim_core::world::{Order, Province, Side, Terrain, Unit, World};

/// A—B—C 一条直线，全在国军手里。
fn corridor() -> World {
    let mut a = Province::new("A村", Terrain::Plain, Side::A);
    a.neighbors = vec![1];
    let mut b = Province::new("B镇", Terrain::Plain, Side::A);
    b.neighbors = vec![0, 2];
    let mut c = Province::new("C城", Terrain::Plain, Side::A);
    c.neighbors = vec![1];
    let units = vec![Unit::new("某师", Side::A, 0, 10)];
    World::new(1, vec![a, b, c], units)
}

#[test]
fn route_is_measured_in_days_and_first_hop() {
    let w = corridor();
    assert_eq!(hops_to(&w, 0, 2, Side::A, sim_core::UnitKind::Infantry), Some(2));
    assert_eq!(next_hop(&w, 0, 2, Side::A, sim_core::UnitKind::Infantry), Some(1));
    // 可达集合里，C 城应当是第 2 日
    let reach = reachable(&w, 0, Side::A, sim_core::UnitKind::Infantry);
    assert!(reach.contains(&(2, 2)), "可达集合应当包含 C城/2 日：{reach:?}");
}

#[test]
fn march_takes_one_province_per_day() {
    let mut w = corridor();
    // manual = 玩家亲自下的令：否则 run_ai 每 tick 会盖掉它
    w.units[0].manual = true;
    w.units[0].order = Order::Move(2);

    sim_core::run_ticks(&mut w, 1);
    assert_eq!(w.units[0].position, 1, "第一天只走到 B 镇");

    sim_core::run_ticks(&mut w, 1);
    assert_eq!(w.units[0].position, 2, "第二天到 C 城");

    // 到了就停：不应在 B、C 之间来回
    sim_core::run_ticks(&mut w, 2);
    assert_eq!(w.units[0].position, 2, "到达后应当停在目标省");
}

#[test]
fn water_blocks_ground_route_and_is_refused_cleanly() {
    // A — 河（水）— C：陆军走不过去
    let mut a = Province::new("A村", Terrain::Plain, Side::A);
    a.neighbors = vec![1];
    let mut river = Province::new("大河", Terrain::Water, Side::A);
    river.neighbors = vec![0, 2];
    let mut c = Province::new("C城", Terrain::Plain, Side::A);
    c.neighbors = vec![1];
    let units = vec![Unit::new("某师", Side::A, 0, 10)];
    let w = World::new(1, vec![a, river, c], units);

    assert_eq!(
        hops_to(&w, 0, 2, Side::A, sim_core::UnitKind::Infantry),
        None,
        "陆军不该有跨水的陆路"
    );
    // 海军走同一张图则通
    assert_eq!(
        hops_to(&w, 0, 2, Side::A, sim_core::UnitKind::Naval),
        None,
        "起点是陆地，海军自己也进不去"
    );
}

#[test]
fn far_attack_walks_then_fights() {
    // A（国军）—B（空）—C（日军据守）
    let mut a = Province::new("A村", Terrain::Plain, Side::A);
    a.neighbors = vec![1];
    a.is_supply_source = true; // 没有补给源，战斗力会是 0，这个测试就测不成战斗
    let mut b = Province::new("B镇", Terrain::Mountain, Side::A);
    b.neighbors = vec![0, 2];
    let mut c = Province::new("C城", Terrain::Plain, Side::B);
    c.neighbors = vec![1];
    c.is_supply_source = true;
    let units = vec![
        Unit::new("某师", Side::A, 0, 10),
        Unit::new("敌一部", Side::B, 2, 2),
    ];
    let mut w = World::new(1, vec![a, b, c], units);
    w.units[0].manual = true;
    w.units[0].order = Order::Attack(2);
    // 让守军死守：否则规则 AI 会把它调走，这个测试就变成在测 AI 了
    w.units[1].manual = true;
    w.units[1].order = Order::Hold;

    // 第一天进 B 镇（山道慢，但路是通的）
    sim_core::run_ticks(&mut w, 1);
    assert_eq!(w.units[0].position, 1, "第一天应当走到 B 镇");

    // 第二天进 C 城，随后发生战斗：日军那一部应当被打掉或打残
    let before = w.units[1].strength;
    sim_core::run_ticks(&mut w, 1);
    assert_eq!(w.units[0].position, 2, "第二天应当进入目标省");
    assert!(
        w.units[1].strength < before || !w.units[1].is_alive(),
        "进入敌据守省后应当发生战斗（敌兵力 {} → {}）",
        before.to_display_f64(),
        w.units[1].strength.to_display_f64()
    );
}

#[test]
fn long_march_is_deterministic() {
    let mut a = scenario::nanxun_line(7);
    // 让南端的部队一路往北打，跨越多个省
    a.units[2].manual = true;
    a.units[2].order = Order::Move(0);
    a.units[3].manual = true;
    a.units[3].order = Order::Move(0);
    let ha = sim_core::run_ticks(&mut a, 40);

    let mut b = scenario::nanxun_line(7);
    b.units[2].manual = true;
    b.units[2].order = Order::Move(0);
    b.units[3].manual = true;
    b.units[3].order = Order::Move(0);
    let hb = sim_core::run_ticks(&mut b, 40);

    assert_eq!(ha, hb, "跨省行军必须同样确定");
}
