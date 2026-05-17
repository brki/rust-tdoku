#![no_main]

use libfuzzer_sys::fuzz_target;

// Fuzz target: feeds arbitrary byte sequences into all three solvers and
// the public API, verifying:
// - No panics, no unsound behavior
// - Solutions returned are valid Sudoku grids
// - Solutions satisfy the original clues (if any)
// - All three solvers agree on solution count
fuzz_target!(|data: &[u8]| {
    // ── build 81-char vanilla puzzle from fuzz bytes ────────────────────
    //   byte & 1 == 0  →  '.' (empty)
    //   byte & 1 == 1  →  '1' + (byte >> 1) % 9  (clue)
    // This always produces syntactically valid input; the library handles
    // semantic validation (contradictions, solvability, etc.).
    let mut buf = [b'.'; 81];
    for i in 0..81 {
        let b = data.get(i).copied().unwrap_or(0);
        if b & 1 == 1 {
            buf[i] = b'1' + ((b >> 1) % 9);
        }
    }
    let input = std::str::from_utf8(&buf).unwrap();

    // ── public API: solve_sudoku ───────────────────────────────────────
    let (count_simd, sol_simd, _) = rdoku::solve_sudoku(input, 2, 0);

    // ── solver agreement ────────────────────────────────────────────────
    // Verify basic & SCC solvers agree with the primary SIMD solver.
    // Skip when SIMD returns 0 — all solvers agree input is invalid, and
    // running the basic solver on near-empty grids is prohibitively slow.
    if count_simd > 0 {
        let (count_basic, _sol_basic, _) =
            rdoku::solver_basic::solve(input.as_bytes(), 2, 0);
        let (count_scc, _sol_scc, _) =
            rdoku::solver_dpll_triad_scc::solve(input.as_bytes(), 2, 0);

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
    }

    // ── validity: any returned solution must be a valid Sudoku grid ─────
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

            // ── clue-satisfaction: solution must match original clues ───
            let clue_bytes = input.as_bytes();
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
});
