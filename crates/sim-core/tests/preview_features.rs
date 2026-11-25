//! 机制开关的"毕业"测试（1.0）。
//!
//! 历史：0.12 时这几个机制是"模块显式声明才生效"的预览版，当时这一组测试
//! 守的是"默认全关，定稿版本一个字节都不变"。1.0 把它们并回主线，
//! 于是这一组改成守三件事：
//!
//! 1. **默认全开**——不写预置就是正式基线；
//! 2. **显式关掉仍能复现旧语义**（`Features::none()`），给需要对照的测试用；
//! 3. 状态哈希对开关敏感：开了就要进哈希，否则回放管不到它们。

use sim_core::scenario;
use sim_core::Features;

#[test]
fn mainline_default_has_everything_on() {
    let w = scenario::straight_line(7);
    assert!(w.fx.ammo_fuel && w.fx.rail_damage && w.fx.politics && w.fx.front_line);
    assert!(w.fx.disabled().is_empty(), "默认没有关掉的机制");
    assert_eq!(w.fx.names().len(), 4);
}

#[test]
fn legacy_baseline_is_still_expressible() {
    let w = scenario::straight_line(7).with_features(Features::none());
    assert!(!w.fx.any());
    assert_eq!(w.fx.disabled().len(), 4);
}

#[test]
fn same_input_same_hash() {
    let a = scenario::straight_line(7);
    let b = scenario::straight_line(7);
    assert_eq!(a.state_hash(), b.state_hash());
}

#[test]
fn switching_a_state_mechanism_changes_the_hash() {
    // 三个机制会引入新状态，各自必须进哈希——不然"回放一致"就管不到它们。
    let base = scenario::straight_line(7).with_features(Features::none());
    let cases = [
        Features {
            ammo_fuel: true,
            ..Features::none()
        },
        Features {
            rail_damage: true,
            ..Features::none()
        },
        Features {
            politics: true,
            ..Features::none()
        },
    ];
    for fx in cases {
        let other = scenario::straight_line(7).with_features(fx);
        assert_ne!(base.state_hash(), other.state_hash());
    }
}

#[test]
fn front_line_is_a_rule_switch_not_state() {
    // `front_line` 不引入新状态，它只改"怎么算"（围歼/占领/展开宽度）。
    // 所以初始世界哈希一样——它的影响通过位置、兵力、归属这些既有状态体现，
    // 那些本来就在哈希里。这条测试把这个事实钉住，免得以后误以为它漏进哈希了。
    let a = scenario::straight_line(7).with_features(Features::none());
    let b = scenario::straight_line(7).with_features(Features {
        front_line: true,
        ..Features::none()
    });
    assert_eq!(a.state_hash(), b.state_hash());
}

#[test]
fn feature_names_are_stable() {
    let fx = Features::default();
    assert_eq!(
        fx.names(),
        vec!["ammo_fuel", "rail_damage", "politics", "front_line"]
    );
    assert_eq!(
        Features::none().names(),
        Vec::<&str>::new(),
        "全关时没有能力名（界面据此不挂角标）"
    );
}
