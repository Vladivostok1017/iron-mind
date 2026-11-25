//! 情报系统：把"真实状态"变成"你看到的"。
//!
//! 核心原则：**玩家和将领都不是上帝**。
//! 模拟核心里有完整真值，但每个阵营只能看到自己侦察到的东西，
//! 而且看到的还有误差、会过期。
//!
//! 每 tick 的情报来源，按精度从高到低：
//!
//! 1. **战斗接触**——同省交战，敌军番号与规模全暴露（精确）
//! 2. **航空侦察**——空军单位半径内的敌方省（精确，但会随部队离开而衰减）
//! 3. **邻接观察**——己方单位相邻的省（近似，误差 ±40%）
//! 4. **传闻**——更远的省只知道自己有没有控制，兵力未知
//!
//! 所有情报超过 3 天没有更新就降一级，直到变回未知。
//! 这一条让"侦察"成为持续投入，而不是一次性开图。

use crate::fixed::Fixed;
use crate::rng::{stream, Rng};
use crate::world::{Accuracy, Side, World};

/// 近似情报的误差幅度（±40%）。
const APPROX_ERROR: i64 = 40;

/// 为双方刷新情报。
pub fn run_intel(world: &mut World) {
    let seed = world.seed;
    let tick = world.tick;
    for side in [Side::A, Side::B] {
        refresh_side(world, side, seed, tick);
    }
}

fn refresh_side(world: &mut World, side: Side, seed: u64, tick: u32) {
    let enemy = side.opponent();
    let n = world.provinces.len();
    let comms = world.comms;

    // ---- 第一步：送达已在路上的报告 ----
    // 先收后写，避免与后面的生成逻辑互相干扰。
    let mut delivered: Vec<(u32, Fixed, Accuracy, bool)> = Vec::new();
    {
        let pending = world.pending_mut(side);
        let mut still_on_road = Vec::with_capacity(pending.len());
        for r in pending.drain(..) {
            if r.arrive_tick <= tick {
                delivered.push((r.province, r.enemy_strength, r.accuracy, r.identity_known));
            } else {
                still_on_road.push(r);
            }
        }
        *pending = still_on_road;
    }
    {
        let intel = world.intel_mut(side);
        for (p, est, acc, ident) in delivered {
            let slot = &mut intel[p as usize];
            if acc > slot.accuracy || (acc == slot.accuracy && tick >= slot.seen_tick) {
                slot.enemy_strength = est;
                slot.accuracy = acc;
                slot.seen_tick = tick;
                slot.identity_known = ident;
            }
        }
    }

    // ---- 第二步：生成本 tick 的新观察，按来源计算送达延迟 ----
    // 元组：(省, 兵力估计, 精度, 识别番号, 延迟天数)
    let mut fresh: Vec<(u32, Fixed, Accuracy, bool, u32)> = Vec::new();

    // 来源 1：战斗接触（本单位视野，延迟最小）
    for p in 0..n {
        let units = world.units_in(p as u32);
        let has_mine = units.iter().any(|&i| world.units[i as usize].side == side);
        let has_enemy = units.iter().any(|&i| world.units[i as usize].side == enemy);
        if has_mine && has_enemy {
            let est = world.strength_at(p as u32, enemy);
            // 番号要**接触两次**才认得出：第一次只确定"对面有多少人"，
            // 番号得等俘虏、缴获文件、下一次接触去核对。
            // 以前第一次接触就直接给"精确 + 番号已知"，情报系统没有过程。
            // "之前就知道点什么"才算第一次之后：seen_tick 在 tick 0 时是 0，
            // 用它判断"接触过没有"会永远为假。用精度更可靠。
            let seen_before = world.intel(side)[p].accuracy != Accuracy::Unknown;
            fresh.push((p as u32, est, Accuracy::Exact, seen_before, comms.report_delay()));
        }
    }

    // 来源 2：航空侦察（精确，但报告要汇总）
    for u in world.units.iter() {
        if u.side != side || !u.is_alive() || u.recon_radius == 0 {
            continue;
        }
        for p in within_radius(world, u.position, u.recon_radius) {
            if world.provinces[p as usize].owner == side {
                continue;
            }
            let est = world.strength_at(p, enemy);
            if est.raw() > 0 {
                // 空中侦察一次就认得出番号：飞机看得见番号旗与队形
                fresh.push((p, est, Accuracy::Exact, true, comms.report_delay()));
            }
        }
    }

    // 来源 3：邻接观察（近似，靠地面传令）
    for u in world.units.iter() {
        if u.side != side || !u.is_alive() || !u.kind.is_ground() {
            continue;
        }
        let neighbors = world.provinces[u.position as usize].neighbors.clone();
        for nb in neighbors {
            if world.provinces[nb as usize].owner == side {
                continue;
            }
            let est = world.strength_at(nb, enemy);
            if est.raw() == 0 {
                fresh.push((nb, Fixed::ZERO, Accuracy::Approximate, false, comms.report_delay()));
            } else {
                let jittered = jitter(est, seed, tick, nb);
                fresh.push((nb, jittered, Accuracy::Approximate, false, comms.report_delay()));
            }
        }
    }

    // ---- 第三步：投递 ----
    // 延迟为 0 的立即入视图，其余进"在路上"队列。
    {
        let intel = world.intel_mut(side);
        let mut delayed: Vec<crate::world::PendingReport> = Vec::new();

        for (p, est, acc, ident, delay) in fresh {
            if delay == 0 {
                let slot = &mut intel[p as usize];
                if acc > slot.accuracy || (acc == slot.accuracy && tick >= slot.seen_tick) {
                    slot.enemy_strength = est;
                    slot.accuracy = acc;
                    slot.seen_tick = tick;
                    slot.identity_known = ident;
                }
            } else {
                delayed.push(crate::world::PendingReport {
                    arrive_tick: tick + delay,
                    province: p,
                    enemy_strength: est,
                    accuracy: acc,
                    identity_known: ident,
                });
            }
        }

        if !delayed.is_empty() {
            let pending = world.pending_mut(side);
            // 同一省只保留"最早到达"的那份。
            //
            // 不能简单用新报告覆盖旧的：每 tick 都会重新侦察，
            // 新报告的到达时间更晚，会把在路上的旧报告无限推后，
            // 结果永远送不到。
            for d in delayed {
                match pending.iter_mut().find(|r| r.province == d.province) {
                    Some(existing) => {
                        if d.arrive_tick < existing.arrive_tick {
                            *existing = d;
                        }
                    }
                    None => pending.push(d),
                }
            }
        }
    }

    // ---- 第四步：时效衰减 ----
    let intel = world.intel_mut(side);
    for slot in intel.iter_mut() {
        slot.decay(tick);
    }
}

/// 给估计值加误差抖动（±40%）。用确定性 RNG，保证可重放。
fn jitter(value: Fixed, seed: u64, tick: u32, province: u32) -> Fixed {
    let mut rng = Rng::derive(seed, stream::EVENTS, tick, province);
    let span = APPROX_ERROR * 2 + 1;
    let delta = rng.range_i32(-(APPROX_ERROR as i32), APPROX_ERROR as i32);
    let _ = span;
    let factor = Fixed::from_int(100 + delta) / Fixed::from_int(100);
    (value * factor).max(Fixed::ZERO)
}

/// 从某省出发 `r` 跳以内可达的所有省（含自身）。
fn within_radius(world: &World, origin: u32, r: u32) -> Vec<u32> {
    let n = world.provinces.len();
    let mut seen = vec![false; n];
    let mut out = Vec::new();
    if (origin as usize) >= n {
        return out;
    }
    seen[origin as usize] = true;
    out.push(origin);

    let mut frontier = vec![origin];
    for _ in 0..r {
        let mut next = Vec::new();
        for cur in frontier {
            for &nb in &world.provinces[cur as usize].neighbors {
                if !seen[nb as usize] {
                    seen[nb as usize] = true;
                    out.push(nb);
                    next.push(nb);
                }
            }
        }
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    out
}

/// 情报摘要，给 UI 与战报用。
#[derive(Clone, Debug)]
pub struct IntelSummary {
    /// 已知有敌军的省数。
    pub known_contacts: u32,
    /// 其中精确掌握的省数。
    pub exact_contacts: u32,
    /// 对敌军总兵力的估计。
    pub enemy_total_estimate: Fixed,
}

pub fn summarize(world: &World, side: Side) -> IntelSummary {
    let intel = world.intel(side);
    let mut contacts = 0;
    let mut exact = 0;
    let mut total = Fixed::ZERO;
    for i in intel {
        if i.enemy_strength.raw() > 0 && i.is_known() {
            contacts += 1;
            if i.accuracy == Accuracy::Exact {
                exact += 1;
            }
            total += i.enemy_strength;
        }
    }
    IntelSummary {
        known_contacts: contacts,
        exact_contacts: exact,
        enemy_total_estimate: total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Province, Terrain, Unit};

    fn two_side_map() -> World {
        let provinces = vec![
            Province::new("A", Terrain::Plain, Side::A)
                .with_neighbors(&[1])
                .as_source(),
            Province::new("B", Terrain::Plain, Side::A).with_neighbors(&[0, 2]),
            Province::new("C", Terrain::Plain, Side::B)
                .with_neighbors(&[1])
                .as_source(),
        ];
        let units = vec![
            Unit::new("cn", Side::A, 0, 10),
            Unit::new("jp", Side::B, 2, 10),
        ];
        World::new(1, provinces, units)
    }

    #[test]
    fn adjacent_enemy_is_detected_approximately() {
        let mut w = two_side_map();
        w.comms = crate::world::CommsConfig::instant();
        // 把国军挪到 B，邻接日军所在的 C
        w.units[0].position = 1;
        run_intel(&mut w);
        let intel = w.intel(Side::A);
        assert!(
            intel[2].is_known(),
            "邻接的敌军应当被侦察到，实际 {:?}",
            intel[2].accuracy
        );
    }

    #[test]
    fn far_enemy_stays_unknown() {
        let mut w = two_side_map();
        w.comms = crate::world::CommsConfig::instant();
        // 国军在 A，离 C 两跳
        run_intel(&mut w);
        let intel = w.intel(Side::A);
        assert_eq!(intel[2].accuracy, Accuracy::Unknown, "远处敌军不应被看到");
    }

    #[test]
    fn identity_needs_two_contacts() {
        let mut w = two_side_map();
        w.comms = crate::world::CommsConfig::instant();
        w.units[0].position = 1;
        w.units[1].position = 1;

        run_intel(&mut w);
        let first = w.intel(Side::A)[1];
        assert_eq!(first.accuracy, Accuracy::Exact, "第一次接触就该知道兵力");
        assert!(!first.identity_known, "第一次接触不该认得番号");

        w.tick += 1;
        run_intel(&mut w);
        let second = w.intel(Side::A)[1];
        assert!(second.identity_known, "第二次接触才认出番号");
    }
    #[test]
    fn combat_gives_exact_intel() {
        let mut w = two_side_map();
        w.comms = crate::world::CommsConfig::instant();
        // 双方挤在 B 交火
        w.units[0].position = 1;
        w.units[1].position = 1;
        run_intel(&mut w);
        let intel = w.intel(Side::A);
        assert_eq!(intel[1].accuracy, Accuracy::Exact, "战斗接触应当精确");
    }

    #[test]
    fn recon_aircraft_sees_farther() {
        let mut w = two_side_map();
        w.comms = crate::world::CommsConfig::instant();
        // 给国军一架侦察机，半径 2
        w.units[0].recon_radius = 2;
        run_intel(&mut w);
        let intel = w.intel(Side::A);
        assert_eq!(intel[2].accuracy, Accuracy::Exact, "侦察机应当看到远程敌军");
    }

    #[test]
    fn reports_arrive_after_delay() {
        // 通信延迟：情报发现了，但报告还在路上，上级此时什么都不知道。
        let mut w = two_side_map();
        w.comms = crate::world::CommsConfig {
            kind: crate::world::CommsKind::WireTelegraph,
            transmission: 0,
            staff: 2,
            order_delay: 0,
        };
        w.units[0].position = 1;

        run_intel(&mut w);
        assert!(!w.intel(Side::A)[2].is_known(), "报告未送达前不应可见");
        assert_eq!(w.pending_count(Side::A), 1, "应有一份在途报告");

        w.tick = 1;
        run_intel(&mut w);
        assert!(!w.intel(Side::A)[2].is_known(), "第 1 天仍未送达");

        w.tick = 2;
        run_intel(&mut w);
        assert!(w.intel(Side::A)[2].is_known(), "第 2 天应送达");
        // 旧的已送达并被消费，只剩本 tick 新签发的一份`n        assert!(w.pending_count(Side::A) <= 1);
    }

    #[test]
    fn intel_decays_without_refresh() {        let mut w = two_side_map();
        w.comms = crate::world::CommsConfig::instant();
        w.units[0].position = 1;
        run_intel(&mut w); // 首次侦察
        let first = w.intel(Side::A)[2].accuracy;
        assert!(first > Accuracy::Unknown);

        // 国军撤走，不再有观察
        w.units[0].position = 0;
        for _ in 0..10 {
            w.tick += 1;
            run_intel(&mut w);
        }
        assert_eq!(
            w.intel(Side::A)[2].accuracy,
            Accuracy::Unknown,
            "长期无侦察的情报应当失效"
        );
    }
}










