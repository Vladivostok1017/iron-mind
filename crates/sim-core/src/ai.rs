//! 规则 AI：给每个单位一个意图。
//!
//! 第一版刻意**不接 LLM**。理由：先要验证"有指挥"和"没指挥"的差别有多大。
//! 如果纯规则都打不出变化，接 LLM 也没用。
//!
//! 四条规则，按优先级：
//!
//! 1. **兵力枯竭（< 35% 编制）→ 撤退**。保存老兵，这是整场战役的核心矛盾。
//! 2. **邻接有敌省，且我方战力 > 守军×工事×1.2 → 进攻**。
//! 3. **在己方省且打不动 → 固守**（吃工事加成，等援军）。
//! 4. **在敌省且打不动 → 撤回己方省**（不要卡在敌境被饿死）。
//!
//! 第 1、4 条是之前那版没有的。之前所有单位无脑冲，攻方一定会被拖死。

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::fixed::Fixed;
use crate::rng::{stream, Rng};
use crate::world::{Commander, Order, Side, Stance, Terrain, UnitKind, World};

/// 进入敌方控制省的额外代价。
///
/// 这一项让部队尽量沿己方控制的走廊行军，而不是直线穿敌。
/// 不是禁止穿越（唯一通道时仍可过），而是"能绕就绕"。
const ENEMY_ENTRY_PENALTY: u32 = 40;

/// 加权最短路：多源 Dijkstra。
///
/// 成本 = 地形成本 × 10 + 进入敌省的惩罚。
/// 相比原来的纯 BFS，这样部队会绕开山地和敌控区，
/// 而不是"直线穿过任何省"。
///
/// **水域不参与**：这个代价场只服务地面单位（`run_ai` 只给 `kind.is_ground()`
/// 的单位下令）。如果让水域进入松弛，地面单位会算出"顺长江而下"的最短路，
/// 到移动阶段又被 `can_enter` 挡住，表现为原地不动。
/// 省一两个水域省时看不出来，加长水道之后必然踩。
fn cost_field(world: &World, side: Side, sources: &[u32]) -> Vec<u32> {
    let n = world.provinces.len();
    let mut dist = vec![u32::MAX; n];
    let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();

    for &s in sources {
        if (s as usize) < n {
            dist[s as usize] = 0;
            heap.push(Reverse((0, s)));
        }
    }

    while let Some(Reverse((d, cur))) = heap.pop() {
        if d > dist[cur as usize] {
            continue;
        }
        let neighbors = world.provinces[cur as usize].neighbors.clone();
        for nb in neighbors {
            let np = &world.provinces[nb as usize];
            if np.terrain == Terrain::Water {
                continue;
            }
            // 地形移动成本（1.0 → 10，2.5 → 25）
            let terr = ((np.terrain.move_cost().raw() as u64 * 10) >> 16) as u32;
            let terr = terr.max(1);
            let penalty = if np.owner == side { 0 } else { ENEMY_ENTRY_PENALTY };
            let nd = d.saturating_add(terr + penalty);
            if nd < dist[nb as usize] {
                dist[nb as usize] = nd;
                heap.push(Reverse((nd, nb)));
            }
        }
    }

    dist
}

/// 到最近敌方控制省的成本场（多源）。
///
/// 水域省不算"敌方目标"——否则地面单位会朝江面推进。
fn enemy_cost_field(world: &World, side: Side) -> Vec<u32> {
    let sources: Vec<u32> = world
        .provinces
        .iter()
        .enumerate()
        .filter(|(_, p)| p.owner != side && p.terrain != Terrain::Water)
        .map(|(i, _)| i as u32)
        .collect();
    cost_field(world, side, &sources)
}

/// 兵力低于编制的这个比例就撤。
const EXHAUSTED_RATIO: Fixed = Fixed::ratio_const(35, 100);

/// 将领自己的主张。
///
/// 这是"独立思考"的落点：将领不是等着执行命令的机器人，
/// 他对战场有自己的判断，而且可能跟上级不一样。
///
/// 注意：**这段判断完全在引擎里，不交给 LLM**。
/// 原因很实际——大模型经过 RLHF 训练，天然倾向服从"用户的要求"，
/// 让它决定要不要抗命，它几乎永远会说"服从命令"。
/// 所以"会不会抗命"必须是确定性的规则，"为什么抗命"才交给 LLM 去写。
fn own_judgment(cmd: &Commander, ctx: &CommandContext) -> Option<Order> {
    // 好战且看到便宜：主张打
    if cmd.aggression > Fixed::ratio(65, 100) && ctx.has_weak_enemy {
        if let Some(target) = ctx.weakest_neighbor {
            return Some(Order::Attack(target));
        }
    }
    // 谨慎且劣势：主张守
    if cmd.caution > Fixed::ratio(70, 100) && ctx.outmatched {
        return Some(Order::Hold);
    }
    None
}

/// 将领对命令的解释。
///
/// 这是"意图式指挥"的核心机制：上级下达 `intended`，将领结合自己的性格
/// 和当下处境，给出**实际执行**的 `actual`。
///
/// 三条约束，保证"将在外"不至于离谱：
/// 1. 将领只能改方向，不能改阵营（永远不会投敌）。
/// 2. 抗命只会朝"更保守"或"更激进"两个方向偏，不会产生无意义动作。
/// 3. 抗命概率受 `obedience` 硬约束，服从度高的将领基本不会抗命。
fn interpret(
    cmd: &Commander,
    intended: Order,
    ctx: &CommandContext,
    rng: &mut Rng,
) -> (Order, Option<&'static str>) {
    let roll = rng.unit_fixed();

    // ---- 独立思考 ----
    // 将领先形成自己的主张。如果与上级命令不同，且他的"自主性"够强，
    // 就会按自己的想法来。这是"将在外"的主动形态，而不是被动抗命。
    if let Some(own) = own_judgment(cmd, ctx) {
        if own != intended {
            let dissent = (Fixed::ONE - cmd.obedience) * cmd.initiative;
            if roll < dissent {
                let reason = match own {
                    Order::Attack(_) => "战机可乘，本官自作主张",
                    Order::Hold => "势不可为，按兵不动",
                    _ => "本官另有计较",
                };
                return (own, Some(reason));
            }
            // 顺从了上级，但记录下一次"想抗命"的摩擦
        }
    }

    match intended {
        // 劣势强攻：谨慎的将领倾向按下不动，等增援。
        Order::Attack(_) if ctx.outmatched => {
            let resist = (Fixed::ONE - cmd.obedience) * cmd.caution * Fixed::ratio(15, 10)
                + cmd.initiative * Fixed::ratio(3, 10);
            if roll < resist {
                return (Order::Hold, Some("兵力不足，未敢强攻"));
            }
        }
        // 固守时看到战机：好战的将领可能主动出击。
        Order::Hold if ctx.has_weak_enemy => {
            if cmd.aggression > Fixed::ratio(7, 10) {
                let eager = (cmd.aggression - Fixed::ratio(7, 10))
                    * (Fixed::ONE - cmd.obedience)
                    * Fixed::ratio(6, 10);
                if roll < eager {
                    if let Some(target) = ctx.weakest_neighbor {
                        return (Order::Attack(target), Some("战机难得，主动出击"));
                    }
                }
            }
        }
        // 撤退命令：好战又不服的将领可能抗命死守。
        Order::Retreat(_) => {
            if cmd.aggression > Fixed::ratio(6, 10) {
                let refuse = (Fixed::ONE - cmd.obedience) * cmd.aggression * Fixed::ratio(4, 10);
                if roll < refuse {
                    return (Order::Hold, Some("未奉撤退之命，死守此地"));
                }
            }
        }
        _ => {}
    }

    (intended, None)
}

/// 将领做判断时需要的处境信息。
struct CommandContext {
    /// 我方战力是否明显不足以完成进攻。
    outmatched: bool,
    /// 邻接是否有"很弱"的敌人（主动出击的诱因）。
    has_weak_enemy: bool,
    weakest_neighbor: Option<u32>,
}

/// 给非玩家阵营的"假指挥官"：统帅部的作战方针。
///
/// 之前非玩家阵营是**没有指挥部的**：八个师团各自看一眼邻省、各自决定打不打，
/// 行动像八个独立军阀，而不是一支有作战方针的军。史实里第 11 军司令部是有的
/// （"沿南浔线南下、相机夺取南昌"），而在模拟里它不存在。
///
/// 这里补的是一份**确定性、不额外花钱**的方针：只看战场事实
/// （兵力比、目标价值、够不够得着），不叫第二次模型。
/// 规则 AI 直接照它办事；LLM 将领在提示词里看到它，然后**可以不同意**——
/// 这才是真实指挥链：司令部给方向，师团长给判断。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TheaterPlan {
    /// 主要用兵方向（按优先级排序的目标省）。
    pub focus: Vec<u32>,
    pub posture: Posture,
    /// 一句话说明，直接进提示词。
    pub summary: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Posture {
    /// 全面推进
    Advance,
    /// 巩固现有战线（实力不足或补给不济）
    Hold,
    /// 收缩（被打残了）
    Withdraw,
}

impl Posture {
    pub fn label(self) -> &'static str {
        match self {
            Posture::Advance => "推进",
            Posture::Hold => "巩固",
            Posture::Withdraw => "收缩",
        }
    }
}

/// 算出某一方的作战方针。
///
/// 判据三条，全部来自战场事实，不掺任何国别设定：
/// 1. 现存兵力比决定姿态（0.8 倍以上才全面推进）；
/// 2. 目标是敌方控制的省，价值 = 补给源 3 + 铁路 2 + 工事 1；
/// 3. 只考虑**够得着**的（邻接本方部队），同级按省号保序。
pub fn theater_plan(world: &World, side: Side) -> TheaterPlan {
    let enemy = side.opponent();
    // 用**有效战斗力**而不是人头：历史上进攻方普遍人数劣势
    // （淞沪 1:2.6），只看人头会让入侵者永远被建议"收缩"。
    let own_power = world.total_combat_power(side);
    let foe_power = world.total_combat_power(enemy);

    // 阈值也按有效战力重标：1.0 / 0.7 一档。
    // 原来的 1.25 / 2.0 倍人头口径，在"人少但精锐"的进攻方身上永远判成收缩。
    let posture = if own_power.is_zero() || foe_power.is_zero() {
        Posture::Hold
    } else if own_power > foe_power {
        Posture::Advance
    } else if own_power * Fixed::ratio(7, 10) > foe_power {
        Posture::Hold
    } else {
        Posture::Withdraw
    };

    let mut cands: Vec<(u32, i32)> = Vec::new();
    for p in 0..world.provinces.len() {
        let prov = &world.provinces[p];
        if prov.owner == side || prov.terrain == Terrain::Water {
            continue;
        }
        let reachable = prov.neighbors.iter().any(|&nb| {
            world.units_in(nb).iter().any(|&j| {
                let x = &world.units[j as usize];
                x.side == side && x.kind.is_ground() && x.is_alive()
            })
        });
        if !reachable {
            continue;
        }
        let mut value = 0i32;
        if prov.is_supply_source {
            value += 3;
        }
        if prov.rail {
            value += 2;
        }
        if prov.fort > 0 {
            value += 1;
        }
        cands.push((p as u32, value));
    }
    cands.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let focus: Vec<u32> = cands.iter().take(2).map(|c| c.0).collect();

    let names: Vec<String> = focus
        .iter()
        .map(|&p| world.provinces[p as usize].name.clone())
        .collect();
    let ratio = if foe_power.is_zero() {
        9.99
    } else {
        own_power.to_display_f64() / foe_power.to_display_f64()
    };
    let summary = if names.is_empty() {
        format!("{}：当面没有可攻目标，守住现有战线。", posture.label())
    } else {
        format!(
            "{}：主攻方向 {}（兵力比 {:.2}）。",
            posture.label(),
            names.join("、"),
            ratio
        )
    };

    TheaterPlan { focus, posture, summary }
}

/// 每 tick 为双方重算意图。
pub fn run_ai(world: &mut World) {
    let seed = world.seed;
    let tick = world.tick;
    for side in [Side::A, Side::B] {
        decide_side(world, side, seed, tick);
    }
}

fn decide_side(world: &mut World, side: Side, seed: u64, tick: u32) {
    let enemy_field = enemy_cost_field(world, side);

    let my_units: Vec<usize> = (0..world.units.len())
        .filter(|&i| {
            let u = &world.units[i];
            u.side == side && u.is_alive() && u.kind.is_ground()
                && !u.manual
        })
        .collect();

    let mut decisions: Vec<(usize, Order)> = Vec::with_capacity(my_units.len());

    for &i in &my_units {
        let u = &world.units[i];
        let pos = u.position;
        let exhausted = u.strength < u.max_strength * EXHAUSTED_RATIO;
        let neighbors = world.provinces[pos as usize].neighbors.clone();

        // 找最值得打的邻接敌省
        let (weakest, empty) = pick_target(world, &neighbors, side, u.kind);

        // ---- 先把手里的省吃下来 ----
        //
        // 站在一个**还没归我**的省里、省里又没有敌军：那就别急着走。
        // 不走才轮得到占领判定（要"邻接没有敌军"才生效）。
        //
        // 没有这条会发生什么（实测）：第101师团在星子/隘口之间一天掉一次头，
        // 连着二十天，两个省都还是国军的——因为每次它刚到，旁边有我们的
        // 部队，占不下来；下一 tick 它又挑另一个空省走。玩家看到的是
        // "敌军在我后院里跳舞，旗子却一直没换"。
        let here = &world.provinces[pos as usize];
        let enemy_here = world.units_in(pos).iter().any(|&j| {
            let x = &world.units[j as usize];
            x.side != side && x.kind.is_ground() && x.is_alive()
        });
        // 只有"守得住"才值得停下来等占领：邻接省里还有敌军时，
        // 占领判定本来就不会生效（见 run_movement 里的 enemy_adjacent），
        // 停下来就是干等——那种情况应该去打旁边那支敌军。
        let other = if side == Side::A { Side::B } else { Side::A };
        let capture_blocked = here.neighbors.iter().any(|&n| {
            world.units_in(n).iter().any(|&j| {
                let x = &world.units[j as usize];
                x.side == other && x.kind.is_ground() && x.is_alive()
            })
        });
        let need_consolidate = here.owner != side && !enemy_here && !capture_blocked;

        // ---- 上级意图（统帅部） ----
        //
        // 关键：**统帅部不管兵力够不够，只给方向**。
        // 打不打得动、要不要抗命，是将领的判断（见 `interpret`）。
        //
        // 之前这里由 AI 自己判断"打不过就不打"，结果将领永远没有抗命的理由——
        // 因为它跟自己完全一致。分了这两级，才有"上级要打、我按兵不动"的空间。
        let intended = if enemy_here {
            // 敌军就在这个省里：**打完再说**。
            //
            // 少了这条会看到什么（实测）：第101师团追着国军进星子，
            // 一进去就"挑下一个空省"走到隘口，回头看见星子有守军又走回来——
            // 一天掉一次头，连着二十天，两边都在挨打却谁也打不着谁。
            // 挨着打的时候不许换目标，这是常识。
            Order::Attack(pos)
        } else if need_consolidate && !exhausted {
            // 站在还没归我的省里，先守住，等占领判定生效
            Order::Hold
        } else if exhausted {
            // 打残了：这是将领自己的保存实力判断，优先级最高
            match retreat_target(world, &neighbors, side, &enemy_field) {
                Some(dst) => Order::Retreat(dst),
                None => Order::Hold,
            }
        } else if u.stance == Stance::Defend {
            // 防守姿态：**不主动开拔**。
            //
            // 以前的写法是"邻接没有敌人就朝最近的敌人行军"，防守方于是主动
            // 放弃阵地去迎敌，兵力再多也守不住。现在只做一件事：守住。
            //
            // 为什么不顺便"去占回空的敌省"：那样所有防守单位会同时扑向
            // 同一个空省，形成七支部队挤在一个山头的堆叠。反攻是玩家的决定。
            //
            // 注意这不是"不许动"：将领的抗命逻辑仍然可以在战机出现时出击。
            //
            // 闲着就挖工事。守方唯一的"生产性动作"就是这个，
            // 否则不打仗的日子全是空转。
            //
            // 条件里**不要求**"邻接没有敌人"：敌军还在隔壁省时正是该修工事的时候。
            // 只有敌军已经进了同一个省才挖不动——那一条由
            // `apply_stationary_orders` 里的 under_fire 判定。
            let node_fort = world.provinces[pos as usize]
                .position_at(u.node)
                .map(|p| p.fort)
                .unwrap_or(0);
            if node_fort < 3 {
                Order::Fortify
            } else {
                Order::Hold
            }
        } else {
            // 有敌人就打敌人；没有敌人才去占空省。
            //
            // 顺序颠倒过一次，后果很严重：以前按"兵力估计最小"挑目标，
            // 而空省永远是 0，于是日军一路绕开国军去涂空省——60 天里
            // 一场陆战都没打，战线在第 5 天就冻住，双方只是被非战斗减员
            // 慢慢磨。进攻方的目标必须是**对方的部队**，不是地图颜色。
            match weakest {
                Some((target, _)) => Order::Attack(target),
                None => match empty {
                    Some(target) => Order::Move(target),
                    None => match advance_target(&neighbors, &enemy_field) {
                        Some(dst) => Order::Move(dst),
                        None => Order::Hold,
                    },
                },
            }
        };

        decisions.push((i, intended));
    }

    // ---- 将领解释 ----
    // 上面的 `order` 是"上级意图"，现在交给将领过一遍。
    // 同一个进攻命令，谨慎的将领可能按下，好战的将领可能加倍。
    let mut final_decisions: Vec<(usize, Order, Option<&'static str>)> =
        Vec::with_capacity(decisions.len());

    for (i, intended) in decisions {
        let u = &world.units[i];
        let power = u.combat_power();
        let neighbors = world.provinces[u.position as usize].neighbors.clone();

        // 和上级意图用同一个目标口径：先打有敌人的省，再考虑空省
        let (weakest, _) = pick_target(world, &neighbors, side, u.kind);

        // 进攻方需要明显优势才"打得动"，所以判定门槛要留余量：
        // 上级下令进攻时，只要我方战力不到守军的 1.3 倍（含工事折算），
        // 将领就有理由认为这一仗不该打。
        let outmatched = match weakest {
            Some((t, ep)) => {
                let def = world.provinces[t as usize].defense_multiplier();
                power < ep * def * Fixed::ratio(13, 10)
            }
            None => false,
        };
        let has_weak_enemy = weakest
            .map(|(_, ep)| ep.raw() > 0 && ep < power * Fixed::ratio(5, 10))
            .unwrap_or(false);

        let ctx = CommandContext {
            outmatched,
            has_weak_enemy,
            weakest_neighbor: weakest.map(|(t, _)| t),
        };

        let mut rng = Rng::derive(seed, stream::AI, tick, i as u32);
        // 兵团部在不在，会影响实际服从度（见 World::effective_obedience）
        let mut cmd = u.commander.clone();
        cmd.obedience = world.effective_obedience(i);
        let (actual, reason) = interpret(&cmd, intended, &ctx, &mut rng);
        final_decisions.push((i, actual, reason));
    }

    for (i, o, reason) in final_decisions {
        let u = &mut world.units[i];
        u.order = o;
        u.disobeyed = reason.is_some();
        u.dissent_reason = reason.map(|s| s.to_string());
        if reason.is_some() {
            u.dissent_count += 1;
            let name = u.name.clone();
            let cmd = u.commander.name.clone();
            let why = u.dissent_reason.clone().unwrap_or_default();
            let side = u.side;
            // 抗命是内部通报：只有这一方的玩家能看到，敌方将领的抱怨不该出现在我方战报里
            world.report_for(
                crate::world::EventKind::Dissent,
                format!("{name}（{cmd}）{why}"),
                side,
            );
        }
    }
}

/// 撤退目标：己方控制的邻接省中，离敌人最远的那个。
fn retreat_target(world: &World, neighbors: &[u32], side: Side, field: &[u32]) -> Option<u32> {
    let mut best: Option<(u32, u32)> = None; // (dist, province)
    for &nb in neighbors {
        if world.provinces[nb as usize].owner != side {
            continue;
        }
        let d = field[nb as usize];
        if d == u32::MAX {
            continue;
        }
        if best.map_or(true, |(bd, _)| d > bd) {
            best = Some((d, nb));
        }
    }
    best.map(|(_, nb)| nb)
}

/// 挑目标：**先打有敌人的省，再考虑空省**。
///
/// 返回 `(有敌人的最弱邻省, 第一个可进的空邻省)`。
///
/// 为什么要把这两种分开：以前的写法是"按兵力估计挑最小的"，而空省永远
/// 是 0，于是进攻方一路绕开守军去涂空省——60 天里一场陆战都没打，
/// 战线第 5 天就冻住，双方只是被非战斗减员磨死。
/// 进攻方的目标是**对方的部队**，不是地图上的颜色。
fn pick_target(
    world: &World,
    neighbors: &[u32],
    side: Side,
    kind: UnitKind,
) -> (Option<(u32, Fixed)>, Option<u32>) {
    let mut weakest: Option<(u32, Fixed)> = None;
    let mut empty: Option<u32> = None;
    for &nb in neighbors {
        let np = &world.provinces[nb as usize];
        // 水域不是地面单位的目标。不加这一条，沿江的每一步都会让部队
        // "进攻"江面，到移动阶段被 `can_enter` 挡住，表现为原地不动。
        if np.owner == side || np.terrain == Terrain::Water {
            continue;
        }
        // 还得真的走得到。战线不连续时（比如目标只在湖对岸有我们的旗子）
        // 这条命令落不了地，部队会揣着一个永远走不到的目标干站着。
        if !crate::systems::can_step(world, side, kind, nb) {
            continue;
        }
        let enemy_present = world.units_in(nb).iter().any(|&j| {
            let x = &world.units[j as usize];
            x.side != side && x.kind.is_ground() && x.is_alive()
        });
        if enemy_present {
            let mut ep = world.enemy_estimate(side, nb); // 基于情报，不是真值
            // 没侦察到的敌军，估计值是 0。不能拿 0 当"这里没有守军"：
            // 那会让进攻方闭着眼睛撞进两三个师的阵地（实测：日军 60 天
            // 把自己打光，只剩 9.5 千）。按"至少一个师"折算，
            // 逼它先把侦察与判断做完再下手。
            if ep.is_zero() {
                ep = Fixed::from_int(10);
            }
            if weakest.map_or(true, |(_, p)| ep < p) {
                weakest = Some((nb, ep));
            }
        } else if empty.is_none() {
            empty = Some(nb);
        }
    }
    (weakest, empty)
}

/// 推进目标：邻接省中离敌人最近的那个。
fn advance_target(neighbors: &[u32], field: &[u32]) -> Option<u32> {
    step_toward(neighbors, field)
}

/// 朝距离场更小的方向走一步。
///
/// 这是"朝某省前进"的唯一实现，被 `advance_target` 与目标承诺共用。
fn step_toward(neighbors: &[u32], field: &[u32]) -> Option<u32> {
    let mut best: Option<(u32, u32)> = None;
    for &nb in neighbors {
        let d = field[nb as usize];
        if d == u32::MAX {
            continue;
        }
        if best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, nb));
        }
    }
    best.map(|(_, nb)| nb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Province, Terrain, Unit};

    fn line() -> World {
        // A(源) - B - C(源)
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
    fn units_get_advance_order_when_no_contact() {
        let mut w = line();
        w.comms = crate::world::CommsConfig::instant();
        crate::intel::run_intel(&mut w);
        run_ai(&mut w);
        // 双方都朝对方推进
        assert!(matches!(w.units[0].order, Order::Move(_)));
        assert!(matches!(w.units[1].order, Order::Move(_)));
    }

    #[test]
    fn stronger_unit_attacks() {
        let mut w = line();
        w.comms = crate::world::CommsConfig::instant();
        // 让国军强得多
        w.units[0].strength = Fixed::from_int(50);
        w.units[0].max_strength = Fixed::from_int(50);
        // 把国军挪到中间，日军的 B 邻接
        w.units[0].position = 1;
        crate::intel::run_intel(&mut w);
        run_ai(&mut w);
        let cn = &w.units[0];
        assert!(
            matches!(cn.order, Order::Attack(2)),
            "优势方应进攻，实际 {:?}",
            cn.order
        );
    }

    #[test]
    fn exhausted_unit_retreats() {
        let mut w = line();
        w.comms = crate::world::CommsConfig::instant();
        w.units[0].position = 1;
        w.units[0].strength = Fixed::from_int(2); // 编制 10，打残
        crate::intel::run_intel(&mut w);
        run_ai(&mut w);
        assert!(
            matches!(w.units[0].order, Order::Retreat(_)),
            "打残的单位应撤退，实际 {:?}",
            w.units[0].order
        );
    }

    #[test]
    fn cautious_commander_refuses_bad_attack() {
        // 谨慎且不服的将领，面对"劣势强攻"应当有一定概率按下不动
        let cmd = Commander::new("谨慎者", 0.2, 0.9, 0.4, 0.9);
        let ctx = CommandContext {
            outmatched: true,
            has_weak_enemy: false,
            weakest_neighbor: None,
        };
        let mut refused = 0;
        for seed in 0..200u64 {
            let mut rng = Rng::from_seed(seed);
            let (o, reason) = interpret(&cmd, Order::Attack(1), &ctx, &mut rng);
            if matches!(o, Order::Hold) && reason.is_some() {
                refused += 1;
            }
        }
        assert!(refused > 40, "应当经常抗命，实际 {refused}/200");
    }

    #[test]
    fn obedient_commander_always_obeys() {
        // 服从度 1.0 的将领永远不会抗命
        let cmd = Commander::new("绝对服从", 1.0, 0.0, 0.5, 0.9);
        let ctx = CommandContext {
            outmatched: true,
            has_weak_enemy: true,
            weakest_neighbor: Some(2),
        };
        for seed in 0..200u64 {
            let mut rng = Rng::from_seed(seed);
            let (o, reason) = interpret(&cmd, Order::Attack(1), &ctx, &mut rng);
            assert!(matches!(o, Order::Attack(_)), "服从度 1.0 不该抗命");
            assert!(reason.is_none());
        }
    }

    #[test]
    fn aggressive_commander_may_refuse_retreat() {
        // 好战又不服的将领可能抗命死守
        let cmd = Commander::new("悍将", 0.1, 0.8, 0.95, 0.2);
        let ctx = CommandContext {
            outmatched: false,
            has_weak_enemy: false,
            weakest_neighbor: None,
        };
        let mut refused = 0;
        for seed in 0..200u64 {
            let mut rng = Rng::from_seed(seed);
            let (o, reason) = interpret(&cmd, Order::Retreat(1), &ctx, &mut rng);
            if matches!(o, Order::Hold) && reason.is_some() {
                refused += 1;
            }
        }
        assert!(refused > 20, "悍将应当常抗命死守，实际 {refused}/200");
    }

    #[test]
    fn commander_never_changes_side() {
        // 无论怎么解释，意图的目标都必须是合法的省或固守，不能凭空产生
        let cmd = Commander::new("任意", 0.0, 1.0, 1.0, 1.0);
        let ctx = CommandContext {
            outmatched: true,
            has_weak_enemy: true,
            weakest_neighbor: Some(7),
        };
        for seed in 0..500u64 {
            let mut rng = Rng::from_seed(seed);
            let (o, _) = interpret(&cmd, Order::Attack(1), &ctx, &mut rng);
            match o {
                Order::Hold => {}
                Order::Attack(t) => assert!(t == 1 || t == 7),
                other => panic!("不应产生 {other:?}"),
            }
        }
    }

    #[test]
    fn obedient_commander_attacks_even_when_weak() {
        // 语义变更：现在"打不打得动"不再是 AI 的过滤条件，
        // 而是将领的判断。服从度高的将领即使劣势也会执行进攻。
        let mut w = line();
        w.comms = crate::world::CommsConfig::instant();
        w.units[1].strength = Fixed::from_int(60);
        w.units[1].max_strength = Fixed::from_int(60);
        w.units[0].position = 1; // 在己方 B，邻接敌省 C
        // 默认将领：完全服从
        crate::intel::run_intel(&mut w);
        run_ai(&mut w);
        assert!(
            matches!(w.units[0].order, Order::Attack(2)),
            "服从的将领应执行进攻，实际 {:?}",
            w.units[0].order
        );
    }

    #[test]
    fn field_positions_do_not_stall() {
        // 回归测试：不能全体原地不动。
        // （"目标=当前位置"只是移动完成的瞬间快照，不是 bug；
        //   真正的 bug 是连续多天位置完全不变。）
        use crate::scenario;
        use crate::systems;

        let mut w = scenario::nanxun_line(1);
        let mut still = 0;
        let mut prev: Vec<u32> = w.units.iter().map(|u| u.position).collect();
        for _ in 0..30 {
            systems::tick(&mut w);
            let now: Vec<u32> = w.units.iter().map(|u| u.position).collect();
            if now == prev {
                still += 1;
            }
            prev = now;
        }
        assert!(still < 25, "30 天里有 {still} 天完全没动，AI 可能僵死");
    }

    #[test]
    fn cautious_commander_holds_instead_of_dying() {
        // 同一场景，换一个谨慎且不服的将领：他会拒绝这次进攻。
        let mut base = line();
        base.comms = crate::world::CommsConfig::instant();
        base.units[1].strength = Fixed::from_int(60);
        base.units[1].max_strength = Fixed::from_int(60);
        base.units[0].position = 1;
        base.units[0].commander = Commander::new("谨慎者", 0.1, 0.9, 0.3, 0.95);
        // 先获得情报，否则 AI 会把敌省当成空的
        crate::intel::run_intel(&mut base);

        let mut held = 0;
        for _ in 0..40 {
            let mut w = base.clone();
            w.units[0].order = Order::Hold;
            run_ai(&mut w);
            if matches!(w.units[0].order, Order::Hold) {
                held += 1;
            }
        }
        assert!(held > 0, "谨慎的将领应当至少拒绝一次进攻");
    }
}











