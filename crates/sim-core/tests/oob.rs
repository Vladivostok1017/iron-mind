//! 编制改变：全军覆没、将领殉国、二次整编。
//!
//! 这三件事以前是没人管的：兵打光就把 `strength` 置零，番号、将领、命令
//! 却原样留着——AI 会继续去问一个已经不存在的军长"你要怎么打"，
//! 而玩家看到的只是一行"被全歼"，然后那支部队永远躺在列表里 0.0 千。
//!
//! 现在：覆灭是一个**状态**（不是删除，因为命令流按单位下标引用部队），
//! 将领有结局（殉国/被俘/去职/突围），而二次整编要人活着、要后方有补给源。

use sim_core::world::{Fate, Order, Province, Side, Terrain, Unit, World};
use sim_core::Fixed;

/// 一个弱师（0.4 千）和一个强师（20 千）挤在同一个省。
/// 第一 tick 战斗结算后弱师就会被打到 1 千以下 → 判定覆灭。
fn doomed(reform_days: u32, reform_ratio: f64) -> World {
    let mut p = Province::new("战场", Terrain::Plain, Side::Cn);
    p.is_supply_source = true;
    let mut home = Province::new("后方", Terrain::Plain, Side::Cn);
    home.is_supply_source = true;
    home.neighbors = vec![0];

    let mut weak = Unit::new("第187师", Side::Cn, 0, 1);
    weak.strength = Fixed::from_f64_input(0.4);
    weak.max_strength = Fixed::from_int(6);
    weak.reform_days = reform_days;
    weak.reform_ratio = Fixed::from_f64_input(reform_ratio);
    weak.commander = sim_core::world::Commander::new("孔可权", 0.8, 0.5, 0.4, 0.6);

    let strong = Unit::new("第106师团", Side::Jp, 0, 20);
    let mut w = World::new(7, vec![p, home], vec![weak, strong]);
    w.units[1].manual = true;
    w.units[1].order = Order::Hold;
    w
}

#[test]
fn destroyed_unit_is_marked_not_deleted() {
    let mut w = doomed(0, 0.0);
    sim_core::run_ticks(&mut w, 1);

    assert_eq!(w.units.len(), 2, "单位不能被从 Vec 里删掉：下标就是命令流的坐标");
    assert!(w.units[0].ended_day.is_some(), "应当记下覆灭日");
    assert!(!w.units[0].in_being());
    assert_eq!(w.units[0].order, Order::Hold, "死掉的部队不该还挂着进攻命令");
    assert!(!w.units[0].manual, "也不该继续锁在玩家手里");
}

#[test]
fn commander_has_a_fate_and_it_affects_the_unit() {
    let mut w = doomed(0, 0.0);
    sim_core::run_ticks(&mut w, 1);
    let f = w.units[0].commander.fate;
    assert_ne!(f, Fate::Active, "被全歼时将领不该仍然在任（测试前提：本种子未生还）");
    assert!(matches!(f, Fate::Killed | Fate::Captured | Fate::Relieved));
}

#[test]
fn fate_is_deterministic() {
    let mut a = doomed(0, 0.0);
    let mut b = doomed(0, 0.0);
    sim_core::run_ticks(&mut a, 30);
    sim_core::run_ticks(&mut b, 30);
    assert_eq!(
        a.units[0].commander.fate.code(),
        b.units[0].commander.fate.code(),
        "同一种子，将领结局必须一致"
    );
    assert_eq!(a.state_hash(), b.state_hash());
}

#[test]
fn reform_needs_a_living_commander_and_a_supply_source() {
    // 扫一批种子，找出将领突围生还的那一种，验证整编真的会发生。
    let mut reformed = 0;
    let mut scheduled = 0;
    for seed in 1..=40u64 {
        let mut w = doomed(3, 0.5);
        w.seed = seed;
        sim_core::run_ticks(&mut w, 1);
        let fate = w.units[0].commander.fate;
        let day = w.units[0].reform_day;
        if fate.can_lead() {
            assert!(day.is_some(), "将领生还、且模块声明了整编，就应当排上日子");
            scheduled += 1;
            // 等到整编那天：3 天 + 后方补给源 = 重新成军
            sim_core::run_ticks(&mut w, 5);
            if !w.units[0].ended_day.is_some() {
                continue;
            }
        } else {
            assert!(day.is_none(), "人没了就不该排整编（没干部，番号拉不起来）");
        }
    }
    for seed in 1..=40u64 {
        let mut w = doomed(3, 0.5);
        w.seed = seed;
        sim_core::run_ticks(&mut w, 8);
        if w.units[0].in_being() && w.units[0].ended_day.is_none() {
            reformed += 1;
        }
    }
    assert!(scheduled > 0, "40 个种子里应当有将领生还");
    assert!(reformed > 0, "生还的应当真的整编回来了");
}

#[test]
fn no_reform_without_module_declaration() {
    let mut w = doomed(0, 0.0);
    sim_core::run_ticks(&mut w, 10);
    assert!(w.units[0].reform_day.is_none(), "没声明整编的部队不该自己复活");
    assert!(!w.units[0].in_being());
}

#[test]
fn oob_events_are_side_tagged() {
    let mut w = doomed(0, 0.0);
    sim_core::run_ticks(&mut w, 1);
    let vis = |side: Side| -> Vec<String> {
        w.log
            .iter()
            .filter(|e| e.side.is_none() || e.side == Some(side))
            .map(|e| e.text.clone())
            .collect()
    };
    let cn = vis(Side::Cn);
    // 将领的生死是内部消息：本方要知道，敌人不该知道
    assert!(
        cn.iter().any(|t| t.contains("全军覆没")),
        "本方应当收到覆灭与将领结局：{cn:?}"
    );
    let jp = vis(Side::Jp);
    assert!(
        !jp.iter().any(|t| t.contains("殉国") || t.contains("被俘")),
        "敌方不该知道我军团长的死活：{jp:?}"
    );
}
