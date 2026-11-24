//! 状态哈希：FNV-1a 64 位。
//!
//! 用途：黄金回放测试。同一场景 + 同一输入序列，每个 tick 结束算一次世界哈希，
//! 两次运行的哈希序列必须逐位相同。任何不一致都意味着确定性被破坏。
//!
//! 这是 CI 里最重要的一条断言——它能在"改了一行代码"的瞬间发现
//! "三个战役的平衡全变了"。

const FNV_OFFSET: u64 = 0xCBF2_9CE4_8422_2325;
const FNV_PRIME: u64 = 0x100_0000_01B3;

/// 从给定种子开始做一次 FNV 混合。
#[inline]
pub fn fnv1a64(seed: u64) -> u64 {
    let mut h = FNV_OFFSET;
    h ^= seed & 0xFF;
    h = h.wrapping_mul(FNV_PRIME);
    h
}

/// 增量哈希器。
#[derive(Clone, Copy, Debug)]
pub struct Hasher {
    h: u64,
}

impl Hasher {
    #[inline]
    pub const fn new() -> Self {
        Self { h: FNV_OFFSET }
    }

    #[inline]
    pub fn write_u8(&mut self, v: u8) {
        self.h ^= v as u64;
        self.h = self.h.wrapping_mul(FNV_PRIME);
    }

    #[inline]
    pub fn write_u32(&mut self, v: u32) {
        for i in 0..4 {
            self.write_u8(((v >> (i * 8)) & 0xFF) as u8);
        }
    }

    #[inline]
    pub fn write_i32(&mut self, v: i32) {
        self.write_u32(v as u32);
    }

    #[inline]
    pub fn write_u64(&mut self, v: u64) {
        for i in 0..8 {
            self.write_u8(((v >> (i * 8)) & 0xFF) as u8);
        }
    }

    /// 写入定长字符串（不足补零，超出截断）。用于名称参与哈希。
    #[inline]
    pub fn write_str(&mut self, s: &str) {
        let bytes = s.as_bytes();
        let n = bytes.len().min(32);
        self.write_u32(n as u32);
        for i in 0..n {
            self.write_u8(bytes[i]);
        }
    }

    #[inline]
    pub fn finish(&self) -> u64 {
        self.h
    }
}

impl Default for Hasher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_input_same_hash() {
        let mut a = Hasher::new();
        let mut b = Hasher::new();
        a.write_u32(1);
        a.write_u64(0xDEAD_BEEF);
        b.write_u32(1);
        b.write_u64(0xDEAD_BEEF);
        assert_eq!(a.finish(), b.finish());
    }

    #[test]
    fn order_matters() {
        let mut a = Hasher::new();
        let mut b = Hasher::new();
        a.write_u32(1);
        a.write_u32(2);
        b.write_u32(2);
        b.write_u32(1);
        assert_ne!(a.finish(), b.finish());
    }

    #[test]
    fn single_bit_flip_changes_hash() {
        let mut a = Hasher::new();
        let mut b = Hasher::new();
        a.write_u32(0b1000);
        b.write_u32(0b1001);
        assert_ne!(a.finish(), b.finish());
    }
}
