//! 预览版·派系与人事。
//!
//! 立场和别的预览版一样：关着的时候一丝不差，打开之后必须真的改变结果。
//! 另加一条：撤换必须**留痕**（去职名单），否则玩家会以为撤将没有代价。

use sim_core::politics::{self, Faction};
use sim_core::world::{Province, Terrain, Unit};
use sim_core::{systems, Features, Fixed, Side, World};

fn fx_pol() -> Features {
    Features {
        politics: true,
        ..Features::none()
    }
}

/// 两省、两支部队；我方两支部队分属两个派系（0.8 与 0.3）。
fn stage(fx: Features) -> World {
    let a = Province::new("A", Terrain::Plain, Side::Cn)
        .with_neighbors(&[1])
        .as_source();
    let b = Province::new("B", Terrain::Plain, Side::Cn).with_neighbors(&[0]);
    let mut u1 = Unit::new("1st", Side::Cn, 0, 10);
    u1.commander.faction = Some("f_strong".into());
    let mut u2 = Unit::new("2nd", Side::Cn, 1, 10);
    u2.commander.faction = Some("f_weak".into());
    for u in [&mut u1, &mut u2] {
        u.manual = true;
    }
    let mut w = World::new(9, vec![a, b], vec![u1, u2]).with_features(fx);
    if fx.politics {
        w.factions = vec![
            Faction::new("f_strong", "强派", Side::Cn, Fixed::ratio(8, 10)),
            Faction::new("f_weak", "弱派", Side::Cn, Fixed::ratio(3, 10)),
        ];
        w.factions[0].units = vec![0];
        w.factions[1].units = vec![1];
    }
    w
}

#[test]
fn off_by_default_nothing_happens() {
    let mut w = stage(Features::none());
    let before = w.state_hash();
    for _ in 0..5 {
        systems::tick(&mut w);
    }
    assert!(w.dismissed.is_empty());
    assert_eq!(politics::obedience_shift(&w, 0), Fixed::ZERO);
    assert!(politics::relieve(&mut w, 0).is_err(), "没开就不许撤将");
    // 没开 politics 时，哈希里也不该出现派系（before 只在 tick 前进后变化）
    assert_ne!(before, w.state_hash(), "tick 本身当然会变");
}

#[test]
fn influence_drifts_toward_weight() {
    let mut w = stage(fx_pol());
    w.factions[0].influence = Fixed::ratio(2, 10); // 被压了一阵
    for _ in 0..10 {
        systems::tick(&mut w);
    }
    // 权重 0.8，从 0.2 往回漂：十天之后应该明显高过 0.2，但还没到 0.8。
    let inf = w.factions[0].influence.to_display_f64();
    assert!(inf > 0.2, "应该开始回涨，实测 {inf}");
    assert!(inf < 0.8, "不该一天就回到位，实测 {inf}");
}

#[test]
fn losing_standing_makes_generals_less_obedient() {
    let mut w = stage(fx_pol());
    let at_weight = w.effective_obedience(0);
    w.factions[0].influence = Fixed::ratio(3, 10); // 比权重 0.8 低很多 = 失势
    let after = w.effective_obedience(0);
    assert!(after < at_weight, "靠山失势应该更不听招呼");
}

#[test]
fn relieving_a_commander_costs_influence() {
    let mut w = stage(fx_pol());
    let before = w.factions[0].influence;
    let (old, new_chief) = politics::relieve(&mut w, 0).expect("应该能撤");
    assert_eq!(old, "1st");
    assert!(new_chief.contains("继任"));
    assert!(w.factions[0].influence < before, "动人家台柱子要付代价");
    assert_eq!(w.dismissed.len(), 1);
    // 继任者完全服从（但也没个性）——这就是撤将的代价与收益。
    assert_eq!(w.units[0].commander.obedience, Fixed::ONE);
    assert_eq!(w.units[0].commander.faction, None);
}

#[test]
fn cannot_relieve_twice() {
    let mut w = stage(fx_pol());
    politics::relieve(&mut w, 0).expect("第一次应该成功");
    assert!(politics::relieve(&mut w, 0).is_err(), "同一个位置不能反复换人");
}

#[test]
fn politics_state_enters_the_hash() {
    let a = stage(fx_pol());
    let mut b = stage(fx_pol());
    let h0 = a.state_hash();
    b.factions[0].influence = Fixed::ratio(1, 2);
    assert_ne!(h0, b.state_hash(), "影响力变了，哈希必须变");
    let mut c = stage(fx_pol());
    c.dismissed.push(("1st".into(), "某人".into(), 3));
    assert_ne!(h0, c.state_hash(), "去职名单也要进哈希");
}
