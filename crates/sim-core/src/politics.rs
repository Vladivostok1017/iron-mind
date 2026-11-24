//! 派系与人事（预览版）。
//!
//! 只在模块用 `scenario.preview.politics = true` 显式开启时才参与模拟。
//! 关闭时这里的东西一个字节都不会跑，也一个字节都不会进哈希。
//!
//! 设计意图（为什么是"派系"而不是"忠诚度"）：
//! - 将领的个人数据（服从/主动性/进攻性/谨慎）是**他自己**的；
//! - 派系是**他的靠山**。靠山在朝里说话有没有分量，决定他敢不敢抗命、
//!   以及上面敢不敢撤他；
//! - 所以派系不是第五个人格维度，而是一层**外部约束**：它调制服从，
//!   并提供"人事"这个玩家动作（撤换）。
//!
//! 数值都往"慢"里调：影响力一天只挪 5% 的缺口，撤一个人掉 12 点。
//! 这是政治，不是体力活——它该比打仗慢一个数量级，玩家才会觉得
//! "我刚撤了一个军长，三个月后前线开始不听招呼"是因果，而不是随机惩罚。

use crate::fixed::Fixed;
use crate::hash::Hasher;
use crate::world::{Commander, EventKind, Side, World};

/// 每天朝声明权重回归的比例。
const DRIFT: (i32, i32) = (5, 100);
/// 撤换一个将领，他那一派掉多少影响力。
const PURGE_COST: (i32, i32) = (12, 100);
/// 靠山失势对服从的打击系数（influence 比权重低多少，服从就低 0.25 倍那么多）。
const RESENT: (i32, i32) = (25, 100);

/// 一个派系在模拟里的状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Faction {
    /// 模块声明的 id（内容层的稳定标识）。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 归属阵营。派系只在**本方**内部竞争，不跨阵营。
    pub side: Side,
    /// 朝中分量 0-1。由权重漂移与撤换推动。
    pub influence: Fixed,
    /// 初始权重（模块声明的"这一派本来就该有多重"）。
    pub weight: Fixed,
    /// 本派系将领的部队下标（构建世界时算好，避免每 tick 按名字查）。
    pub units: Vec<usize>,
}

impl Faction {
    pub fn new(id: &str, name: &str, side: Side, weight: Fixed) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            side,
            influence: weight,
            weight,
            units: Vec::new(),
        }
    }

    /// 这一派现在比"本该有的分量"重多少（正 = 得势，负 = 失势）。
    pub fn standing(&self) -> Fixed {
        self.influence - self.weight
    }
}

/// 每天结算：影响力朝权重回归。
pub fn run(world: &mut World) {
    if !world.fx.politics {
        return;
    }
    let drift = Fixed::ratio(DRIFT.0, DRIFT.1);
    for f in world.factions.iter_mut() {
        let gap = f.weight - f.influence;
        f.influence = (f.influence + gap * drift).clamp(Fixed::ZERO, Fixed::ONE);
    }
    // 阵营整体失势时给一句战报，但只在"跨过整 0.2"这种拐点上说一次——
    // 每天一句"某某派系又掉了一点点"等于没说。
    let mut notes: Vec<(String, bool, Side)> = Vec::new();
    for f in world.factions.iter() {
        let standing = f.standing();
        let abs = if standing < Fixed::ZERO {
            Fixed::ZERO - standing
        } else {
            standing
        };
        if abs > Fixed::ratio(18, 100) && world.tick % 7 == 0 {
            notes.push((f.name.clone(), standing >= Fixed::ZERO, f.side));
        }
    }
    for (name, up, side) in notes {
        let what = if up { "得势" } else { "失势" };
        world.report_for(
            EventKind::Rule,
            format!("{name} 在朝中{what}，军中议论纷纷"),
            side,
        );
    }
}

/// 某一派系。给"撤换"这类操作找它用。
pub fn faction_of(world: &World, unit_idx: usize) -> Option<usize> {
    let u = world.units.get(unit_idx)?;
    let fid = u.commander.faction.as_deref()?;
    world.factions.iter().position(|f| f.id == fid && f.side == u.side)
}

/// 撤换一个将领（人事动作）。
///
/// 规则：
/// - 撤掉的人进"已去职"名单（这一局不再回来，不能反复用他刷服从度）；
/// - 他那一派的影响力掉一截（动了人家的台柱子）；
/// - 接任的是个**没有派系**的参谋军官：完全服从，但没有个性，
///   也就是"听话，但不会打仗"——这是撤将的真实代价。
///
/// 返回 (被撤者, 新主官) 或者拒绝原因。
pub fn relieve(world: &mut World, unit_idx: usize) -> Result<(String, String), String> {
    if !world.fx.politics {
        return Err("这个战役没有开「派系与人事」（scenario.json 的 preview.politics）".into());
    }
    let u = world.units.get(unit_idx).ok_or("没有这支部队")?;
    if !u.is_alive() {
        return Err("这支部队已经不存在了".into());
    }
    if u.commander.fate != crate::world::Fate::Active {
        return Err("主官已经不在了，无人可撤".into());
    }
    let old = u.commander.name.clone();
    let new_chief = format!("{old}（继任）");
    if old.contains("继任") {
        return Err("这支部队的主官已经是继任者了，再撤就没人带兵".into());
    }
    let fidx = faction_of(world, unit_idx);
    {
        let u = &mut world.units[unit_idx];
        // 继任者：完全服从的参谋。名字带"继任"，避免和原主官混淆，
        // 也挡住"反复撤同一个人"的取巧。
        let mut c = Commander::default_for(&new_chief);
        c.title = "代理主官".into();
        c.persona = "临时接任的参谋军官：熟悉条令，但不熟悉这支部队。".into();
        c.agenda = "不求有功，但求无过。".into();
        c.obedience = Fixed::ONE;
        c.initiative = Fixed::ratio(2, 10);
        c.aggression = Fixed::ratio(4, 10);
        c.caution = Fixed::ratio(7, 10);
        u.commander = c;
        u.disobeyed = false;
        u.dissent_reason = None;
    }
    if let Some(fi) = fidx {
        let fname = world.factions[fi].name.clone();
        let side = world.factions[fi].side;
        let f = &mut world.factions[fi];
        f.influence = (f.influence - Fixed::ratio(PURGE_COST.0, PURGE_COST.1))
            .clamp(Fixed::ZERO, Fixed::ONE);
        let unit_name = world.units[unit_idx].name.clone();
        world.dismissed.push((unit_name, old.clone(), world.tick));
        world.report_for(
            EventKind::Oob,
            format!("{fname} 出身的 {old} 被去职，{fname} 上下震动"),
            side,
        );
    } else {
        let unit_name = world.units[unit_idx].name.clone();
        let side = world.units[unit_idx].side;
        world.dismissed.push((unit_name, old.clone(), world.tick));
        world.report_for(EventKind::Oob, format!("{old} 被去职，由参谋接任"), side);
    }
    Ok((old, new_chief))
}

/// 派系影响力怎么改一个将领的服从度。
///
/// 关着 politics 时必须返回 0（`effective_obedience` 会直接跳开这段）。
/// 规则：靠山比"本该有的分量"低 → 他有怨气，更可能抗命。
pub fn obedience_shift(world: &World, unit_idx: usize) -> Fixed {
    if !world.fx.politics {
        return Fixed::ZERO;
    }
    match faction_of(world, unit_idx) {
        Some(fi) => {
            let f = &world.factions[fi];
            // standing 为负（失势）时给出负的偏移：夹到 [-1, 1] 再乘系数。
            // （Fixed 没有一元负号，边界用 ZERO - ONE 表示。）
            let s = f.standing();
            let lo = Fixed::ZERO - Fixed::ONE;
            let s = if s > Fixed::ONE {
                Fixed::ONE
            } else if s < lo {
                lo
            } else {
                s
            };
            s * Fixed::ratio(RESENT.0, RESENT.1)
        }
        None => Fixed::ZERO,
    }
}

/// 派系状态进哈希。只有 `Features::politics` 开启时才被调用。
///
/// 顺序即协议：先派系数、再逐个（id 字符串 + 影响力），最后是去职名单。
pub fn hash_into(h: &mut Hasher, w: &World) {
    h.write_u32(w.factions.len() as u32);
    for f in &w.factions {
        h.write_str(&f.id);
        h.write_i32(f.influence.raw());
    }
    h.write_u32(w.dismissed.len() as u32);
    for (unit, who, day) in &w.dismissed {
        h.write_str(unit);
        h.write_str(who);
        h.write_u32(*day);
    }
}
