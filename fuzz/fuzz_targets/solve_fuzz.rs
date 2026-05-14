#![no_main]

use libfuzzer_sys::fuzz_target;

/// Fuzz target: feeds arbitrary byte sequences into all three solvers and
/// the public API, verifying:
/// - No panics, no unsound behavior
/// - Solutions returned are valid Sudoku grids
/// - Solutions satisfy the original clues (if any)
/// - All three solvers agree on solution count
fuzz_target!(|data: &[u8]| {
    // Interpret fuzz input as ASCII; strip non-ASCII.
    let input: String = data
        .iter()
        .filter_map(|&b| if b.is_ascii() { Some(b as char) } else { None })
        .collect();
    if input.is_empty() {
        return;
    }

    // ── safety: all solvers must not panic ──────────────────────────────────
    let (count_simd, sol_simd, _) = rdoku::solve_sudoku(&input, 2, 0);
    let (count_basic, sol_basic, _) = rdoku::solver_basic::solve(input.as_bytes(), 2, 0);
    let (count_scc, sol_scc, _) = rdoku::solver_dpll_triad_scc::solve(input.as_bytes(), 2, 0);

    // ── agreement: all solvers must agree on count ──────────────────────────
    assert_eq!(
        count_simd, count_basic,
        "simd count {} != basic count {} for input {:?}",
        count_simd, count_basic, &input[..input.len().min(80)]
    );
    assert_eq!(
        count_simd, count_scc,
        "simd count {} != scc count {} for input {:?}",
        count_simd, count_scc, &input[..input.len().min(80)]
    );

    // ── validity: any returned solution must be a valid Sudoku grid ─────────
    if count_simd >= 1 && !sol_simd.is_empty() {
        let sol_bytes = sol_simd.as_bytes();
        if sol_bytes.len() == 81 && sol_bytes.iter().all(|&b| (b'1'..=b'9').contains(&b)) {
            // Verify rows, cols, boxes all contain 1-9 exactly once.
            for i in 0..9 {
                let mut row = 0u16;
                let mut col = 0u16;
                let mut bx = 0u16;
                for j in 0..9 {
                    let rv = 1u16 << (sol_bytes[i * 9 + j] - b'1');
                    let cv = 1u16 << (sol_bytes[j * 9 + i] - b'1');
                    let bv = 1u16 << (sol_bytes[(i / 3 * 3 + j / 3) * 9 + (i % 3 * 3 + j % 3)] - b'1');
                    row |= rv;
                    col |= cv;
                    bx |= bv;
                }
                assert_eq!(row, 0x1ff, "invalid row {} in solution", i);
                assert_eq!(col, 0x1ff, "invalid col {} in solution", i);
                assert_eq!(bx, 0x1ff, "invalid box {} in solution", i);
            }

            // ── clue-satisfaction: solution must match original clues ──────
            if input.len() == 81 || input.len() == 729 {
                let clue_bytes = input.as_bytes();
                // 81-char vanilla format
                if input.len() == 81 {
                    for i in 0..81 {
                        if (b'1'..=b'9').contains(&clue_bytes[i]) {
                            assert_eq!(
                                sol_bytes[i], clue_bytes[i],
                                "solution cell {} differs from clue", i
                            );
                        }
                    }
                }
            }
        }
    }
});
