//! 派系与人事（预览版）。
//!
//! 只在模块用 `scenario.preview.politics = true` 显式开启时才参与模拟。
//! 关闭时这里的东西一个字节都不会跑，也一个字节都不会进哈希。
//!
//! 设计意图（为什么是"派系"而不是"忠诚度"）：
//! - 将领的个人数据（服从/主动性/进攻性/谨慎）是**他自己**的；
//! - 派系是**他的靠山**。靠山在朝里说话有没有分量，决定他敢不敢抗命、
//!   以及上面敢不敢撤他。
//! - 所以派系不是第五个人格维度，而是一层**外部约束**：它调制服从，
//!   并提供"人事"这个玩家动作（撤换、启用、贬职）。
//!
//! 第一版发行版没有这个概念，也不该有：定稿的行为由 `Features::politics`
//! 明确关闭，老模块的哈希序列不变。

use crate::hash::Hasher;
use crate::world::World;

/// 一个派系在模拟里的状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Faction {
    /// 模块声明的 id（内容层的稳定标识）。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 归属阵营。派系只在**本方**内部竞争，不跨阵营。
    pub side: crate::world::Side,
    /// 朝中分量 0-1。由权重漂移与战功/败绩推动。
    pub influence: crate::fixed::Fixed,
    /// 初始权重（模块声明的"这一派本来就该有多重"）。
    pub weight: crate::fixed::Fixed,
    /// 本派系将领的部队下标（构建世界时算好，避免每 tick 按名字查）。
    pub units: Vec<usize>,
}

impl Faction {
    pub fn new(id: &str, name: &str, side: crate::world::Side, weight: crate::fixed::Fixed) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            side,
            influence: weight,
            weight,
            units: Vec::new(),
        }
    }
}

/// 派系状态进哈希。只有 `Features::politics` 开启时才被调用。
///
/// 顺序即协议：先派系数、再逐个（id 字符串 + 影响力）。
pub fn hash_into(h: &mut Hasher, w: &World) {
    h.write_u32(w.factions.len() as u32);
    for f in &w.factions {
        h.write_str(&f.id);
        h.write_i32(f.influence.raw());
    }
}
