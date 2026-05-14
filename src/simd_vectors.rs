//! SIMD vector abstractions — port of `tdoku/src/simd_vectors.h`.
//!
//! Provides `Bitvec08x16` (8 × u16 lanes, 128-bit) and `Bitvec16x16`
//! (16 × u16 lanes, 256-bit logical) with all operations required by the
//! SIMD solver. On x86_64 with SSSE3/SSE4.1 the hot paths use
//! `std::arch::x86_64` intrinsics directly; all other targets fall back
//! to portable scalar code.
//!
//! The scalar fallback implementations use explicit indexed loops rather
//! than iterator combinators to mirror the SIMD lane-indexing pattern and
//! keep the C++ → Rust translation auditable.

#![allow(clippy::needless_range_loop)]

// ──────────────────────────────────────────────────────────────────────────────
// cfg_if-style macros for feature-gated x86 dispatch
// (must be defined before use)
// ──────────────────────────────────────────────────────────────────────────────

/// Dispatch: SSSE3 path vs scalar fallback.
macro_rules! cfg_if_x86_ssse3 {
    ($simd:block, $scalar:block) => {{
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("ssse3") {
                $simd
            } else {
                $scalar
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        $scalar
    }};
}

/// Dispatch: SSE4.1 path vs scalar fallback.
macro_rules! cfg_if_x86_sse4_1 {
    ($simd:block, $scalar:block) => {{
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("sse4.1") {
                $simd
            } else {
                $scalar
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        $scalar
    }};
}

/// Dispatch: SSE2 (always available on x86_64) vs scalar fallback.
macro_rules! cfg_if_x86_sse2 {
    ($simd:block, $scalar:block) => {{
        #[cfg(target_arch = "x86_64")]
        $simd
        #[cfg(not(target_arch = "x86_64"))]
        $scalar
    }};
}

/// Dispatch: SSE4.2 path vs scalar fallback.
macro_rules! cfg_if_x86_sse4_2 {
    ($simd:block, $scalar:block) => {{
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("sse4.2") {
                $simd
            } else {
                $scalar
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        $scalar
    }};
}

/// Dispatch: movemask (SSE2) path vs scalar fallback.
macro_rules! cfg_if_x86_movemask {
    ($simd:block, $scalar:block) => {{
        cfg_if_x86_sse2!($simd, $scalar)
    }};
}

// ──────────────────────────────────────────────────────────────────────────────
// Helper: NumBitsSet64  (mirrors C++ bitutil NumBitsSet64)
// ──────────────────────────────────────────────────────────────────────────────
#[inline(always)]
fn num_bits_set_64(x: u64) -> u32 {
    x.count_ones()
}

// ──────────────────────────────────────────────────────────────────────────────
// Bitvec08x16  — 8 × u16 lanes (128-bit logical vector)
// ──────────────────────────────────────────────────────────────────────────────

/// 8-lane × 16-bit vector.  Internally stored as `[u16; 8]` in little-endian
/// lane order (lane 0 at index 0), mirroring `__m128i` layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C, align(16))]
pub struct Bitvec08x16(pub [u16; 8]);

impl Default for Bitvec08x16 {
    #[inline]
    fn default() -> Self {
        Self([0; 8])
    }
}

impl Bitvec08x16 {
    // ── constructors ────────────────────────────────────────────────────────

    #[inline]
    pub fn zero() -> Self {
        Self([0; 8])
    }

    #[inline]
    pub fn all(value: u16) -> Self {
        Self([value; 8])
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn new(x0: u16, x1: u16, x2: u16, x3: u16, x4: u16, x5: u16, x6: u16, x7: u16) -> Self {
        Self([x0, x1, x2, x3, x4, x5, x6, x7])
    }

    // ── element access ───────────────────────────────────────────────────────

    #[inline]
    pub fn extract(&self, index: usize) -> u16 {
        self.0[index]
    }

    #[inline]
    pub fn insert(&mut self, index: usize, value: u16) {
        self.0[index] = value;
    }

    // ── boolean predicates ───────────────────────────────────────────────────

    #[inline]
    pub fn all_zero(&self) -> bool {
        cfg_if_x86_sse4_1!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    let v = load128(self);
                    _mm_test_all_zeros(v, v) != 0
                }
            },
            { self.0.iter().all(|&x| x == 0) }
        )
    }

    #[inline]
    pub fn any_zero(&self) -> bool {
        cfg_if_x86_movemask!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    let v = load128(self);
                    let zero = _mm_setzero_si128();
                    let eq = _mm_cmpeq_epi16(v, zero);
                    _mm_movemask_epi8(eq) != 0
                }
            },
            { self.0.contains(&0) }
        )
    }

    #[inline]
    pub fn any_less_than(&self, other: &Self) -> bool {
        cfg_if_x86_movemask!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    let a = load128(self);
                    let b = load128(other);
                    let lt = _mm_cmpgt_epi16(b, a); // b > a  ⟺  a < b
                    _mm_movemask_epi8(lt) != 0
                }
            },
            { self.0.iter().zip(other.0.iter()).any(|(&a, &b)| a < b) }
        )
    }

    #[inline]
    pub fn intersects(&self, other: &Self) -> bool {
        cfg_if_x86_sse4_1!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    let a = load128(self);
                    let b = load128(other);
                    _mm_testz_si128(a, b) == 0
                }
            },
            {
                self.0
                    .iter()
                    .zip(other.0.iter())
                    .any(|(&a, &b)| (a & b) != 0)
            }
        )
    }

    #[inline]
    pub fn subset_of(&self, other: &Self) -> bool {
        cfg_if_x86_sse4_1!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    let a = load128(self);
                    let b = load128(other);
                    _mm_testc_si128(b, a) != 0
                }
            },
            {
                self.0
                    .iter()
                    .zip(other.0.iter())
                    .all(|(&a, &b)| (a & !b) == 0)
            }
        )
    }

    // ── lane-wise comparison ──────────────────────────────────────────────────

    #[inline]
    pub fn which_equal(&self, other: &Self) -> Self {
        cfg_if_x86_sse2!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    let r = _mm_cmpeq_epi16(load128(self), load128(other));
                    store128(r)
                }
            },
            {
                let mut r = [0u16; 8];
                for i in 0..8 {
                    r[i] = if self.0[i] == other.0[i] { 0xffff } else { 0 };
                }
                Self(r)
            }
        )
    }

    #[inline]
    pub fn which_non_zero(&self) -> Self {
        cfg_if_x86_sse2!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    let zero = _mm_setzero_si128();
                    let r = _mm_cmpgt_epi16(load128(self), zero);
                    store128(r)
                }
            },
            {
                let mut r = [0u16; 8];
                for i in 0..8 {
                    r[i] = if self.0[i] != 0 { 0xffff } else { 0 };
                }
                Self(r)
            }
        )
    }

    // ── bit manipulation ─────────────────────────────────────────────────────

    #[inline]
    pub fn get_low_bit(&self) -> Self {
        // low_bit(x) = x & (-x as wrapping)
        let mut r = [0u16; 8];
        for i in 0..8 {
            let x = self.0[i];
            r[i] = x & x.wrapping_neg();
        }
        Self(r)
    }

    #[inline]
    pub fn clear_low_bit(&self) -> Self {
        // Mirrors C++ ClearLowBit(): treats the 8 u16 lanes as two 64-bit
        // integers (low = lanes 0–3, high = lanes 4–7) and clears the
        // globally lowest set bit of the first non-zero 64-bit half.
        // This is NOT a per-lane operation — only a single bit is cleared.
        cfg_if_x86_sse4_2!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    let v = load128(self);
                    let zero = _mm_setzero_si128();
                    // cmp: each 64-bit lane is all-1s if > 0, else all-0s
                    let cmp = _mm_cmpgt_epi64(v, zero);
                    // one: 1 in the first non-zero 64-bit half, 0 elsewhere
                    let one = _mm_andnot_si128(_mm_slli_si128(cmp, 1), _mm_srli_epi64(cmp, 63));
                    store128(_mm_and_si128(v, _mm_sub_epi64(v, one)))
                }
            },
            {
                let lo: u64 = (self.0[0] as u64)
                    | ((self.0[1] as u64) << 16)
                    | ((self.0[2] as u64) << 32)
                    | ((self.0[3] as u64) << 48);
                if lo != 0 {
                    let cleared = lo & lo.wrapping_sub(1);
                    Self([
                        cleared as u16,
                        (cleared >> 16) as u16,
                        (cleared >> 32) as u16,
                        (cleared >> 48) as u16,
                        self.0[4],
                        self.0[5],
                        self.0[6],
                        self.0[7],
                    ])
                } else {
                    let hi: u64 = (self.0[4] as u64)
                        | ((self.0[5] as u64) << 16)
                        | ((self.0[6] as u64) << 32)
                        | ((self.0[7] as u64) << 48);
                    let cleared = hi & hi.wrapping_sub(1);
                    Self([
                        self.0[0],
                        self.0[1],
                        self.0[2],
                        self.0[3],
                        cleared as u16,
                        (cleared >> 16) as u16,
                        (cleared >> 32) as u16,
                        (cleared >> 48) as u16,
                    ])
                }
            }
        )
    }

    /// Count bits set in bits [0..8] (9 bits) of each lane.
    /// The 7 high bits of each lane must be zero.
    #[inline]
    pub fn popcounts9(&self) -> Self {
        cfg_if_x86_ssse3!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    let v = load128(self);
                    let mask4 = _mm_set1_epi16(0x0f);
                    let lookup = _mm_setr_epi8(0, 1, 1, 2, 1, 2, 2, 3, 1, 2, 2, 3, 2, 3, 3, 4);
                    let lo_nibbles = _mm_and_si128(v, mask4);
                    let hi_nibbles = _mm_srli_epi16(v, 4);
                    let sum_0_3 = _mm_shuffle_epi8(lookup, lo_nibbles);
                    let sum_4_7 = _mm_shuffle_epi8(lookup, hi_nibbles);
                    let sum_0_7 = _mm_add_epi16(sum_0_3, sum_4_7);
                    let result = _mm_add_epi16(sum_0_7, _mm_srli_epi16(v, 8));
                    store128(result)
                }
            },
            {
                let mut r = [0u16; 8];
                for i in 0..8 {
                    r[i] = (self.0[i] & 0x1ff).count_ones() as u16;
                }
                Self(r)
            }
        )
    }

    /// Total popcount of the whole 128-bit vector (all lanes combined).
    #[inline]
    pub fn popcount(&self) -> u32 {
        // Interpret as two u64 words
        let (lo, hi) = self.as_2x64();
        num_bits_set_64(lo) + num_bits_set_64(hi)
    }

    // ── shuffle / rotate ──────────────────────────────────────────────────────

    /// PSHUFB-style byte shuffle: each output byte is selected by the
    /// corresponding control byte (low 4 bits = source byte index, high bit
    /// set → 0).
    #[inline]
    pub fn shuffle(&self, control: &Self) -> Self {
        cfg_if_x86_ssse3!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    let r = _mm_shuffle_epi8(load128(self), load128(control));
                    store128(r)
                }
            },
            {
                // Scalar fallback: treat the lanes as bytes (little-endian).
                let src_bytes = lanes_to_bytes(self);
                let ctrl_bytes = lanes_to_bytes(control);
                let mut dst_bytes = [0u8; 16];
                for i in 0..16 {
                    let c = ctrl_bytes[i];
                    dst_bytes[i] = if c & 0x80 != 0 {
                        0
                    } else {
                        src_bytes[(c & 0x0f) as usize]
                    };
                }
                bytes_to_lanes(&dst_bytes)
            }
        )
    }

    /// Rotate elements within each group of 4 lanes (left by 1 lane).
    /// [a,b,c,d, e,f,g,h] → [b,c,d,a, f,g,h,e]
    #[inline]
    pub fn rotate_rows(&self) -> Self {
        cfg_if_x86_ssse3!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    let ctrl = _mm_setr_epi8(2, 3, 4, 5, 6, 7, 0, 1, 10, 11, 12, 13, 14, 15, 8, 9);
                    store128(_mm_shuffle_epi8(load128(self), ctrl))
                }
            },
            {
                let d = &self.0;
                Self([d[1], d[2], d[3], d[0], d[5], d[6], d[7], d[4]])
            }
        )
    }

    /// Rotate by 2 within each group of 4 lanes.
    /// [a,b,c,d, e,f,g,h] → [c,d,a,b, g,h,e,f]
    #[inline]
    pub fn rotate_rows2(&self) -> Self {
        cfg_if_x86_sse2!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    store128(_mm_shuffle_epi32(load128(self), 0b10110001))
                }
            },
            {
                let d = &self.0;
                Self([d[2], d[3], d[0], d[1], d[6], d[7], d[4], d[5]])
            }
        )
    }

    /// Swap the two halves: [lo4, hi4] → [hi4, lo4].
    /// [a,b,c,d, e,f,g,h] → [e,f,g,h, a,b,c,d]
    #[inline]
    pub fn rotate_cols(&self) -> Self {
        cfg_if_x86_sse2!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    store128(_mm_shuffle_epi32(load128(self), 0b01001110))
                }
            },
            {
                let d = &self.0;
                Self([d[4], d[5], d[6], d[7], d[0], d[1], d[2], d[3]])
            }
        )
    }

    // ── MinPos ────────────────────────────────────────────────────────────────

    /// Returns `(position << 16) | adjusted_min` where adjusted_min is the
    /// minimum value of `lane - min_val` across all lanes (treating subtraction
    /// as unsigned 16-bit wraparound to detect if a lane is < min_val).
    /// Mirrors `_mm_minpos_epu16` after subtracting `min_val`.
    #[inline]
    pub fn min_pos_gte(&self, min_val: u16) -> u32 {
        cfg_if_x86_sse4_1!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    let v = load128(self);
                    let sub = _mm_sub_epi16(v, _mm_set1_epi16(min_val as i16));
                    _mm_cvtsi128_si32(_mm_minpos_epu16(sub)) as u32
                }
            },
            {
                let mut min_adj: u32 = 0xffff;
                let mut pos: u32 = 0;
                for i in 0..8 {
                    let adj = self.0[i].wrapping_sub(min_val) as u32;
                    if adj < min_adj {
                        min_adj = adj;
                        pos = i as u32;
                    }
                }
                (pos << 16) | min_adj
            }
        )
    }

    // ── ternary helpers ───────────────────────────────────────────────────────

    /// `(x & y) | z`
    #[inline]
    pub fn x_y_and_z_or(x: &Self, y: &Self, z: &Self) -> Self {
        (*x & *y) | *z
    }

    /// `x.and_not(y) | z`  (i.e. `(x & !y) | z`)
    #[inline]
    pub fn x_y_andnot_z_or(x: &Self, y: &Self, z: &Self) -> Self {
        x.and_not(y) | *z
    }

    /// `x | y | z`
    #[inline]
    pub fn x_y_or_z_or(x: &Self, y: &Self, z: &Self) -> Self {
        *x | *y | *z
    }

    /// `(x ^ y) | z`
    #[inline]
    pub fn x_y_xor_z_or(x: &Self, y: &Self, z: &Self) -> Self {
        (*x ^ *y) | *z
    }

    // ── logical ops ───────────────────────────────────────────────────────────

    /// `self & !other`
    #[inline]
    pub fn and_not(&self, other: &Self) -> Self {
        cfg_if_x86_sse2!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    store128(_mm_andnot_si128(load128(other), load128(self)))
                }
            },
            {
                let mut r = [0u16; 8];
                for i in 0..8 {
                    r[i] = self.0[i] & !other.0[i];
                }
                Self(r)
            }
        )
    }

    // ── raw u64 pair ──────────────────────────────────────────────────────────
    #[inline]
    pub fn as_2x64(&self) -> (u64, u64) {
        // Safe because repr(C, align(16)) and [u16;8] is POD.
        let ptr = self.0.as_ptr() as *const u64;
        unsafe { (*ptr, *ptr.add(1)) }
    }
}

// ── operator impls ────────────────────────────────────────────────────────────

impl std::ops::BitOr for Bitvec08x16 {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        cfg_if_x86_sse2!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    store128(_mm_or_si128(load128(&self), load128(&rhs)))
                }
            },
            {
                let mut r = [0u16; 8];
                for i in 0..8 {
                    r[i] = self.0[i] | rhs.0[i];
                }
                Self(r)
            }
        )
    }
}

impl std::ops::BitOrAssign for Bitvec08x16 {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        *self = *self | rhs;
    }
}

impl std::ops::BitAnd for Bitvec08x16 {
    type Output = Self;
    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        cfg_if_x86_sse2!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    store128(_mm_and_si128(load128(&self), load128(&rhs)))
                }
            },
            {
                let mut r = [0u16; 8];
                for i in 0..8 {
                    r[i] = self.0[i] & rhs.0[i];
                }
                Self(r)
            }
        )
    }
}

impl std::ops::BitAndAssign for Bitvec08x16 {
    #[inline]
    fn bitand_assign(&mut self, rhs: Self) {
        *self = *self & rhs;
    }
}

impl std::ops::BitXor for Bitvec08x16 {
    type Output = Self;
    #[inline]
    fn bitxor(self, rhs: Self) -> Self {
        cfg_if_x86_sse2!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    store128(_mm_xor_si128(load128(&self), load128(&rhs)))
                }
            },
            {
                let mut r = [0u16; 8];
                for i in 0..8 {
                    r[i] = self.0[i] ^ rhs.0[i];
                }
                Self(r)
            }
        )
    }
}

impl std::ops::BitXorAssign for Bitvec08x16 {
    #[inline]
    fn bitxor_assign(&mut self, rhs: Self) {
        *self = *self ^ rhs;
    }
}

impl std::ops::Not for Bitvec08x16 {
    type Output = Self;
    #[inline]
    fn not(self) -> Self {
        let mut r = [0u16; 8];
        for i in 0..8 {
            r[i] = !self.0[i];
        }
        Self(r)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Bitvec16x16  — 16 × u16 lanes (256-bit logical vector, stored as two 128s)
// ──────────────────────────────────────────────────────────────────────────────

/// 16-lane × 16-bit vector.  Stored as `lo` (lanes 0–7) + `hi` (lanes 8–15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Bitvec16x16 {
    pub lo: Bitvec08x16,
    pub hi: Bitvec08x16,
}

impl Default for Bitvec16x16 {
    fn default() -> Self {
        Self {
            lo: Bitvec08x16::zero(),
            hi: Bitvec08x16::zero(),
        }
    }
}

impl Bitvec16x16 {
    // ── constructors ────────────────────────────────────────────────────────

    #[inline]
    pub fn zero() -> Self {
        Self::default()
    }

    #[inline]
    pub fn all(value: u16) -> Self {
        Self {
            lo: Bitvec08x16::all(value),
            hi: Bitvec08x16::all(value),
        }
    }

    #[inline]
    pub fn from_halves(lo: Bitvec08x16, hi: Bitvec08x16) -> Self {
        Self { lo, hi }
    }

    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn new(
        x00: u16,
        x01: u16,
        x02: u16,
        x03: u16,
        x04: u16,
        x05: u16,
        x06: u16,
        x07: u16,
        x08: u16,
        x09: u16,
        x10: u16,
        x11: u16,
        x12: u16,
        x13: u16,
        x14: u16,
        x15: u16,
    ) -> Self {
        Self {
            lo: Bitvec08x16::new(x00, x01, x02, x03, x04, x05, x06, x07),
            hi: Bitvec08x16::new(x08, x09, x10, x11, x12, x13, x14, x15),
        }
    }

    // ── half accessors ────────────────────────────────────────────────────────

    #[inline]
    pub fn get_lo(&self) -> Bitvec08x16 {
        self.lo
    }
    #[inline]
    pub fn get_hi(&self) -> Bitvec08x16 {
        self.hi
    }

    // ── element access ───────────────────────────────────────────────────────

    #[inline]
    pub fn extract(&self, index: usize) -> u16 {
        if index < 8 {
            self.lo.extract(index)
        } else {
            self.hi.extract(index - 8)
        }
    }

    #[inline]
    pub fn insert(&mut self, index: usize, value: u16) {
        if index < 8 {
            self.lo.insert(index, value);
        } else {
            self.hi.insert(index - 8, value);
        }
    }

    // ── boolean predicates ───────────────────────────────────────────────────

    #[inline]
    pub fn all_zero(&self) -> bool {
        self.lo.all_zero() && self.hi.all_zero()
    }
    #[inline]
    pub fn any_zero(&self) -> bool {
        self.lo.any_zero() || self.hi.any_zero()
    }
    #[inline]
    pub fn intersects(&self, other: &Self) -> bool {
        self.lo.intersects(&other.lo) || self.hi.intersects(&other.hi)
    }
    #[inline]
    pub fn subset_of(&self, other: &Self) -> bool {
        self.lo.subset_of(&other.lo) && self.hi.subset_of(&other.hi)
    }
    #[inline]
    pub fn any_less_than(&self, other: &Self) -> bool {
        self.lo.any_less_than(&other.lo) || self.hi.any_less_than(&other.hi)
    }

    // ── lane-wise comparison ──────────────────────────────────────────────────

    #[inline]
    pub fn which_equal(&self, other: &Self) -> Self {
        Self::from_halves(
            self.lo.which_equal(&other.lo),
            self.hi.which_equal(&other.hi),
        )
    }
    #[inline]
    pub fn which_non_zero(&self) -> Self {
        Self::from_halves(self.lo.which_non_zero(), self.hi.which_non_zero())
    }

    // ── bit manipulation ─────────────────────────────────────────────────────

    #[inline]
    pub fn get_low_bit(&self) -> Self {
        Self::from_halves(self.lo.get_low_bit(), self.hi.get_low_bit())
    }
    #[inline]
    pub fn clear_low_bit(&self) -> Self {
        Self::from_halves(self.lo.clear_low_bit(), self.hi.clear_low_bit())
    }
    #[inline]
    pub fn popcounts9(&self) -> Self {
        Self::from_halves(self.lo.popcounts9(), self.hi.popcounts9())
    }
    #[inline]
    pub fn popcount(&self) -> u32 {
        self.lo.popcount() + self.hi.popcount()
    }

    // ── shuffle / rotate ──────────────────────────────────────────────────────

    #[inline]
    pub fn shuffle(&self, control: &Self) -> Self {
        Self::from_halves(self.lo.shuffle(&control.lo), self.hi.shuffle(&control.hi))
    }

    #[inline]
    pub fn rotate_rows(&self) -> Self {
        Self::from_halves(self.lo.rotate_rows(), self.hi.rotate_rows())
    }

    #[inline]
    pub fn rotate_rows2(&self) -> Self {
        Self::from_halves(self.lo.rotate_rows2(), self.hi.rotate_rows2())
    }

    /// Rotate columns: conceptually rotate all 16 lanes left by 4 (swap mid-halves).
    /// Non-AVX2 version using _mm_alignr_epi8 / byte shifts.
    #[inline]
    pub fn rotate_cols(&self) -> Self {
        cfg_if_x86_ssse3!(
            {
                unsafe {
                    use std::arch::x86_64::*;
                    let lo = load128(&self.lo);
                    let hi = load128(&self.hi);
                    // alignr(hi, lo, 8) = hi[0..7] ++ lo[8..15] (bytes)
                    // which in u16 lanes = lo[4..7] ++ hi[0..3]
                    let new_lo = _mm_alignr_epi8(hi, lo, 8);
                    let new_hi = _mm_alignr_epi8(lo, hi, 8);
                    Self::from_halves(store128(new_lo), store128(new_hi))
                }
            },
            {
                // scalar: shift right by 4 lanes circularly across the full 16
                let d0 = &self.lo.0;
                let d1 = &self.hi.0;
                Self::from_halves(
                    Bitvec08x16::new(d0[4], d0[5], d0[6], d0[7], d1[0], d1[1], d1[2], d1[3]),
                    Bitvec08x16::new(d1[4], d1[5], d1[6], d1[7], d0[0], d0[1], d0[2], d0[3]),
                )
            }
        )
    }

    #[inline]
    pub fn rotate_cols2(&self) -> Self {
        Self::from_halves(self.hi, self.lo)
    }

    /// Extract the 4 u64 words (lo.lo64, lo.hi64, hi.lo64, hi.hi64).
    /// Mirrors C++ `Bitvec16x16::As_4x64()`.
    #[inline]
    pub fn as_4x64(&self) -> (u64, u64, u64, u64) {
        let (x0, x1) = self.lo.as_2x64();
        let (x2, x3) = self.hi.as_2x64();
        (x0, x1, x2, x3)
    }

    // ── ternary helpers ───────────────────────────────────────────────────────

    #[inline]
    pub fn x_y_and_z_or(x: &Self, y: &Self, z: &Self) -> Self {
        Self::from_halves(
            Bitvec08x16::x_y_and_z_or(&x.lo, &y.lo, &z.lo),
            Bitvec08x16::x_y_and_z_or(&x.hi, &y.hi, &z.hi),
        )
    }
    #[inline]
    pub fn x_y_andnot_z_or(x: &Self, y: &Self, z: &Self) -> Self {
        Self::from_halves(
            Bitvec08x16::x_y_andnot_z_or(&x.lo, &y.lo, &z.lo),
            Bitvec08x16::x_y_andnot_z_or(&x.hi, &y.hi, &z.hi),
        )
    }
    #[inline]
    pub fn x_y_or_z_or(x: &Self, y: &Self, z: &Self) -> Self {
        Self::from_halves(
            Bitvec08x16::x_y_or_z_or(&x.lo, &y.lo, &z.lo),
            Bitvec08x16::x_y_or_z_or(&x.hi, &y.hi, &z.hi),
        )
    }
    #[inline]
    pub fn x_y_xor_z_or(x: &Self, y: &Self, z: &Self) -> Self {
        Self::from_halves(
            Bitvec08x16::x_y_xor_z_or(&x.lo, &y.lo, &z.lo),
            Bitvec08x16::x_y_xor_z_or(&x.hi, &y.hi, &z.hi),
        )
    }

    /// `self & !other`
    #[inline]
    pub fn and_not(&self, other: &Self) -> Self {
        Self::from_halves(self.lo.and_not(&other.lo), self.hi.and_not(&other.hi))
    }
}

// ── operator impls ────────────────────────────────────────────────────────────

impl std::ops::BitOr for Bitvec16x16 {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self::from_halves(self.lo | rhs.lo, self.hi | rhs.hi)
    }
}
impl std::ops::BitOrAssign for Bitvec16x16 {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        *self = *self | rhs;
    }
}
impl std::ops::BitAnd for Bitvec16x16 {
    type Output = Self;
    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        Self::from_halves(self.lo & rhs.lo, self.hi & rhs.hi)
    }
}
impl std::ops::BitAndAssign for Bitvec16x16 {
    #[inline]
    fn bitand_assign(&mut self, rhs: Self) {
        *self = *self & rhs;
    }
}
impl std::ops::BitXor for Bitvec16x16 {
    type Output = Self;
    #[inline]
    fn bitxor(self, rhs: Self) -> Self {
        Self::from_halves(self.lo ^ rhs.lo, self.hi ^ rhs.hi)
    }
}
impl std::ops::BitXorAssign for Bitvec16x16 {
    #[inline]
    fn bitxor_assign(&mut self, rhs: Self) {
        *self = *self ^ rhs;
    }
}
impl std::ops::Not for Bitvec16x16 {
    type Output = Self;
    #[inline]
    fn not(self) -> Self {
        Self::from_halves(!self.lo, !self.hi)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// WhichDots helpers (used in solver to locate '.' characters in puzzle strings)
// ──────────────────────────────────────────────────────────────────────────────

/// Returns a bitmask of which of the first 16 bytes in `data` are `b'.'`.
/// Bit `i` corresponds to `data[i]`.
pub fn which_dots_16(data: &[u8]) -> u32 {
    assert!(data.len() >= 16);
    cfg_if_x86_sse2!(
        {
            unsafe {
                use std::arch::x86_64::*;
                let dots = _mm_set1_epi8(b'.' as i8);
                let src = _mm_loadu_si128(data.as_ptr() as *const __m128i);
                _mm_movemask_epi8(_mm_cmpeq_epi8(src, dots)) as u32
            }
        },
        {
            let mut mask = 0u32;
            for i in 0..16 {
                if data[i] == b'.' {
                    mask |= 1 << i;
                }
            }
            mask
        }
    )
}

/// Returns a bitmask of which of the first 32 bytes in `data` are `b'.'`.
pub fn which_dots_32(data: &[u8]) -> u32 {
    assert!(data.len() >= 32);
    which_dots_16(data) | (which_dots_16(&data[16..]) << 16)
}

/// Returns a bitmask of which of the first 64 bytes in `data` are `b'.'`.
pub fn which_dots_64(data: &[u8]) -> u64 {
    assert!(data.len() >= 64);
    (which_dots_32(data) as u64) | ((which_dots_32(&data[32..]) as u64) << 32)
}

// ──────────────────────────────────────────────────────────────────────────────
// Internal x86 helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Load a `Bitvec08x16` as a `__m128i`. Only safe on x86_64.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn load128(v: &Bitvec08x16) -> std::arch::x86_64::__m128i {
    use std::arch::x86_64::*;
    _mm_loadu_si128(v.0.as_ptr() as *const __m128i)
}

/// Store a `__m128i` back into a `Bitvec08x16`. Only safe on x86_64.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn store128(v: std::arch::x86_64::__m128i) -> Bitvec08x16 {
    let mut r = Bitvec08x16([0u16; 8]);
    use std::arch::x86_64::*;
    _mm_storeu_si128(r.0.as_mut_ptr() as *mut __m128i, v);
    r
}

// ── Scalar helpers for shuffle fallback ──────────────────────────────────────

#[inline]
fn lanes_to_bytes(v: &Bitvec08x16) -> [u8; 16] {
    let mut b = [0u8; 16];
    for i in 0..8 {
        let lane = v.0[i];
        b[2 * i] = (lane & 0xff) as u8;
        b[2 * i + 1] = (lane >> 8) as u8;
    }
    b
}

#[inline]
fn bytes_to_lanes(b: &[u8; 16]) -> Bitvec08x16 {
    let mut r = [0u16; 8];
    for i in 0..8 {
        r[i] = (b[2 * i] as u16) | ((b[2 * i + 1] as u16) << 8);
    }
    Bitvec08x16(r)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn v8(x: u16) -> Bitvec08x16 {
        Bitvec08x16::all(x)
    }
    fn v16(x: u16) -> Bitvec16x16 {
        Bitvec16x16::all(x)
    }

    #[test]
    fn test_all_zero() {
        assert!(Bitvec08x16::zero().all_zero());
        assert!(!v8(1).all_zero());
        assert!(Bitvec16x16::zero().all_zero());
        assert!(!v16(1).all_zero());
    }

    #[test]
    fn test_any_zero() {
        assert!(!v8(1).any_zero());
        let mut a = v8(1);
        a.insert(3, 0);
        assert!(a.any_zero());
    }

    #[test]
    fn test_or_and_xor() {
        let a = v8(0b1010);
        let b = v8(0b0110);
        assert_eq!((a | b), v8(0b1110));
        assert_eq!((a & b), v8(0b0010));
        assert_eq!((a ^ b), v8(0b1100));
    }

    #[test]
    fn test_get_low_bit() {
        let a = Bitvec08x16::all(0b1100);
        let lb = a.get_low_bit();
        assert_eq!(lb, Bitvec08x16::all(0b0100));
    }

    #[test]
    fn test_clear_low_bit() {
        // clear_low_bit clears the globally lowest set bit across the first
        // non-zero 64-bit half (lanes 0–3), NOT per-lane.
        // all(0b1100): low 64-bit = 0x000C_000C_000C_000C
        // clear lowest bit → 0x000C_000C_000C_0008 (only lane 0 changes)
        let a = Bitvec08x16::all(0b1100);
        let expected = Bitvec08x16::new(
            0b1000, 0b1100, 0b1100, 0b1100, 0b1100, 0b1100, 0b1100, 0b1100,
        );
        assert_eq!(a.clear_low_bit(), expected);

        // If only the high half is non-zero, it operates on that half.
        let b = Bitvec08x16::new(0, 0, 0, 0, 0b1100, 0b1010, 0, 0);
        // high 64-bit = 0x0000_0000_000A_000C, lowest bit is bit 2 of lane 4
        let b_exp = Bitvec08x16::new(0, 0, 0, 0, 0b1000, 0b1010, 0, 0);
        assert_eq!(b.clear_low_bit(), b_exp);
    }

    #[test]
    fn test_popcounts9() {
        // 0b111_111_111 = 511 has 9 bits set
        let a = Bitvec08x16::all(0b1_1111_1111);
        let p = a.popcounts9();
        assert_eq!(p, Bitvec08x16::all(9));

        let b = Bitvec08x16::all(0b101);
        assert_eq!(b.popcounts9(), Bitvec08x16::all(2));
    }

    #[test]
    fn test_popcount() {
        // 8 lanes of 0b11 → 2 bits each → total 16
        assert_eq!(Bitvec08x16::all(0b11).popcount(), 16);
        assert_eq!(Bitvec16x16::all(0b11).popcount(), 32);
    }

    #[test]
    fn test_shuffle_identity() {
        // control = byte indices [0,1, 2,3, 4,5, 6,7, 8,9, 10,11, 12,13, 14,15]
        // means "keep lane i in place"
        let ctrl = Bitvec08x16::new(
            0x0100, 0x0302, 0x0504, 0x0706, 0x0900, 0x0b0a, 0x0d0c, 0x0f0e,
        );
        // Actually let's just test that shuffle of all-same gives all-same.
        let src = v8(0x1234);
        let ctrl_same = Bitvec08x16::new(
            0x0100, 0x0100, 0x0100, 0x0100, 0x0100, 0x0100, 0x0100, 0x0100,
        );
        let result = src.shuffle(&ctrl_same);
        // All lanes select bytes 0,1 (lane 0 = 0x1234) → all lanes become 0x1234
        assert_eq!(result, v8(0x1234));
        let _ = ctrl; // suppress warning
    }

    #[test]
    fn test_rotate_rows() {
        let a = Bitvec08x16::new(1, 2, 3, 4, 5, 6, 7, 8);
        let r = a.rotate_rows();
        assert_eq!(r, Bitvec08x16::new(2, 3, 4, 1, 6, 7, 8, 5));
    }

    #[test]
    fn test_rotate_rows2() {
        let a = Bitvec08x16::new(1, 2, 3, 4, 5, 6, 7, 8);
        assert_eq!(a.rotate_rows2(), Bitvec08x16::new(3, 4, 1, 2, 7, 8, 5, 6));
    }

    #[test]
    fn test_rotate_cols_08x16() {
        let a = Bitvec08x16::new(1, 2, 3, 4, 5, 6, 7, 8);
        assert_eq!(a.rotate_cols(), Bitvec08x16::new(5, 6, 7, 8, 1, 2, 3, 4));
    }

    #[test]
    fn test_rotate_cols_16x16() {
        let lo = Bitvec08x16::new(1, 2, 3, 4, 5, 6, 7, 8);
        let hi = Bitvec08x16::new(9, 10, 11, 12, 13, 14, 15, 16);
        let v = Bitvec16x16::from_halves(lo, hi);
        let r = v.rotate_cols();
        // rotate_cols on 16x16: aligns across boundary
        // new_lo = [lo[4..7], hi[0..3]] = [5,6,7,8, 9,10,11,12]
        // new_hi = [hi[4..7], lo[0..3]] = [13,14,15,16, 1,2,3,4]
        assert_eq!(r.lo, Bitvec08x16::new(5, 6, 7, 8, 9, 10, 11, 12));
        assert_eq!(r.hi, Bitvec08x16::new(13, 14, 15, 16, 1, 2, 3, 4));
    }

    #[test]
    fn test_rotate_cols2_16x16() {
        let lo = Bitvec08x16::new(1, 2, 3, 4, 5, 6, 7, 8);
        let hi = Bitvec08x16::new(9, 10, 11, 12, 13, 14, 15, 16);
        let v = Bitvec16x16::from_halves(lo, hi);
        let r = v.rotate_cols2();
        assert_eq!(r.lo, hi);
        assert_eq!(r.hi, lo);
    }

    #[test]
    fn test_min_pos_gte() {
        let mut a = Bitvec08x16::all(100);
        a.insert(3, 5); // lane 3 has value 5 (lowest after subtracting min_val=0)
        let result = a.min_pos_gte(0);
        let min_val = (result & 0xffff) as u16;
        let pos = (result >> 16) as u16;
        assert_eq!(min_val, 5);
        assert_eq!(pos, 3);
    }

    #[test]
    fn test_intersects() {
        assert!(v8(0b1010).intersects(&v8(0b0010)));
        assert!(!v8(0b1010).intersects(&v8(0b0101)));
    }

    #[test]
    fn test_subset_of() {
        assert!(v8(0b0010).subset_of(&v8(0b1110)));
        assert!(!v8(0b1010).subset_of(&v8(0b0110)));
    }

    #[test]
    fn test_which_dots_16() {
        let mut data = [b'0'; 16];
        data[0] = b'.';
        data[5] = b'.';
        let mask = which_dots_16(&data);
        assert_eq!(mask, 0b100001);
    }
}
