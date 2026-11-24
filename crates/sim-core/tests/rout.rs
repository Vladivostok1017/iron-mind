//! 溃退与围歼。
//!
//! 这条是"伤亡不对称"的来源。以前伤亡只是双方火力对冲的结果，
//! 谁火力占比高谁掉得少——于是双方伤亡必然接近对称。
//! 而武汉会战的实际形状是中方伤亡约为日军的 3–4 倍：差距不在对射，
//! 而在**崩了之后**——丢装备、掉队、被追、被围歼。

use sim_core::world::{Order, Province, Side, Terrain, Unit, World};
use sim_core::Fixed;

/// 崩了的一方 + 当面之敌 + 一条退路
fn routable(retreat_available: bool) -> World {
    // 0 前线（国军守，已崩）— 1 后方（国军空省，可选作退路）— 2 敌占
    let mut front = Province::new("前线", Terrain::Plain, Side::Cn);
    front.is_supply_source = true;
    front.neighbors = if retreat_available { vec![1, 2] } else { vec![2] };
    let mut rear = Province::new("后方", Terrain::Plain, Side::Cn);
    rear.neighbors = vec![0];
    let mut foe_prov = Province::new("敌阵地", Terrain::Plain, Side::Jp);
    foe_prov.is_supply_source = true;
    foe_prov.neighbors = vec![0];

    let mut broken = Unit::new("第187师", Side::Cn, 0, 6);
    broken.morale = Fixed::ratio(2, 10); // 已经崩了
    broken.manual = true;
    broken.order = Order::Hold;

    let mut foe = Unit::new("第106师团", Side::Jp, 0, 12);
    foe.manual = true;
    foe.order = Order::Hold;

    let mut w = World::new(1, vec![front, rear, foe_prov], vec![broken, foe]);
    if !retreat_available {
        // 唯一的邻省上驻着敌人 —— 这才是"四面皆敌"，不是"背后没自己的旗子"
        let mut blocker = Unit::new("敌守备队", Side::Jp, 2, 4);
        blocker.manual = true;
        blocker.order = Order::Hold;
        w.units.push(blocker);
    }
    w.campaign_days = u32::MAX;
    w.comms.transmission = 0;
    w.comms.staff = 0;
    w
}

#[test]
fn broken_unit_retreats_and_loses_men() {
    let mut w = routable(true);
    let before = w.units[0].strength.to_display_f64();
    sim_core::run_ticks(&mut w, 1);
    let after = w.units[0].strength.to_display_f64();
    assert_eq!(w.units[0].position, 1, "崩了应当退向后方空省");
    assert!(
        after < before * 0.95,
        "溃退要丢掉辎重与掉队的人（{before} → {after}）"
    );
    assert!(
        w.log.iter().any(|e| e.text.contains("溃退")),
        "溃退要有战报：{:?}",
        w.log.iter().map(|e| &e.text).collect::<Vec<_>>()
    );
    assert!(!w.units[0].manual, "溃退之后该交还 AI，不该还听玩家");
}

#[test]
fn no_retreat_means_annihilation() {
    let mut w = routable(false);
    sim_core::run_ticks(&mut w, 1);
    assert!(
        w.units[0].ended_day.is_some(),
        "无路可退就是被围歼（实际存活 {}）",
        w.units[0].in_being()
    );
    assert!(w.units[0].strength.is_zero());
    assert!(
        w.log.iter().any(|e| e.text.contains("被围歼")),
        "围歼要有战报"
    );
    // 将领结局照常结算（这是"殉国"最典型的场合）
    assert_ne!(
        w.units[0].commander.fate,
        sim_core::world::Fate::Active,
        "被围歼时将领不会仍在任"
    );
}

#[test]
fn low_morale_without_contact_is_not_a_rout() {
    let mut w = routable(true);
    // 把敌人挪走：没有当面之敌，士气低也只是低
    w.units[1].position = 2;
    sim_core::run_ticks(&mut w, 1);
    assert_eq!(w.units[0].position, 0, "没交火就不该自己跑");
    assert!(!w.log.iter().any(|e| e.text.contains("溃退")));
}

#[test]
fn rout_is_deterministic() {
    let mut a = routable(true);
    let mut b = routable(true);
    let ha = sim_core::run_ticks(&mut a, 20);
    let hb = sim_core::run_ticks(&mut b, 20);
    assert_eq!(ha, hb, "溃退必须同样确定");
}
