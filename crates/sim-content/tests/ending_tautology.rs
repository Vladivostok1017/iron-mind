//! 结局子句的"恒真"问题（上游报告 §1）。
//!
//! 踩过的坑：`ClauseDef.value` 原来是 `f64` + `#[serde(default)]`，省略即 0.0，
//! 而默认比较是 `ge`，于是 `x >= 0` 恒成立。一条 `holds_province` 结局不写 value，
//! 战役在**第 1 天**就判出来并结束（淞沪模块 E7 就是这么死的）。

use sim_content::model::{ClauseDef, EndingDef, ScenarioDef};

fn scen(require: Vec<ClauseDef>) -> ScenarioDef {
    ScenarioDef {
        id: "s".into(),
        name: "s".into(),
        start_date: "1937-08-13".into(),
        days: 90,
        seed: 1,
        comms: None,
        endings: vec![EndingDef {
            id: "E7".into(),
            name: "金山卫".into(),
            text: "…".into(),
            priority: 5,
            at_end_only: false,
            require,
        }],
        rules: Vec::new(),
        arrivals: Vec::new(),
        preview: None,
    }
}

fn clause(metric: &str, province: Option<&str>, cmp: Option<&str>, value: Option<f64>) -> ClauseDef {
    ClauseDef {
        metric: metric.into(),
        side: None,
        cmp: cmp.map(|s| s.to_string()),
        value,
        province: province.map(|s| s.to_string()),
    }
}

// 阵营 id 顺序 = A/B。这组用例不关心是谁，只关心条款怎么补默认值。
fn ids() -> Vec<String> {
    vec!["cn".to_string(), "jp".to_string()]
}

#[test]
fn holds_province_without_value_is_not_tautological() {
    // 不写 value 的 holds_province 必须是"确实控制着"（1.0），不再是 `>= 0`。
    let s = scen(vec![clause("holds_province", Some("松江"), None, None)]);
    let e = sim_content::endings_from(&s.endings, &ids());
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].clauses[0].value, 1.0, "缺省必须是 1.0，不能是 0.0");
}

#[test]
fn other_metrics_still_default_to_zero() {
    // 其余指标沿用 0.0（`provinces >= 0` 这种写法仍然"恒真"，由校验器提醒，
    // 不在转换时硬改语义——改语义会让既有模块的结局突然变严）。
    let s = scen(vec![clause("day", None, None, None)]);
    let e = sim_content::endings_from(&s.endings, &ids());
    assert_eq!(e[0].clauses[0].value, 0.0);
}

#[test]
fn validator_warns_about_tautologies() {
    let s = scen(vec![
        clause("provinces", None, Some("ge"), Some(0.0)),
        clause("day", None, None, None),
        clause("holds_province", Some("德安"), None, None), // 现在不是恒真，不该报
    ]);
    let issues = sim_content::validate::validate_scenario(&s);
    let codes: Vec<&str> = issues.iter().map(|i| i.code).collect();
    assert_eq!(
        codes.iter().filter(|c| **c == "ending.clause_tautology").count(),
        2,
        "两条恒真子句应该各报一次，实测 {codes:?}"
    );
    // 警告不是错误：能加载，只是提醒。
    assert!(issues.iter().all(|i| !i.is_error()));
}

#[test]
fn explicit_zero_with_le_is_also_flagged() {
    // `provinces <= 0` 对"至少占 0 个省"的场面同样恒真。
    let s = scen(vec![clause("provinces", None, Some("le"), Some(0.0))]);
    let issues = sim_content::validate::validate_scenario(&s);
    assert!(issues.iter().any(|i| i.code == "ending.clause_tautology"));
}

#[test]
fn real_conditions_are_not_flagged() {
    let s = scen(vec![
        clause("provinces", None, Some("ge"), Some(30.0)),
        clause("strength", Some("cn"), Some("lt"), Some(80.0)),
    ]);
    let issues = sim_content::validate::validate_scenario(&s);
    assert!(issues.is_empty(), "正常子句不该被报：{:?}", issues.len());
}
