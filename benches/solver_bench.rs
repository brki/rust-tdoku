use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

const PUZZLE: &str =
    ".5..83.17...1..4..3.4..56.8....3...9.9.8245....6....7...9....5...729..861.36.72.4";
const EMPTY: &str =
    ".................................................................................";

/// Valid solution for PUZZLE.
const SOLUTION: &str =
    "652483917978162435314975628825736149791824563436519872269348751547291386183657294";

fn bench_solve_unique(c: &mut Criterion) {
    c.bench_function("solve_simd_unique", |b| {
        b.iter(|| {
            let (count, sol, _) = rdoku::solve_sudoku(black_box(PUZZLE), 1, 0);
            assert_eq!(count, 1);
            assert_eq!(sol, SOLUTION);
        })
    });

    c.bench_function("solve_basic_unique", |b| {
        b.iter(|| {
            let (count, _, _) = rdoku::solver_basic::solve(black_box(PUZZLE.as_bytes()), 1, 0);
            assert_eq!(count, 1);
        })
    });

    c.bench_function("solve_scc_unique", |b| {
        b.iter(|| {
            let (count, _, _) =
                rdoku::solver_dpll_triad_scc::solve(black_box(PUZZLE.as_bytes()), 1, 3);
            assert_eq!(count, 1);
        })
    });
}

fn bench_count_only(c: &mut Criterion) {
    c.bench_function("count_simd", |b| {
        b.iter(|| {
            let (count, _, _) = rdoku::solve_sudoku(black_box(PUZZLE), 2, 0);
            assert_eq!(count, 1);
        })
    });

    c.bench_function("count_basic", |b| {
        b.iter(|| {
            let (count, _, _) = rdoku::solver_basic::solve(black_box(PUZZLE.as_bytes()), 2, 0);
            assert_eq!(count, 1);
        })
    });
}

fn bench_empty_grid(c: &mut Criterion) {
    c.bench_function("count_empty_simd_limit100", |b| {
        b.iter(|| {
            let (count, _, _) = rdoku::solve_sudoku(black_box(EMPTY), 100, 0);
            assert!(count >= 100);
        })
    });
}

fn bench_enumerate(c: &mut Criterion) {
    c.bench_function("enumerate_100_simd", |b| {
        b.iter(|| {
            let count = rdoku::enumerate(black_box(EMPTY), 100, |_sol| {});
            assert!(count >= 100);
        })
    });
}

criterion_group!(
    benches,
    bench_solve_unique,
    bench_count_only,
    bench_empty_grid,
    bench_enumerate
);
criterion_main!(benches);
