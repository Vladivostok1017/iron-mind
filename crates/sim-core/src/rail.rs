//! 铁路被炸与抢修（预览版）。
//!
//! 只在模块用 `scenario.preview.rail_damage = true` 显式开启时参与模拟。
//! 关闭时损坏恒为 0、补给损耗与定稿版本逐位一致。
//!
//! 设计意图：第一版里铁路只是一个布尔值——有它就省补给，没它就费。
//! 于是"炸桥""破路"这些 1938 年最要紧的战术在引擎里根本不存在，
//! 玩家也没有任何手段去断敌人的补给线。这一版把它变成**可破坏、可修复**的状态：
//!
//! - 空军可以炸（`Order::BombRail`），每炸一次 +1 级，3 级 = 断；
//! - 断了就退回普通陆路的损耗（0.75），前线补给立刻塌一截；
//! - 有兵驻守的省可以就地抢修，工兵专门抢修快一倍。
//!
//! 为什么是 3 级而不是"断/通"两态：一段铁路不是一次轰炸就能彻底废掉的，
//! 反过来也不是修一次就完好。中间那两级让"反复争夺"有意义。

use crate::fixed::Fixed;
use crate::world::{EventKind, Order, Side, UnitKind, World};

/// 修好一级需要攒几天。工兵（`Order::RepairRail`）按两倍速度攒。
pub const REPAIR_DAYS: u8 = 2;

/// 损坏上限：3 级 = 断。
pub const MAX_DAMAGE: u8 = 3;

/// 每天结算一次：炸路、抢修。
pub fn run_rail(world: &mut World) {
    if !world.fx.rail_damage {
        return;
    }
    run_bombing(world);
    run_repair(world);
}

/// 空军执行 `BombRail`：把目标省的铁路炸坏一级。
///
/// 一次轰炸**只炸一级**，而且已经有 3 级时不再累加——
/// 这样"炸断"要飞三个架次，也给守方留出抢修的时间窗口。
fn run_bombing(world: &mut World) {
    let mut hits: Vec<(usize, u32)> = Vec::new();
    for i in 0..world.units.len() {
        let u = &world.units[i];
        if !u.is_alive() || u.kind != UnitKind::Air {
            continue;
        }
        if let Order::BombRail(prov) = u.order {
            // 目标必须真的在这支空军的作战半径内，否则等于全球打击。
            let Ok(dist) = world.hops_between(u.position, prov) else {
                continue;
            };
            if dist == 0 || u.recon_radius == 0 || dist > u.recon_radius {
                continue;
            }
            hits.push((i, prov));
        }
    }
    for (i, prov) in hits {
        let p = match world.provinces.get_mut(prov as usize) {
            Some(p) => p,
            None => continue,
        };
        if !p.rail || p.rail_damage >= MAX_DAMAGE {
            continue;
        }
        p.rail_damage += 1;
        p.rail_repair = 0;
        let (name, dmg) = (world.provinces[prov as usize].name.clone(), world.provinces[prov as usize].rail_damage);
        let unit = world.units[i].name.clone();
        let side = world.units[i].side;
        let what = if dmg >= MAX_DAMAGE {
            "铁路被炸断"
        } else {
            "铁路被炸坏一段"
        };
        world.report_for(
            EventKind::Intel,
            format!("{unit} 空袭 {name}，{what}（{dmg}/{MAX_DAMAGE}）"),
            side,
        );
    }
}

/// 抢修：本省有本方地面部队就能修，专门的抢修命令快一倍。
///
/// 为什么不需要"工兵单位"这个新兵种：1938 年铁路抢修靠的是沿线驻军、
/// 铁路员工和民夫，不是编制表里的工程兵。让"有兵在"就是"有人修"，
/// 既贴近史实，也不需要内容层多写一类单位。
fn run_repair(world: &mut World) {
    for p in 0..world.provinces.len() {
        if !world.provinces[p].rail || world.provinces[p].rail_damage == 0 {
            continue;
        }
        let owner = world.provinces[p].owner;
        // 谁在这修：本省归属方（或已经占住这里的部队）的地面单位
        let mut helpers = 0u8;
        for i in 0..world.units.len() {
            let u = &world.units[i];
            if !u.is_alive() || !u.kind.is_ground() || u.position != p as u32 {
                continue;
            }
            if u.side != owner {
                continue;
            }
            // 正在这里打仗就顾不上修
            let contested = world.units_in(p as u32).iter().any(|&j| {
                let v = &world.units[j as usize];
                v.is_alive() && v.kind.is_ground() && v.side != owner
            });
            if contested {
                continue;
            }
            helpers += if matches!(u.order, Order::RepairRail(_)) {
                2
            } else {
                1
            };
        }
        if helpers == 0 {
            continue;
        }
        let p_ref = &mut world.provinces[p];
        p_ref.rail_repair = p_ref.rail_repair.saturating_add(helpers);
        if p_ref.rail_repair >= REPAIR_DAYS {
            p_ref.rail_repair = 0;
            p_ref.rail_damage -= 1;
            let (name, dmg) = (world.provinces[p].name.clone(), world.provinces[p].rail_damage);
            let msg = if dmg == 0 {
                format!("{name} 铁路抢修完毕，全线恢复通车")
            } else {
                format!("{name} 铁路抢修推进（还剩 {dmg}/{MAX_DAMAGE}）")
            };
            world.report_for(EventKind::Supply, msg, owner);
        }
    }
}

impl World {
    /// 两省之间的跳数（用于空军作战半径判定）。走的是与地面相同的邻接图。
    pub fn hops_between(&self, from: u32, to: u32) -> Result<u32, ()> {
        if from == to {
            return Ok(0);
        }
        let n = self.provinces.len();
        let mut dist = vec![u32::MAX; n];
        let mut q = std::collections::VecDeque::new();
        dist[from as usize] = 0;
        q.push_back(from);
        while let Some(cur) = q.pop_front() {
            let d = dist[cur as usize];
            for &nb in &self.provinces[cur as usize].neighbors {
                if dist[nb as usize] != u32::MAX {
                    continue;
                }
                dist[nb as usize] = d + 1;
                q.push_back(nb);
            }
        }
        let d = dist[to as usize];
        if d == u32::MAX {
            Err(())
        } else {
            Ok(d)
        }
    }
}

/// 供界面/服务端使用的"这条线还能不能运量"系数（1.0 = 完好）。
pub fn rail_throughput(world: &World, p: u32) -> Fixed {
    let dmg = world.provinces[p as usize].rail_damage;
    if dmg >= MAX_DAMAGE {
        return Fixed::ratio(75, 100); // 断：退回普通陆路
    }
    Fixed::ratio(95, 100) - Fixed::ratio(18, 100) * Fixed::from_int(dmg as i32)
}

/// 该省铁路是不是断的（界面用）。
pub fn is_cut(world: &World, p: u32) -> bool {
    world.provinces[p as usize].rail_damage >= MAX_DAMAGE
}

/// 该省归谁修（界面用：不是自己人的线就不该显示"抢修中"）。
pub fn owner_of(world: &World, p: u32) -> Side {
    world.provinces[p as usize].owner
}
