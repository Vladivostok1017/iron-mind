//! 亲自指挥（`CommandMode::Direct`）、投入率（`Unit::committed`）、残部并编。
//!
//! 这一组守的是那次尺度反思的结论：在战役尺度上 `(动词, 省份)` 已经是一次
//! 完整的决定，所以将领不再决定打不打、打哪儿——他只决定摆多少兵上去
//! （投入率）和怎么把战况报上来。命令是玩家的。

use sim_core::ai::issue_order;
use sim_core::world::{CommandMode, Order, Province, Terrain, Unit, COMMIT_MIN};
use sim_core::{systems, Features, Fixed, Side, World};

/// A(我方) 邻 B(敌方)，A 里放一个满编师。
fn stage(mode: CommandMode) -> World {
    let a = Province::new("A", Terrain::Plain, Side::A).with_neighbors(&[1]);
    let b = Province::new("B", Terrain::Plain, Side::B)
        .with_neighbors(&[0])
        .as_source();
    let u = Unit::new("师", Side::A, 0, 10);
    let mut w = World::new(7, vec![a, b], vec![u]).with_features(Features::none());
    w.command_mode = mode;
    w.player_side = Some(Side::A);
    w
}

#[test]
fn direct_mode_waits_for_orders_and_delegated_mode_does_not() {
    // 亲自指挥：没下令就不动。战线静止是**玩家的事**，不是 AI 的。
    let mut d = stage(CommandMode::Direct);
    systems::tick(&mut d);
    assert_eq!(d.units[0].position, 0, "没人下令，部队不该自己走进敌省");

    // 委诸将领：同样的一天，将领会自己找仗打。
    let mut g = stage(CommandMode::Delegated);
    systems::tick(&mut g);
    assert_eq!(
        g.units[0].position, 1,
        "委诸将领下，邻接敌省应当被 AI 推过去（这是对照，不是回归）"
    );
}

#[test]
fn player_order_is_obeyed_immediately() {
    let mut w = stage(CommandMode::Direct);
    issue_order(&mut w, 0, Order::Attack(1), None).expect("我方部队、相邻敌省，命令应当成立");
    assert_eq!(w.units[0].order, Order::Attack(1));
    assert!(w.units[0].manual, "直令要标成玩家亲自下的，AI 不许覆盖");
    assert!(!w.units[0].disobeyed, "亲自指挥下不存在抗命");

    systems::tick(&mut w);
    assert_eq!(w.units[0].position, 1, "命令下了就要动");
}

#[test]
fn wrong_side_cannot_be_commanded() {
    let mut w = stage(CommandMode::Direct);
    w.player_side = Some(Side::B);
    assert!(issue_order(&mut w, 0, Order::Hold, None).is_err());
}

#[test]
fn commitment_is_clamped_at_the_floor() {
    let mut w = stage(CommandMode::Direct);
    // 想留七成预备队？不许。"派三个连去意思一下"不是谨慎，是不打。
    issue_order(&mut w, 0, Order::Hold, Some(Fixed::ratio(1, 10))).unwrap();
    assert_eq!(w.units[0].committed, COMMIT_MIN);
}

/// 同一个省里对打，只改我方的投入率。
fn battle(commit: Fixed) -> World {
    let a = Province::new("A", Terrain::Plain, Side::A).with_neighbors(&[]);
    let mut mine = Unit::new("我", Side::A, 0, 10);
    let mut foe = Unit::new("敌", Side::B, 0, 10);
    mine.manual = true;
    foe.manual = true;
    mine.order = Order::Fortify;
    foe.order = Order::Fortify;
    mine.set_commit(commit);
    let mut w = World::new(11, vec![a], vec![mine, foe]).with_features(Features::none());
    w.command_mode = CommandMode::Direct;
    w.player_side = Some(Side::A);
    w
}

#[test]
fn commitment_trades_tempo_for_blood() {
    let mut hi = battle(Fixed::ONE);
    let mut lo = battle(Fixed::ratio(4, 10));
    systems::tick(&mut hi);
    systems::tick(&mut lo);

    let hi_loss = (hi.units[0].losses + hi.units[1].losses).to_display_f64();
    let lo_loss = (lo.units[0].losses + lo.units[1].losses).to_display_f64();
    assert!(
        lo_loss < hi_loss,
        "少摆兵上去就该少流血：全压 {hi_loss:.3} 千 vs 四成 {lo_loss:.3} 千"
    );

    // 代价在另一头：打得少，敌人也就被打得少。
    let hi_foe_lost = hi.units[1].losses.to_display_f64();
    let lo_foe_lost = lo.units[1].losses.to_display_f64();
    assert!(
        lo_foe_lost < hi_foe_lost,
        "少投入不能白占便宜：给敌人的伤亡也该少 {lo_foe_lost:.3} < {hi_foe_lost:.3}"
    );
}

/// 同省两支我方部队：一支只剩 2 千（满编 10 千），一支 8 千。
fn merge_stage() -> World {
    let a = Province::new("A", Terrain::Plain, Side::A).with_neighbors(&[]);
    let mut remnant = Unit::new("残部", Side::A, 0, 2);
    remnant.max_strength = Fixed::from_int(10);
    let mut full = Unit::new("主力", Side::A, 0, 8);
    full.max_strength = Fixed::from_int(10);
    let mut w = World::new(13, vec![a], vec![remnant, full]).with_features(Features::none());
    w.command_mode = CommandMode::Direct;
    w.player_side = Some(Side::A);
    w
}

#[test]
fn remnant_can_be_folded_into_a_friend() {
    let mut w = merge_stage();
    issue_order(&mut w, 0, Order::Merge(1), None).expect("同省友军，应当可以并编");
    systems::tick(&mut w);

    assert!(!w.units[0].is_alive(), "番号应当撤销");
    assert!(w.units[0].ended_day.is_some(), "撤销日要记下来，回放才对得上");
    assert!(
        w.units[0].reform_day.is_none(),
        "番号划掉了就没有二次整编——人还在，编制没了"
    );
    assert!(
        w.units[1].strength.to_display_f64() > 8.0,
        "主力应当补进残部的兵：{}",
        w.units[1].strength.to_display_f64()
    );
}

#[test]
fn healthy_unit_refuses_to_be_folded() {
    let mut w = merge_stage();
    // 主力还是八成编制，不许拆：并编是"打残了"的出路，不是洗编制的捷径。
    assert!(
        issue_order(&mut w, 1, Order::Merge(0), None).is_err(),
        "引擎这一层就该拒绝，不能静默什么都不发生"
    );
    systems::tick(&mut w);
    assert!(w.units[1].is_alive(), "还剩八成编制的部队不该被自己拆掉");
}
