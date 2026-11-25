//! 尺度决定默认模型（1.0 的裁决）。
//!
//! 同样一条"邻接有敌不准占领"：
//! - 在 35 km/省 的战役图上是对的（防穿插、防渗透，一个省是一个"区"）；
//! - 在 10 km/省 的正面图上会把战线冻住（一个省是一个"阵地"，四邻皆敌是常态）。
//!
//! 所以 `front_line` 的默认值**跟着 scale 走**，而不是一刀切。

use sim_content::model::ScaleDef;
use sim_content::{features_from, model::PreviewDef};

fn fx(scale: ScaleDef, def: Option<PreviewDef>) -> sim_core::Features {
    features_from(def.as_ref(), scale)
}

#[test]
fn theater_and_naval_default_to_the_coarse_rule() {
    for scale in [ScaleDef::Theater, ScaleDef::Naval] {
        let f = fx(scale, None);
        assert!(!f.front_line, "{} 应该有侧翼概念，默认用粗口径", scale.as_str());
        // 其余三项对所有尺度都默认开
        assert!(f.ammo_fuel && f.rail_damage && f.politics);
    }
}

#[test]
fn dense_scales_default_to_the_front_line_model() {
    for scale in [ScaleDef::Front, ScaleDef::Battle, ScaleDef::Siege] {
        let f = fx(scale, None);
        assert!(f.front_line, "{} 是阵地粒度，默认用连续正面模型", scale.as_str());
    }
}

#[test]
fn module_can_override_the_scale_default_in_both_directions() {
    // 战役级地图偏要用密模型
    let on = fx(
        ScaleDef::Theater,
        Some(PreviewDef {
            front_line: Some(true),
            ..Default::default()
        }),
    );
    assert!(on.front_line, "模块显式写 true 要能盖过尺度默认");
    // 正面级地图偏要用粗口径
    let off = fx(
        ScaleDef::Front,
        Some(PreviewDef {
            front_line: Some(false),
            ..Default::default()
        }),
    );
    assert!(!off.front_line, "模块显式写 false 也要能盖过尺度默认");
}

#[test]
fn partial_override_leaves_other_mechanisms_on() {
    let f = fx(
        ScaleDef::Theater,
        Some(PreviewDef {
            politics: Some(false),
            ..Default::default()
        }),
    );
    assert!(!f.politics);
    assert!(f.ammo_fuel && f.rail_damage, "没提到的保持默认");
}

#[test]
fn simulated_result_differs_by_scale_on_the_same_map() {
    // 同一张 35 km/省 的地图，仅因为 scale 标签不同，拿到的占领语义就不同。
    let coarse = fx(ScaleDef::Theater, None);
    let dense = fx(ScaleDef::Front, None);
    assert_ne!(coarse, dense);
    assert_eq!(coarse.disabled(), vec!["front_line"]);
    assert!(dense.disabled().is_empty());
}
