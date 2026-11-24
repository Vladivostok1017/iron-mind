//! 模块校验。
//!
//! 分两层：
//!
//! - **静态校验**（本文件）：结构、引用、邻接、连通性。不跑模拟就能查。
//! - **平衡校验**（未实现）：无头跑批，看结局分布。
//!
//! 校验的目标不是"证明模块正确"，而是"让作者在发布前知道自己做错了什么"。
//! 所以每条问题都要有稳定的 `code` 和能指导修改的文案。

use std::collections::{HashSet, VecDeque};

use crate::error::Issue;
use crate::model::{MapDef, ModuleMeta, OobDef, SideDef};

/// 对整个模块做静态校验。返回全部问题（含 Warning）。
pub fn validate(meta: &ModuleMeta, map: &MapDef, oob: &OobDef) -> Vec<Issue> {
    let mut issues = Vec::new();

    // ---------- 元数据 ----------
    if meta.id.trim().is_empty() {
        issues.push(Issue::error("meta.empty_id", "module.json 的 id 不能为空"));
    }
    if meta.engine.min.trim().is_empty() {
        issues.push(Issue::error(
            "meta.empty_engine_min",
            "module.json 必须声明 engine.min",
        ));
    }
    if meta.content_schema > crate::model::CURRENT_CONTENT_SCHEMA {
        issues.push(Issue::error(
            "meta.future_schema",
            format!(
                "content_schema {} 高于引擎支持的 {}，本引擎无法加载",
                meta.content_schema,
                crate::model::CURRENT_CONTENT_SCHEMA
            ),
        ));
    }

    // ---------- 地图结构 ----------
    let n = map.provinces.len();
    if n == 0 {
        issues.push(Issue::error("map.empty", "地图没有任何省份"));
        return issues;
    }

    // id 必须从 0 连续（引擎内部按索引访问）
    let mut seen = HashSet::new();
    for p in &map.provinces {
        if !seen.insert(p.id) {
            issues.push(Issue::error(
                "map.duplicate_id",
                format!("省份 id {} 重复", p.id),
            ));
        }
        if p.id as usize >= n {
            issues.push(Issue::error(
                "map.id_out_of_range",
                format!("省份 id {} 超出范围（共 {} 个省份，id 必须 0..{}）", p.id, n, n - 1),
            ));
        }
        if p.name.trim().is_empty() {
            issues.push(Issue::error(
                "map.empty_name",
                format!("省份 id {} 没有名字", p.id),
            ));
        }
        if p.fort > 5 {
            issues.push(Issue::error(
                "map.fort_range",
                format!("{} 的 fort={} 超出 0-5", p.name, p.fort),
            ));
        }
        if !(0.0..=1.0).contains(&p.supply_throughput) {
            issues.push(Issue::error(
                "map.throughput_range",
                format!("{} 的 supply_throughput 应在 0-1 之间", p.name),
            ));
        }
        for &nb in &p.neighbors {
            if nb as usize >= n {
                issues.push(Issue::error(
                    "map.neighbor_out_of_range",
                    format!("{} 的邻接 {} 超出范围", p.name, nb),
                ));
            }
            if nb == p.id {
                issues.push(Issue::error(
                    "map.self_loop",
                    format!("{} 的邻接里包含自己", p.name),
                ));
            }
        }
    }

    // 邻接对称性
    for p in &map.provinces {
        for &nb in &p.neighbors {
            if (nb as usize) >= n {
                continue;
            }
            let other = &map.provinces[nb as usize];
            if !other.neighbors.contains(&p.id) {
                issues.push(Issue::error(
                    "map.asymmetric_neighbor",
                    format!(
                        "{} → {} 有邻接，但 {} → {} 没有（邻接必须双向）",
                        p.name, other.name, other.name, p.name
                    ),
                ));
            }
        }
    }

    // 邻接数范围
    for p in &map.provinces {
        if p.neighbors.len() > 12 {
            issues.push(Issue::warn(
                "map.too_many_neighbors",
                format!("{} 有 {} 个邻接，可能切得太碎", p.name, p.neighbors.len()),
            ));
        }
    }

    // ---------- 补给源 ----------
    for side in [SideDef::Cn, SideDef::Jp] {
        let has_source = map
            .provinces
            .iter()
            .any(|p| p.supply_source && p.owner == Some(side));
        if !has_source {
            issues.push(Issue::error(
                "map.no_supply_source",
                format!("{} 没有任何补给源省份", side.as_str()),
            ));
        }
    }

    // ---------- 归属 ----------
    let unowned: Vec<&str> = map
        .provinces
        .iter()
        .filter(|p| p.owner.is_none())
        .map(|p| p.name.as_str())
        .collect();
    if !unowned.is_empty() {
        issues.push(Issue::error(
            "map.unowned_province",
            format!("以下省份没有归属：{}", unowned.join("、")),
        ));
    }

    // ---------- 单位 ----------
    if oob.units.is_empty() {
        issues.push(Issue::error("oob.empty", "oob.json 没有任何单位"));
    }

    let mut unit_ids = HashSet::new();
    for u in &oob.units {
        if !unit_ids.insert(&u.id) {
            issues.push(Issue::error(
                "oob.duplicate_id",
                format!("单位 id {} 重复", u.id),
            ));
        }
        if u.strength <= 0.0 {
            issues.push(Issue::error(
                "oob.strength_nonpositive",
                format!("{} 的兵力必须为正", u.name),
            ));
        }
        if u.strength > 200.0 {
            issues.push(Issue::warn(
                "oob.strength_extreme",
                format!("{} 的兵力 {} 千人异常大，确认是否写错单位", u.name, u.strength),
            ));
        }
        if u.position as usize >= n {
            issues.push(Issue::error(
                "oob.position_out_of_range",
                format!("{} 的 position {} 超出范围", u.name, u.position),
            ));
            continue;
        }
        // 单位在自己没有补给源的敌省出生 → 警告
        let prov = &map.provinces[u.position as usize];
        if prov.owner != Some(u.side) {
            issues.push(Issue::warn(
                "oob.starts_in_enemy_land",
                format!("{} 初始位置 {} 不属于本方", u.name, prov.name),
            ));
        }
    }

    for side in [SideDef::Cn, SideDef::Jp] {
        if !oob.units.iter().any(|u| u.side == side) {
            issues.push(Issue::error(
                "oob.side_without_units",
                format!("{} 没有任何单位，无法对战", side.as_str()),
            ));
        }
    }

    // ---------- 连通性 ----------
    issues.extend(check_reachability(map, oob));

    issues
}

/// 从每方补给源出发，检查其控制省与单位是否可达。
fn check_reachability(map: &MapDef, oob: &OobDef) -> Vec<Issue> {
    let mut issues = Vec::new();
    let n = map.provinces.len();

    for side in [SideDef::Cn, SideDef::Jp] {
        let mut reachable = vec![false; n];
        let mut queue: VecDeque<u32> = VecDeque::new();

        for p in &map.provinces {
            if p.supply_source && p.owner == Some(side) {
                reachable[p.id as usize] = true;
                queue.push_back(p.id);
            }
        }

        while let Some(cur) = queue.pop_front() {
            let neighbors = map.provinces[cur as usize].neighbors.clone();
            for nb in neighbors {
                if (nb as usize) >= n {
                    continue;
                }
                if map.provinces[nb as usize].owner == Some(side) && !reachable[nb as usize] {
                    reachable[nb as usize] = true;
                    queue.push_back(nb);
                }
            }
        }

        // 该方控制的省是否有孤岛
        let orphans: Vec<&str> = map
            .provinces
            .iter()
            .filter(|p| p.owner == Some(side) && !reachable[p.id as usize])
            .map(|p| p.name.as_str())
            .collect();
        if !orphans.is_empty() {
            issues.push(Issue::warn(
                "map.owner_orphans",
                format!(
                    "{} 控制的以下省份从补给源不可达（会成为死地）：{}",
                    side.as_str(),
                    orphans.join("、")
                ),
            ));
        }

        // 单位在不可达省 → 开局即断补给
        for u in oob.units.iter().filter(|u| u.side == side) {
            if (u.position as usize) < n && !reachable[u.position as usize] {
                issues.push(Issue::warn(
                    "oob.unit_unreachable",
                    format!(
                        "{} 初始位置 {} 从本方补给源不可达，开局即断补给",
                        u.name, map.provinces[u.position as usize].name
                    ),
                ));
            }
        }
    }

    issues
}
