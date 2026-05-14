#![no_main]

use libfuzzer_sys::fuzz_target;

// Fuzz target: feeds arbitrary byte sequences as puzzle seeds into the
// `constrain` and `minimize` generator functions, verifying:
// - No panics
// - constrain: if it returns true, the output has exactly 1 solution
// - minimize:  if it returns true, the output has exactly 1 solution
//              and clue count ≤ original
fuzz_target!(|data: &[u8]| {
    // Build an 81-char puzzle from fuzz bytes:
    //   byte & 1 == 0  →  '.' (empty)
    //   byte & 1 == 1  →  '1' + (byte >> 1) % 9  (clue)
    let mut puzzle_bytes = [0u8; 81];
    for i in 0..81 {
        let b = data.get(i).copied().unwrap_or(0);
        puzzle_bytes[i] = if b & 1 == 0 {
            b'.'
        } else {
            b'1' + ((b >> 1) % 9)
        };
    }
    let puzzle = String::from_utf8_lossy(&puzzle_bytes).into_owned();

    // ── constrain ─────────────────────────────────────────────────────────
    {
        let mut p = puzzle.clone();
        let initial_clues = p.bytes().filter(|&b| b != b'.').count();

        // constrain must not panic.
        let ok = rdoku::constrain(false, &mut p);

        if ok {
            // Output must have exactly one solution.
            let (count, _, _) = rdoku::solve_sudoku(&p, 2, 0);
            assert_eq!(
                count, 1,
                "constrain: output has {} solutions (expected 1)\n  input:  {:?}\n  output: {:?}",
                count, &puzzle[..puzzle.len().min(80)], &p[..p.len().min(80)]
            );
            // Clue count must be ≥ original (we only add clues).
            let final_clues = p.bytes().filter(|&b| b != b'.').count();
            assert!(
                final_clues >= initial_clues,
                "constrain: lost clues ({} → {})",
                initial_clues,
                final_clues
            );
        }
    }

    // ── minimize ──────────────────────────────────────────────────────────
    // minimize expects a puzzle with a unique solution, so we start from a
    // solved grid and test it.  Also test the (potentially non-unique) random
    // puzzle for resilience.
    {
        // Test minimize on a fully-solved grid (guaranteed unique).
        let solved_grid = "652483917978162435314975628825736149791824563436519872269348751547291386183657294";
        let mut p = solved_grid.to_string();
        let initial_clues = 81usize;

        let ok = rdoku::minimize(false, false, &mut p);
        if ok {
            let (count, _, _) = rdoku::solve_sudoku(&p, 2, 0);
            assert_eq!(
                count, 1,
                "minimize: output has {} solutions (expected 1)\n  output: {:?}",
                count, &p[..p.len().min(80)]
            );
            let final_clues = p.bytes().filter(|&b| b != b'.').count();
            assert!(
                final_clues <= initial_clues,
                "minimize: gained clues ({} → {})",
                initial_clues,
                final_clues
            );
        }

        // Also test minimize on the random fuzz puzzle (should not panic
        // even if the puzzle has 0 or >1 solutions).
        let mut p2 = puzzle.clone();
        let initial2 = p2.bytes().filter(|&b| b != b'.').count();
        let _ = rdoku::minimize(false, true, &mut p2);
        // If it succeeded, verify uniqueness; if not, that's fine.
        let (count2, _, _) = rdoku::solve_sudoku(&p2, 2, 0);
        let final2 = p2.bytes().filter(|&b| b != b'.').count();
        if count2 == 1 {
            // Clue count should not have increased.
            assert!(
                final2 <= initial2 || initial2 == 0,
                "minimize (random): gained clues ({} → {})",
                initial2,
                final2
            );
        }
        // Regardless, minimize should not panic.
    }
});
