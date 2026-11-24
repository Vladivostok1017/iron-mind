//! 空军与海军到底能干什么。
//!
//! 这不是"锦上添花"的测试，是因为玩家真的问过："我不明白这个空军水军
//! 现在能干什么"。答案必须是可验证的行为，不是文档里的一句描述。
//!
//! 四条：
//! 1. 空军每天只集中打**一个**省（以前半径内所有敌控省一起炸，一天顶三支师）；
//! 2. 有明确命令就打指定的那个省；
//! 3. 出击会折损飞机与机组（"飞机打一架少一架"不能是假话）；
//! 4. 舰队能沿水道移动，且舰炮支援真的加战斗力。

use sim_core::world::{Order, Province, Side, Terrain, Unit, UnitKind, World};

/// 一点在中间，三个省围着它：1 是国军地面部队，2、3 是日军。
fn star() -> World {
    let mut a = Province::new("基地", Terrain::Plain, Side::Cn);
    a.neighbors = vec![1, 2, 3];
    a.is_supply_source = true;
    let mut b = Province::new("我方阵地", Terrain::Plain, Side::Cn);
    b.neighbors = vec![0];
    b.is_supply_source = true;
    let mut c = Province::new("敌甲", Terrain::Plain, Side::Jp);
    c.neighbors = vec![0];
    c.is_supply_source = true;
    let mut d = Province::new("敌乙", Terrain::Plain, Side::Jp);
    d.neighbors = vec![0];
    d.is_supply_source = true;

    let mut air = Unit::new("某飞行团", Side::Cn, 0, 1);
    air.kind = UnitKind::Air;
    air.recon_radius = 2;
    air.strike_power = sim_core::Fixed::ratio(35, 100);

    let units = vec![
        air,
        Unit::new("我军", Side::Cn, 1, 10),
        Unit::new("敌甲队", Side::Jp, 2, 6),
        Unit::new("敌乙队", Side::Jp, 3, 6),
    ];
    let mut w = World::new(1, vec![a, b, c, d], units);
    // 地面部队全部钉住：这个测试测的是空军，不是规则 AI 的调度。
    // （不管住的话 AI 会把三支部队全调进 0 号省打成一锅粥，什么也测不出来。）
    for i in 1..4 {
        w.units[i].manual = true;
        w.units[i].order = Order::Hold;
    }
    w
}

#[test]
fn air_strikes_one_province_per_day() {
    let mut w = star();
    sim_core::run_ticks(&mut w, 1);

    let hit_a = (w.units[2].strength - w.units[2].max_strength).raw() < 0;
    let hit_b = (w.units[3].strength - w.units[3].max_strength).raw() < 0;
    assert!(
        hit_a ^ hit_b,
        "空军每天应当只集中打一个省（敌甲 {:?} / 敌乙 {:?}）",
        w.units[2].strength.to_display_f64(),
        w.units[3].strength.to_display_f64()
    );
}

#[test]
fn air_obeys_explicit_target() {
    let mut w = star();
    w.units[0].manual = true;
    w.units[0].order = Order::Attack(3);
    sim_core::run_ticks(&mut w, 1);

    assert!(
        w.units[3].strength.to_display_f64() < 6.0,
        "指定目标敌乙应当被打（实际 {}）",
        w.units[3].strength.to_display_f64()
    );
    assert_eq!(
        w.units[2].strength.to_display_f64(),
        6.0,
        "没有指定的敌甲不该被炸"
    );
}

#[test]
fn air_attrition_is_real() {
    let mut w = star();
    let before = w.units[0].strength;
    sim_core::run_ticks(&mut w, 10);
    assert!(
        w.units[0].strength < before,
        "出击十天，飞机与机组应当有折损（{} → {}）",
        before.to_display_f64(),
        w.units[0].strength.to_display_f64()
    );
    assert!(
        w.units[0].strength.to_display_f64() > 0.5,
        "十天不该把飞行团打光（实际 {}）",
        w.units[0].strength.to_display_f64()
    );
}

#[test]
fn air_reports_what_it_hit() {
    let mut w = star();
    sim_core::run_ticks(&mut w, 1);
    let events: Vec<&str> = w
        .log
        .iter()
        .map(|e| e.text.as_str())
        .filter(|t| t.contains("空袭"))
        .collect();
    assert_eq!(events.len(), 1, "每天应当只有一条空袭战报：{events:?}");
    assert!(events[0].contains("损失"), "战报要带上损失：{events:?}");
}

#[test]
fn fleet_moves_along_water_and_shells_the_shore() {
    // 岸上省 0 —— 水道 1 —— 水道 2
    let mut shore = Province::new("江岸", Terrain::Plain, Side::Cn);
    shore.neighbors = vec![1];
    let mut w1 = Province::new("长江·上", Terrain::Water, Side::Cn);
    w1.neighbors = vec![0, 2];
    let mut w2 = Province::new("长江·下", Terrain::Water, Side::Cn);
    w2.neighbors = vec![1];

    let mut ship = Unit::new("某战队", Side::Jp, 2, 3);
    ship.kind = UnitKind::Naval;

    let mut w = World::new(1, vec![shore, w1, w2], vec![ship]);
    // 舰队在上游时，江岸战斗拿不到支援
    assert_eq!(
        sim_core::systems::naval_support(&w, 0, Side::Jp).to_display_f64(),
        1.0
    );

    // 命令它开到下游邻接江岸的水道
    w.units[0].manual = true;
    w.units[0].order = Order::Move(1);
    sim_core::run_ticks(&mut w, 1);
    assert_eq!(w.units[0].position, 1, "舰队应当沿水道移动");

    let b = sim_core::systems::naval_support(&w, 0, Side::Jp).to_display_f64();
    assert!(b > 1.0, "到位后应当给江岸战斗加成（实际 ×{b}）");
}

