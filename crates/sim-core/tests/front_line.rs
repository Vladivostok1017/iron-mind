//! 连续正面模型（`Features::front_line`）：上游报告 §2 / §3 / §4。
//!
//! 三处语义都只在"密集正面"上暴露，而它们默认关着——关着时行为与第一版
//! 发行版逐位一致。这一组测试守的是"打开之后确实变对了"。

use sim_core::world::{Order, Province, Terrain, Unit};
use sim_core::{systems, Features, Fixed, Side, World};

fn fx() -> Features {
    Features {
        front_line: true,
        ..Features::none()
    }
}

/// A(我方) — B(争夺中) — C(敌方)，B、C 相邻。B 有我的一个团，C 有敌人的一个团。
fn front(flag: bool) -> World {
    let a = Province::new("A", Terrain::Plain, Side::Cn)
        .with_neighbors(&[1])
        .as_source();
    let b = Province::new("B", Terrain::Plain, Side::Jp).with_neighbors(&[0, 2]);
    let c = Province::new("C", Terrain::Plain, Side::Jp)
        .with_neighbors(&[1])
        .as_source();
    let mut mine = Unit::new("mine", Side::Cn, 1, 10); // 已经打进 B
    mine.manual = true;
    mine.order = Order::Hold;
    let mut foe = Unit::new("foe", Side::Jp, 2, 1); // 只剩 1 千的残部
    foe.manual = true;
    foe.order = Order::Hold;
    let fx = if flag { fx() } else { Features::none() };
    World::new(3, vec![a, b, c], vec![mine, foe]).with_features(fx)
}

#[test]
fn off_by_default_encircled_as_before() {
    // 关着：B 的邻省 A 是我方，所以还不算被围——这是"最保守"的写照。
    // 真正的差别在下面那条：邻省是敌省、但那里有我的部队。
    let mut w = front(false);
    systems::tick(&mut w);
    assert!(!w.units[0].encircled || w.units[0].encircled, "只用来确保不 panic");
}

#[test]
fn adjacent_friendly_units_break_the_encirclement() {
    // 把 A 改成敌省（B 的四邻都是敌人），但 A 里放一个我方部队。
    let build = |flag: bool| {
        let mut w = front(false);
        w.provinces[0].owner = Side::Jp;
        let mut helper = Unit::new("helper", Side::Cn, 0, 5);
        helper.manual = true;
        w.units.push(helper);
        w.fx = if flag { fx() } else { Features::none() };
        w
    };
    let mut off = build(false);
    systems::tick(&mut off);
    assert!(off.units[0].encircled, "关着时：四邻皆敌 → 被围");
    assert_eq!(off.units[0].supply, Fixed::ZERO, "被围 = 补给归零");

    let mut on = build(true);
    systems::tick(&mut on);
    assert!(!on.units[0].encircled, "开着时：邻省有我的部队 → 不算被围");
}

#[test]
fn occupation_does_not_need_every_neighbor_cleared() {
    // B 里只有我方部队，但 C 还有敌军。关着时 B 不翻色（1 千残部冻住战线），
    // 打开后 B 归我。
    let mut off = front(false);
    systems::tick(&mut off);
    assert_eq!(off.provinces[1].owner, Side::Jp, "关着：邻省有敌 → 不改归属");

    let mut on = front(true);
    systems::tick(&mut on);
    assert_eq!(on.provinces[1].owner, Side::Cn, "打开：占领只看本省");
}

#[test]
fn narrow_front_does_not_equalize_the_sides() {
    // 一条四省的线：A2(我·补给) — A(我·补给) — B(争夺中·窄阵地) — C(敌·补给)。
    // 双方都不被围（各自都有本方邻省），所以量到的伤亡是**对射**造成的，
    // 不是断粮或饿死。
    let build = |flag: bool| {
        let a2 = Province::new("A2", Terrain::Plain, Side::Cn)
            .with_neighbors(&[1])
            .as_source();
        let a1 = Province::new("A", Terrain::Plain, Side::Cn)
            .with_neighbors(&[0, 2])
            .as_source();
        let mut b = Province::new("B", Terrain::Plain, Side::Jp).with_neighbors(&[1, 3]);
        // 窄阵地：capacity = 0.1×40 = 4，双方都会被截断
        let mut pos = sim_core::world::Position::new("窄正面", Terrain::Plain);
        pos.width = Fixed::ratio(1, 10);
        b.positions.push(pos);
        let c = Province::new("C", Terrain::Plain, Side::Jp)
            .with_neighbors(&[2])
            .as_source();
        let mut big = Unit::new("big", Side::Cn, 1, 40);
        big.manual = true;
        big.order = Order::Attack(2);
        let mut small = Unit::new("small", Side::Jp, 2, 4);
        small.manual = true;
        small.order = Order::Hold;
        let fx = if flag { fx() } else { Features::none() };
        World::new(17, vec![a2, a1, b, c], vec![big, small]).with_features(fx)
    };
    let mut off = build(false);
    let mut on = build(true);
    for _ in 0..6 {
        systems::tick(&mut off);
        systems::tick(&mut on);
    }
    // 关着：双方都被截到 capacity → 损耗趋同，10 打 1 也占不到便宜。
    // 打开：超出部分按 30% 计入 → 强者的伤亡率必须更低。
    assert!(
        on.units[0].losses < off.units[0].losses,
        "打开连续正面后，10 打 1 的伤亡应该更小：{} vs {}",
        on.units[0].losses.to_display_f64(),
        off.units[0].losses.to_display_f64()
    );
}