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
    rules.run(world, Phase::PostSupply);
    run_movement(world);
    rules.run(world, Phase::PostMovement);
    run_air(world);
    rules.run(world, Phase::PreCombat);
    run_combat(world, seed, now);
    rules.run(world, Phase::PostCombat);
    run_morale(world);
    rules.run(world, Phase::PostTick);

    world.tick += 1;
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
            // 补给不走水路：海运补给是另一套机制，这里先跳过
            if np.terrain == Terrain::Water {
                continue;
            }
            let cand = cur_level * decay * np.supply_throughput;
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

        world.units[i].supply = effective.clamp(Fixed::ZERO, Fixed::ONE);
    }
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
        if !u.is_alive() || !u.kind.is_ground() {
            continue;
        }
        let target = match u.order.target() {
            Some(t) => t,
            None => continue, // 固守/筑工事/整补/调兵
        };
        // 必须是邻接，且地形允许该兵种进入。
        // AI 给出非法目标时静默忽略，而不是 panic——内容层永远可能出错。
        let prov = &world.provinces[u.position as usize];
        if !prov.neighbors.contains(&target) {
            continue;
        }
        let dest_terrain = world.provinces[target as usize].terrain;
        if !can_enter(u.kind, dest_terrain) {
            continue;
        }
        // 进入哪个阵地：进攻打兵力最薄的，行军挑最空的。
        let attacking = matches!(u.order, crate::world::Order::Attack(_));
        let node = pick_node(world, target, u.side, attacking);
        moves.push((i, target, node));
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
                let adjacent = world.provinces[here as usize].neighbors.contains(&t);
                if here == t || !adjacent {
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

        // 兵力低于 1 人的单位撤销
        for &i in cn.iter().chain(jp.iter()) {
            let u = &mut world.units[i as usize];
            if u.strength < Fixed::ONE {
                u.strength = Fixed::ZERO;
                let name = u.name.clone();
                let prov = world.provinces[p].name.clone();
                world.report(
                    crate::world::EventKind::Combat,
                    format!("{name} 在 {prov} 被全歼"),
                );
            }
        }

        // 阵地归属由 tick 开头的占领结算按"阵地数过半"统一处理，
        // 这里不再直接改省的颜色：一个阵地打赢了不代表整省易手。
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
fn naval_support(world: &World, province: u32, side: Side) -> Fixed {
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

/// 空中打击：`strike_power` 终于有用了。
///
/// 每 tick，空军对侦察半径内**敌方控制省**的地面部队造成损耗。
/// 损耗按对方兵力占比分配，强度与打击力、目标规模成正比。
///
/// 第一版没有制空权对抗、没有战损、没有架次限制。
/// 这些都要等"空军对空军"的机制做出来才有意义。
fn run_air(world: &mut World) {
    let mut strikes: Vec<(usize, Fixed)> = Vec::new();

    for u in world.units.iter() {
        if !u.is_alive() || u.kind != UnitKind::Air || u.strike_power.is_zero() {
            continue;
        }
        if u.strength.is_zero() {
            continue;
        }
        let enemy = u.side.opponent();
        let power = u.strike_power * u.strength;
        let reach = within_radius(world, u.position, u.recon_radius);

        for p in reach {
            if world.provinces[p as usize].owner == u.side {
                continue;
            }
            // 只打地面部队
            let targets: Vec<usize> = world
                .units_in(p)
                .into_iter()
                .map(|x| x as usize)
                .filter(|&j| {
                    let tj = &world.units[j];
                    tj.side == enemy && tj.kind.is_ground() && tj.is_alive()
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

            for &j in &targets {
                let share = world.units[j].strength / total;
                // 0.1 是空袭效率系数：一个飞行团每天造成几十人伤亡的量级
                let dmg = power * share * Fixed::ratio(1, 10);
                strikes.push((j, dmg));
            }
        }
    }

    for (j, dmg) in strikes {
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
}


