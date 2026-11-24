//! 定点数：Q16.16。
//!
//! 模拟层**禁止浮点**。所有兵力、补给、士气、加成都是定点数，
//! 这样同一输入在任何平台、任何编译器版本下都得到逐位一致的结果。
//!
//! 取值范围：±32768.0，精度 1/65536 ≈ 0.000015。
//! 兵力（0-10000）、比例（0-1）、加成（-10..10）都在范围内。

/// 原始整数表示。
pub type Raw = i32;

/// 小数位宽。
pub const FRAC_BITS: u32 = 16;

/// 1.0 的原始值。
pub const ONE_RAW: Raw = 1 << FRAC_BITS;

/// Q16.16 定点数。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Fixed(Raw);

impl Fixed {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(ONE_RAW);
    pub const HALF: Self = Self(ONE_RAW / 2);

    #[inline]
    pub const fn from_raw(raw: Raw) -> Self {
        Self(raw)
    }

    #[inline]
    pub const fn raw(self) -> Raw {
        self.0
    }

    /// 整数构造。`Fixed::from_int(3)` == 3.0
    #[inline]
    pub const fn from_int(v: i32) -> Self {
        Self(v * ONE_RAW)
    }

    /// 分子/分母构造，例如 `Fixed::ratio(3, 4)` == 0.75
    #[inline]
    pub fn ratio(num: i32, den: i32) -> Self {
        debug_assert!(den != 0, "定点数除零");
        Self((((num as i64) << FRAC_BITS) / den as i64) as Raw)
    }

    /// 编译期可用的比例构造（用于常量）。
    #[inline]
    pub const fn ratio_const(num: i32, den: i32) -> Self {
        Self(((num as i64 * ONE_RAW as i64) / den as i64) as Raw)
    }

    /// 从浮点构造。
    ///
    /// **仅限内容加载**：作者在数据文件里写 `0.75` 这种自然写法，
    /// 加载时一次性转成定点数。模拟逻辑里禁止调用。
    #[inline]
    pub fn from_f64_input(v: f64) -> Self {
        Self((v * ONE_RAW as f64).round() as Raw)
    }

    #[inline]
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub fn max(self, other: Self) -> Self {
        if self.0 >= other.0 { self } else { other }
    }

    #[inline]
    pub fn min(self, other: Self) -> Self {
        if self.0 <= other.0 { self } else { other }
    }

    /// 夹到 [lo, hi]。
    #[inline]
    pub fn clamp(self, lo: Self, hi: Self) -> Self {
        if self.0 < lo.0 {
            lo
        } else if self.0 > hi.0 {
            hi
        } else {
            self
        }
    }

    /// 仅用于显示/渲染。**不要**在模拟逻辑里调用。
    #[inline]
    pub fn to_display_f64(self) -> f64 {
        self.0 as f64 / ONE_RAW as f64
    }
}

impl std::ops::Add for Fixed {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self(self.0.wrapping_add(rhs.0))
    }
}

impl std::ops::AddAssign for Fixed {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.0 = self.0.wrapping_add(rhs.0);
    }
}

impl std::ops::Sub for Fixed {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0.wrapping_sub(rhs.0))
    }
}

impl std::ops::SubAssign for Fixed {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.0 = self.0.wrapping_sub(rhs.0);
    }
}

impl std::ops::Mul for Fixed {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        Self((((self.0 as i64) * (rhs.0 as i64)) >> FRAC_BITS) as Raw)
    }
}

impl std::ops::Div for Fixed {
    type Output = Self;
    #[inline]
    fn div(self, rhs: Self) -> Self {
        debug_assert!(rhs.0 != 0, "定点数除零");
        Self(((((self.0 as i64) << FRAC_BITS) / rhs.0 as i64)) as Raw)
    }
}

impl PartialEq<i32> for Fixed {
    fn eq(&self, other: &i32) -> bool {
        self.0 == other * ONE_RAW
    }
}

impl From<i32> for Fixed {
    fn from(v: i32) -> Self {
        Self::from_int(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_roundtrip() {
        for v in [-10, -1, 0, 1, 7, 1000] {
            assert_eq!(Fixed::from_int(v).raw(), v * ONE_RAW);
            assert_eq!(Fixed::from_int(v).to_display_f64(), v as f64);
        }
    }

    #[test]
    fn ratio_is_exact_for_powers_of_two() {
        assert_eq!(Fixed::ratio(1, 2).raw(), ONE_RAW / 2);
        assert_eq!(Fixed::ratio(1, 4).raw(), ONE_RAW / 4);
        assert_eq!(Fixed::ratio(3, 4).raw(), (ONE_RAW * 3) / 4);
    }

    #[test]
    fn arithmetic() {
        let a = Fixed::ratio(3, 2); // 1.5
        let b = Fixed::from_int(2);
        assert_eq!((a + b).raw(), ONE_RAW * 7 / 2);
        assert_eq!((b - a).raw(), ONE_RAW / 2);
        assert_eq!((a * b).raw(), ONE_RAW * 3);
        assert_eq!((b / a).raw(), (ONE_RAW as i64 * 4 / 3) as i32);
    }

    #[test]
    fn mul_keeps_precision() {
        // 0.5 * 0.5 == 0.25，不应被截断成 0
        let half = Fixed::ratio(1, 2);
        assert_eq!((half * half).raw(), ONE_RAW / 4);
    }

    #[test]
    fn clamp_works() {
        let v = Fixed::from_int(100);
        assert_eq!(v.clamp(Fixed::ZERO, Fixed::ONE).raw(), ONE_RAW);
    }
}
