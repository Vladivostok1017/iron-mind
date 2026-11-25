//! 预览版开关的闸门测试。
//!
//! 这一组测试守的不是"新功能好不好玩"，而是**定稿的第一版发行版有没有被改动**：
//!
//! 1. 默认全关——不声明就一定是发行版行为；
//! 2. 关着的时候，世界哈希与"根本没有这些字段"时逐位一致（协议字节流不变）；
//! 3. 打开任意一个功能，哈希流就必须变（否则它等于没进状态，回放校验也抓不到它）。

use sim_core::scenario;
use sim_core::Features;

#[test]
fn features_are_off_by_default() {
    let w = scenario::straight_line(7);
    assert!(!w.fx.any(), "不声明就必须是定稿行为");
    assert!(w.fx.names().is_empty());
}

#[test]
fn legacy_hash_stream_is_untouched() {
    // 同一场景跑 10 天：开不开"新字段的存在"都不影响字节流。
    // 这里比的是"两把一模一样的默认世界"，真正的回归由 golden_replay.rs 守
    // （那里钉死了具体数值）。这条只是提醒：默认世界里不许夹带新状态。
    let a = scenario::straight_line(7);
    let b = scenario::straight_line(7);
    assert_eq!(a.state_hash(), b.state_hash());
    assert_eq!(a.state_hash(), {
        let w = scenario::straight_line(7).with_features(Features::default());
        w.state_hash()
    });
}

#[test]
fn turning_a_feature_on_changes_the_hash() {
    let base = scenario::straight_line(7);
    let ammo = scenario::straight_line(7).with_features(Features {
        ammo_fuel: true,
        ..Default::default()
    });
    let rail = scenario::straight_line(7).with_features(Features {
        rail_damage: true,
        ..Default::default()
    });
    let pol = scenario::straight_line(7).with_features(Features {
        politics: true,
        ..Default::default()
    });
    // 三个功能各自把新状态写进哈希——不然"回放一致"就管不到它们。
    assert_ne!(base.state_hash(), ammo.state_hash());
    assert_ne!(base.state_hash(), rail.state_hash());
    assert_ne!(base.state_hash(), pol.state_hash());
}

#[test]
fn feature_names_are_stable() {
    // 界面与 SDK 都按这些名字判断"引擎支持什么"，顺序也是契约。
    let fx = Features {
        ammo_fuel: true,
        rail_damage: true,
        politics: true,
        front_line: true,
    };
    assert_eq!(fx.names(), vec!["ammo_fuel", "rail_damage", "politics", "front_line"]);
}
