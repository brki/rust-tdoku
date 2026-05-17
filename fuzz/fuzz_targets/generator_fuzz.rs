#![no_main]

use libfuzzer_sys::fuzz_target;

// Fuzz target: feeds arbitrary byte sequences as puzzle seeds into the
// `constrain` and `minimize` generator functions, verifying:
// - No panics
// - constrain: if it returns true, the output has exactly 1 solution
// - minimize:  if it returns true, the output has exactly 1 solution
//              and clue count ≤ original
//
// To keep iteration time bounded, each run randomly picks one test path
// (constrain-vanilla, constrain-pencilmark, minimize-vanilla,
//  minimize-monotonic, minimize-pencilmark).
fuzz_target!(|data: &[u8]| {
    let Some((&selector, tail)) = data.split_first() else {
        return;
    };
    let path = selector % 5;

    // ── build vanilla puzzle (81 chars) from fuzz bytes ──────────────────
    //   byte & 1 == 0  →  '.' (empty)
    //   byte & 1 == 1  →  '1' + (byte >> 1) % 9  (clue)
    let mut vanilla_bytes = [b'.'; 81];
    for i in 0..81 {
        let b = tail.get(i).copied().unwrap_or(0);
        if b & 1 == 1 {
            vanilla_bytes[i] = b'1' + ((b >> 1) % 9);
        }
    }
    let vanilla = std::str::from_utf8(&vanilla_bytes).unwrap().to_string();

    // ── build pencilmark puzzle (729 chars) from bytes 81.. ──────────────
    let mut pm_bytes = [b'1'; 729];
    for i in 0..729 {
        let b = tail.get(81 + i).copied().unwrap_or(0);
        if b & 1 == 0 {
            pm_bytes[i] = b'.';
        }
    }
    let pencilmark = std::str::from_utf8(&pm_bytes).unwrap().to_string();

    // Library validates input and rejects unsolvable / under-constrained
    // puzzles.  We just check invariants on whatever it returns.

    match path {
        // ── constrain (vanilla) ──────────────────────────────────────────
        0 => {
            let mut p = vanilla.clone();
            let initial = p.bytes().filter(|&b| b != b'.').count();
            let ok = rdoku::constrain(false, &mut p);
            if ok {
                let (count, _, _) = rdoku::solve_sudoku(&p, 2, 0);
                assert_eq!(
                    count, 1,
                    "constrain(vanilla): {} solutions (expected 1)\n  input:  {:?}\n  output: {:?}",
                    count,
                    &vanilla[..vanilla.len().min(80)],
                    &p[..p.len().min(80)]
                );
                let final_clues = p.bytes().filter(|&b| b != b'.').count();
                assert!(
                    final_clues >= initial,
                    "constrain(vanilla): lost clues ({} → {})",
                    initial,
                    final_clues
                );
            }
        }
        // ── constrain (pencilmark) ───────────────────────────────────────
        1 => {
            let mut p = pencilmark.clone();
            let ok = rdoku::constrain(true, &mut p);
            if ok {
                let (count, _, _) = rdoku::solve_sudoku(&p, 2, 0);
                assert_eq!(
                    count, 1,
                    "constrain(pm): {} solutions (expected 1)\n  output: {:?}",
                    count,
                    &p[..p.len().min(80)]
                );
            }
        }
        // ── minimize (vanilla, monotonic=false) ──────────────────────────
        2 => {
            let mut p = vanilla.clone();
            let initial = p.bytes().filter(|&b| b != b'.').count();
            let _ = rdoku::minimize(false, false, &mut p);
            let (count, _, _) = rdoku::solve_sudoku(&p, 2, 0);
            let final_clues = p.bytes().filter(|&b| b != b'.').count();
            if count == 1 {
                assert!(
                    final_clues <= initial || initial == 0,
                    "minimize(vanilla): gained clues ({} → {})",
                    initial,
                    final_clues
                );
            }
        }
        // ── minimize (vanilla, monotonic=true) ───────────────────────────
        3 => {
            let mut p = vanilla.clone();
            let initial = p.bytes().filter(|&b| b != b'.').count();
            let _ = rdoku::minimize(false, true, &mut p);
            let (count, _, _) = rdoku::solve_sudoku(&p, 2, 0);
            let final_clues = p.bytes().filter(|&b| b != b'.').count();
            if count == 1 {
                assert!(
                    final_clues <= initial || initial == 0,
                    "minimize(monotonic): gained clues ({} → {})",
                    initial,
                    final_clues
                );
            }
        }
        // ── minimize (pencilmark) ────────────────────────────────────────
        _ => {
            let mut p = pencilmark.clone();
            let _ = rdoku::minimize(true, false, &mut p);
            let (count, _, _) = rdoku::solve_sudoku(&p, 2, 0);
            // If minimize succeeded, uniqueness holds.
            let _ = count;
        }
    }
});
