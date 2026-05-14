//! Sudoku grid enumeration utilities — port of `tdoku/src/grid_lib.h` / `grid_lib.cc`.
//!
//! Provides canonical puzzle-pattern generation and indexed grid retrieval
//! used by the puzzle generator.

use crate::solver_basic;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const PERMUTATIONS: [[usize; 3]; 6] = [
    [0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0],
];

/// Template with digits 1-9 filling box 1 (top-left 3×3), rest empty.
const BOX1_PATTERN_TEMPLATE: &[u8; 81] =
    b"123......456......789............................................................";

/// Total number of band configurations: 28 * 6^4.
const N_BAND_CONFIGS: usize = 28 * 6 * 6 * 6 * 6;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Read a little-endian `u32` from `bytes` starting at `offset`.
#[inline]
fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

/// Read a little-endian `u16` from `bytes` starting at `offset`.
#[inline]
fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

/// Maps (band, position) → flat index using horizontal (row-major) indexing.
#[inline]
fn horiz_indexing(x: usize, y: usize) -> usize {
    x * 9 + y
}

/// Maps (band, position) → flat index using vertical (col-major) indexing.
#[inline]
fn verti_indexing(x: usize, y: usize) -> usize {
    y * 9 + x
}

/// Fills the horizontal or vertical bands of `pattern` according to `configuration`.
/// Direct port of C++ `BandInit`.
fn band_init(configuration: usize, indexing: fn(usize, usize) -> usize, pattern: &mut [u8; 81]) {
    let p: [[usize; 2]; 3] = [
        [0, 0],
        [configuration % 6, (configuration / 6) % 6],
        [(configuration / 36) % 6, (configuration / 216) % 6],
    ];
    let picks_raw = configuration / 1296;
    let pick: [usize; 3] = [picks_raw % 3, (picks_raw / 3) % 3, picks_raw / 9];

    for i in 0..3 {
        if picks_raw == 27 {
            for j in 0..3 {
                let src0 = pattern[indexing((i + 1) % 3, j)];
                let src1 = pattern[indexing((i + 2) % 3, j)];
                pattern[indexing(i, PERMUTATIONS[p[i][0]][j] + 3)] = src0;
                pattern[indexing(i, PERMUTATIONS[p[i][1]][j] + 6)] = src1;
            }
        } else {
            pattern[indexing(i, PERMUTATIONS[p[i][0]][pick[i] % 3] + 3)] =
                pattern[indexing((i + 2) % 3, pick[(i + 2) % 3] % 3)];
            pattern[indexing(i, PERMUTATIONS[p[i][0]][(pick[i] + 1) % 3] + 3)] =
                pattern[indexing((i + 1) % 3, (pick[(i + 1) % 3] + 1) % 3)];
            pattern[indexing(i, PERMUTATIONS[p[i][0]][(pick[i] + 2) % 3] + 3)] =
                pattern[indexing((i + 1) % 3, (pick[(i + 1) % 3] + 2) % 3)];

            pattern[indexing(i, PERMUTATIONS[p[i][1]][pick[i] % 3] + 6)] =
                pattern[indexing((i + 1) % 3, pick[(i + 1) % 3] % 3)];
            pattern[indexing(i, PERMUTATIONS[p[i][1]][(pick[i] + 1) % 3] + 6)] =
                pattern[indexing((i + 2) % 3, (pick[(i + 2) % 3] + 1) % 3)];
            pattern[indexing(i, PERMUTATIONS[p[i][1]][(pick[i] + 2) % 3] + 6)] =
                pattern[indexing((i + 2) % 3, (pick[(i + 2) % 3] + 2) % 3)];
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns the 81-byte pattern string for the given `pattern_id`.
///
/// Port of C++ `GetPattern`. The returned array contains ASCII digits `'1'`–`'9'`
/// and `'.'` characters representing the canonical pattern for that id.
pub fn get_pattern(pattern_id: usize) -> [u8; 81] {
    let mut pattern = *BOX1_PATTERN_TEMPLATE;
    band_init(pattern_id % N_BAND_CONFIGS, horiz_indexing, &mut pattern);
    band_init(pattern_id / N_BAND_CONFIGS, verti_indexing, &mut pattern);
    pattern
}

/// Stride between index entries: every 2^20 grids gets one entry.
#[allow(dead_code)]
const INDEX_STEP: usize = 1 << 20;

/// Returns the solved grid at position `grid_idx` using precomputed `index` and `table` data.
///
/// `index` is the contents of `grid.index` (6 bytes per entry: u32 pattern_idx + u16 offset).
/// `table` is the contents of `grid.counts` (u16 per pattern: number of completions).
///
/// Port of C++ `GetGrid`.
pub fn get_grid(grid_idx: usize, index: &[u8], table: &[u8]) -> [u8; 81] {
    let indexed_grid_idx = grid_idx & !((1usize << 20) - 1);

    let entry_base = (grid_idx >> 20) * 6;
    let current_pattern_idx_init = read_u32_le(index, entry_base) as usize;
    let indexed_grid_offset = read_u16_le(index, entry_base + 4) as usize;

    let mut to_skip = indexed_grid_offset + (grid_idx - indexed_grid_idx);
    let mut current_pattern_idx = current_pattern_idx_init;
    let mut pattern_count = read_u16_le(table, current_pattern_idx * 2) as usize;

    while to_skip >= pattern_count {
        to_skip -= pattern_count;
        current_pattern_idx += 1;
        pattern_count = read_u16_le(table, current_pattern_idx * 2) as usize;
    }

    let pattern = get_pattern(current_pattern_idx);
    let mut solution = [0u8; 81];
    let (_, sol, _) = solver_basic::solve(&pattern, to_skip + 1, 1);
    solution.copy_from_slice(&sol);
    solution
}

/// Enumerates `count` grids starting at `first_grid_idx`, calling `callback` for each.
///
/// Port of C++ `EnumerateGrids`. Uses `index`/`table` binary data to navigate patterns,
/// then enumerates completions via the basic solver.
pub fn enumerate_grids(
    first_grid_idx: usize,
    count: usize,
    index: &[u8],
    table: &[u8],
    mut callback: impl FnMut(&[u8; 81]),
) {
    let indexed_grid_idx = first_grid_idx & !((1usize << 20) - 1);

    let entry_base = (first_grid_idx >> 20) * 6;
    let current_pattern_idx_init = read_u32_le(index, entry_base) as usize;
    let indexed_grid_offset = read_u16_le(index, entry_base + 4) as usize;

    let mut to_skip = indexed_grid_offset + (first_grid_idx - indexed_grid_idx);
    let mut current_pattern_idx = current_pattern_idx_init;
    let mut pattern_count = read_u16_le(table, current_pattern_idx * 2) as usize;

    while to_skip >= pattern_count {
        to_skip -= pattern_count;
        current_pattern_idx += 1;
        pattern_count = read_u16_le(table, current_pattern_idx * 2) as usize;
    }

    let mut remaining = count;
    while remaining > 0 {
        let limit = to_skip + remaining;
        let limit = limit.min(pattern_count);

        let pattern = get_pattern(current_pattern_idx);

        // Enumerate completions: collect via the basic solver (limit solutions)
        // and deliver to callback, skipping the first `to_skip`.
        let mut local_skip = to_skip;

        // We enumerate one solution at a time using rank (solve up to rank+1, take last).
        // For correctness, iterate solution ranks from to_skip to limit-1.
        for rank in to_skip..limit {
            let (n, sol, _) = solver_basic::solve(&pattern, rank + 1, 1);
            if n == rank + 1 {
                if local_skip > 0 {
                    local_skip -= 1;
                } else {
                    callback(&sol);
                    remaining -= 1;
                    if remaining == 0 {
                        break;
                    }
                }
            }
        }

        to_skip = 0;
        current_pattern_idx += 1;
        if current_pattern_idx * 2 + 2 <= table.len() {
            pattern_count = u16::from_le_bytes(
                table[current_pattern_idx * 2..current_pattern_idx * 2 + 2]
                    .try_into()
                    .unwrap(),
            ) as usize;
        } else {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_pattern_template_unchanged() {
        // Pattern 0 with config 0: BandInit(0, horiz) picks_raw=0 (< 27)
        // and BandInit(0, verti) picks_raw=0. Both configs have picks_raw = 0/1296 = 0.
        // pick = [0,0,0], p = [[0,0],[0,0],[0,0]].
        // This fills in rows of zeros so let's just check basic shape.
        let pattern = get_pattern(0);
        assert_eq!(pattern.len(), 81);
        // The first 3 bytes should be b'1', b'2', b'3' (from template)
        assert_eq!(&pattern[0..3], b"123");
    }

    #[test]
    fn test_get_pattern_only_digits_and_dots() {
        // All bytes in a pattern should be ASCII digits 1-9 or '.'.
        let pattern = get_pattern(0);
        for &b in pattern.iter() {
            assert!(
                b == b'.' || (b >= b'1' && b <= b'9'),
                "unexpected byte {}",
                b
            );
        }
    }

    #[test]
    fn test_get_pattern_varies_with_id() {
        let p0 = get_pattern(0);
        let p1 = get_pattern(1);
        assert_ne!(p0, p1);
    }
}
