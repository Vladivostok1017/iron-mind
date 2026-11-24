//! 系统：补给、移动、战斗、士气。
//!
//! tick 内的执行顺序是**规格的一部分**，不能随意调换：
//!
//! ```text
//! 1. 补给网络扩散   —— 决定此后所有系统的输入
//! 2. 移动与接触     —— 位置变化后才有新的战斗
//! 3. 战斗解算       —— 用本 tick 的位置与补给
//! 4. 士气与恢复     —— 用本 tick 的战斗结果
//! ```
//!
//! 补给放在移动之前，是因为"断补给立刻掉战斗力"的因果必须成立。
//! 战斗放在移动之后，是因为要先到位才打。

use std::collections::VecDeque;

use crate::fixed::Fixed;
use crate::rng::{stream, Rng};
use crate::world::{Side, Terrain, UnitKind, World, Order};

/// 一个单位能否进入某种地形。
///
/// 这是海陆军分离的基础：
/// - 海军只走水域（`Terrain::Water`）
/// - 地面部队只走陆地
/// - 空军不移动（第一版是固定基地出动）
pub fn can_enter(kind: UnitKind, terrain: Terrain) -> bool {
    match kind {
        UnitKind::Naval => terrain == Terrain::Water,
        UnitKind::Air => false,
        UnitKind::Infantry | UnitKind::Armor => terrain != Terrain::Water,
    }
}

/// 一次广度松弛的结果：到各省的代价、第一步往哪走、还要走几步。
struct Walk {
    /// 到各省的最小加权代价（沿途地形的通行消耗之和）。
    dist: Vec<Option<Fixed>>,
    /// 从起点出发的**第一步**。路径本身不存，每 tick 重算。
    next: Vec<Option<u32>>,
    /// 还要经过几个省界（= 还要几天）。
    hops: Vec<Option<u32>>,
}

/// 从 `from` 出发的最省力走法。
///
/// 为什么不要求相邻才下令：**千里奔袭是常态**。
/// 1938 年国军一个军从赣北调到万家岭要走上百公里；日军第 106 师团
/// 也是从九江一路走进来的。只允许"打隔壁"，等于把战役压缩成一连串
/// 相邻省对撞，玩家说"抽两个师去守万家岭"就得一天一天点。
///
/// 但也不是瞬移：**一天只走一个省**（约 35 公里），路上会被打、会掉队。
/// 这样"远距离调动有代价"这条史实就保住了。
///
/// 权重 = 目标省的地形通行消耗 × (自己的地 ×1 / 别人的地 ×1.5)，
/// 再给已经挤满的省加价——否则部队会一直卡在人堆里等位置。
fn walk(world: &World, from: u32, side: Side, kind: UnitKind) -> Walk {
    let n = world.provinces.len();
    let mut w = Walk {
        dist: vec![None; n],
        next: vec![None; n],
        hops: vec![None; n],
    };
    if from as usize >= n {
        return w;
    }
    // 本省已经站了多少地面部队：挤满的省加价绕开，而不是傻等
    let mut crowd = vec![0u32; n];
    for u in world.units.iter() {
        if u.is_alive() && u.kind.is_ground() && (u.position as usize) < n {
            crowd[u.position as usize] += 1;
        }
    }
    w.dist[from as usize] = Some(Fixed::ZERO);
    w.hops[from as usize] = Some(0);

    // 87 个省，O(n²) 松弛比堆更简单，且**顺序固定**——确定性免费拿到。
    for _ in 0..=n {
        let mut changed = false;
        for p in 0..n {
            let dp = match w.dist[p] {
                Some(d) => d,
                None => continue,
            };
            let hp = w.hops[p].unwrap_or(0);
            for &q in &world.provinces[p].neighbors {
                let qi = q as usize;
                if qi >= n || qi == p {
                    continue;
                }
                if !can_enter(kind, world.provinces[qi].terrain) {
                    continue;
                }
                let mut cost = world.provinces[qi].terrain.move_cost();
                if world.provinces[qi].owner != side {
                    cost = cost * Fixed::ratio(3, 2);
                }
                if crowd[qi] >= STACK_LIMIT {
                    cost = cost * Fixed::ratio(3, 1);
                }
                let nd = dp + cost;
                let better = match w.dist[qi] {
                    None => true,
                    Some(d) => nd < d,
                };
                if better {
                    w.dist[qi] = Some(nd);
                    w.hops[qi] = Some(hp + 1);
                    w.next[qi] = if p == from as usize { Some(q) } else { w.next[p] };
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    w
}

/// 从 `from` 去 `to` 的下一站。不相邻也能给出第一步；走不通返回 `None`。
pub fn next_hop(world: &World, from: u32, to: u32, side: Side, kind: UnitKind) -> Option<u32> {
    if from == to {
        return None;
    }
    let w = walk(world, from, side, kind);
    w.next.get(to as usize).copied().flatten()
}

/// 还要几天才能到（经过几个省界）。走不通返回 `None`。
///
/// 前端用它显示"距目标 3 日"，玩家才知道远距离调动到底有多慢。
pub fn hops_to(world: &World, from: u32, to: u32, side: Side, kind: UnitKind) -> Option<u32> {
    if from == to {
        return Some(0);
    }
    let w = walk(world, from, side, kind);
    w.hops.get(to as usize).copied().flatten()
}

/// 从某省出发能走到哪些省、各要几天。按 (天数, 省号) 排序，确定性。
///
/// 两个地方要用：
/// 1. 将领的提示词——要让他知道"再走三天能到万家岭"，否则他只会盯着邻省；
/// 2. 前端——显示"距目标 3 日"。
pub fn reachable(world: &World, from: u32, side: Side, kind: UnitKind) -> Vec<(u32, u32)> {
    let w = walk(world, from, side, kind);
    let mut out: Vec<(u32, u32)> = w
        .hops
        .iter()
        .enumerate()
        .filter_map(|(i, h)| match h {
            Some(n) if i as u32 != from && *n > 0 => Some((i as u32, *n)),
            _ => None,
        })
        .collect();
    out.sort_by_key(|&(p, h)| (h, p));
    out
}

/// 航空兵的侦察与打击半径内可达的省（含自身）。
fn within_radius(world: &World, origin: u32, r: u32) -> Vec<u32> {
    let n = world.provinces.len();
    let mut seen = vec![false; n];
    let mut out = Vec::new();
    if (origin as usize) >= n || r == 0 {
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

/// 执行一个完整 tick。
///
/// 顺序（规格的一部分，不可随意调换）：
///
/// ```text
/// 1. AI 决策    → 给每个单位设置意图
/// 2. 补给扩散   → 决定此后所有系统的输入
/// 3. 执行意图   → 移动并结算占领
/// 4. 战斗解算   → 用本 tick 的位置与补给
/// 5. 士气恢复   → 用本 tick 的战斗结果
/// ```
///
/// AI 在最前：先有意图，才有移动。补给在移动前：
/// "断补给立刻掉战斗力"的因果必须成立。
pub fn tick(world: &mut World) {
    let seed = world.seed;
    let now = world.tick;

    use crate::rules::Phase;

    // 规则集的 Arc 先取出来，避免和 &mut world 的借用冲突。
    let rules = world.rules.clone();

    rules.run(world, Phase::PreSupply);
    crate::ai::run_ai(world);
    crate::intel::run_intel(world);
    run_supply(world);
    // 整编在补给之后：它要知道本方到底还控制着哪些补给源
    run_reform(world);
    rules.run(world, Phase::PostSupply);
    run_movement(world);
    rules.run(world, Phase::PostMovement);
    run_air(world, now);
    rules.run(world, Phase::PreCombat);
    run_combat(world, seed, now);
    // 江面上的炮战：舰队之间终于会互相开火了
    run_naval(world, seed, now);
    rules.run(world, Phase::PostCombat);
    run_morale(world);
    // 断粮：补给见底就真的会死人。放在士气之后——
    // 它读本 tick 的补给值，减员从下一 tick 起影响战斗力。
    run_starvation(world, now);
    // 打光或饿光的部队统一判定覆灭。
    // 以前只在战斗结算里做，于是"饿到 0.4 千"的部队会一直挂在编成里，
    // 既不能打，也不消失。
    sweep_ended(world, now, seed);
    rules.run(world, Phase::PostTick);
    // 今天的电报到了才进日志：长官部不是全知视角。
    world.flush_in_transit();

    world.tick += 1;

    // 结局判定放在最后：`tick` 已经走到"今天结束"。
    // 一旦判出结局，这一局就结束了，服务端不再往下推进。
    if world.outcome.is_none() {
        if let Some(o) = crate::endings::evaluate(world) {
            world.report(crate::world::EventKind::Rule, format!("战役结束：{}", o.name));
            world.outcome = Some(o);
        }
    }
}

// ---------------------------------------------------------------- 补给

/// 从某方的补给源做 BFS 扩散，得到每个省的补给等级 0-1。
fn compute_supply(world: &World, side: Side) -> Vec<Fixed> {
    let n = world.provinces.len();
    let mut level = vec![Fixed::ZERO; n];
    let mut queue: VecDeque<u32> = VecDeque::new();

    for i in 0..n {
        let p = &world.provinces[i];
        if p.is_supply_source && p.owner == side {
            level[i] = Fixed::ONE;
            queue.push_back(i as u32);
        }
    }

    let decay = Fixed::ratio(75, 100);

    while let Some(cur) = queue.pop_front() {
        let cur_level = level[cur as usize];
        let neighbors = world.provinces[cur as usize].neighbors.clone();
        for nb in neighbors {
            let np = &world.provinces[nb as usize];
            if np.owner != side {
                continue;
            }
            // 补给可以走水道（日军靠长江运补给就是这么来的），
            // 以前这里是直接跳过——于是"日军靠长江补给"这条史实在引擎里不存在。
            // 现在水道通，但损耗比铁路大（见下面的 base）。
            // 敌军的行军纵队踩在这条补给线上：过得去，但运不上多少东西。
            //
            // 这条是"切断补给线"的落点。以前只认**归属**：只要省还是我的，
            // 补给照常流，哪怕敌人一个联队正蹲在公路上。于是穿插、袭击后方、
            // 绕过据点断粮这些事在引擎里根本不存在。
            // 现在按 30% 通行能力折算——截两段就基本断了（0.3×0.3≈0.09）。
            let interdicted = world.units_in(nb).iter().any(|&i| {
                let u = &world.units[i as usize];
                u.is_alive() && u.kind.is_ground() && u.side != side
            });
            let pass = if interdicted {
                Fixed::ratio(3, 10)
            } else {
                Fixed::ONE
            };
            // 三种通路三种损耗：铁路 0.95、水道 0.85、普通陆路 0.75。
            // 有干线的省几乎不掉，这就是"守住铁路"的意义。
            let base = if np.rail {
                Fixed::ratio(95, 100)
            } else if np.terrain == Terrain::Water {
                Fixed::ratio(85, 100)
            } else {
                decay
            };
            let cand = cur_level * base * np.supply_throughput * pass;
            if cand > level[nb as usize] {
                level[nb as usize] = cand;
                queue.push_back(nb);
            }
        }
    }

    level
}

fn run_supply(world: &mut World) {
    let cn = compute_supply(world, Side::Cn);
    let jp = compute_supply(world, Side::Jp);

    // 就地征发的地板：即使补给线断了，也不会立刻归零。
    const FORAGE: Fixed = Fixed::ratio_const(50, 100);

    // 先把"是否在敌省"算出来，避免借用冲突。
    let mut in_enemy = Vec::with_capacity(world.units.len());
    for u in &world.units {
        let enemy_owned = world.provinces[u.position as usize].owner != u.side;
        in_enemy.push(enemy_owned);
    }

    for i in 0..world.units.len() {
        let side = world.units[i].side;
        let pos = world.units[i].position;
        let reach = world.units[i].supply_reach;

        let level = match side {
            Side::Cn => cn[pos as usize],
            Side::Jp => jp[pos as usize],
        };

        // 补给延伸：把"离补给源的距离差距"按单位能力补上一部分。
        //   reach = 0   → 完全按 BFS 结果，越远越差
        //   reach = 0.5 → 缺口补一半
        // 日军有汽车与铁路，reach 高；国军靠人力挑运，reach 低。
        let extended = level + (Fixed::ONE - level) * reach;

        let effective = if in_enemy[i] {
            // 敌境：取就地征发与延伸补给的高者
            extended.max(FORAGE)
        } else {
            extended
        };

        // ---- 合围 ----
        //
        // 所有邻省都不在自己手里 = 被围。这时候补给归零（不是"变差"，是断），
        // 断粮判定（<0.15）会接着每天减员——两条规则自然接上，不用特判。
        //
        // 为什么这条重要：以前断供只能靠"补给线上有敌人"，而真正的围歼
        // （万家岭）是"四面都是敌人"。玩家把三个师摆成一个圈，
        // 效果会立刻出现在被围部队的补给上。
        let encircled = is_encircled(world, pos, side);
        let effective = if encircled { Fixed::ZERO } else { effective };
        if encircled && !world.units[i].encircled {
            let name = world.units[i].name.clone();
            let where_ = world.provinces[pos as usize].name.clone();
            world.report_for(
                crate::world::EventKind::Supply,
                format!("{name} 在 {where_} 被合围"),
                side,
            );
        }
        world.units[i].encircled = encircled;

        world.units[i].supply = effective.clamp(Fixed::ZERO, Fixed::ONE);
    }
}

/// 这个省的所有邻省都不在本方手里 → 被围。
///
/// 只看**省归属**，不看敌军动向：围歼的判定要稳定、可预期，
/// 玩家能看懂"我这支部队四面都是敌人的旗子"。敌军巡逻队恰好路过
/// 不应该让一支部队突然断粮。
fn is_encircled(world: &World, pos: u32, side: Side) -> bool {
    let p = &world.provinces[pos as usize];
    if p.neighbors.is_empty() {
        return false;
    }
    p.neighbors
        .iter()
        .all(|&nb| world.provinces[nb as usize].owner != side)
}

// ---------------------------------------------------------------- 移动

/// 一个省最多容纳几个地面单位。
///
/// 没有这条上限，所有部队会挤进同一个省：AI 的最短路口径一致，天然会撞车，
/// 结果是一座山头堆七个师、而没有部队的省被对手一路走过去。
/// 上限也顺带表达了"一个省的展开正面放不下更多部队"。
pub const STACK_LIMIT: u32 = 3;

/// 执行意图：按 AI（或玩家）设置的 `order` 移动。
///
/// 移动系统本身**不做决策**，只负责"合法就执行"。
/// 意图由 `ai` 模块产生，将来也可以由玩家下达——
/// 换掉决策者不需要改这个函数，这正是"意图式指挥"的分层点。
fn run_movement(world: &mut World) {
    // ---- 省内调兵与原地命令 ----
    //
    // 调兵不消耗天数（阵地之间只有几公里），所以在移动之前直接结算。
    // 筑工事、整补也在这里生效，它们都不动身子。
    apply_stationary_orders(world);

    let mut moves: Vec<(usize, u32, u8)> = Vec::new();

    for i in 0..world.units.len() {
        let u = &world.units[i];
        // 舰队也能动——沿水道换段，舰炮支援才覆盖得到要打的那一仗。
        // 空军不动：第一版空军是"固定基地出动"，转场还没做。
        let can_move = u.kind.is_ground() || u.kind == UnitKind::Naval;
        if !u.is_alive() || !can_move {
            continue;
        }
        let target = match u.order.target() {
            Some(t) => t,
            None => continue, // 固守/筑工事/整补/调兵
        };
        // 目标可以不相邻：远距离调动沿最省力的路一天走一省（见 `walk`）。
        // AI 给出非法目标时静默忽略，而不是 panic——内容层永远可能出错。
        let prov = &world.provinces[u.position as usize];
        let step = if prov.neighbors.contains(&target) {
            target
        } else {
            match next_hop(world, u.position, target, u.side, u.kind) {
                Some(n) => n,
                None => continue, // 走不通（要跨海、或根本没有陆路）
            }
        };
        let dest_terrain = world.provinces[step as usize].terrain;
        if !can_enter(u.kind, dest_terrain) {
            continue;
        }
        // 进入哪个阵地：进攻打兵力最薄的，行军挑最空的。
        let attacking = matches!(u.order, crate::world::Order::Attack(_));
        let node = pick_node(world, step, u.side, attacking);
        moves.push((i, step, node));
    }

    // ---- 堆叠上限 ----
    //
    // 先算"留在原地的地面单位"，再按单位序号依次放行。
    // 顺序固定，所以结果可重放；挤不进去的单位原地不动，下一 tick 再试。
    let mut occupancy = vec![0u32; world.provinces.len()];
    let mut is_moving = vec![false; world.units.len()];
    for &(i, _, _) in &moves {
        is_moving[i] = true;
    }
    for (i, u) in world.units.iter().enumerate() {
        if u.kind.is_ground() && u.is_alive() && !is_moving[i] {
            occupancy[u.position as usize] += 1;
        }
    }
    let mut accepted: Vec<(usize, u32, u8)> = Vec::with_capacity(moves.len());
    for &(i, dest, node) in &moves {
        let d = dest as usize;
        if occupancy[d] >= STACK_LIMIT {
            continue; // 挤不进去，按兵不动
        }
        occupancy[d] += 1;
        accepted.push((i, dest, node));
    }
    let moves = accepted;

    // 记录移动前的位置，用于检测"对向"。
    let old_positions: Vec<u32> = world.units.iter().map(|u| u.position).collect();
    let planned: Vec<(usize, u32, u8)> = moves.clone();

    // 先应用全部移动，再统一结算占领。
    //
    // 不能在移动循环里逐个判占领：那样后移动的单位会看到前一个单位
    // 已经离开的省，导致"省里明明有兵，归属却没变"的错乱。
    for &(i, dest, node) in &moves {
        world.units[i].position = dest;
        world.units[i].node = node;
    }

    // ---- 遭遇战：处理对向穿插 ----
    //
    // 这是必须的。双方在同一条走廊上对向行军时，A 从 X 到 Y、B 从 Y 到 X，
    // 两者会"擦肩而过"——因为不同省不触发战斗，结果谁也打不着谁，
    // 部队就在地图上来回横跳，战线永远不动。
    //
    // 处理方式：检测位置互换，把其中一方拉到对方的落点，形成同省遭遇战。
    for a in 0..planned.len() {
        let (i, dest_i, node_i) = planned[a];
        for b in (a + 1)..planned.len() {
            let (j, dest_j, _) = planned[b];
            if world.units[i].side == world.units[j].side {
                continue;
            }
            // i 去的地方正是 j 原来的省，且 j 去的地方正是 i 原来的省
            if dest_i == old_positions[j] && dest_j == old_positions[i] {
                world.units[j].position = dest_i;
                world.units[j].node = node_i;
            }
        }
    }

    // 一次性命令到点就结束：到达目标省后回到固守，交还给 AI。
    // 不清理的话玩家下过命令的部队会永远锁在手动状态，AI 再也调不动它。
    // 同时清理够不着的目标（例如已经被打退到别处），否则它会一直干等。
    for i in 0..world.units.len() {
        let u = &world.units[i];
        if !u.manual {
            continue;
        }
        match u.order.target() {
            Some(t) => {
                let here = u.position;
                // 判定"走完了没有"，**不能用"是否相邻"**：
                // 远距离调动在路上时本来就不相邻，用相邻判定会把玩家的命令
                // 在第一个 tick 就清成固守——"千里奔袭"永远走不出去。
                let done = if here == t {
                    true
                } else if u.kind.is_ground() {
                    next_hop(world, here, t, u.side, u.kind).is_none()
                } else {
                    // 空海军不"走"，目标只表示打击/巡逻方向，相邻即算有效
                    !world.provinces[here as usize].neighbors.contains(&t)
                };
                if done {
                    world.units[i].order = Order::Hold;
                    world.units[i].manual = false;
                }
            }
            None => {}
        }
    }
    // 统一占领：某省只有一方且无另一方存活单位时，归该方。
    // 只能由地面部队占领，且只占陆地省——海军占不了城市，空军更不行。
    for p in 0..world.provinces.len() {
        if world.provinces[p].terrain == Terrain::Water {
            continue;
        }
        let here = world.units_in(p as u32);
        if here.is_empty() {
            continue;
        }
        // 按阵地计票：谁手里的阵地多，省归谁。双方各占一部分就是拉锯，
        // 归属不变——这才是"反复易手"该有的表现。
        let nodes = world.provinces[p].node_count();
        let mut cn_nodes = 0u32;
        let mut jp_nodes = 0u32;
        for node in 0..nodes {
            let mut cn = false;
            let mut jp = false;
            for &i in &here {
                let u = &world.units[i as usize];
                if !u.is_alive() || !u.kind.is_ground() || u.node != node {
                    continue;
                }
                match u.side {
                    Side::Cn => cn = true,
                    Side::Jp => jp = true,
                }
            }
            // 双方都在同一个阵地上：这个阵地还没定，不算票
            if cn && !jp {
                cn_nodes += 1;
            } else if jp && !cn {
                jp_nodes += 1;
            }
        }
        if cn_nodes > jp_nodes {
            if !enemy_adjacent(world, p as u32, Side::Jp) {
                if world.provinces[p].owner != Side::Cn {
                    let name = world.provinces[p].name.clone();
                    world.report(crate::world::EventKind::Occupy, format!("国军控制 {name}"));
                    world.provinces[p].owner = Side::Cn;
                }
            }
        } else if jp_nodes > cn_nodes {
            if !enemy_adjacent(world, p as u32, Side::Cn) {
                if world.provinces[p].owner != Side::Jp {
                    let name = world.provinces[p].name.clone();
                    world.report(crate::world::EventKind::Occupy, format!("日军攻占 {name}"));
                    world.provinces[p].owner = Side::Jp;
                }
            }
        }
    }
}

/// 原地命令：省内调兵、筑工事、整补。
///
/// 这三条都不需要"走一格"，所以不进移动流程。调兵只有几公里，
/// 当预备队用；筑工事和整补是把日子花在原地。
fn apply_stationary_orders(world: &mut World) {
    let tick = world.tick;
    for i in 0..world.units.len() {
        let alive = world.units[i].is_alive();
        let kind = world.units[i].kind;
        if !alive || !kind.is_ground() {
            continue;
        }
        let order = world.units[i].order;
        let pos = world.units[i].position;
        let side = world.units[i].side;
        let nodes = world.provinces[pos as usize].node_count();

        match order {
            Order::Shift(n) => {
                // 越界就当成最后一个阵地，不 panic（内容层可能写错）
                world.units[i].node = n.min(nodes.saturating_sub(1));
                world.units[i].order = Order::Hold;
            }
            Order::Fortify => {
                // 脚下有敌军就挖不动
                let under_fire = world.units_in(pos).iter().any(|&j| {
                    let u = &world.units[j as usize];
                    u.side != side && u.kind.is_ground() && u.is_alive()
                });
                if !under_fire && tick % 3 == 0 {
                    let node = world.units[i].node.min(nodes.saturating_sub(1));
                    if let Some(p) = world.provinces[pos as usize]
                        .positions
                        .get_mut(node as usize)
                    {
                        if p.fort < 5 {
                            p.fort += 1;
                        }
                    }
                }
            }
            Order::Rest => {
                let s = world.units[i].supply + Fixed::ratio(1, 10);
                world.units[i].supply = if s > Fixed::ONE { Fixed::ONE } else { s };
                let m = world.units[i].morale + Fixed::ratio(1, 20);
                world.units[i].morale = if m > Fixed::ONE { Fixed::ONE } else { m };
                // 补充兵员：整补不只是歇口气，是把新兵补进来。
                //
                // 以前 `Rest` 只回士气和补给，兵力只减不增。77 天的仗打到后期，
                // 所有部队都是残废，而且**永远恢复不了**——于是"守住了"的
                // 部队下一天依旧是一千人的空架子。这不合理。
                //
                // 条件：补给 > 0.5（后方才有人有粮）。每天补满编的 0.4%，
                // 一支 12 千的军从 6 千补到 9 千要 60 天左右——够慢，
                // 不会让"打残"这件事变得无所谓。
                let u = &world.units[i];
                if u.supply > Fixed::ratio(5, 10) && u.strength < u.max_strength {
                    let add = u.max_strength * Fixed::ratio(4, 1000);
                    let room = u.max_strength - u.strength;
                    let add = if add > room { room } else { add };
                    world.units[i].strength += add;
                }
            }
            _ => {}
        }
    }
}

/// 进入某省时挑一个阵地。
///
/// **进攻必须打到有敌军的那个阵地。** 这是硬约束，不是启发式：
/// 如果允许"挑最空的阵地"绕进去，攻方就会专门绕过守军去占一个空山头，
/// 双方在同一省里各占一个阵地、永不交战，省的颜色来回翻——
/// 看起来在打仗，实际谁也没碰到谁。
///
/// 只有"省里已经没有敌军"时才退化成挑最空的（那就是在接管）。
/// 平手取编号最小的，保证可重放。
fn pick_node(world: &World, province: u32, side: Side, attacking: bool) -> u8 {
    let n = world.provinces[province as usize].node_count().max(1);
    let mut best = 0u8;
    let mut best_score: Option<Fixed> = None;

    if attacking {
        // 第一优先：有敌军的阵地里最薄弱的那个
        for node in 0..n {
            let mut enemy = Fixed::ZERO;
            for &i in &world.units_in(province) {
                let u = &world.units[i as usize];
                if !u.is_alive() || !u.kind.is_ground() || u.node != node || u.side == side {
                    continue;
                }
                enemy = enemy + u.strength;
            }
            if enemy.is_zero() {
                continue;
            }
            if best_score.map_or(true, |b: Fixed| enemy < b) {
                best_score = Some(enemy);
                best = node;
            }
        }
        if best_score.is_some() {
            return best;
        }
    }

    // 省里没有敌军（或不是进攻）：挑友军最少的阵地，别堵在一起
    for node in 0..n {
        let mut mine = Fixed::ZERO;
        for &i in &world.units_in(province) {
            let u = &world.units[i as usize];
            if !u.is_alive() || !u.kind.is_ground() || u.node != node || u.side != side {
                continue;
            }
            mine = mine + u.strength;
        }
        if best_score.map_or(true, |b: Fixed| mine < b) {
            best_score = Some(mine);
            best = node;
        }
    }
    best
}

/// 邻接省里是否有敌方地面单位。
///
/// 占领规则的核心：**敌军还在旁边，就不算拿下了**。
///
/// 没有这一条，六个师团每天可以各翻一个省，四十天就能把九十个省全涂成自己的颜色——
/// 因为它们只是"路过"，而路过也要算占领。加上这一条之后，只有真正把对手从
/// 邻近赶走（或吃掉）才能改颜色，战线才会真的停在接触面上。
fn enemy_adjacent(world: &World, province: u32, enemy: Side) -> bool {
    for &nb in &world.provinces[province as usize].neighbors {
        for &i in &world.units_in(nb) {
            let u = &world.units[i as usize];
            if u.is_alive() && u.kind.is_ground() && u.side == enemy {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------- 战斗

fn run_combat(world: &mut World, seed: u64, now: u32) {
    let n = world.provinces.len();

    for p in 0..n {
        let idx = world.units_in(p as u32);
        if idx.len() < 2 {
            continue;
        }
        // 逐阵地打：不同阵地上的部队不打照面（一个在张古山，一个在扁担山）。
        let nodes = world.provinces[p].node_count();
        for node in 0..nodes {
        let mut cn: Vec<u32> = Vec::new();
        let mut jp: Vec<u32> = Vec::new();
        for &i in &idx {
            let u = &world.units[i as usize];
            if !u.kind.is_ground() || u.node != node {
                continue;
            }
            match u.side {
                Side::Cn => cn.push(i),
                Side::Jp => jp.push(i),
            }
        }
        if cn.is_empty() || jp.is_empty() {
            continue;
        }

        let mut cn_power = Fixed::ZERO;
        for &i in &cn {
            cn_power += world.units[i as usize].combat_power();
        }
        let mut jp_power = Fixed::ZERO;
        for &i in &jp {
            jp_power += world.units[i as usize].combat_power();
        }

        let def_mult = world.provinces[p].node_defense(node);
        // 展开宽度：阵地越窄，能同时投入的兵力越少。
        // 这是"战场宽度"的落点——多了的部队只能当预备队，不能堆进火力。
        let width = world.provinces[p].node_width(node);
        let capacity = width * Fixed::from_int(40);
        let owner = world.provinces[p].owner;
        // 地形与工事给守方加成，舰炮给攻方与守方都可能加成
        let cn_raw = if owner == Side::Cn { cn_power * def_mult } else { cn_power };
        let jp_raw = if owner == Side::Jp { jp_power * def_mult } else { jp_power };
        let cn_eff = cn_raw.clamp(Fixed::ZERO, capacity)
            * naval_support(world, p as u32, Side::Cn);
        let jp_eff = jp_raw.clamp(Fixed::ZERO, capacity)
            * naval_support(world, p as u32, Side::Jp);

        let total = cn_eff + jp_eff;
        if total.is_zero() {
            continue;
        }

        // 基础损耗率：每 tick 最多 6%，按对方战力占比分配。
        // 太高会让一场战斗三周内结束，体现不出"反复易手"。
        let intensity = Fixed::ratio(6, 100);
        let mut cn_loss = jp_eff / total * intensity;
        let mut jp_loss = cn_eff / total * intensity;

        // 随机扰动 ±20%，双方各用独立流（含阵地号，否则同一省的几个阵地
        // 会拿到同一组随机数）
        let base = (p as u32) * 8 + node as u32;
        let mut rc = Rng::derive(seed, stream::COMBAT, now, base * 2);
        let mut rj = Rng::derive(seed, stream::COMBAT, now, base * 2 + 1);
        let cn_jitter = Fixed::ratio(8, 10) + rc.unit_fixed() * Fixed::ratio(4, 10);
        let jp_jitter = Fixed::ratio(8, 10) + rj.unit_fixed() * Fixed::ratio(4, 10);
        cn_loss = (cn_loss * cn_jitter).clamp(Fixed::ZERO, Fixed::ratio(60, 100));
        jp_loss = (jp_loss * jp_jitter).clamp(Fixed::ZERO, Fixed::ratio(60, 100));

        // 应用损耗
        let mut cn_lost = Fixed::ZERO;
        let mut jp_lost = Fixed::ZERO;
        for &i in &cn {
            let u = &mut world.units[i as usize];
            let delta = u.strength * cn_loss;
            u.strength -= delta;
            u.losses += delta;
            u.morale -= cn_loss * Fixed::ratio(5, 10);
            u.morale = u.morale.clamp(Fixed::ratio(15, 100), Fixed::ONE);
            cn_lost += delta;
        }
        for &i in &jp {
            let u = &mut world.units[i as usize];
            let delta = u.strength * jp_loss;
            u.strength -= delta;
            u.losses += delta;
            u.morale -= jp_loss * Fixed::ratio(5, 10);
            u.morale = u.morale.clamp(Fixed::ratio(15, 100), Fixed::ONE);
            jp_lost += delta;
        }

        // 战报：每三日报一次交战，避免每 tick 刷屏
        if now % 3 == 0 {
            let where_ = format!(
                "{}·{}",
                world.provinces[p].name,
                world.provinces[p]
                    .position_at(node)
                    .map(|x| x.name.as_str())
                    .unwrap_or("主阵地")
            );
            let cn_name = cn
                .iter()
                .map(|&i| world.units[i as usize].name.clone())
                .next()
                .unwrap_or_default();
            let jp_name = jp
                .iter()
                .map(|&i| world.units[i as usize].name.clone())
                .next()
                .unwrap_or_default();
            world.report(
                crate::world::EventKind::Combat,
                format!(
                    "{where_}：{cn_name} 损失 {:.1} 千，{jp_name} 损失 {:.1} 千",
                    cn_lost.to_display_f64(),
                    jp_lost.to_display_f64()
                ),
            );
        }

        // 覆灭判定统一放在 `sweep_ended`（含饿死的），这里不再重复一次：
        // 两处各判一次的话，阈值一改就会有一处漏改。

        // 阵地归属由 tick 开头的占领结算按"阵地数过半"统一处理，
        // 这里不再直接改省的颜色：一个阵地打赢了不代表整省易手。
        }
    }
}

// ---------------------------------------------------------------- 断粮

/// 断粮断水：补给见底就不只是"打不动"，是真的会死人。
///
/// 之前补给只影响 `combat_power`（兵力 × 士气 × 补给 × 系数），于是补给 0 的
/// 部队"打不了"但**永远不减员**——它可以蹲在包围圈里待到战役结束，兵一个不少。
///
/// 规则很直白：
/// - 补给 < 0.15：日损 0.5% 兵力、士气 −2%
/// - 每 3 日报一次，否则满屏都是"又饿了一天"
///
/// 0.5%/日 的量级：一支 8 千人的军断粮一个月掉到约 6.9 千。
/// 够痛（逼得你去打通补给线），但不至于三天蒸发（留出救援窗口）。
const STARVE_LEVEL: (i32, i32) = (15, 100);
const STARVE_LOSS: (i32, i32) = (5, 1000);

fn run_starvation(world: &mut World, now: u32) {
    use crate::world::EventKind;

    for i in 0..world.units.len() {
        let u = &world.units[i];
        if !u.in_being() || !u.kind.is_ground() {
            continue;
        }
        if u.supply >= Fixed::ratio(STARVE_LEVEL.0, STARVE_LEVEL.1) {
            continue;
        }
        let side = u.side;
        let name = u.name.clone();
        let loss = u.strength * Fixed::ratio(STARVE_LOSS.0, STARVE_LOSS.1);
        let u = &mut world.units[i];
        u.strength -= loss;
        u.losses += loss;
        u.morale -= Fixed::ratio(2, 100);
        u.morale = u.morale.clamp(Fixed::ratio(5, 100), Fixed::ONE);

        if now % 3 == 0 {
            // 断粮是内部消息：敌人不知道你还有几天粮。
            world.report_for(
                EventKind::Supply,
                format!("{name} 断粮，日损 {:.2} 千", loss.to_display_f64()),
                side,
            );
        }
    }
}

/// 兵不足千人的部队：判定覆灭（打光或饿光）。
///
/// 阈值 **200 人**。老口径是"不足 1 千"，那是错的：一个被打残到 900 人的师
/// 仍然是师，番号还在；而空军一个飞行团本来就只有 800–1500 人，
/// 按 1 千判"全歼"会把整个空军一夜之间判死。
pub const WIPED_OUT: Fixed = Fixed::ratio_const(2, 10);

fn sweep_ended(world: &mut World, now: u32, seed: u64) {
    for i in 0..world.units.len() {
        let u = &world.units[i];
        if u.ended_day.is_some() || u.strength >= WIPED_OUT {
            continue;
        }
        world.units[i].strength = Fixed::ZERO;
        let prov = world.units[i].position;
        end_unit(world, i, prov, now, seed);
    }
}

// ---------------------------------------------------------------- 编制变动

/// 一支部队打光了：记下场、定将领的结局、排定能不能二次整编。
///
/// 三件事必须一起做，否则会留下"兵没了但军长还在下命令"这种鬼状态——
/// 之前就是这样：`strength` 归零，但将领、番号、命令全都原样保留，
/// AI 还会继续问一个已经不存在的人"你要怎么打"。
///
/// 将领结局是**掷骰**决定的，不是写死：突围、阵亡、被俘、去职，
/// 用独立的随机流（`stream::FATE`），所以同一种子必然同一结局。
fn end_unit(world: &mut World, idx: usize, prov: u32, now: u32, seed: u64) {
    use crate::world::{EventKind, Fate};

    let name = world.units[idx].name.clone();
    let side = world.units[idx].side;
    let where_ = world.provinces[prov as usize].name.clone();
    let general = world.units[idx].commander.name.clone();

    // 掷将领的结局。序列固定 → 确定性。
    let mut rng = Rng::derive(seed, stream::FATE, now, idx as u32);
    let roll = rng.unit_fixed().to_display_f64();
    let fate = if roll < 0.35 {
        Fate::Killed
    } else if roll < 0.50 {
        Fate::Captured
    } else if roll < 0.70 {
        Fate::Relieved
    } else {
        Fate::Active // 突围生还
    };
    world.units[idx].commander.fate = fate;
    world.units[idx].ended_day = Some(now);
    world.units[idx].order = crate::world::Order::Hold;
    world.units[idx].manual = false;
    world.units[idx].dissent_reason = None;
    world.units[idx].disobeyed = false;

    // 全军覆没是战场上看得见的事：双方都能报。
    // 但要走电报/参谋那一段路——长官部不是当场就知道的。
    let delay = world.comms.report_delay();
    world.report_delayed(delay, EventKind::Combat, format!("{name} 在 {where_} 被全歼"), None);
    // 将领的生死是内部消息：只报给本方（敌人不知道对面的军长死没死）。
    let fate_line = match fate {
        Fate::Killed => format!("{name}（{general}）全军覆没，{general} 殉国"),
        Fate::Captured => format!("{name}（{general}）全军覆没，{general} 下落不明，恐已被俘"),
        Fate::Relieved => format!("{name}（{general}）全军覆没，{general} 已去职"),
        Fate::Active => format!("{name}（{general}）全军覆没，{general} 突围生还"),
    };
    world.report_delayed(delay, EventKind::Oob, fate_line, Some(side));

    // 这支部队如果是它所属兵团的**兵团部**，兵团部覆灭就是一场编制地震：
    // 下属各部暂时没人压得住，服从度会掉一截（见 `World::effective_obedience`）。
    let hq_info = world
        .formation_of(idx)
        .filter(|f| f.hq == Some(idx) && f.units.len() > 1)
        .map(|f| (f.name.clone(), f.units.len()));
    if let Some((fname, n)) = hq_info {
        world.report_delayed(
            delay,
            EventKind::Oob,
            format!(
                "{fname} 兵团部覆灭，下属 {} 部暂归战区直辖",
                n.saturating_sub(1)
            ),
            Some(side),
        );
    }

    // 二次整编：模块声明过、且**还有将领活着**才排得上。
    // 没有人的部队不会被"自动重建"——那需要干部、番号与补充兵。
    if world.units[idx].reform_ratio.raw() > 0 && fate.can_lead() {
        let days = world.units[idx].reform_days;
        world.units[idx].reform_day = Some(now + days);
        let delay = world.comms.report_delay();
        world.report_delayed(
            delay,
            EventKind::Oob,
            format!("{name} 将于 {days} 日后在后方整编"),
            Some(side),
        );
    }
}

/// 二次整编：到日子了，就在本方控制的补给源重新成军。
///
/// 为什么放在补给源：整编要兵、要枪、要粮食，这些东西只在后方枢纽有。
/// 也正因为如此，"后方被抄" 会真的影响重建速度——找不到补给源就一直等。
fn run_reform(world: &mut World) {
    use crate::world::{EventKind, Order};

    let now = world.tick;
    for idx in 0..world.units.len() {
        let u = &world.units[idx];
        let due = match u.reform_day {
            Some(d) if d <= now => d,
            _ => continue,
        };
        let _ = due;
        let side = u.side;
        let ratio = u.reform_ratio;
        // 本方控制的补给源（下标最小的那个，确定）
        let home = (0..world.provinces.len()).find(|&p| {
            world.provinces[p].is_supply_source && world.provinces[p].owner == side
        });
        let Some(home) = home else {
            continue; // 后方还没着落，明年再说
        };

        let home_name = world.provinces[home].name.clone();
        let name = world.units[idx].name.clone();
        let max = world.units[idx].max_strength;
        let u = &mut world.units[idx];
        u.strength = (max * ratio).clamp(Fixed::ZERO, max);
        u.morale = Fixed::ratio(6, 10);
        u.supply = Fixed::ratio(6, 10);
        u.position = home as u32;
        u.node = 0;
        u.moving_to = None;
        u.ended_day = None;
        u.reform_day = None;
        u.order = Order::Hold;
        u.manual = false;
        u.disobeyed = false;
        u.dissent_reason = None;
        let delay = world.comms.report_delay();
        world.report_delayed(
            delay,
            EventKind::Oob,
            format!(
                "{name} 在 {home_name} 整编完毕，补入新兵 {:.1} 千",
                world.units[idx].strength.to_display_f64()
            ),
            Some(side),
        );
    }
}

// ---------------------------------------------------------------- 海战

/// 舰队对轰：同处一段水道的两支敌对舰队互相开火。
///
/// 以前舰队是"浮动加成器"——两支舰队停在同一段江面上互相看不见，
/// 这不合理。江面上的战斗是炮战：谁先发现、谁的炮多，谁就能把对方赶走。
///
/// 规则刻意简单（第一版）：
/// - 火力 = 战斗力（兵力 × 士气 × 补给 × 系数），不吃地形、不吃工事；
/// - 每天双方各损失敌我火力占比的 6%，与陆战同一个量级；
/// - 士气同时掉。水面没有阵地，打不过就是撤或沉。
fn run_naval(world: &mut World, seed: u64, now: u32) {
    const INTENSITY: (i32, i32) = (6, 100);

    // (省, 中方单位, 日方单位)
    let mut fights: Vec<(u32, Vec<usize>, Vec<usize>)> = Vec::new();
    for p in 0..world.provinces.len() {
        if world.provinces[p].terrain != Terrain::Water {
            continue;
        }
        let mut cn = Vec::new();
        let mut jp = Vec::new();
        for &i in &world.units_in(p as u32) {
            let u = &world.units[i as usize];
            if !u.is_alive() || u.kind != UnitKind::Naval {
                continue;
            }
            match u.side {
                Side::Cn => cn.push(i as usize),
                Side::Jp => jp.push(i as usize),
            }
        }
        if !cn.is_empty() && !jp.is_empty() {
            fights.push((p as u32, cn, jp));
        }
    }

    for (p, cn, jp) in fights {
        let mut cn_power = Fixed::ZERO;
        for &i in &cn {
            cn_power += world.units[i].combat_power();
        }
        let mut jp_power = Fixed::ZERO;
        for &i in &jp {
            jp_power += world.units[i].combat_power();
        }
        let total = cn_power + jp_power;
        if total.is_zero() {
            continue;
        }
        let intensity = Fixed::ratio(INTENSITY.0, INTENSITY.1);
        let mut cn_loss = jp_power / total * intensity;
        let mut jp_loss = cn_power / total * intensity;
        // 炮战也有运气：独立随机流，含省号
        let mut rc = Rng::derive(seed, stream::COMBAT, now, 9000 + p);
        let mut rj = Rng::derive(seed, stream::COMBAT, now, 9500 + p);
        cn_loss = (cn_loss * (Fixed::ratio(8, 10) + rc.unit_fixed() * Fixed::ratio(4, 10)))
            .clamp(Fixed::ZERO, Fixed::ratio(40, 100));
        jp_loss = (jp_loss * (Fixed::ratio(8, 10) + rj.unit_fixed() * Fixed::ratio(4, 10)))
            .clamp(Fixed::ZERO, Fixed::ratio(40, 100));

        let mut cn_lost = Fixed::ZERO;
        let mut jp_lost = Fixed::ZERO;
        for &i in &cn {
            let u = &mut world.units[i];
            let d = u.strength * cn_loss;
            u.strength -= d;
            u.losses += d;
            u.morale -= cn_loss * Fixed::ratio(5, 10);
            u.morale = u.morale.clamp(Fixed::ratio(15, 100), Fixed::ONE);
            cn_lost += d;
        }
        for &i in &jp {
            let u = &mut world.units[i];
            let d = u.strength * jp_loss;
            u.strength -= d;
            u.losses += d;
            u.morale -= jp_loss * Fixed::ratio(5, 10);
            u.morale = u.morale.clamp(Fixed::ratio(15, 100), Fixed::ONE);
            jp_lost += d;
        }
        if now % 2 == 0 {
            let where_ = world.provinces[p as usize].name.clone();
            let a = world.units[cn[0]].name.clone();
            let b = world.units[jp[0]].name.clone();
            world.report(
                crate::world::EventKind::Combat,
                format!(
                    "{where_}：{a} 与 {b} 炮战，损失 {:.2} / {:.2} 千",
                    cn_lost.to_display_f64(),
                    jp_lost.to_display_f64()
                ),
            );
        }
    }
}
// ---------------------------------------------------------------- 士气

fn run_morale(world: &mut World) {
    // 士气有下限：不能让一支部队因为士气归零而彻底失去战斗力，
    // 否则双方战斗力都变成 0，战斗系统会静默跳过，战线僵死在原地。
    const FLOOR: Fixed = Fixed::ratio_const(15, 100);
    const CEIL: Fixed = Fixed::ONE;

    for i in 0..world.units.len() {
        let u = &mut world.units[i];
        if !u.is_alive() {
            continue;
        }
        let recovering = u.supply > Fixed::ratio(5, 10);
        let starving = u.supply < Fixed::ratio(3, 10);

        if recovering {
            u.morale += Fixed::ratio(3, 100);
        } else if starving {
            u.morale -= Fixed::ratio(1, 100);
        } else {
            u.morale += Fixed::ratio(1, 100);
        }

        u.morale = u.morale.clamp(FLOOR, CEIL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Province, Terrain, Unit};

    fn line_world() -> World {
        // A(源) - B - C，Cn 在 A，Jp 在 C
        let provinces = vec![
            Province::new("A", Terrain::Plain, Side::Cn)
                .with_neighbors(&[1])
                .as_source(),
            Province::new("B", Terrain::Plain, Side::Cn).with_neighbors(&[0, 2]),
            Province::new("C", Terrain::Plain, Side::Jp)
                .with_neighbors(&[1])
                .as_source(),
        ];
        let units = vec![
            Unit::new("cn-1", Side::Cn, 0, 10),
            Unit::new("jp-1", Side::Jp, 2, 10),
        ];
        World::new(99, provinces, units)
    }

    #[test]
    fn supply_decays_with_distance() {
        let w = line_world();
        let lvl = compute_supply(&w, Side::Cn);
        assert_eq!(lvl[0], Fixed::ONE);
        assert!(lvl[1] < lvl[0]);
        assert_eq!(lvl[2], Fixed::ZERO); // 敌省不扩散
    }

    #[test]
    fn units_advance_toward_enemy() {
        let mut w = line_world();
        tick(&mut w); // 移动
        // Cn 从 A 推进到 B，Jp 从 C 推进到 B
        assert_eq!(w.units[0].position, 1);
        assert_eq!(w.units[1].position, 1);
    }

    #[test]
    fn combat_reduces_strength() {
        let mut w = line_world();
        tick(&mut w);
        // 两队各 10 千人
        let total_before = Fixed::from_int(20);
        let total_after = w.units[0].strength + w.units[1].strength;
        assert!(total_after < total_before, "接触后应有损耗");
    }

    #[test]
    fn tick_advances() {
        let mut w = line_world();
        assert_eq!(w.tick, 0);
        tick(&mut w);
        assert_eq!(w.tick, 1);
    }
}





// ---------------------------------------------------------------- 海军支援

/// 某省战斗中，某方能得到的舰炮支援倍率。
///
/// 只有**邻接水域**的省才有舰炮支援，且需要己方海军在那个水域省里。
/// 每 1000 吨舰艇（以兵力千人近似）提供 5% 战斗力加成，上限 60%。
///
/// 这是海军的第一个真实作用：它不占领、不登陆，但能让沿岸的陆战好打得多。
///
/// 公开是因为前端要显示它。一个能改变战斗结果、但玩家完全看不见的东西，
/// 等于不存在——玩家只会觉得"我有一支舰队，它什么都没干"。
pub fn naval_support(world: &World, province: u32, side: Side) -> Fixed {
    let mut bonus = Fixed::ZERO;
    let neighbors = world.provinces[province as usize].neighbors.clone();
    for nb in neighbors {
        if world.provinces[nb as usize].terrain != Terrain::Water {
            continue;
        }
        for &ui in &world.units_in(nb) {
            let u = &world.units[ui as usize];
            if u.side == side && u.kind == UnitKind::Naval && u.is_alive() {
                bonus += u.strength * Fixed::ratio(5, 100);
            }
        }
    }
    Fixed::ONE + bonus.clamp(Fixed::ZERO, Fixed::ratio(60, 100))
}

// ---------------------------------------------------------------- 空军打击


/// 本方在整个战场上的空中战力占比（0-1）。没有飞机 → 0。
fn air_share(world: &World, side: Side) -> Fixed {
    let mut own = Fixed::ZERO;
    let mut all = Fixed::ZERO;
    for u in world.units.iter() {
        if u.kind != UnitKind::Air || !u.is_alive() {
            continue;
        }
        let p = u.strike_power * u.strength;
        all += p;
        if u.side == side {
            own += p;
        }
    }
    if all.is_zero() {
        Fixed::ZERO
    } else {
        own / all
    }
}
/// 空中打击。
///
/// 每 tick，空军对**侦察半径内敌方控制省**的部队造成损耗：地面部队能炸，
/// 江面上的舰船也能炸（1938 年日军飞机轰炸长江上的中国舰艇是常态）。
///
/// 三条规则决定了"它到底能干什么"：
///
/// 1. **有目标就打目标**。将领下了"打某省"的命令（`Order::Attack`），
///    就只打那一个省，集中力量；没有目标才自动在半径内挑敌控省。
///    以前完全不看命令，于是电报对空军毫无意义——玩家只能看着它自己打。
/// 2. **出击有损耗**。每打一天，机组与飞机折损 0.3%。命令里那句
///    "飞机打一架少一架" 以前是假话（空军永不损失），现在是真的。
/// 3. **打得动人**。系数 0.25：一个满编飞行团对单个目标约 0.13 千/日
///    （一百多人），量级与一支师级部队一天的战斗减员相当。
///    以前是 0.1，一天几十人，打一个月也看不出变化，等于没有。
fn run_air(world: &mut World, now: u32) {
    /// 空袭效率：`strike_power × 兵力 × 系数`
    const EFF: (i32, i32) = (25, 100);
    /// 每出击一天的兵力折损（机组与飞机）
    const WEAR: (i32, i32) = (3, 1000);

    // (攻击方, 省, 受击单位, 伤害)
    let mut strikes: Vec<(usize, u32, usize, Fixed)> = Vec::new();
    let mut worn: Vec<usize> = Vec::new();

    for i in 0..world.units.len() {
        let u = &world.units[i];
        if !u.is_alive() || u.kind != UnitKind::Air || u.strength.is_zero() {
            continue;
        }
        if u.strike_power.is_zero() {
            continue;
        }
        let enemy = u.side.opponent();
        let power = u.strike_power * u.strength;
        // 有明确目标就只打它；没有目标时**也只挑一个省**。
        //
        // 自动模式以前是"半径内所有敌控省一起炸"，于是一个飞行团能在同一天
        // 同时压制三个省，加起来比一支师还能打。航空兵是要集中使用的：
        // 挑一个最值得炸的地方——最好是正在打的仗——其余放掉。
        let reach: Vec<u32> = match u.order.target() {
            Some(t) if (t as usize) < world.provinces.len() => vec![t],
            _ => within_radius(world, u.position, u.recon_radius)
                .into_iter()
                .filter(|&p| {
                    (p as usize) < world.provinces.len()
                        && world.provinces[p as usize].owner != u.side
                        && world
                            .units_in(p)
                            .iter()
                            .any(|&j| world.units[j as usize].side == enemy)
                })
                .max_by_key(|&p| {
                    // 自己地面部队在场的省优先（那就是在打的仗），其次看敌军兵力
                    let supporting = world
                        .units_in(p)
                        .iter()
                        .any(|&j| world.units[j as usize].side == u.side);
                    let mass: i32 = world
                        .units_in(p)
                        .iter()
                        .filter(|&&j| world.units[j as usize].side == enemy)
                        .map(|&j| (world.units[j as usize].strength.raw() >> 16).max(0))
                        .sum();
                    (supporting as i32, mass, -(p as i32))
                })
                .map(|p| vec![p])
                .unwrap_or_default(),
        };

        for p in reach {
            if world.provinces[p as usize].owner == u.side {
                continue;
            }
            // 地面部队与舰船都能炸
            let targets: Vec<usize> = world
                .units_in(p)
                .into_iter()
                .map(|x| x as usize)
                .filter(|&j| {
                    let tj = &world.units[j];
                    tj.side == enemy
                        && tj.is_alive()
                        && (tj.kind.is_ground() || tj.kind == UnitKind::Naval)
                })
                .collect();
            if targets.is_empty() {
                continue;
            }

            let mut total = Fixed::ZERO;
            for &j in &targets {
                total += world.units[j].strength;
            }
            if total.is_zero() {
                continue;
            }

            // 制空权：本方空中战力占比越高，投弹越从容。
            // 没有敌机时 share = 1 → 系数 1.0（等于原样）；被完全压制时只剩 0.35。
            let supremacy = Fixed::ratio(35, 100)
                + Fixed::ratio(65, 100) * air_share(world, u.side);
            let mut hit = false;
            for &j in &targets {
                let share = world.units[j].strength / total;
                // 行军纵队：在路上的部队没有工事、也没散开，最好打。
                // 106 师团在万家岭被围，一半原因就是行军途中被咬住。
                let in_column = world.units[j].order.target().is_some()
                    && world.units[j].order.target() != Some(world.units[j].position);
                let column = if in_column {
                    Fixed::ratio(14, 10)
                } else {
                    Fixed::ONE
                };
                let dmg = power * share * Fixed::ratio(EFF.0, EFF.1) * supremacy * column;
                strikes.push((i, p, j, dmg));
                hit = true;
            }
            if hit && !worn.contains(&i) {
                worn.push(i);
            }
        }
    }

    // 按 (攻击方, 省) 汇总成一条战报：逐单位刷屏没人看得下去
    let mut reports: Vec<(usize, u32, Fixed)> = Vec::new();
    for (i, p, _, _) in &strikes {
        match reports.iter_mut().find(|(a, b, _)| a == i && b == p) {
            Some(_) => {}
            None => reports.push((*i, *p, Fixed::ZERO)),
        }
    }

    for (i, p, j, dmg) in strikes {
        {
            let u = &mut world.units[j];
            if !u.is_alive() {
                continue;
            }
            let actual = dmg.min(u.strength);
            u.strength -= actual;
            u.losses += actual;
            u.morale -= Fixed::ratio(1, 100);
            u.morale = u.morale.clamp(Fixed::ratio(15, 100), Fixed::ONE);
        }
        if let Some(r) = reports.iter_mut().find(|(a, b, _)| *a == i && *b == p) {
            r.2 += dmg;
        }
    }

    // 出击损耗
    for i in worn {
        let u = &mut world.units[i];
        let wear = u.strength * Fixed::ratio(WEAR.0, WEAR.1);
        u.strength -= wear;
        u.losses += wear;
    }

    // 战报：双方都看得见（天上过的飞机不是秘密）
    for (i, p, dmg) in reports {
        if dmg.is_zero() {
            continue;
        }
        let who = world.units[i].name.clone();
        let where_ = world.provinces[p as usize].name.clone();
        world.report(
            crate::world::EventKind::Combat,
            format!("{who} 空袭 {where_}：敌军损失 {:.2} 千", dmg.to_display_f64()),
        );
    }
    let _ = now;
}








