//! 规则引擎：让模块在不修改引擎代码的前提下，定义新机制。
//!
//! 设计要点：
//!
//! 1. **模块只能写声明式规则**，不能写任意代码。
//!    原语、条件都是引擎预先定义的，模块只能组合。
//! 2. **求值顺序固定**：`(phase, priority, id)`，不依赖文件书写顺序。
//! 3. **规则不能有随机、IO、系统时间**。要随机必须走引擎的确定性 RNG。
//!
//! 这一层是"万物皆模块"里性价比最高的部分：不改内核、不改系统，
//! 只在固定钩子上插一段声明，就能让战役有特殊机制。

use crate::fixed::Fixed;
use crate::world::{Side, Terrain, UnitKind, World};

/// 规则在 tick 里的哪个时间点求值。
///
/// 钩子数量要克制：每加一个就多一处分叉。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Phase {
    PreSupply = 0,
    PostSupply = 1,
    PostMovement = 2,
    PreCombat = 3,
    PostCombat = 4,
    PostTick = 5,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::PreSupply => "补给前",
            Phase::PostSupply => "补给后",
            Phase::PostMovement => "移动后",
            Phase::PreCombat => "战斗前",
            Phase::PostCombat => "战斗后",
            Phase::PostTick => "tick 末",
        }
    }
}

/// 条件：可组合的谓词。
#[derive(Clone, Debug)]
pub enum Condition {
    Always,
    Never,
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
    /// 规则涉及的单位所在省的地形。
    ProvinceTerrain(Terrain),
    /// 规则涉及的单位类型。
    UnitKindIs(UnitKind),
    /// 兵力低于编制的某个比例。
    StrengthBelow(Fixed),
    /// 所在省的归属方。
    ProvinceOwner(Side),
    /// 规则涉及的**单位**属于哪一方。
    ///
    /// 没有这一条就写不出单向机制：任何"某地的部队掉兵/掉补给"的规则
    /// 都会同时作用在双方身上。淞沪的"国军弹药与炮兵火力不足"
    /// 这类事实，只能靠 `side` 表达。
    UnitSide(Side),
    /// 该单位当下是否真的在交战（同省内有敌方地面单位）。
    ///
    /// 为什么需要：以前想写"巷战绞肉"，只能写成 `terrain == urban` + 每天掉 X%，
    /// 那是对**所有**驻军收税，不分敌我、不分打没打——实测 91 天固定 −32%，
    /// 把胜负彻底搅浑。有了这个谓词，"只伤害正在交战的部队"才写得出来。
    InCombat,
}

/// 原语：引擎提供的原子操作。模块**只能**调用这些改状态。
#[derive(Clone, Debug)]
pub enum Primitive {
    /// 按当前兵力的百分比增减（-0.02 表示损失 2%）。
    ModifyStrengthPct(Fixed),
    /// 士气增减。
    ModifyMorale(Fixed),
    /// 补给增减。
    ModifySupply(Fixed),
    /// 写一条战报。
    Log(String),
}

impl Primitive {
    /// 这个原语是否改变了世界状态（`Log` 不算）。
    pub fn mutates_state(&self) -> bool {
        !matches!(self, Primitive::Log(_))
    }
}

/// 一条规则。
#[derive(Clone, Debug)]
pub struct Rule {
    pub id: String,
    pub phase: Phase,
    /// 同一阶段内，小的先执行。
    pub priority: i32,
    pub when: Condition,
    pub then: Vec<Primitive>,
}

/// 规则集。
#[derive(Clone, Debug, Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

impl RuleSet {
    /// 构造时**固定排序**：phase → priority → id 字典序。
    ///
    /// 不能依赖 YAML 里的书写顺序——那样作者改一下排版就改变了模拟结果。
    pub fn new(mut rules: Vec<Rule>) -> Self {
        rules.sort_by(|a, b| {
            (a.phase, a.priority, a.id.as_str()).cmp(&(b.phase, b.priority, b.id.as_str()))
        });
        Self { rules }
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// 在某个阶段执行所有匹配的规则。
    ///
    /// 求值单位是"每个省 × 该省的每个存活单位"。
    /// 这是第一版的简化：将来如果需要"全局规则"（比如 tick 计数），
    /// 再加一个不绑定单位的求值模式。
    pub fn run(&self, world: &mut World, phase: Phase) {
        if self.rules.is_empty() {
            return;
        }

        let matching: Vec<&Rule> = self.rules.iter().filter(|r| r.phase == phase).collect();
        if matching.is_empty() {
            return;
        }

        // 先把要执行的动作收集起来，再统一执行。
        // 不能在遍历过程中改状态，否则后规则的求值会看到前规则的结果，
        // 顺序敏感性会变得难以推理。
        let mut planned: Vec<(usize, usize)> = Vec::new();

        for p in 0..world.provinces.len() {
            let units = world.units_in(p as u32);
            for &u in &units {
                let uidx = u as usize;
                for (ri, rule) in matching.iter().enumerate() {
                    if eval(&rule.when, world, p, uidx) {
                        planned.push((uidx, ri));
                    }
                }
            }
        }

        // 逐 (单位, 规则) 执行，这样能在同一组动作前后取状态差，
        // 把"到底减员多少"写进战报——否则战报只有一句"出现非战斗减员"，等于没说。
        for (uidx, ri) in planned {
            let rule = matching[ri];
            let before_s = world.units[uidx].strength;
            let before_m = world.units[uidx].morale;
            let mut logged = false;
            for prim in &rule.then {
                logged |= matches!(prim, Primitive::Log(_));
                apply(world, uidx, prim);
            }
            if logged {
                let ds = world.units[uidx].strength - before_s;
                let dm = world.units[uidx].morale - before_m;
                // 只报真的变了的项，别写"士气 +0.00"占地方
                let mut parts: Vec<String> = Vec::new();
                if !ds.is_zero() {
                    parts.push(format!("兵力 {:+.2} 千", ds.to_display_f64()));
                }
                if !dm.is_zero() {
                    parts.push(format!("士气 {:+.2}", dm.to_display_f64()));
                }
                if !parts.is_empty() {
                    if let Some(ev) = world.log.last_mut() {
                        ev.text.push_str(&format!("（{}）", parts.join("，")));
                    }
                }
            }
        }
    }
}

/// 求值一个条件。
fn eval(c: &Condition, w: &World, p: usize, u: usize) -> bool {
    match c {
        Condition::Always => true,
        Condition::Never => false,
        Condition::All(v) => v.iter().all(|x| eval(x, w, p, u)),
        Condition::Any(v) => v.iter().any(|x| eval(x, w, p, u)),
        Condition::Not(x) => !eval(x, w, p, u),
        Condition::ProvinceTerrain(t) => w.provinces[p].terrain == *t,
        Condition::UnitKindIs(k) => w.units[u].kind == *k,
        Condition::StrengthBelow(r) => w.units[u].strength < w.units[u].max_strength * *r,
        Condition::ProvinceOwner(s) => w.provinces[p].owner == *s,
        Condition::UnitSide(s) => w.units[u].side == *s,
        Condition::InCombat => {
            let mine = w.units[u].side;
            w.units_in(p as u32).iter().any(|&i| {
                let x = &w.units[i as usize];
                x.is_alive() && x.kind.is_ground() && x.side != mine
            })
        }
    }
}

/// 执行一个原语。
fn apply(w: &mut World, u: usize, prim: &Primitive) {
    if !w.units[u].is_alive() && prim.mutates_state() {
        return;
    }
    match prim {
        Primitive::ModifyStrengthPct(pct) => {
            let delta = w.units[u].strength * *pct;
            let next = (w.units[u].strength + delta).max(Fixed::ZERO);
            w.units[u].strength = next;
        }
        Primitive::ModifyMorale(d) => {
            w.units[u].morale = (w.units[u].morale + *d).clamp(Fixed::ZERO, Fixed::ONE);
        }
        Primitive::ModifySupply(d) => {
            w.units[u].supply = (w.units[u].supply + *d).clamp(Fixed::ZERO, Fixed::ONE);
        }
        Primitive::Log(s) => {
            // 战报限长，避免无限增长
            if w.log.len() < 400 {
                let tick = w.tick;
                // 规则是按单位触发的，所以必须写清**是哪个单位**。
                // 以前只写"山地行军，部队出现非战斗减员"，一排全一样，等于没说。
                let name = w.units[u].name.clone();
                let text = format!("{name}：{s}");
                let side = w.units[u].side;
                let _ = tick;
                w.report_for(crate::world::EventKind::Rule, text, side);
            }
        }
    }
}

/// 静态校验：检查规则是否违反了确定性约束。
///
/// 当前只能检查结构性错误（空 id、重复 id、空动作）。
/// "有没有偷偷用随机"这种要等 DSL 落地后才能查。
pub fn validate_rules(rules: &[Rule]) -> Vec<String> {
    let mut issues = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in rules {
        if r.id.trim().is_empty() {
            issues.push("规则缺少 id".to_string());
        }
        if !seen.insert(r.id.clone()) {
            issues.push(format!("规则 id 重复：{}", r.id));
        }
        if r.then.is_empty() {
            issues.push(format!("规则 {} 没有动作", r.id));
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Province, Unit};

    fn world_with(kind: UnitKind, terrain: Terrain) -> World {
        let provinces = vec![
            Province::new("A", terrain, Side::A)
                .with_neighbors(&[1])
                .as_source(),
            Province::new("B", Terrain::Plain, Side::B)
                .with_neighbors(&[0])
                .as_source(),
        ];
        let mut u = Unit::new("cn", Side::A, 0, 10);
        u.kind = kind;
        World::new(1, provinces, vec![u])
    }

    #[test]
    fn rule_fires_when_condition_matches() {
        let mut w = world_with(UnitKind::Armor, Terrain::Mountain);
        let rules = RuleSet::new(vec![Rule {
            id: "armor_in_mountain".into(),
            phase: Phase::PostMovement,
            priority: 0,
            when: Condition::All(vec![
                Condition::ProvinceTerrain(Terrain::Mountain),
                Condition::UnitKindIs(UnitKind::Armor),
            ]),
            then: vec![Primitive::ModifyStrengthPct(Fixed::ratio(-2, 100))],
        }]);

        let before = w.units[0].strength;
        rules.run(&mut w, Phase::PostMovement);
        assert!(w.units[0].strength < before, "规则应当生效");
    }

    #[test]
    fn rule_does_not_fire_when_condition_fails() {
        let mut w = world_with(UnitKind::Infantry, Terrain::Mountain);
        let rules = RuleSet::new(vec![Rule {
            id: "armor_in_mountain".into(),
            phase: Phase::PostMovement,
            priority: 0,
            when: Condition::All(vec![
                Condition::ProvinceTerrain(Terrain::Mountain),
                Condition::UnitKindIs(UnitKind::Armor),
            ]),
            then: vec![Primitive::ModifyStrengthPct(Fixed::ratio(-2, 100))],
        }]);

        let before = w.units[0].strength;
        rules.run(&mut w, Phase::PostMovement);
        assert_eq!(w.units[0].strength.raw(), before.raw(), "条件不满足不应生效");
    }

    #[test]
    fn wrong_phase_does_not_fire() {
        let mut w = world_with(UnitKind::Armor, Terrain::Mountain);
        let rules = RuleSet::new(vec![Rule {
            id: "r".into(),
            phase: Phase::PreCombat,
            priority: 0,
            when: Condition::Always,
            then: vec![Primitive::ModifyMorale(Fixed::ratio(-1, 10))],
        }]);
        let before = w.units[0].morale;
        rules.run(&mut w, Phase::PostCombat);
        assert_eq!(w.units[0].morale.raw(), before.raw());
    }

    #[test]
    fn ordering_is_deterministic_regardless_of_input_order() {
        let mk = |id: &str, prio: i32| Rule {
            id: id.into(),
            phase: Phase::PostTick,
            priority: prio,
            when: Condition::Always,
            then: vec![Primitive::Log(id.into())],
        };
        let a = RuleSet::new(vec![mk("b", 1), mk("a", 1), mk("c", 0)]);
        let b = RuleSet::new(vec![mk("c", 0), mk("a", 1), mk("b", 1)]);

        assert_eq!(a.rules()[0].id, "c", "priority 小的先执行");
        assert_eq!(a.rules()[1].id, "a", "同优先级按 id 字典序");
        assert_eq!(a.rules()[2].id, "b");

        let ids_a: Vec<&str> = a.rules().iter().map(|r| r.id.as_str()).collect();
        let ids_b: Vec<&str> = b.rules().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids_a, ids_b, "输入顺序不应影响执行顺序");
    }

    #[test]
    fn validate_catches_duplicate_and_empty() {
        let issues = validate_rules(&[
            Rule {
                id: "".into(),
                phase: Phase::PostTick,
                priority: 0,
                when: Condition::Always,
                then: vec![],
            },
            Rule {
                id: "dup".into(),
                phase: Phase::PostTick,
                priority: 0,
                when: Condition::Always,
                then: vec![Primitive::Log("x".into())],
            },
            Rule {
                id: "dup".into(),
                phase: Phase::PostTick,
                priority: 0,
                when: Condition::Always,
                then: vec![Primitive::Log("y".into())],
            },
        ]);
        assert!(issues.iter().any(|s| s.contains("缺少 id")));
        assert!(issues.iter().any(|s| s.contains("重复")));
        assert!(issues.iter().any(|s| s.contains("没有动作")));
    }

    #[test]
    fn dead_unit_is_not_modified() {
        let mut w = world_with(UnitKind::Armor, Terrain::Mountain);
        w.units[0].strength = Fixed::ZERO;
        let rules = RuleSet::new(vec![Rule {
            id: "r".into(),
            phase: Phase::PostTick,
            priority: 0,
            when: Condition::Always,
            then: vec![Primitive::ModifyMorale(Fixed::ratio(1, 2))],
        }]);
        let before = w.units[0].morale;
        rules.run(&mut w, Phase::PostTick);
        assert_eq!(w.units[0].morale.raw(), before.raw(), "阵亡单位不应被规则改动");
    }
}
