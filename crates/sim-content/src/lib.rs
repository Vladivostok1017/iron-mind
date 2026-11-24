//! 模块加载与构建。
//!
//! 一个模块是一个目录（或打包后的 `.imod`），至少包含四个文件：
//!
//! ```text
//! module.json     元数据、引擎版本约束
//! scenario.json   战役定义（长度、种子）
//! map.json        省份与邻接
//! oob.json        单位
//! ```
//!
//! 加载流程：读 → 解析 → 校验 → 构建 `World`。
//! 校验有任何 Error 级问题就拒绝加载——宁可启动失败，也不要跑一个坏模块。

pub mod error;
pub mod model;
pub mod validate;

use std::fs;
use std::path::{Path, PathBuf};

use sim_core::fixed::Fixed;
use sim_core::world::{Province, Side, Terrain, Unit, World};

pub use error::{Issue, LoadError, Severity};
pub use model::*;

/// 加载成功的模块。
#[derive(Debug, Clone)]
pub struct LoadedModule {
    pub meta: ModuleMeta,
    pub scenario: ScenarioDef,
    pub map: MapDef,
    pub oob: OobDef,
    /// 校验过程中的 Warning（Error 已导致加载失败）。
    pub warnings: Vec<Issue>,
}

impl LoadedModule {
    /// 模块目录名。
    pub fn dir_name(&self) -> &str {
        self.meta.id.rsplit('.').next().unwrap_or(&self.meta.id)
    }
}

fn read_json<T: for<'de> serde::Deserialize<'de>>(path: &Path) -> Result<T, LoadError> {
    let text = fs::read_to_string(path).map_err(|e| LoadError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    serde_json::from_str(&text).map_err(|e| LoadError::Parse {
        path: path.to_path_buf(),
        message: format!("{}（第 {} 行第 {} 列）", e, e.line(), e.column()),
    })
}

/// 从目录加载一个模块。
pub fn load_module(dir: &Path) -> Result<LoadedModule, LoadError> {
    let base = dir.to_path_buf();
    let meta: ModuleMeta = read_json(&base.join("module.json"))?;
    let scenario: ScenarioDef = read_json(&base.join("scenario.json"))?;
    let map: MapDef = read_json(&base.join("map.json"))?;
    let oob: OobDef = read_json(&base.join("oob.json"))?;

    let issues = validate::validate(&meta, &map, &oob);
    let errors: Vec<Issue> = issues.iter().filter(|i| i.is_error()).cloned().collect();
    if !errors.is_empty() {
        return Err(LoadError::Validation {
            module: meta.id.clone(),
            issues: errors,
        });
    }

    let warnings = issues.into_iter().filter(|i| !i.is_error()).collect();

    Ok(LoadedModule {
        meta,
        scenario,
        map,
        oob,
        warnings,
    })
}

/// 从内存中的四个片段构建，不需要文件系统（测试用）。
pub fn from_parts(
    meta: ModuleMeta,
    scenario: ScenarioDef,
    map: MapDef,
    oob: OobDef,
) -> Result<LoadedModule, LoadError> {
    let issues = validate::validate(&meta, &map, &oob);
    let errors: Vec<Issue> = issues.iter().filter(|i| i.is_error()).cloned().collect();
    if !errors.is_empty() {
        return Err(LoadError::Validation {
            module: meta.id.clone(),
            issues: errors,
        });
    }
    let warnings = issues.into_iter().filter(|i| !i.is_error()).collect();
    Ok(LoadedModule {
        meta,
        scenario,
        map,
        oob,
        warnings,
    })
}

fn terrain_of(t: TerrainDef) -> Terrain {
    match t {
        TerrainDef::Plain => Terrain::Plain,
        TerrainDef::Hill => Terrain::Hill,
        TerrainDef::Mountain => Terrain::Mountain,
        TerrainDef::Marsh => Terrain::Marsh,
        TerrainDef::Water => Terrain::Water,
        TerrainDef::Urban => Terrain::Urban,
    }
}

fn side_of(s: SideDef) -> Side {
    match s {
        SideDef::Cn => Side::Cn,
        SideDef::Jp => Side::Jp,
    }
}

/// 把加载好的模块构建成模拟核心的 `World`。
///
/// 这是浮点 → 定点的唯一转换点，之后全程定点。
pub fn build_world(module: &LoadedModule) -> World {
    // 按 id 排序，保证索引与 id 一致
    let mut defs: Vec<&ProvinceDef> = module.map.provinces.iter().collect();
    defs.sort_by_key(|p| p.id);

    let mut provinces = Vec::with_capacity(defs.len());
    for d in defs {
        let owner = d.owner.map(side_of).unwrap_or(Side::Cn);
        let mut p = Province::new(&d.name, terrain_of(d.terrain), owner);
        p.fort = d.fort.min(5);
        p.supply_throughput = Fixed::from_f64_input(d.supply_throughput).clamp(Fixed::ZERO, Fixed::ONE);
        p.neighbors = d.neighbors.clone();
        p.is_supply_source = d.supply_source;
        p.lon = Fixed::from_f64_input(d.lon.unwrap_or(0.0));
        p.lat = Fixed::from_f64_input(d.lat.unwrap_or(0.0));
        // 阵地：没写就生成一个（省名 + 省地形 + 全宽度），老模块因此不用改。
        // 写了就按书写顺序编号——编号也是确定性的一部分。
        if d.positions.is_empty() {
            p.positions
                .push(sim_core::world::Position::new(&d.name, terrain_of(d.terrain)));
        } else {
            for pd in &d.positions {
                let terr = pd.terrain.map(terrain_of).unwrap_or(terrain_of(d.terrain));
                let mut node = sim_core::world::Position::new(&pd.name, terr);
                node.width = Fixed::from_f64_input(pd.width.clamp(0.2, 1.0));
                node.fort = pd.fort.min(5);
                p.positions.push(node);
            }
        }
        provinces.push(p);
    }

    let mut units = Vec::with_capacity(module.oob.units.len());
    // 保持 oob 中的书写顺序（也是确定性的一部分）
    for u in &module.oob.units {
        let strength_k = u.strength.round() as i32;
        let mut unit = Unit::new(&u.name, side_of(u.side), u.position, strength_k);
        // 支持小数兵力
        let frac = u.strength - u.strength.round();
        if frac.abs() > 1e-9 {
            let extra = Fixed::from_f64_input(frac);
            unit.strength += extra;
            unit.max_strength += extra;
        }
        unit.effectiveness = Fixed::from_f64_input(u.effectiveness.max(0.0));
        unit.supply_reach = Fixed::from_f64_input(u.supply_reach.clamp(0.0, 1.0));
        unit.kind = match u.kind {
            UnitKindDef::Infantry => sim_core::UnitKind::Infantry,
            UnitKindDef::Armor => sim_core::UnitKind::Armor,
            UnitKindDef::Air => sim_core::UnitKind::Air,
            UnitKindDef::Naval => sim_core::UnitKind::Naval,
        };
        unit.formation = u.parent.clone().unwrap_or_default();
        unit.recon_radius = u.recon_radius;
        unit.stance = match u.stance {
            StanceDef::Attack => sim_core::world::Stance::Attack,
            StanceDef::Defend => sim_core::world::Stance::Defend,
        };
        unit.strike_power = Fixed::from_f64_input(u.strike_power.max(0.0));
        if let Some(c) = &u.commander {
            unit.commander = sim_core::world::Commander::new(
                &c.name,
                c.obedience,
                c.initiative,
                c.aggression,
                c.caution,
            )
            .with_persona(&c.title, &c.persona, &c.agenda);
        }
        units.push(unit);
    }

    let seed = if module.scenario.seed != 0 {
        module.scenario.seed
    } else {
        // 没写种子时，由模块 id 派生，保证同模块同结果
        simple_hash(&module.meta.id)
    };

    let mut world = World::new(seed, provinces, units);
    world.comms = comms_from(module.scenario.comms.as_ref());

    // 编译模块声明的规则
    let (rule_set, _problems) = rules_from(&module.scenario.rules);
    world.rules = std::sync::Arc::new(rule_set);

    world
}

/// 从模块配置解析通信条件。
///
/// 引擎不认识"年代"，只认识延迟天数与通信手段的名字。
/// 这样 1805 年和 1944 年用的是同一套代码，只是参数不同。
pub fn comms_from(def: Option<&CommsDef>) -> sim_core::world::CommsConfig {
    use sim_core::world::{CommsConfig, CommsKind};

    let mut cfg = CommsConfig::telegraph_era();
    let Some(d) = def else {
        return cfg;
    };

    if let Some(preset) = &d.preset {
        cfg = match preset.as_str() {
            "courier" => CommsConfig::courier_era(),
            "radio" => CommsConfig::radio_era(),
            "wire_telegraph" | "telegraph" => CommsConfig::telegraph_era(),
            other => {
                // 未知预设：保持默认，不崩溃。向前兼容的关键。
                let _ = other;
                CommsConfig::telegraph_era()
            }
        };
    }

    if let Some(v) = d.transmission {
        cfg.transmission = v;
    }
    if let Some(v) = d.staff {
        cfg.staff = v;
    }
    if let Some(v) = d.order_delay {
        cfg.order_delay = v;
    }

    // 手段名只影响叙事，这里按预设校准一下
    if d.preset.is_none() {
        cfg.kind = CommsKind::WireTelegraph;
    }
    cfg
}

fn simple_hash(s: &str) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01B3);
    }
    h
}

/// 引擎版本约束检查。只是提示，不阻断加载。
pub fn check_engine_compat(meta: &ModuleMeta, engine_version: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    if !version_ge(engine_version, &meta.engine.min) {
        issues.push(Issue::error(
            "engine.too_old",
            format!(
                "模块要求引擎 >= {}，当前 {}",
                meta.engine.min, engine_version
            ),
        ));
    }
    if let Some(max) = &meta.engine.max {
        if version_ge(engine_version, max) {
            issues.push(Issue::warn(
                "engine.too_new",
                format!("模块声明最高支持 {}，当前 {}，可能不兼容", max, engine_version),
            ));
        }
    }
    issues
}

/// 极简语义化版本比较（主.次.修订）。无法解析的段按 0 处理。
fn version_ge(a: &str, b: &str) -> bool {
    let pa = parse_version(a);
    let pb = parse_version(b);
    pa >= pb
}

fn parse_version(s: &str) -> (u32, u32, u32) {
    let core = s.split(['-', '+']).next().unwrap_or("");
    let mut it = core.split('.');
    let major = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let minor = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let patch = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

/// 模块文件的路径集合，便于工具打印。
pub fn module_files(dir: &Path) -> Vec<PathBuf> {
    ["module.json", "scenario.json", "map.json", "oob.json"]
        .iter()
        .map(|f| dir.join(f))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(version_ge("0.2.0", "0.1.0"));
        assert!(version_ge("0.1.0", "0.1.0"));
        assert!(!version_ge("0.0.9", "0.1.0"));
        assert!(version_ge("1.0.0", "0.9.9"));
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(simple_hash("a"), simple_hash("a"));
        assert_ne!(simple_hash("a"), simple_hash("b"));
    }
}


// ---------------------------------------------------------------- 规则

/// 把模块里的规则声明编译成引擎的 `RuleSet`。
///
/// 遇到写错的条目时**跳过并收集警告**，不整体失败——
/// 一条规则写错不该让整个战役装不进去。
pub fn rules_from(defs: &[RuleDef]) -> (sim_core::RuleSet, Vec<String>) {
    use sim_core::{Phase, Rule};

    let mut rules = Vec::new();
    let mut problems = Vec::new();

    for d in defs {
        let phase = match d.phase {
            PhaseDef::PreSupply => Phase::PreSupply,
            PhaseDef::PostSupply => Phase::PostSupply,
            PhaseDef::PostMovement => Phase::PostMovement,
            PhaseDef::PreCombat => Phase::PreCombat,
            PhaseDef::PostCombat => Phase::PostCombat,
            PhaseDef::PostTick => Phase::PostTick,
        };

        let when = match condition_from(&d.when) {
            Ok(c) => c,
            Err(e) => {
                problems.push(format!("规则 {} 的条件无效：{}", d.id, e));
                continue;
            }
        };

        let mut then = Vec::new();
        for p in &d.then {
            match primitive_from(p) {
                Ok(x) => then.push(x),
                Err(e) => {
                    problems.push(format!("规则 {} 的动作无效：{}", d.id, e));
                }
            }
        }
        if then.is_empty() {
            problems.push(format!("规则 {} 没有任何有效动作，已跳过", d.id));
            continue;
        }

        rules.push(Rule {
            id: d.id.clone(),
            phase,
            priority: d.priority,
            when,
            then,
        });
    }

    (sim_core::RuleSet::new(rules), problems)
}

fn terrain_from(s: &str) -> Result<sim_core::Terrain, String> {
    match s {
        "plain" => Ok(sim_core::Terrain::Plain),
        "hill" => Ok(sim_core::Terrain::Hill),
        "mountain" => Ok(sim_core::Terrain::Mountain),
        "marsh" => Ok(sim_core::Terrain::Marsh),
        "water" => Ok(sim_core::Terrain::Water),
        "urban" => Ok(sim_core::Terrain::Urban),
        other => Err(format!("未知地形 `{other}`")),
    }
}

fn kind_from(s: &str) -> Result<sim_core::UnitKind, String> {
    match s {
        "infantry" => Ok(sim_core::UnitKind::Infantry),
        "armor" => Ok(sim_core::UnitKind::Armor),
        "air" => Ok(sim_core::UnitKind::Air),
        "naval" => Ok(sim_core::UnitKind::Naval),
        other => Err(format!("未知兵种 `{other}`")),
    }
}

fn side_from(s: &str) -> Result<sim_core::Side, String> {
    match s {
        "cn" => Ok(sim_core::Side::Cn),
        "jp" => Ok(sim_core::Side::Jp),
        other => Err(format!("未知阵营 `{other}`")),
    }
}

fn condition_from(c: &ConditionDef) -> Result<sim_core::Condition, String> {
    use sim_core::Condition as C;
    Ok(match c {
        ConditionDef::Always => C::Always,
        ConditionDef::Never => C::Never,
        ConditionDef::All(v) => {
            let mut out = Vec::new();
            for x in v {
                out.push(condition_from(x)?);
            }
            C::All(out)
        }
        ConditionDef::Any(v) => {
            let mut out = Vec::new();
            for x in v {
                out.push(condition_from(x)?);
            }
            C::Any(out)
        }
        ConditionDef::Not(x) => C::Not(Box::new(condition_from(x)?)),
        ConditionDef::Terrain(s) => C::ProvinceTerrain(terrain_from(s)?),
        ConditionDef::UnitKind(s) => C::UnitKindIs(kind_from(s)?),
        ConditionDef::StrengthBelow(r) => C::StrengthBelow(Fixed::from_f64_input(*r)),
        ConditionDef::ProvinceOwner(s) => C::ProvinceOwner(side_from(s)?),
    })
}

fn primitive_from(p: &PrimitiveDef) -> Result<sim_core::Primitive, String> {
    use sim_core::Primitive as P;
    Ok(match p {
        PrimitiveDef::ModifyStrengthPct(v) => P::ModifyStrengthPct(Fixed::from_f64_input(*v)),
        PrimitiveDef::ModifyMorale(v) => P::ModifyMorale(Fixed::from_f64_input(*v)),
        PrimitiveDef::ModifySupply(v) => P::ModifySupply(Fixed::from_f64_input(*v)),
        PrimitiveDef::Log(s) => P::Log(s.clone()),
    })
}



