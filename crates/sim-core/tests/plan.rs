//! 战区态势：**只报事实，不给判断**。
//!
//! 这组测试守的是一条边界：`theater_facts()` 只许说"敌我各多少兵、哪些敌方省
//! 够得着、那些省里有什么设施和守军"，**不许说"该推进/该守/该退"**。
//!
//! 为什么较这条劲：这段文字会进 LLM 将领的提示词。一旦它带着结论
//! （以前是 `posture: 推进` + "主攻方向 X、Y"），将领"抗命"时引用的就是
//! 那个结论 —— 判断实际上是函数做的，模型的角色退化成找理由。
//! 所以 `summary_contains_no_recommendation` 是这里最重要的一条。

use sim_core::ai::theater_facts;
use sim_core::world::{Province, Side, Terrain, Unit, World};

/// 0 我方(源) - 1 中间 - 2 敌方(源) - 3 敌后方(源, 铁路)
fn map() -> World {
    let mut a = Province::new("九江", Terrain::Plain, Side::A);
    a.is_supply_source = true;
    a.neighbors = vec![1];
    let mut b = Province::new("沙河", Terrain::Plain, Side::B);
    b.neighbors = vec![0, 2];
    let mut c = Province::new("德安", Terrain::Plain, Side::B);
    c.is_supply_source = true;
    c.rail = true;
    c.neighbors = vec![1, 3];
    let mut d = Province::new("南昌", Terrain::Plain, Side::B);
    d.is_supply_source = true;
    d.neighbors = vec![2];

    let units = vec![
        Unit::new("第9战区", Side::A, 0, 40),
        Unit::new("第106师团", Side::B, 1, 10),
    ];
    let mut w = World::new(1, vec![a, b, c, d], units);
    w.campaign_days = u32::MAX;
    // 情报表：给"沙河"一条估计（10 千），模拟"我们大致知道那边有多少人"。
    // `theater_facts` **只读情报表** —— 这正是迷雾模型的边界：
    // 部队真值再怎么变，只要情报没刷新，报出来的就该是旧估计。
    w.intel_mut(Side::A)[1].enemy_strength = sim_core::Fixed::from_int(10);
    w
}

#[test]
fn facts_are_deterministic() {
    let w = map();
    let a = theater_facts(&w, Side::A);
    let b = theater_facts(&w, Side::A);
    assert_eq!(a, b, "同一态势必须给出同一份事实");
}

#[test]
fn only_reachable_enemy_provinces_are_listed() {
    let w = map();
    let f = theater_facts(&w, Side::A);
    assert!(!f.targets.is_empty(), "应当有够得着的敌方省：{:?}", f.summary);
    // 够得着的只有"沙河"（与九江相邻）；德安、南昌都在后面
    assert_eq!(f.targets[0].province, 1, "只该列出相邻的沙河");
    assert_eq!(f.targets[0].name, "沙河");
    assert_eq!(f.targets.len(), 1, "德安、南昌够不着，不该进列表");
}

#[test]
fn reports_power_ratio_and_garrison_as_facts() {
    let w = map();
    let f = theater_facts(&w, Side::A);
    assert!((f.own_power - 40.0).abs() < 0.01, "我方 40 千：{}", f.own_power);
    assert!((f.foe_power - 10.0).abs() < 0.01, "敌方 10 千：{}", f.foe_power);
    assert!((f.ratio - 4.0).abs() < 0.01, "兵力比 4.00：{}", f.ratio);
    // 守军兵力是事实，得报出来 —— 将领要拿它算敢不敢打
    assert!(
        (f.targets[0].garrison - 10.0).abs() < 0.01,
        "沙河守军 10 千：{}",
        f.targets[0].garrison
    );
    assert!(f.summary.contains("守军约"), "{}", f.summary);
}

#[test]
fn facilities_are_listed_not_ranked_as_a_plan() {
    let mut w = map();
    // 让德安也够得着：它同时有补给源与铁路（两个设施）
    w.provinces[2].neighbors = vec![0, 1, 3];
    let f = theater_facts(&w, Side::A);
    let de_an = f
        .targets
        .iter()
        .find(|t| t.name == "德安")
        .expect("德安现在够得着");
    assert!(de_an.facilities.contains(&"补给源".to_string()));
    assert!(de_an.facilities.contains(&"铁路".to_string()));
    // 设施多的排在前面（按设施多寡排序），但没有任何"打哪个"的字样
    assert_eq!(f.targets[0].name, "德安", "设施多的排在前面");
}

#[test]
fn summary_contains_no_recommendation() {
    let w = map();
    let s = theater_facts(&w, Side::A).summary;
    for banned in ["推进", "巩固", "收缩", "建议", "主攻", "应当", "应该", "方针"] {
        assert!(
            !s.contains(banned),
            "态势里不许出现判断性措辞「{banned}」：{s}"
        );
    }
    // 该有的都得有：双方兵力、兵力比、够得着的省
    assert!(s.contains("战力比"), "{}", s);
    assert!(s.contains("沙河"), "{}", s);
}

#[test]
fn enemy_numbers_come_from_intel_not_truth() {
    // 迷雾边界：真值抬到 100 千，但情报表还停在 10 千 —— 报出来的必须是 10。
    let mut w = map();
    w.units[1].strength = sim_core::Fixed::from_int(100);
    w.units[1].max_strength = sim_core::Fixed::from_int(100);
    let f = theater_facts(&w, Side::A);
    assert!(
        (f.foe_power - 10.0).abs() < 0.01,
        "敌方总兵力只该用情报里的估计：{}",
        f.foe_power
    );
    assert!(
        (f.targets[0].garrison - 10.0).abs() < 0.01,
        "守军也只该用估计，不许读真值：{}",
        f.targets[0].garrison
    );
}

#[test]
fn without_intel_it_says_so() {
    let mut w = map();
    w.intel_mut(Side::A)[1].enemy_strength = sim_core::Fixed::ZERO;
    let f = theater_facts(&w, Side::A);
    assert!((f.foe_power - 0.0).abs() < 0.01);
    assert!(
        f.summary.contains("情报不足"),
        "没有情报就要说没有，不能编一个比值：{}",
        f.summary
    );
}

#[test]
fn no_reachable_enemy_reports_empty_targets() {
    let mut w = map();
    // 把所有敌方省都变成我方的
    for p in w.provinces.iter_mut() {
        p.owner = Side::A;
    }
    let f = theater_facts(&w, Side::A);
    assert!(f.targets.is_empty(), "没有够得着的敌方省");
    assert!(
        f.summary.contains("没有与我方地面部队相邻的敌方省份"),
        "{}",
        f.summary
    );
}
