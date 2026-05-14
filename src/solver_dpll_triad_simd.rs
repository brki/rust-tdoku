//! Fastest solver: DPLL with triad constraints and SIMD constraint propagation —
//! port of `tdoku/src/solver_dpll_triad_simd.cc`.
//!
//! Represents puzzle state as SIMD vectors of candidate bitmasks for boxes
//! and band configuration masks. Uses `Bitvec08x16` / `Bitvec16x16` from
//! `simd_vectors` for all hot-path operations.

use std::sync::OnceLock;

use crate::bitutil::{clear_low_bit, clear_low_bit64, low_order_bit_index, low_order_bit_index64};
use crate::simd_vectors::{which_dots_16, which_dots_64, Bitvec08x16, Bitvec16x16};

// ──────────────────────────────────────────────────────────────────────────────
// Debug trace (enabled via --features debug-trace)
// Format (emitted to stderr, prefixed "DT:"):
//   DT:INIT ok=<0|1> pcs=<p00>,<p01>,<p02>,<p10>,<p11>,<p12>
//   DT:C d=<depth> pcs=<p00>,<p01>,<p02>,<p10>,<p11>,<p12>
//   DT:T d=<depth> best=<0-5 or 4294967295=NONE>
//   DT:S d=<depth> n=<total>
// Emits at most DT_MAX events to prevent flooding on empty-grid inputs.
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "debug-trace")]
const DT_MAX: i32 = 2000;

#[cfg(feature = "debug-trace")]
thread_local! {
    static DT_DEPTH:  std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    static DT_EVENTS: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// Increment event counter; returns true if this event should be emitted.
#[cfg(feature = "debug-trace")]
#[inline]
fn dt_check_and_inc() -> bool {
    DT_EVENTS.with(|e| {
        let v = e.get();
        e.set(v + 1);
        v < DT_MAX
    })
}

/// Format the six band configuration popcounts from `state`.
#[cfg(feature = "debug-trace")]
fn dt_pcs(state: &State) -> String {
    format!(
        "pcs={},{},{},{},{},{}",
        state.bands[0][0].configurations.popcount(),
        state.bands[0][1].configurations.popcount(),
        state.bands[0][2].configurations.popcount(),
        state.bands[1][0].configurations.popcount(),
        state.bands[1][1].configurations.popcount(),
        state.bands[1][2].configurations.popcount(),
    )
}

// ──────────────────────────────────────────────────────────────────────────────
// Constants matching C++ shufXX and kAll
// ──────────────────────────────────────────────────────────────────────────────

const K_ALL: u16 = 0x1ff;

// 16-bit shuffle control words that address 16-bit cells by their two constituent bytes.
const SHUF00: u16 = 0x0100;
const SHUF01: u16 = 0x0302;
const SHUF02: u16 = 0x0504;
const SHUF03: u16 = 0x0706;
const SHUF04: u16 = 0x0908;
const SHUF05: u16 = 0x0b0a;
const SHUF06: u16 = 0x0d0c;
const SHUF07: u16 = 0x0f0e;

// ──────────────────────────────────────────────────────────────────────────────
// Data structures
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Box {
    cells: Bitvec16x16,
}

impl Default for Box {
    fn default() -> Self {
        Self {
            cells: Bitvec16x16::all(K_ALL),
        }
    }
}

#[derive(Clone, Copy)]
struct Band {
    configurations: Bitvec08x16,
    eliminations: Bitvec08x16,
}

impl Default for Band {
    fn default() -> Self {
        Self {
            configurations: Bitvec08x16::new(K_ALL, K_ALL, K_ALL, K_ALL, K_ALL, K_ALL, 0, 0),
            eliminations: Bitvec08x16::default(),
        }
    }
}

#[derive(Clone, Default)]
struct State {
    bands: [[Band; 3]; 2],
    boxen: [Box; 9],
}

#[derive(Clone, Copy, Default)]
struct BoxIndexing {
    box_i: u8,
    box_j: u8,
    r#box: u8,
    elem_i: u8,
    elem_j: u8,
    elem: u8,
}

impl BoxIndexing {
    fn from_cell(cell: usize) -> Self {
        let box_i = (cell / 27) as u8;
        let box_j = ((cell % 9) / 3) as u8;
        let r#box = box_i * 3 + box_j;
        let elem_i = ((cell / 9) % 3) as u8;
        let elem_j = (cell % 3) as u8;
        let elem = elem_i * 4 + elem_j;
        Self {
            box_i,
            box_j,
            r#box,
            elem_i,
            elem_j,
            elem,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tables — initialized once via OnceLock
// ──────────────────────────────────────────────────────────────────────────────

struct Tables {
    cell_assignment_eliminations: [[Bitvec16x16; 16]; 9],
    peer_x_elem_to_config_mask: [[Bitvec08x16; 4]; 3],
    triads_shift0_to_config_elims: [Bitvec08x16; 3],
    triads_shift1_to_config_elims: [Bitvec08x16; 3],
    triads_shift2_to_config_elims: [Bitvec08x16; 3],
    triads_shift0_to_config_elims16: [Bitvec16x16; 9],
    triads_shift1_to_config_elims16: [Bitvec16x16; 9],
    triads_shift2_to_config_elims16: [Bitvec16x16; 9],
    shuffle_configs_to_triads: [Bitvec16x16; 2],
    pos_triads_to_candidates: [[Bitvec16x16; 2]; 2],
    cell3x3_mask: Bitvec16x16,
    row_rotate_3x3_1: Bitvec16x16,
    row_rotate_3x3_2: Bitvec16x16,
    #[allow(dead_code)]
    one_value_mask: [Bitvec08x16; 9],
    box_peers: [[[usize; 3]; 3]; 2],
    div3: [usize; 9],
    mod3: [usize; 9],
    box_indexing: [BoxIndexing; 81],
}

impl Tables {
    fn new() -> Self {
        let mut t = Self {
            cell_assignment_eliminations: [[Bitvec16x16::default(); 16]; 9],
            peer_x_elem_to_config_mask: [
                [
                    Bitvec08x16::new(0, K_ALL, K_ALL, K_ALL, 0, K_ALL, 0, 0),
                    Bitvec08x16::new(K_ALL, 0, K_ALL, K_ALL, K_ALL, 0, 0, 0),
                    Bitvec08x16::new(K_ALL, K_ALL, 0, 0, K_ALL, K_ALL, 0, 0),
                    Bitvec08x16::new(0, 0, 0, 0, 0, 0, 0, 0),
                ],
                [
                    Bitvec08x16::new(K_ALL, K_ALL, 0, K_ALL, K_ALL, 0, 0, 0),
                    Bitvec08x16::new(0, K_ALL, K_ALL, 0, K_ALL, K_ALL, 0, 0),
                    Bitvec08x16::new(K_ALL, 0, K_ALL, K_ALL, 0, K_ALL, 0, 0),
                    Bitvec08x16::new(0, 0, 0, 0, 0, 0, 0, 0),
                ],
                [
                    Bitvec08x16::new(K_ALL, 0, K_ALL, 0, K_ALL, K_ALL, 0, 0),
                    Bitvec08x16::new(K_ALL, K_ALL, 0, K_ALL, 0, K_ALL, 0, 0),
                    Bitvec08x16::new(0, K_ALL, K_ALL, K_ALL, K_ALL, 0, 0, 0),
                    Bitvec08x16::new(0, 0, 0, 0, 0, 0, 0, 0),
                ],
            ],
            triads_shift0_to_config_elims: [
                Bitvec08x16::new(
                    SHUF04, SHUF05, SHUF06, SHUF06, SHUF04, SHUF05, 0xffff, 0xffff,
                ),
                Bitvec08x16::new(
                    SHUF05, SHUF06, SHUF04, SHUF05, SHUF06, SHUF04, 0xffff, 0xffff,
                ),
                Bitvec08x16::new(
                    SHUF06, SHUF04, SHUF05, SHUF04, SHUF05, SHUF06, 0xffff, 0xffff,
                ),
            ],
            triads_shift1_to_config_elims: [
                Bitvec08x16::new(
                    SHUF05, SHUF06, SHUF04, SHUF04, SHUF05, SHUF06, 0xffff, 0xffff,
                ),
                Bitvec08x16::new(
                    SHUF06, SHUF04, SHUF05, SHUF06, SHUF04, SHUF05, 0xffff, 0xffff,
                ),
                Bitvec08x16::new(
                    SHUF04, SHUF05, SHUF06, SHUF05, SHUF06, SHUF04, 0xffff, 0xffff,
                ),
            ],
            triads_shift2_to_config_elims: [
                Bitvec08x16::new(
                    SHUF06, SHUF04, SHUF05, SHUF05, SHUF06, SHUF04, 0xffff, 0xffff,
                ),
                Bitvec08x16::new(
                    SHUF04, SHUF05, SHUF06, SHUF04, SHUF05, SHUF06, 0xffff, 0xffff,
                ),
                Bitvec08x16::new(
                    SHUF05, SHUF06, SHUF04, SHUF06, SHUF04, SHUF05, 0xffff, 0xffff,
                ),
            ],
            triads_shift0_to_config_elims16: [Bitvec16x16::default(); 9],
            triads_shift1_to_config_elims16: [Bitvec16x16::default(); 9],
            triads_shift2_to_config_elims16: [Bitvec16x16::default(); 9],
            shuffle_configs_to_triads: [
                Bitvec16x16::new(
                    SHUF00, SHUF01, SHUF02, 0xffff, SHUF02, SHUF00, SHUF01, 0xffff, SHUF01, SHUF02,
                    SHUF00, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff,
                ),
                Bitvec16x16::new(
                    SHUF04, SHUF05, SHUF03, 0xffff, SHUF05, SHUF03, SHUF04, 0xffff, SHUF03, SHUF04,
                    SHUF05, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff,
                ),
            ],
            pos_triads_to_candidates: [
                // horizontal
                [
                    Bitvec16x16::new(
                        SHUF00, SHUF00, SHUF00, SHUF01, SHUF01, SHUF01, SHUF01, SHUF02, SHUF02,
                        SHUF02, SHUF02, SHUF00, SHUF03, SHUF03, SHUF03, SHUF03,
                    ),
                    Bitvec16x16::new(
                        SHUF00, SHUF00, SHUF00, SHUF02, SHUF01, SHUF01, SHUF01, SHUF00, SHUF02,
                        SHUF02, SHUF02, SHUF01, SHUF03, SHUF03, SHUF03, SHUF03,
                    ),
                ],
                // vertical
                [
                    Bitvec16x16::new(
                        SHUF00, SHUF01, SHUF02, SHUF03, SHUF00, SHUF01, SHUF02, SHUF03, SHUF00,
                        SHUF01, SHUF02, SHUF03, SHUF01, SHUF02, SHUF00, SHUF03,
                    ),
                    Bitvec16x16::new(
                        SHUF00, SHUF01, SHUF02, SHUF03, SHUF00, SHUF01, SHUF02, SHUF03, SHUF00,
                        SHUF01, SHUF02, SHUF03, SHUF02, SHUF00, SHUF01, SHUF03,
                    ),
                ],
            ],
            cell3x3_mask: Bitvec16x16::new(
                K_ALL, K_ALL, K_ALL, 0, K_ALL, K_ALL, K_ALL, 0, K_ALL, K_ALL, K_ALL, 0, 0, 0, 0, 0,
            ),
            row_rotate_3x3_1: Bitvec16x16::new(
                SHUF01, SHUF02, SHUF00, SHUF03, SHUF05, SHUF06, SHUF04, SHUF07, SHUF01, SHUF02,
                SHUF00, SHUF03, SHUF04, SHUF05, SHUF06, SHUF07,
            ),
            row_rotate_3x3_2: Bitvec16x16::new(
                SHUF02, SHUF00, SHUF01, SHUF03, SHUF06, SHUF04, SHUF05, SHUF07, SHUF02, SHUF00,
                SHUF01, SHUF03, SHUF04, SHUF05, SHUF06, SHUF07,
            ),
            one_value_mask: [
                Bitvec08x16::all(1u16 << 0),
                Bitvec08x16::all(1u16 << 1),
                Bitvec08x16::all(1u16 << 2),
                Bitvec08x16::all(1u16 << 3),
                Bitvec08x16::all(1u16 << 4),
                Bitvec08x16::all(1u16 << 5),
                Bitvec08x16::all(1u16 << 6),
                Bitvec08x16::all(1u16 << 7),
                Bitvec08x16::all(1u16 << 8),
            ],
            box_peers: [
                [[0, 1, 2], [3, 4, 5], [6, 7, 8]],
                [[0, 3, 6], [1, 4, 7], [2, 5, 8]],
            ],
            div3: [0, 0, 0, 1, 1, 1, 2, 2, 2],
            mod3: [0, 1, 2, 0, 1, 2, 0, 1, 2],
            box_indexing: [BoxIndexing::default(); 81],
        };

        // Build cell_assignment_eliminations
        for i in [0usize, 1, 2, 4, 5, 6, 8, 9, 10] {
            for value in 0..9usize {
                let mask = &mut t.cell_assignment_eliminations[value][i];
                for j in 0..15usize {
                    if j == i {
                        // asserted cell: clear all bits except the one asserted
                        mask.insert(j, K_ALL ^ (1u16 << value));
                    } else if j / 4 < 3 && j % 4 < 3 {
                        // conflict cell: clear the asserted bit
                        mask.insert(j, 1u16 << value);
                    } else if j / 4 == i / 4 || j % 4 == i % 4 {
                        // row/col triad: clear the asserted bit
                        mask.insert(j, 1u16 << value);
                    }
                }
            }
        }

        // Build triads_shiftN_to_config_elims16 as Cells16 pairs
        for i in 0..3usize {
            for j in 0..3usize {
                let idx = i * 3 + j;
                t.triads_shift0_to_config_elims16[idx] = Bitvec16x16::from_halves(
                    t.triads_shift0_to_config_elims[i],
                    t.triads_shift0_to_config_elims[j],
                );
                t.triads_shift1_to_config_elims16[idx] = Bitvec16x16::from_halves(
                    t.triads_shift1_to_config_elims[i],
                    t.triads_shift1_to_config_elims[j],
                );
                t.triads_shift2_to_config_elims16[idx] = Bitvec16x16::from_halves(
                    t.triads_shift2_to_config_elims[i],
                    t.triads_shift2_to_config_elims[j],
                );
            }
        }

        // Build box_indexing
        for i in 0..81usize {
            t.box_indexing[i] = BoxIndexing::from_cell(i);
        }

        t
    }
}

// SAFETY: Tables contains only Copy types with no interior mutability.
unsafe impl Sync for Tables {}
unsafe impl Send for Tables {}

static TABLES: OnceLock<Tables> = OnceLock::new();

fn tables() -> &'static Tables {
    TABLES.get_or_init(Tables::new)
}

// ──────────────────────────────────────────────────────────────────────────────
// The solver struct — generic over solution_mode
// solution_mode == 0 : count solutions only
// solution_mode == 1 : return last/only solution
// ──────────────────────────────────────────────────────────────────────────────

struct SolverDpllTriadSimd<const SOLUTION_MODE: u8> {
    solution: State,
    limit: usize,
    num_solutions: usize,
    num_guesses: usize,
}

impl<const SOLUTION_MODE: u8> Default for SolverDpllTriadSimd<SOLUTION_MODE> {
    fn default() -> Self {
        Self {
            solution: State::default(),
            limit: 1,
            num_solutions: 0,
            num_guesses: 0,
        }
    }
}

impl<const SOLUTION_MODE: u8> SolverDpllTriadSimd<SOLUTION_MODE> {
    // ── BoxRestrict ──────────────────────────────────────────────────────────

    fn box_restrict<const FROM_VERTICAL: bool>(
        state: &mut State,
        box_idx: usize,
        candidates: &Bitvec16x16,
    ) -> bool {
        if state.boxen[box_idx].cells.subset_of(candidates) {
            return true;
        }
        let mut eliminating = state.boxen[box_idx].cells.and_not(candidates);

        let t = tables();
        let box_i = t.div3[box_idx];
        let box_j = t.mod3[box_idx];

        loop {
            state.boxen[box_idx].cells = state.boxen[box_idx].cells.and_not(&eliminating);
            let cells = state.boxen[box_idx].cells;
            let counts = cells.popcounts9();

            let box_minimums = Bitvec16x16::new(1, 1, 1, 6, 1, 1, 1, 6, 1, 1, 1, 6, 6, 6, 6, 0);
            if counts.any_less_than(&box_minimums) {
                return false;
            }

            let triggered = counts.which_equal(&box_minimums);
            let mut all_assertions = cells & triggered;
            Self::gather_triad_clause_assertions(&cells, |x| x.rotate_rows(), &mut all_assertions);
            Self::gather_triad_clause_assertions(&cells, |x| x.rotate_cols(), &mut all_assertions);

            {
                // Split into two mutable slices to avoid aliasing issues.
                // bands[0] and bands[1] are separate array elements.
                let (h_bands, v_bands) = state.bands.split_at_mut(1);
                let h_elims = &mut h_bands[0][box_i].eliminations;
                let v_elims = &mut v_bands[0][box_j].eliminations;
                Self::assertions_to_eliminations(
                    &all_assertions,
                    box_i,
                    box_j,
                    &mut eliminating,
                    h_elims,
                    v_elims,
                );
            }

            if !eliminating.intersects(&state.boxen[box_idx].cells) {
                break;
            }
        }

        #[allow(clippy::if_same_then_else)]
        if FROM_VERTICAL {
            Self::band_eliminate::<false>(state, box_i, box_j)
                && Self::band_eliminate::<true>(state, box_j, box_i)
        } else {
            Self::band_eliminate::<true>(state, box_j, box_i)
                && Self::band_eliminate::<false>(state, box_i, box_j)
        }
    }

    // ── AssertionsToEliminations ─────────────────────────────────────────────

    #[inline]
    fn assertions_to_eliminations(
        assertions: &Bitvec16x16,
        box_i: usize,
        box_j: usize,
        box_eliminations: &mut Bitvec16x16,
        h_band_eliminations: &mut Bitvec08x16,
        v_band_eliminations: &mut Bitvec08x16,
    ) {
        let t = tables();
        let cell_assertions_only = *assertions & t.cell3x3_mask;

        let mut across_rows = cell_assertions_only;
        across_rows |= across_rows.rotate_rows();
        across_rows |= across_rows.rotate_rows2();

        let mut across_cols = cell_assertions_only;
        across_cols |= across_cols.rotate_cols();
        across_cols |= across_cols.rotate_cols2();

        let mut new_box_elims = Bitvec16x16::x_y_or_z_or(
            &across_cols,
            &across_cols.shuffle(&t.row_rotate_3x3_1),
            &across_cols.shuffle(&t.row_rotate_3x3_2),
        );
        new_box_elims = Bitvec16x16::x_y_or_z_or(
            &new_box_elims,
            &across_rows,
            &cell_assertions_only.which_non_zero(),
        );
        *box_eliminations =
            Bitvec16x16::x_y_xor_z_or(&new_box_elims, &cell_assertions_only, box_eliminations);

        let hv_neg_triad_assertions = Bitvec16x16::from_halves(
            Self::horizontal_triads(assertions),
            Self::vertical_triads(assertions),
        );
        let hv_pos_triad_assertions = Bitvec16x16::from_halves(
            Self::horizontal_triads(&new_box_elims),
            Self::vertical_triads(&new_box_elims),
        );

        let elim_idx = box_j * 3 + box_i;
        let new_eliminations = Bitvec16x16::x_y_or_z_or(
            &hv_neg_triad_assertions.shuffle(&t.triads_shift0_to_config_elims16[elim_idx]),
            &hv_pos_triad_assertions.shuffle(&t.triads_shift1_to_config_elims16[elim_idx]),
            &hv_pos_triad_assertions.shuffle(&t.triads_shift2_to_config_elims16[elim_idx]),
        );
        *h_band_eliminations |= new_eliminations.get_lo();
        *v_band_eliminations |= new_eliminations.get_hi();
    }

    #[inline]
    fn vertical_triads(cells: &Bitvec16x16) -> Bitvec08x16 {
        cells.get_hi()
    }

    #[inline]
    fn horizontal_triads(cells: &Bitvec16x16) -> Bitvec08x16 {
        let split_triads = cells.shuffle(&Bitvec16x16::new(
            0xffff, 0xffff, 0xffff, 0xffff, SHUF03, SHUF07, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff,
            0xffff, 0xffff, 0xffff, SHUF03, 0xffff,
        ));
        split_triads.get_lo() | split_triads.get_hi()
    }

    #[inline]
    fn gather_triad_clause_assertions<F>(
        cells: &Bitvec16x16,
        rotate: F,
        assertions: &mut Bitvec16x16,
    ) where
        F: Fn(Bitvec16x16) -> Bitvec16x16,
    {
        let mut one_or_more = *cells;
        let mut rotated = rotate(*cells);
        let mut two_or_more = one_or_more & rotated;
        one_or_more |= rotated;
        rotated = rotate(rotated);
        two_or_more = Bitvec16x16::x_y_and_z_or(&one_or_more, &rotated, &two_or_more);
        one_or_more |= rotated;
        rotated = rotate(rotated);
        two_or_more = Bitvec16x16::x_y_and_z_or(&one_or_more, &rotated, &two_or_more);
        *assertions = Bitvec16x16::x_y_andnot_z_or(cells, &two_or_more, assertions);
    }

    // ── BandEliminate ────────────────────────────────────────────────────────

    fn band_eliminate<const VERTICAL: bool>(
        state: &mut State,
        band_idx: usize,
        from_peer: usize,
    ) -> bool {
        let t = tables();
        let vert = if VERTICAL { 1 } else { 0 };

        if !state.bands[vert][band_idx]
            .configurations
            .intersects(&state.bands[vert][band_idx].eliminations)
        {
            return true;
        }

        let elims = state.bands[vert][band_idx].eliminations;
        state.bands[vert][band_idx].configurations =
            state.bands[vert][band_idx].configurations.and_not(&elims);

        let mut triads =
            Self::configurations_to_positive_triads(&state.bands[vert][band_idx].configurations);
        let counts = triads.popcounts9();

        let asserting = triads & counts.which_equal(&Bitvec16x16::all(3));
        let lo = asserting.get_lo();
        let hi = asserting.get_hi();

        let elims1 = Bitvec08x16::x_y_or_z_or(
            &lo.rotate_cols()
                .shuffle(&t.triads_shift1_to_config_elims[0]),
            &lo.rotate_cols()
                .shuffle(&t.triads_shift2_to_config_elims[0]),
            &lo.shuffle(&t.triads_shift1_to_config_elims[1]),
        );
        state.bands[vert][band_idx].configurations =
            state.bands[vert][band_idx].configurations.and_not(&elims1);

        let elims2 = Bitvec08x16::x_y_or_z_or(
            &lo.shuffle(&t.triads_shift2_to_config_elims[1]),
            &hi.rotate_cols()
                .shuffle(&t.triads_shift1_to_config_elims[2]),
            &hi.rotate_cols()
                .shuffle(&t.triads_shift2_to_config_elims[2]),
        );
        state.bands[vert][band_idx].configurations =
            state.bands[vert][band_idx].configurations.and_not(&elims2);

        triads =
            Self::configurations_to_positive_triads(&state.bands[vert][band_idx].configurations);

        let peer: [usize; 3] = [t.mod3[from_peer + 1], t.mod3[from_peer + 2], from_peer];
        let box_peers = t.box_peers[vert][band_idx];

        let triads_lo = triads.get_lo();
        let triads_lo_rot = triads_lo.rotate_cols();
        let triads_hi = triads.get_hi();
        let peer_triads: [Bitvec08x16; 3] = [triads_lo, triads_lo_rot, triads_hi];

        let p0 = peer[0];
        let p1 = peer[1];
        let p2 = peer[2];
        let c0 = Self::positive_triads_to_box_candidates(&peer_triads[p0], vert);
        let c1 = Self::positive_triads_to_box_candidates(&peer_triads[p1], vert);
        let c2 = Self::positive_triads_to_box_candidates(&peer_triads[p2], vert);

        if VERTICAL {
            Self::box_restrict::<true>(state, box_peers[p0], &c0)
                && Self::box_restrict::<true>(state, box_peers[p1], &c1)
                && Self::box_restrict::<true>(state, box_peers[p2], &c2)
        } else {
            Self::box_restrict::<false>(state, box_peers[p0], &c0)
                && Self::box_restrict::<false>(state, box_peers[p1], &c1)
                && Self::box_restrict::<false>(state, box_peers[p2], &c2)
        }
    }

    // ── ConfigurationsToPositiveTriads ───────────────────────────────────────

    #[inline]
    fn configurations_to_positive_triads(configurations: &Bitvec08x16) -> Bitvec16x16 {
        let t = tables();
        let tmp = Bitvec16x16::from_halves(*configurations, *configurations);
        tmp.shuffle(&t.shuffle_configs_to_triads[0]) | tmp.shuffle(&t.shuffle_configs_to_triads[1])
    }

    // ── PositiveTriadsToBoxCandidates ────────────────────────────────────────

    #[inline]
    fn positive_triads_to_box_candidates(triads: &Bitvec08x16, orientation: usize) -> Bitvec16x16 {
        let t = tables();
        let triads_with_kall = *triads | Bitvec08x16::new(0, 0, 0, K_ALL, 0, 0, 0, 0);
        let tmp = Bitvec16x16::from_halves(triads_with_kall, triads_with_kall);
        tmp.shuffle(&t.pos_triads_to_candidates[orientation][0])
            | tmp.shuffle(&t.pos_triads_to_candidates[orientation][1])
    }

    // ── ChooseBandAndValueToBranch ───────────────────────────────────────────

    fn choose_band_and_value_to_branch(state: &State) -> (u32, Bitvec08x16) {
        const NONE: u32 = u32::MAX;

        let config_minpos = Bitvec08x16::new(
            state.bands[0][0].configurations.popcount() as u16,
            state.bands[0][1].configurations.popcount() as u16,
            state.bands[0][2].configurations.popcount() as u16,
            state.bands[1][0].configurations.popcount() as u16,
            state.bands[1][1].configurations.popcount() as u16,
            state.bands[1][2].configurations.popcount() as u16,
            0xffff,
            0xffff,
        )
        .min_pos_gte(10);

        if (config_minpos & 0xff00) == 0 {
            let best_band = config_minpos >> 16;
            let t = tables();
            let vert = t.div3[best_band as usize];
            let bi = t.mod3[best_band as usize];
            let configurations = state.bands[vert][bi].configurations;

            let shuffle_rotate = Bitvec08x16::new(
                SHUF01, SHUF02, SHUF03, SHUF04, SHUF05, SHUF00, 0xffff, 0xffff,
            );
            let one = configurations;
            let rotated = one.shuffle(&shuffle_rotate);
            let two = one & rotated;
            let one = one | rotated;
            let rotated = rotated.shuffle(&shuffle_rotate);
            let three = two & rotated;
            let two = two | (one & rotated);
            let one = one | rotated;
            let rotated = rotated.shuffle(&shuffle_rotate);
            let four = three & rotated;
            let three = three | (two & rotated);
            let two = two | (one & rotated);
            let one = one | rotated;
            let rotated = rotated.shuffle(&shuffle_rotate);
            let four = four | (three & rotated);
            let three = three | (two & rotated);
            let two = two | (one & rotated);
            let one = one | rotated;
            let rotated = rotated.shuffle(&shuffle_rotate);
            let four = four | (three & rotated);
            let three = three | (two & rotated);
            let two = two | (one & rotated);
            // suppress unused variable warning for one (last iteration)
            let _ = one;

            let only_two = two.and_not(&three);
            if !only_two.all_zero() {
                return (best_band, only_two.get_low_bit());
            }
            let only_three = three.and_not(&four);
            if !only_three.all_zero() {
                return (best_band, only_three.get_low_bit());
            }
            return (best_band, four.get_low_bit());
        }

        (NONE, Bitvec08x16::all(0))
    }

    // ── BranchOnBandAndValue ─────────────────────────────────────────────────

    fn branch_on_band_and_value<const VERTICAL: bool>(
        &mut self,
        band_idx: usize,
        value_mask: &Bitvec08x16,
        state: &mut State,
    ) {
        let vert = if VERTICAL { 1 } else { 0 };
        let value_configurations = state.bands[vert][band_idx].configurations & *value_mask;
        self.num_guesses += 1;

        let mut state_copy = state.clone();
        let assignment_elims = value_configurations.clear_low_bit();
        state_copy.bands[vert][band_idx].eliminations |= assignment_elims;
        if Self::band_eliminate::<VERTICAL>(&mut state_copy, band_idx, 0) {
            self.count_solutions_consistent_with_partial_assignment(&mut state_copy);
            if self.num_solutions == self.limit {
                return;
            }
        }

        let negation_elims = value_configurations ^ assignment_elims;
        state.bands[vert][band_idx].eliminations |= negation_elims;
        if Self::band_eliminate::<VERTICAL>(state, band_idx, 0) {
            self.count_solutions_consistent_with_partial_assignment(state);
        }
    }

    // ── CountSolutionsConsistentWithPartialAssignment ────────────────────────

    fn count_solutions_consistent_with_partial_assignment(&mut self, state: &mut State) {
        // DT_IN: increment depth before emitting this frame's trace lines.
        #[cfg(feature = "debug-trace")]
        let dt_d = DT_DEPTH.with(|d| {
            let v = d.get() + 1;
            d.set(v);
            v
        });

        // DT_C: emit current state's band config popcounts.
        #[cfg(feature = "debug-trace")]
        if dt_check_and_inc() {
            eprintln!("DT:C d={} {}", dt_d, dt_pcs(state));
        }

        let (best_band, value_mask) = Self::choose_band_and_value_to_branch(state);

        // DT_T: emit the chosen band index (NONE = u32::MAX = 4294967295).
        #[cfg(feature = "debug-trace")]
        if dt_check_and_inc() {
            eprintln!("DT:T d={} best={}", dt_d, best_band);
        }

        if best_band == u32::MAX {
            self.num_solutions += 1;

            // DT_S: emit solution count.
            #[cfg(feature = "debug-trace")]
            if dt_check_and_inc() {
                eprintln!("DT:S d={} n={}", dt_d, self.num_solutions);
            }

            if SOLUTION_MODE == 1 && self.num_solutions == self.limit {
                self.solution = state.clone();
            }
        } else {
            let t = tables();
            if best_band < 3 {
                let bi = t.mod3[best_band as usize];
                self.branch_on_band_and_value::<false>(bi, &value_mask, state);
            } else {
                let bi = t.mod3[best_band as usize];
                self.branch_on_band_and_value::<true>(bi, &value_mask, state);
            }
        }

        // DT_OUT: decrement depth as we return from this frame.
        #[cfg(feature = "debug-trace")]
        DT_DEPTH.with(|d| d.set(d.get() - 1));
    }

    fn safe_count_solutions(&mut self, state: State, limit: usize) -> usize {
        self.limit = limit;
        self.num_solutions = 0;
        let mut state = state;
        self.count_solutions_consistent_with_partial_assignment(&mut state);
        self.num_solutions
    }

    // ── Initialization ───────────────────────────────────────────────────────

    fn init_clue(input: &[u8], state: &mut State, pos: usize) {
        let t = tables();
        let indexing = &t.box_indexing[pos];
        let digit = input[pos];
        let candidate = 1u16 << ((digit - b'1') as u32);

        let elim_mask =
            &t.cell_assignment_eliminations[(digit - b'1') as usize][indexing.elem as usize];
        state.boxen[indexing.r#box as usize].cells = state.boxen[indexing.r#box as usize]
            .cells
            .and_not(elim_mask);

        let cand_all = Bitvec08x16::all(candidate);
        state.bands[0][indexing.box_i as usize].eliminations = Bitvec08x16::x_y_and_z_or(
            &t.peer_x_elem_to_config_mask[indexing.box_j as usize][indexing.elem_i as usize],
            &cand_all,
            &state.bands[0][indexing.box_i as usize].eliminations,
        );
        state.bands[1][indexing.box_j as usize].eliminations = Bitvec08x16::x_y_and_z_or(
            &t.peer_x_elem_to_config_mask[indexing.box_i as usize][indexing.elem_j as usize],
            &cand_all,
            &state.bands[1][indexing.box_j as usize].eliminations,
        );
    }

    fn init_vanilla_by_band(input: &[u8], state: &mut State) -> bool {
        let mut buf = [b'.'; 96];
        let len = input.len().min(81);
        buf[..len].copy_from_slice(&input[..len]);

        // Only cells with digits '1'–'9' are clues. Anything else (including '0', which some
        // puzzle files use as an alternate empty marker) is treated as an empty cell.
        let mut clues64 = which_dots_64(&buf[0..64]) ^ u64::MAX;
        while clues64 != 0 {
            let cell_idx = low_order_bit_index64(clues64) as usize;
            if cell_idx < len && input[cell_idx] >= b'1' && input[cell_idx] <= b'9' {
                Self::init_clue(input, state, cell_idx);
            }
            clues64 = clear_low_bit64(clues64);
        }

        // cells 64..79 (16 cells, but only 16 bytes fit in which_dots_16)
        let dots16 = which_dots_16(&buf[64..80]);
        let mut clues16 = (dots16 ^ 0xffff) & 0xffff;
        while clues16 != 0 {
            let cell_idx = 64 + low_order_bit_index(clues16) as usize;
            if cell_idx < len && input[cell_idx] >= b'1' && input[cell_idx] <= b'9' {
                Self::init_clue(input, state, cell_idx);
            }
            clues16 = clear_low_bit(clues16);
        }

        // cell 80
        if len > 80 && input[80] >= b'1' && input[80] <= b'9' {
            Self::init_clue(input, state, 80);
        }

        Self::band_eliminate::<false>(state, 0, 1)
            && Self::band_eliminate::<true>(state, 0, 1)
            && Self::band_eliminate::<false>(state, 1, 2)
            && Self::band_eliminate::<true>(state, 1, 2)
            && Self::band_eliminate::<false>(state, 2, 0)
            && Self::band_eliminate::<true>(state, 2, 0)
    }

    fn init_pencilmark_by_box(input: &[u8], state: &mut State) -> bool {
        let mut buf = [b'1'; 736];
        let copy_len = input.len().min(729);
        buf[..copy_len].copy_from_slice(&input[..copy_len]);

        for box_i in 0..3usize {
            for box_j in 0..3usize {
                let mut box_candidates = Bitvec16x16::all(K_ALL);
                for elm_i in 0..3usize {
                    for elm_j in 0..3usize {
                        let cell = box_i * 27 + elm_i * 9 + box_j * 3 + elm_j;
                        let cell_eliminations = which_dots_16(&buf[cell * 9..cell * 9 + 16]);
                        box_candidates
                            .insert(elm_i * 4 + elm_j, K_ALL & !(cell_eliminations as u16));
                    }
                }
                let box_idx = box_i * 3 + box_j;
                if !Self::box_restrict::<false>(state, box_idx, &box_candidates) {
                    return false;
                }
            }
        }
        true
    }

    // ── Solution extraction ──────────────────────────────────────────────────

    fn extract_mini_row(minirow: u64, minirow_base: usize, solution: &mut [u8]) {
        solution[minirow_base] = b'1' + low_order_bit_index((minirow & 0xffff) as u32) as u8;
        solution[minirow_base + 1] =
            b'1' + low_order_bit_index(((minirow >> 16) & 0xffff) as u32) as u8;
        solution[minirow_base + 2] =
            b'1' + low_order_bit_index(((minirow >> 32) & 0xffff) as u32) as u8;
    }

    fn extract_solution(state: &State, solution: &mut [u8; 81]) {
        let t = tables();
        for box_idx in 0..9usize {
            let (x0, x1, x2, _x3) = state.boxen[box_idx].cells.as_4x64();
            let box_base = t.div3[box_idx] * 27 + t.mod3[box_idx] * 3;
            Self::extract_mini_row(x0, box_base, solution);
            Self::extract_mini_row(x1, box_base + 9, solution);
            Self::extract_mini_row(x2, box_base + 18, solution);
        }
    }

    // ── SolveSudoku ──────────────────────────────────────────────────────────

    fn solve_sudoku(
        &mut self,
        input: &[u8],
        limit: usize,
        solution: &mut [u8; 81],
        num_guesses: &mut usize,
    ) -> usize {
        self.limit = limit;
        self.num_solutions = 0;
        self.num_guesses = 0;

        // Pencilmark format: exactly 729 bytes (81 cells × 9 candidates).
        let pencilmark = input.len() >= 729;

        // Validate input: reject strings with invalid characters (anything
        // other than '1'–'9', '.', or '0' for vanilla; '1' or '.' for pencilmark).
        // '0' is an alternate empty-cell marker used by some puzzle formats.
        if !pencilmark {
            let len = input.len().min(81);
            if input[..len]
                .iter()
                .any(|&b| !matches!(b, b'0'..=b'9' | b'.'))
            {
                return 0;
            }
        } else if input[..input.len().min(729)]
            .iter()
            .any(|&b| !matches!(b, b'0'..=b'9' | b'.'))
        {
            return 0;
        }

        let mut state = State::default();
        let ok = if pencilmark {
            Self::init_pencilmark_by_box(input, &mut state)
        } else {
            Self::init_vanilla_by_band(input, &mut state)
        };

        // DT_INIT: emit after initialization, before solving.
        #[cfg(feature = "debug-trace")]
        if dt_check_and_inc() {
            eprintln!("DT:INIT ok={} {}", if ok { 1 } else { 0 }, dt_pcs(&state));
        }

        if ok {
            self.count_solutions_consistent_with_partial_assignment(&mut state);
            if SOLUTION_MODE == 1 {
                let sol = self.solution.clone();
                Self::extract_solution(&sol, solution);
            }
        }

        *num_guesses = self.num_guesses;
        self.num_solutions
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Enumeration solver — calls a callback for every solution
// ──────────────────────────────────────────────────────────────────────────────

struct EnumSolver<'a> {
    callback: &'a mut dyn FnMut(&[u8; 81]),
    num_solutions: usize,
    num_guesses: usize,
    limit: usize,
    sol_buf: [u8; 81],
}

impl<'a> EnumSolver<'a> {
    fn count_solutions(&mut self, state: &mut State) {
        let (best_band, value_mask) =
            SolverDpllTriadSimd::<0>::choose_band_and_value_to_branch(state);
        if best_band == u32::MAX {
            self.num_solutions += 1;
            if self.num_solutions <= self.limit {
                SolverDpllTriadSimd::<0>::extract_solution(state, &mut self.sol_buf);
                (self.callback)(&self.sol_buf);
            }
        } else if self.num_solutions < self.limit {
            let t = tables();
            if best_band < 3 {
                let bi = t.mod3[best_band as usize];
                self.branch::<false>(bi, &value_mask, state);
            } else {
                let bi = t.mod3[best_band as usize];
                self.branch::<true>(bi, &value_mask, state);
            }
        }
    }

    fn branch<const VERTICAL: bool>(
        &mut self,
        band_idx: usize,
        value_mask: &Bitvec08x16,
        state: &mut State,
    ) {
        let vert = if VERTICAL { 1 } else { 0 };
        let value_configurations = state.bands[vert][band_idx].configurations & *value_mask;
        self.num_guesses += 1;

        let mut state_copy = state.clone();
        let assignment_elims = value_configurations.clear_low_bit();
        state_copy.bands[vert][band_idx].eliminations |= assignment_elims;
        if SolverDpllTriadSimd::<0>::band_eliminate::<VERTICAL>(&mut state_copy, band_idx, 0) {
            self.count_solutions(&mut state_copy);
            if self.num_solutions >= self.limit {
                return;
            }
        }

        let negation_elims = value_configurations ^ assignment_elims;
        state.bands[vert][band_idx].eliminations |= negation_elims;
        if SolverDpllTriadSimd::<0>::band_eliminate::<VERTICAL>(state, band_idx, 0) {
            self.count_solutions(state);
        }
    }
}

/// Enumerate all solutions of a puzzle up to `limit`, calling `callback` for each.
///
/// Returns the total number of solutions found (capped at `limit`).
pub fn enumerate(input: &[u8], limit: usize, mut callback: impl FnMut(&[u8; 81])) -> usize {
    if limit == 0 {
        return 0;
    }
    // Pencilmark format: exactly 729 bytes (81 cells × 9 candidates).
    let pencilmark = input.len() >= 729;
    let mut state = State::default();
    let ok = if pencilmark {
        SolverDpllTriadSimd::<0>::init_pencilmark_by_box(input, &mut state)
    } else {
        SolverDpllTriadSimd::<0>::init_vanilla_by_band(input, &mut state)
    };

    if ok {
        let mut solver = EnumSolver {
            callback: &mut callback,
            num_solutions: 0,
            num_guesses: 0,
            limit,
            sol_buf: [b'.'; 81],
        };
        solver.count_solutions(&mut state);
        return solver.num_solutions;
    }
    0
}

// ──────────────────────────────────────────────────────────────────────────────
// GeneratorDpllTriadSimd  (used by public constrain/minimize API)
// ──────────────────────────────────────────────────────────────────────────────
#[derive(Default)]
pub struct GeneratorDpllTriadSimd {
    solver: SolverDpllTriadSimd<0>,
    util: crate::util::Util,
}

impl GeneratorDpllTriadSimd {
    pub fn constrain(&mut self, pencilmark: bool, puzzle: &mut [u8]) -> bool {
        let mut state = State::default();
        if pencilmark {
            SolverDpllTriadSimd::<0>::init_pencilmark_by_box(puzzle, &mut state);
        } else {
            SolverDpllTriadSimd::<0>::init_vanilla_by_band(puzzle, &mut state);
        }

        let perm = self.util.permutation(729);
        for &literal in &perm {
            let cell = literal / 9;

            if pencilmark {
                if puzzle[literal] == b'.' {
                    continue;
                }
            } else {
                if cell >= 81 || puzzle[cell] != b'.' {
                    continue;
                }
            }

            let _t = tables();
            let row = cell / 9;
            let col = cell % 9;
            let box_idx = (row / 3) * 3 + (col / 3);
            let elm_idx = (row % 3) * 4 + (col % 3);
            let candidates = state.boxen[box_idx].cells.extract(elm_idx);
            let candidate = 1u16 << (literal % 9) as u32;

            if (candidates & candidate) != 0 && candidates != candidate {
                let mut restrict = state.boxen[box_idx].cells;
                restrict.insert(
                    elm_idx,
                    if pencilmark {
                        candidates ^ candidate
                    } else {
                        candidate
                    },
                );
                let mut test_state = state.clone();
                if SolverDpllTriadSimd::<0>::box_restrict::<false>(
                    &mut test_state,
                    box_idx,
                    &restrict,
                ) {
                    let cell_or_literal = if pencilmark { literal } else { cell };
                    let prior = puzzle[cell_or_literal];
                    puzzle[cell_or_literal] = if pencilmark {
                        b'.'
                    } else {
                        b'1' + (literal % 9) as u8
                    };
                    match self.solver.safe_count_solutions(test_state.clone(), 2) {
                        0 => {
                            puzzle[cell_or_literal] = prior;
                            continue;
                        }
                        1 => return true,
                        _ => {
                            state = test_state;
                            continue;
                        }
                    }
                }
            }
        }
        false
    }

    pub fn minimize(&mut self, pencilmark: bool, monotonic: bool, puzzle: &mut [u8]) -> bool {
        let mut restored_clue = false;
        let perm = self.util.permutation(729);
        for &cell_or_literal in &perm {
            if pencilmark {
                if puzzle[cell_or_literal] != b'.' {
                    continue;
                }
            } else {
                if cell_or_literal >= 81 || puzzle[cell_or_literal] == b'.' {
                    continue;
                }
            }
            let constraint = puzzle[cell_or_literal];
            let mut state = State::default();
            if pencilmark {
                puzzle[cell_or_literal] = b'1' + (cell_or_literal % 9) as u8;
                SolverDpllTriadSimd::<0>::init_pencilmark_by_box(puzzle, &mut state);
            } else {
                puzzle[cell_or_literal] = b'.';
                SolverDpllTriadSimd::<0>::init_vanilla_by_band(puzzle, &mut state);
            }
            if self.solver.safe_count_solutions(state, 2) > 1 {
                puzzle[cell_or_literal] = constraint;
                restored_clue = true;
            } else if monotonic && restored_clue {
                return false;
            }
        }
        true
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Thread-local statics + public solve function
// ──────────────────────────────────────────────────────────────────────────────

thread_local! {
    static SOLVER_NONE: std::cell::RefCell<SolverDpllTriadSimd<0>> =
        std::cell::RefCell::new(SolverDpllTriadSimd::default());
    static SOLVER_LAST: std::cell::RefCell<SolverDpllTriadSimd<1>> =
        std::cell::RefCell::new(SolverDpllTriadSimd::default());
}

/// Solve a Sudoku puzzle using the SIMD DPLL triad solver.
///
/// Returns `(num_solutions, solution_bytes, num_guesses)`.
/// `solution_bytes` is meaningful only when `num_solutions >= 1` and
/// `limit == 1` or `config > 0`.
pub fn solve(input: &[u8], limit: usize, config: u32) -> (usize, [u8; 81], usize) {
    let return_last = limit == 1 || config > 0;
    let mut solution = [b'.'; 81];
    let mut num_guesses = 0usize;

    let count = if return_last {
        SOLVER_LAST.with(|s| {
            s.borrow_mut()
                .solve_sudoku(input, limit, &mut solution, &mut num_guesses)
        })
    } else {
        SOLVER_NONE.with(|s| {
            s.borrow_mut()
                .solve_sudoku(input, limit, &mut solution, &mut num_guesses)
        })
    };

    (count, solution, num_guesses)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const PUZZLE: &[u8] =
        b".5..83.17...1..4..3.4..56.8....3...9.9.8245....6....7...9....5...729..861.36.72.4";
    const SOLUTION: &[u8] =
        b"652483917978162435314975628825736149791824563436519872269348751547291386183657294";

    #[test]
    fn test_simd_solve_unique() {
        let (count, sol, _guesses) = solve(PUZZLE, 1, 1);
        assert_eq!(count, 1);
        assert_eq!(&sol[..], SOLUTION);
    }

    #[test]
    fn test_simd_solve_count_only() {
        let (count, _sol, _guesses) = solve(PUZZLE, usize::MAX, 0);
        assert_eq!(count, 1);
    }

    #[test]
    fn test_simd_invalid_puzzle() {
        // duplicate clue in first row
        let puzzle = b"115......................................................................";
        let (count, _sol, _guesses) = solve(puzzle, 1, 1);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_simd_matches_basic_solver() {
        let (simd_count, simd_sol, _) = solve(PUZZLE, 1, 1);
        let (basic_count, basic_sol, _) = crate::solver_basic::solve(PUZZLE, 1, 1);
        assert_eq!(simd_count, basic_count);
        assert_eq!(&simd_sol[..], &basic_sol[..]);
    }
}
