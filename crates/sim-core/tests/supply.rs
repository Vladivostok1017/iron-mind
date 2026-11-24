//! 补给：能不能被切断、断了会不会死人、战报要不要走邮路。
//!
//! 三件事以前都不成立：
//! 1. 补给只认**归属**，敌人一个联队蹲在你的公路上也不影响运输；
//! 2. 补给 0 的部队"打不了"但**永远不减员**，可以蹲在包围圈里待到战役结束；
//! 3. 所有战报即时进日志，等于长官部在全图每个连部都架了电台。

use sim_core::world::{Commander, Order, Province, Side, Terrain, Unit, World};
use sim_core::Fixed;

fn line() -> World {
    // 0（补给源）— 1 — 2，都是国军的
    let mut a = Province::new("后方", Terrain::Plain, Side::Cn);
    a.neighbors = vec![1];
    a.is_supply_source = true;
    let mut b = Province::new("中转", Terrain::Plain, Side::Cn);
    b.neighbors = vec![0, 2];
    let mut c = Province::new("前线", Terrain::Plain, Side::Cn);
    c.neighbors = vec![1];

    let mut u = Unit::new("前线师", Side::Cn, 2, 8);
    u.supply_reach = Fixed::ZERO;
    u.manual = true;
    u.order = Order::Hold;
    let mut w = World::new(1, vec![a, b, c], vec![u]);
    w.comms.transmission = 0;
    w.comms.staff = 0;
    w
}

#[test]
fn enemy_on_the_road_cuts_supply() {
    let mut w = line();
    sim_core::run_ticks(&mut w, 1);
    let clean = w.units[0].supply.to_display_f64();
    assert!(clean > 0.4, "干净补给线上的前线应当拿到补给：{clean}");

    // 敌人一个纵队插到中转省（省还是我的，但路上有人）
    let mut raider = Unit::new("敌挺进队", Side::Jp, 1, 2);
    raider.manual = true;
    raider.order = Order::Hold;
    w.units.push(raider);
    sim_core::run_ticks(&mut w, 1);
    let cut = w.units[0].supply.to_display_f64();
    assert!(
        cut < clean * 0.5,
        "敌纵队踩在补给线上，运力应当腰斩（{clean} → {cut}）"
    );
}

#[test]
fn starvation_kills_and_reports() {
    // 补给源在，但这支部队**接不上**：孤城和省会之间没有邻接。
    // 这是"断粮"的正确定义——不是"离得远"（那只是缺粮，有征发地板兜着），
    // 而是"在本国领土上却与补给网断开"。
    let mut p = Province::new("孤城", Terrain::Plain, Side::Cn);
    p.neighbors = vec![];
    let mut home = Province::new("省会", Terrain::Plain, Side::Cn);
    home.is_supply_source = true; // 有源，但走不到
    let mut u = Unit::new("孤军", Side::Cn, 0, 8);
    u.max_strength = Fixed::from_int(8);
    u.supply_reach = Fixed::ZERO;
    u.manual = true;
    u.order = Order::Hold;
    let mut w = World::new(1, vec![p, home], vec![u]);
    w.campaign_days = u32::MAX;
    w.comms.transmission = 0;
    w.comms.staff = 0;

    sim_core::run_ticks(&mut w, 10);
    let s = w.units[0].strength.to_display_f64();
    assert!(s < 8.0, "断粮十天应当掉兵（8.0 → {s}）");
    assert!(s > 7.0, "十天不该把一支部队饿光（{s}）");
    let starve: Vec<&str> = w
        .log
        .iter()
        .map(|e| e.text.as_str())
        .filter(|t| t.contains("断粮"))
        .collect();
    assert!(!starve.is_empty(), "断粮要报给本方（实际日志：{:?}）", w.log.len());
    // 断粮是内部消息
    assert!(w.log.iter().any(|e| e.side == Some(Side::Cn) && e.text.contains("断粮")));
}

#[test]
fn small_remnant_is_not_wiped_but_a_skeleton_is() {
    let mut p = Province::new("战场", Terrain::Plain, Side::Cn);
    p.is_supply_source = true;
    let mut w = World::new(1, vec![p], vec![]);

    let mut remnant = Unit::new("残部", Side::Cn, 0, 1);
    remnant.strength = Fixed::from_f64_input(0.9); // 900 人：还是师
    let mut skeleton = Unit::new("空架子", Side::Cn, 0, 1);
    skeleton.strength = Fixed::from_f64_input(0.1); // 100 人：番号保不住
    w.units.push(remnant);
    w.units.push(skeleton);

    sim_core::run_ticks(&mut w, 1);
    assert!(w.units[0].ended_day.is_none(), "900 人的残部仍然是师");
    assert!(w.units[1].ended_day.is_some(), "100 人的空架子应当判覆灭");
}

#[test]
fn reports_travel_at_courier_speed() {
    let mut w = line();
    w.comms.transmission = 1;
    w.comms.staff = 1; // 两天
    w.report_delayed(2, sim_core::world::EventKind::Oob, "前线师 全军覆没", Some(Side::Cn));
    assert!(w.log.is_empty(), "刚发出的电报不该已经躺在日志里");

    sim_core::run_ticks(&mut w, 1);
    assert!(w.log.is_empty(), "第一天还没到");

    sim_core::run_ticks(&mut w, 1);
    assert_eq!(w.log.len(), 1, "第二天应当收到");
    assert_eq!(w.in_transit.len(), 0);
    assert_eq!(w.log[0].tick, 0, "事件日期记的是发生日（第 0 日），不是送达日");
}

#[test]
fn commander_default_has_no_fate_until_it_ends() {
    let c = Commander::default_for("某人");
    assert_eq!(c.fate, sim_core::world::Fate::Active);
    assert!(c.fate.can_lead());
}

#[test]
fn encirclement_zeroes_supply_even_on_own_soil() {
    // 0（我方，守着）— 1（敌方）— 2（我方后方）
    let mut a = Province::new("被围点", Terrain::Plain, Side::Cn);
    a.neighbors = vec![1, 2];
    a.is_supply_source = true;
    let mut b = Province::new("敌阵地", Terrain::Plain, Side::Jp);
    b.neighbors = vec![0];
    let mut c = Province::new("后方", Terrain::Plain, Side::Cn);
    c.neighbors = vec![0];
    c.is_supply_source = true;

    let mut u = Unit::new("守军", Side::Cn, 0, 6);
    u.manual = true;
    u.order = Order::Hold;
    let mut w = World::new(1, vec![a, b, c], vec![u]);
    w.campaign_days = u32::MAX;

    sim_core::run_ticks(&mut w, 1);
    let before = w.units[0].supply.to_display_f64();
    assert!(before > 0.5, "后方连通时补给正常（{before}）");
    assert!(!w.units[0].encircled);

    // 后方也丢了 → 四面皆敌
    w.provinces[2].owner = Side::Jp;
    sim_core::run_ticks(&mut w, 1);
    assert!(w.units[0].encircled, "所有邻省都不是自己的，应当判定被围");
    assert_eq!(w.units[0].supply.to_display_f64(), 0.0, "被围补给归零");
}

#[test]
fn encirclement_is_reported_once() {
    let mut a = Province::new("被围点", Terrain::Plain, Side::Cn);
    a.neighbors = vec![1];
    let mut b = Province::new("敌阵地", Terrain::Plain, Side::Jp);
    b.neighbors = vec![0];
    let mut u = Unit::new("守军", Side::Cn, 0, 6);
    u.manual = true;
    u.order = Order::Hold;
    let mut w = World::new(1, vec![a, b], vec![u]);
    w.campaign_days = u32::MAX;
    w.comms.transmission = 0;
    w.comms.staff = 0;

    sim_core::run_ticks(&mut w, 5);
    let n = w.log.iter().filter(|e| e.text.contains("被合围")).count();
    assert_eq!(n, 1, "只报一次（不是每天一条）：{}", n);
}