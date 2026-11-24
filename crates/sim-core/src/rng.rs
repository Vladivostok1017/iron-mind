//! 确定性随机数：SplitMix64 + 多流派生。
//!
//! 设计要点：**每个子系统、每个实体各有独立流**。
//! 流种子由 `(world_seed, stream_id, tick, entity_id)` 派生，
//! 所以给某个系统加一次随机调用，不会扰动其他系统的随机序列。
//!
//! 这条是确定性回放的关键：否则"改了一个系统的随机数次数，
//! 整个战役的演化全变了"，导致无法定位问题。

use crate::hash::fnv1a64;

/// 流标识。新增系统时追加，不要改动已有编号。
pub mod stream {
    pub const SUPPLY: u64 = 1;
    pub const MOVEMENT: u64 = 2;
    pub const COMBAT: u64 = 3;
    pub const MORALE: u64 = 4;
    pub const EVENTS: u64 = 5;
    pub const AI: u64 = 6;
}

/// SplitMix64 状态。
#[derive(Clone, Copy, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    #[inline]
    pub const fn from_seed(seed: u64) -> Self {
        Self { state: seed }
    }

    /// 从世界种子、流编号、tick、实体派生一个独立的流。
    #[inline]
    pub fn derive(world_seed: u64, stream_id: u64, tick: u32, entity_id: u32) -> Self {
        let mut s = fnv1a64(world_seed);
        s = fnv1a64_combine(s, stream_id);
        s = fnv1a64_combine(s, tick as u64);
        s = fnv1a64_combine(s, entity_id as u64);
        Self::from_seed(s)
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// 闭区间 [lo, hi] 内的整数。`hi <= lo` 时返回 lo。
    #[inline]
    pub fn range_i32(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        let span = (hi as i64 - lo as i64 + 1) as u64;
        // 64 位取模偏差可忽略（span 远小于 2^32）
        let v = self.next_u64() % span;
        (lo as i64 + v as i64) as i32
    }

    /// [0.0, 1.0) 区间的定点数。
    #[inline]
    pub fn unit_fixed(&mut self) -> crate::fixed::Fixed {
        // 取无符号高位再对 2^16 取模，保证落在 [0, 65536)。
        // 注意不能先转 i32——那样负数取模会得到负值。
        let v = (self.next_u64() >> 32) as u32 as u64;
        let raw = (v % crate::fixed::ONE_RAW as u64) as i32;
        crate::fixed::Fixed::from_raw(raw)
    }
}

#[inline]
fn fnv1a64_combine(acc: u64, v: u64) -> u64 {
    let mut h = acc;
    for i in 0..8 {
        h ^= (v >> (i * 8)) & 0xFF;
        h = h.wrapping_mul(0x100_0000_01B3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::from_seed(42);
        let mut b = Rng::from_seed(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seed_differs() {
        let mut a = Rng::from_seed(1);
        let mut b = Rng::from_seed(2);
        let mut same = 0;
        for _ in 0..1000 {
            if a.next_u64() == b.next_u64() {
                same += 1;
            }
        }
        assert!(same < 5, "不同种子不应产生雷同序列");
    }

    #[test]
    fn streams_are_independent() {
        let w = 1234u64;
        let mut supply = Rng::derive(w, stream::SUPPLY, 10, 7);
        let mut combat = Rng::derive(w, stream::COMBAT, 10, 7);
        assert_ne!(supply.next_u64(), combat.next_u64());
    }

    #[test]
    fn range_bounds() {
        let mut r = Rng::from_seed(7);
        for _ in 0..5000 {
            let v = r.range_i32(-5, 5);
            assert!((-5..=5).contains(&v));
        }
    }

    #[test]
    fn unit_fixed_in_range() {
        let mut r = Rng::from_seed(9);
        for _ in 0..5000 {
            let v = r.unit_fixed();
            assert!(v.raw() >= 0);
            assert!(v.raw() < crate::fixed::ONE_RAW);
        }
    }
}
