// debug_solver — minimal driver for the rdoku SIMD solver with optional DT trace.
//
// Usage: debug_solver [puzzle] [limit]
//
//   puzzle   81-char vanilla or 729-char pencilmark puzzle string
//            default: reference puzzle
//   limit    max solutions to count (default: 2)
//
// Build with DT trace:
//   cargo build --release --features debug-trace --bin debug_solver
//
// All trace lines go to stderr (prefixed "DT:").
// Final result line goes to stdout.
//
// Output format (identical to tdoku debug_driver.cc):
//   count=<N> guesses=<N> solution=<81chars or empty>

use rdoku::solver_dpll_triad_simd;

const DEFAULT_PUZZLE: &str =
    ".5..83.17...1..4..3.4..56.8....3...9.9.8245....6....7...9....5...729..861.36.72.4";

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let puzzle = if args.len() > 1 && args[1] != "--help" {
        args[1].as_str()
    } else {
        DEFAULT_PUZZLE
    };
    let limit: usize = if args.len() > 2 {
        args[2].parse().unwrap_or(2)
    } else {
        2
    };

    // Build an input buffer of at least 82 bytes so pencilmark detection works.
    // For vanilla (81-char): buf[81] = 0 (below '.'), signalling vanilla format.
    // For pencilmark (729-char): buf[81] >= '.' already.
    let plen = puzzle.len();
    let mut buf: Vec<u8> = puzzle.as_bytes().to_vec();
    if plen <= 81 {
        // Pad to 82 bytes; ensure buf[81] = 0 so pencilmark check (input[81] >= '.') is false.
        buf.resize(82, 0);
    } else {
        // Pencilmark: pad to at least 82 to ensure safe indexing.
        if buf.len() < 82 {
            buf.resize(82, 0);
        }
    }

    // Call the SIMD solver directly.  config=0 mirrors C++ debug_driver.cc.
    let (count, solution, num_guesses) = solver_dpll_triad_simd::solve(&buf, limit, 0);

    // Solution string: only meaningful when return_last=true (limit==1 or config>0).
    // With config=0 and limit!=1, the solver runs in count-only mode and the solution
    // buffer is never written — print empty string to match C++ (zero-init buffer).
    let solution_str = if limit == 1 {
        String::from_utf8_lossy(&solution).into_owned()
    } else {
        String::new()
    };

    println!("count={} guesses={} solution={}", count, num_guesses, solution_str);
}
