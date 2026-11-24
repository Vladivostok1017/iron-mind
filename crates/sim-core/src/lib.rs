//! Iron Mind 模拟核心。
//!
//! 设计约束（按重要性排序）：
//!
//! 1. **确定性可重放**——同输入必得同输出，逐位一致。
//!    手段：定点数、固定 tick、有序容器、多流 RNG、状态哈希。
//! 2. **对 AI 零依赖**——拔掉 AI 也能完整运行，将领退回启发式。
//! 3. **对内容无知**——不出现任何战役名或硬编码场景。
//! 4. **无上帝视角**——每个阵营只知道自己侦察到的东西。
//!
//! 分层：
//! - `fixed`   定点数
//! - `rng`     多流确定性随机
//! - `hash`    状态哈希
//! - `world`   世界状态（含真值与每方情报视图）
//! - `intel`   情报系统（把真值变成"你看到的"）
//! - `ai`      规则 AI 与将领解释
//! - `systems` 系统（决策/情报/补给/移动/战斗/士气）
//! - `scenario` 场景构造（测试用，正式内容由 sim-content 从文件加载）

pub mod ai;
pub mod fixed;
pub mod hash;
pub mod intel;
pub mod rng;
pub mod rules;
pub mod scenario;
pub mod systems;
pub mod world;

pub use fixed::Fixed;
pub use intel::IntelSummary;
pub use rules::{Condition, Phase, Primitive, Rule, RuleSet};
pub use world::{
    Accuracy, Commander, Order, Province, ProvinceIntel, Side, Terrain, Unit, UnitKind, World,
};

/// 连续跑 `n` 个 tick，返回每个 tick 结束时的世界哈希。
///
/// 这是黄金回放测试的入口：两次调用必须返回完全相同的向量。
pub fn run_ticks(world: &mut World, n: u32) -> Vec<u64> {
    let mut hashes = Vec::with_capacity(n as usize);
    for _ in 0..n {
        systems::tick(world);
        hashes.push(world.state_hash());
    }
    hashes
}

