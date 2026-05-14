//! Bit manipulation utilities — port of `tdoku/src/bitutil.h`.

/// Returns the number of set bits in `x`.
#[inline]
pub fn num_bits_set(x: u32) -> u32 {
    x.count_ones()
}

/// Returns the number of set bits in `x` (64-bit variant).
#[inline]
pub fn num_bits_set64(x: u64) -> u32 {
    x.count_ones()
}

/// Returns `x` with all bits cleared except the lowest set bit.
/// Equivalent to `x & -x` in C.
#[inline]
pub fn get_low_bit(x: u32) -> u32 {
    x & x.wrapping_neg()
}

/// Returns `x` with all bits cleared except the lowest set bit (64-bit variant).
#[inline]
pub fn get_low_bit64(x: u64) -> u64 {
    x & x.wrapping_neg()
}

/// Clears the lowest set bit of `x`. `x` must be non-zero.
#[inline]
pub fn clear_low_bit(x: u32) -> u32 {
    x & (x - 1)
}

/// Clears the lowest set bit of `x`. `x` must be non-zero (64-bit variant).
#[inline]
pub fn clear_low_bit64(x: u64) -> u64 {
    x & (x - 1)
}

/// Returns the 0-based index of the lowest set bit. `x` must be non-zero.
/// Equivalent to `__builtin_ffs(x) - 1` in C.
#[inline]
pub fn low_order_bit_index(x: u32) -> u32 {
    x.trailing_zeros()
}

/// Returns the 0-based index of the lowest set bit. `x` must be non-zero (64-bit variant).
#[inline]
pub fn low_order_bit_index64(x: u64) -> u32 {
    x.trailing_zeros()
}

/// Returns the 0-based index of the highest set bit. `x` must be non-zero.
/// Equivalent to `sizeof(uint32_t)*8 - __builtin_clz(x) - 1` in C.
#[inline]
pub fn high_order_bit_index(x: u32) -> u32 {
    31 - x.leading_zeros()
}

/// Returns the 0-based index of the highest set bit. `x` must be non-zero (64-bit variant).
#[inline]
pub fn high_order_bit_index64(x: u64) -> u32 {
    63 - x.leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_num_bits_set() {
        assert_eq!(num_bits_set(0), 0);
        assert_eq!(num_bits_set(0b1010_1010), 4);
        assert_eq!(num_bits_set(0x1ff), 9);
    }

    #[test]
    fn test_get_low_bit() {
        assert_eq!(get_low_bit(0b1100), 0b0100);
        assert_eq!(get_low_bit(0b0001), 0b0001);
        assert_eq!(get_low_bit(0b1000), 0b1000);
    }

    #[test]
    fn test_clear_low_bit() {
        assert_eq!(clear_low_bit(0b1100), 0b1000);
        assert_eq!(clear_low_bit(0b0001), 0b0000);
    }

    #[test]
    fn test_low_order_bit_index() {
        assert_eq!(low_order_bit_index(0b0001), 0);
        assert_eq!(low_order_bit_index(0b0010), 1);
        assert_eq!(low_order_bit_index(0b1100), 2);
        assert_eq!(low_order_bit_index(1 << 8), 8);
    }

    #[test]
    fn test_high_order_bit_index() {
        assert_eq!(high_order_bit_index(0b0001), 0);
        assert_eq!(high_order_bit_index(0b1010), 3);
        assert_eq!(high_order_bit_index(0x1ff), 8);
    }
}
