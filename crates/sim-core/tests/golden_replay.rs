//! 黄金回放测试。
//!
//! 这是整个项目的安全网。三条断言：
//!
//! 1. **同输入必得同输出**——两次运行逐 tick 哈希完全一致。
//! 2. **黄金哈希**——固定场景 + 固定种子 + 固定 tick 数，末哈希必须等于常量。
//!    任何无意的行为改动都会让它失败。有意改动时更新这个常量，
//!    更新动作本身就是"我确认改动了模拟行为"的声明。
//! 3. **不同种子不同结局**——随机流确实接进了模拟。

use sim_core::scenario;
use sim_core::world::Side;
use sim_core::run_ticks;

const GOLDEN_SEED: u64 = 42;
const GOLDEN_TICKS: u32 = 200;

#[test]
fn same_seed_produces_identical_history() {
    let mut a = scenario::nanxun_line(GOLDEN_SEED);
    let ha = run_ticks(&mut a, GOLDEN_TICKS);

    let mut b = scenario::nanxun_line(GOLDEN_SEED);
    let hb = run_ticks(&mut b, GOLDEN_TICKS);

    assert_eq!(ha.len(), GOLDEN_TICKS as usize);
    assert_eq!(ha, hb, "同种子的哈希序列必须逐位一致");

    // 状态也应该完全一致（不只是哈希）
    assert_eq!(a.state_hash(), b.state_hash());
    assert_eq!(a.units.len(), b.units.len());
    for (x, y) in a.units.iter().zip(b.units.iter()) {
        assert_eq!(x.name, y.name);
        assert_eq!(x.strength.raw(), y.strength.raw());
        assert_eq!(x.position, y.position);
    }
}

#[test]
fn golden_hash_is_stable() {
    let mut w = scenario::nanxun_line(GOLDEN_SEED);
    let hashes = run_ticks(&mut w, GOLDEN_TICKS);
    let last = *hashes.last().unwrap();

    // 若此断言失败：要么是确定性被破坏（必须修），
    // 要么是有意的行为改动（更新这个常量，并在提交里说明原因）。
    assert_eq!(
        last, GOLDEN_LAST_HASH,
        "黄金哈希不匹配。当前值 {:#018x}，期望 {:#018x}",
        last, GOLDEN_LAST_HASH
    );
}

/// 固定场景在 200 tick 后的末哈希。行为变更时手工更新。
///
/// 变更记录：
/// - 2025-09 初版
/// - 加入 `Unit::effectiveness`（战斗力系数）后重算
/// - 修正敌境补给口径（就地征发 0.5，不再额外打折）后重算
/// - 加入 `Unit::supply_reach`（补给延伸）与 `Order`（意图）后重算
/// - 加入 Commander（将领性格）与意图解释后重算
/// - 加入情报系统（AI 改用估计值决策）后重算
/// - 加入通信延迟（战报在途）后重算
/// - 通信拆成传输/参谋/命令三段（电报是秒级的，瓶颈在参谋流程）后重算
/// - 加入海空军（海域省份、舰炮支援、空中打击）；情报改为存兵力而非战斗力
/// - 情报存兵力而非战斗力后重算
/// - 加入堆叠上限（一个省最多 3 个地面单位）后重算。
///   动机：AI 的最短路一致，部队会全部挤进同一个省；上限把兵力逼散到整条战线。
/// - 占领改为"邻接无敌军才算拿下"后重算。
///   动机：路过即占领让六个师团四十天涂满九十个省，战线形同虚设。
/// - 加入省内阵地（`Position`）后重算：战斗改为逐阵地结算、按展开宽度封顶，
///   并新增 `Shift`（省内调兵）、`Fortify`（筑工事）、`Rest`（整补）。
/// - 状态哈希补进 `unit.node` 与 `unit.manual` 后重算。
///   动机：这两个字段会影响战斗结果与"谁在下令"；不进哈希就会出现
///   "两个不同状态算出同一哈希"，回放校验（/api/replay-check）就抓不到偏差。
/// - 状态哈希补进**编制变动**（`ended_day` / `reform_day` / `reform_ratio` /
///   `commander.fate`）后重算。动机同上：覆灭日决定"还能不能被问策、能不能整编"，
///   将领结局决定"这支部队还重建得起来吗"——不同结局必须算出不同哈希。
/// - **补给会真的断**（docs/20）后重算：敌军纵队踩在补给线上把通行能力乘 0.3，
///   断粮（<0.15）每天减员 0.5%。这两条直接改变兵力与补给值，属于行为变更。
/// - 覆灭阈值从"不足 1 千"改为"不足 200 人"后重算：
///   老口径会把 800–1500 人的空军飞行团一夜之间判成全军覆没。
/// - **加入溃退与围歼**（docs/23）后重算：士气 <0.25 且当面打不赢就退向
///   己方空省、丢一成二的兵；无路可退则被围歼。这是"伤亡不对称"的来源
///   ——对射本身几乎对称，差距在崩了之后。
/// - **就地征发的地板也管本国领土 + 断粮改按"是否被切断"判**（docs/20 §5）后重算：
///   以前只有敌境留 0.5 的地板，本国没有，于是国军在自己后方会掉到 0.15
///   以下天天减员，而打进我们省的日军反倒有 0.5——逻辑反了。
///   现在本国 0.25 / 敌境 0.5 兜着；断粮改成
///   `四面皆敌 || (本方有补给源 && BFS 到不了)`。
///   区别是：**离补给源远 = 缺粮（战力差）**，**被切断 = 断粮（日损）**。
const GOLDEN_LAST_HASH: u64 = 0x09fd_d15c_7ee8_37d8;

#[test]
fn different_seeds_diverge() {
    let mut a = scenario::nanxun_line(1);
    let ha = run_ticks(&mut a, 80);

    let mut b = scenario::nanxun_line(2);
    let hb = run_ticks(&mut b, 80);

    assert_ne!(
        ha.last().unwrap(),
        hb.last().unwrap(),
        "不同种子应当得到不同结局"
    );
}

#[test]
fn simulation_terminates_with_a_winner_or_standoff() {
    let mut w = scenario::nanxun_line(GOLDEN_SEED);
    run_ticks(&mut w, GOLDEN_TICKS);

    let cn = w.total_strength(Side::Cn);
    let jp = w.total_strength(Side::Jp);

    // 不是断言谁赢，而是断言"没有崩到两边都没了"
    assert!(
        cn.raw() > 0 || jp.raw() > 0,
        "双方都不应有兵力，模拟可能已损坏"
    );
    // 推进逻辑应当生效：日军至少要打下过省。
    //
    // 这里原来还断言"国军至少留一个省"。加入溃退之后，内置场景
    // （八省一条直线）完全可能被一路推平——那不是 bug，是
    // "溃退—追击—再溃退"这条链跑到底的样子。所以断言改成
    // "日军占到过省"，仍然抓得住"谁也不动"这类真问题。
    let jp_prov = w.provinces.iter().filter(|p| p.owner == Side::Jp).count();
    // "攻占过"而不是"现在占着"：加了战线连续性（`can_step`）之后，
    // 一条八省直线上的日军可能打进去又被推回来。这不影响断言的本意——
    // 抓的是"谁也不动"这种真问题，不是某一天的旗子。
    let captured = w.log.iter().any(|e| e.text.contains("日军攻占"));
    assert!(
        jp_prov > 0 || captured,
        "推进逻辑没生效：日军既没占下省，也从没攻占过任何省"
    );
    assert_eq!(w.units.len(), 5, "单位数量不该变（死亡是状态，不是删除）");
}








