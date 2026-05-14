---
description: Debugging plan for the rdoku generator stack overflow in the SIMD solver
applyTo: "**/rdoku/**,**/tdoku/**"
---

# Debugging Plan: rdoku SIMD Solver Stack Overflow

## Problem

`rdoku` is a Rust port of the `tdoku` C++ Sudoku solver.
The SIMD solver (`solver_dpll_triad_simd.rs`) causes a stack overflow when called
from the `minimize` function with sparse puzzles (few clues), `limit=2`.

The mutual recursion that overflows:
```
count_solutions_consistent_with_partial_assignment (CSPA)
  → branch_on_band_and_value
    → CSPA  (recurses until stack exhausted)
```

The equivalent C++ code handles the same puzzles without overflow — so either
the recursion goes far deeper in Rust (algorithmic difference), or the Rust
stack frames are much larger (data layout difference).

## Goal

Find the **first divergence** between C++ `tdoku` and Rust `rdoku` traces,
then fix the root cause.

## Infrastructure Already Set Up

- **Git branch**: `debug/simd-compare` in `rdoku/`
- **Docker context**: `colima-amd64` (default) — amd64 Linux, needed for C++ SIMD
- **`rdoku/debug/Dockerfile.tdoku`** — C++ debug image (entrypoint builds then runs)
- **`rdoku/debug/Dockerfile.rdoku`** — Rust debug image (entrypoint runs `cargo build` with cached volumes)
- **`rdoku/debug/docker-compose.yml`** — compose file with volume mounts
- **`rdoku/debug/compare.sh`** — automated build + run + diff script
- **`tdoku/src/debug_driver.cc`** — C++ main() that calls the solver and prints result
- **`rdoku/src/bin/debug_solver.rs`** — Rust equivalent
- **`rdoku/Cargo.toml`** — `debug-trace` feature + `debug_solver` binary declared

### Volume Caching (avoids recompilation on each run)

Named volumes used by `compare.sh` and `docker-compose.yml`:
- `tdoku-build` → `/build` — compiled C++ binary
- `rdoku-target` → `/rdoku/target` — Rust build artifacts
- `cargo-registry` → `/usr/local/cargo/registry` — downloaded crates

First run after image build populates the volumes. Subsequent runs are incremental.

## Trace Format

Both implementations emit identical `DT:` lines to stderr:

```
DT:INIT ok=<0|1> pcs=<p00>,<p01>,<p02>,<p10>,<p11>,<p12>
DT:C d=<depth> pcs=<p00>,<p01>,<p02>,<p10>,<p11>,<p12>
DT:T d=<depth> best=<0-5 | 4294967295=NONE>
DT:S d=<depth> n=<total>
```

- `pcs` = `configurations.popcount()` for each of the 6 bands `[horiz/vert][0..3]`
- `best=4294967295` means no branching needed (solution found by unit propagation)
- At most 2000 events are emitted (MAX_EVENTS guard)
- Non-DT: noise lines (CSPA_DEPTH, BE_DEPTH) are filtered by `compare.sh`

## Step-by-Step Debugging Plan

### Phase 1 — Verify Infrastructure

```bash
cd tdoku-to-rust/rdoku
git status   # should be on debug/simd-compare
./debug/compare.sh
# Expected: "Traces are IDENTICAL" for reference puzzle (no branching needed)
```

### Phase 2 — Find a Puzzle That Branches

Test with a puzzle that requires actual DPLL branching (DT:T best != 4294967295):

```bash
# Al Escargot in '.' format (hard, requires many branches)
./debug/compare.sh "8..........3.6.....7..9.2...6..5...3...4..6..5..7...9...3..4..5.5...3...1...8.67.." 2
```

If traces diverge: inspect `rdoku/debug/trace_diff.txt` for first diverging line.
If traces are identical: try a harder/sparser puzzle.

### Phase 3 — Test the Stack Overflow Trigger

The stack overflow is triggered by `minimize` on a newly-generated puzzle (sparse, ~25–30 clues).
Reproduce it in Docker (no stack overflow risk there — amd64 Linux with larger default stack):

```bash
./debug/compare.sh "<sparse-puzzle-string>" 2
# Compare DT:T best= values — Rust may branch far more than C++
```

Key question: **does C++ reach the same recursion depth?**  
If C++ stays shallow while Rust goes deep → algorithmic divergence in CSPA or `choose_band_and_value_to_branch`.

### Phase 4 — Bisect the Divergence

Once the first diverging `DT:` line is identified:

1. Note the state at that point: `d=<N>`, `pcs=...`
2. Compare the `DT:C` line just before — are the popcounts identical?
3. Compare the `DT:T` line — does Rust choose a different `best` band/value?

If `DT:T` diverges first:
- The bug is in `choose_band_and_value_to_branch` (Rust vs C++ choose different branch point)
- Compare `choose_band_and_value_to_branch` in both implementations

If `DT:C pcs` diverges first:
- The bug is in `band_eliminate` / constraint propagation
- The state after branching differs between C++ and Rust
- Compare `band_eliminate` implementations

### Phase 5 — Candidate Root Causes

In priority order:

1. **`choose_band_and_value_to_branch`** differences
   - `min_pos_gte` / `count_zeros_before_position` may return different values
   - SIMD shuffle operations may differ (table init, byte ordering)

2. **`band_eliminate` propagation differences**
   - The SIMD constraint propagation loop may terminate differently
   - Check `peer_configurations` table, `configurations_from_peers_eliminations` logic

3. **State struct layout / padding**
   - `State` is 480 bytes; `clone()` must copy all of it
   - Any uninitialized bytes in `Default` could cause divergence after cloning

4. **SIMD operation differences**
   - `Bitvec08x16` operations in `simd_vectors.rs` may not exactly match C++ `__m128i`
   - Especially: `shuffle_epi8`, `min_epu8`, `cmpeq_epi8`

5. **Stack frame size**
   - Even if algorithms match, Rust frames may be larger (larger `State` clone on stack?)
   - Check: does C++ pass `state_copy` by value or by pointer in hot path?

### Phase 6 — Fix

Once root cause is identified, fix the corresponding Rust code to match C++ behavior.
Run the full integration test suite after fixing:

```bash
cargo test --release
```

## Key File Locations

| File | Purpose |
|------|---------|
| `rdoku/src/solver_dpll_triad_simd.rs` | Rust SIMD solver (main subject) |
| `tdoku/src/solver_dpll_triad_simd.cc` | C++ reference implementation |
| `rdoku/debug/compare.sh` | Run both, diff traces |
| `rdoku/debug/Dockerfile.tdoku` | C++ debug image |
| `rdoku/debug/Dockerfile.rdoku` | Rust debug image |
| `tdoku/src/debug_driver.cc` | C++ debug main() |
| `rdoku/src/bin/debug_solver.rs` | Rust debug main() |
| `rdoku/IMPLEMENTATION_LOG.md` | Phase-by-phase port notes |

## Quick Reference Commands

```bash
# From tdoku-to-rust/ workspace root:

# Build images (only needed when Dockerfiles change):
docker build --platform linux/amd64 -f rdoku/debug/Dockerfile.tdoku -t tdoku-debug tdoku/
docker build --platform linux/amd64 -f rdoku/debug/Dockerfile.rdoku -t rdoku-debug rdoku/

# Run comparison (uses cached volumes for fast incremental builds):
cd rdoku && ./debug/compare.sh [puzzle] [limit]

# Run a single solver manually:
docker run --rm --platform linux/amd64 \
    -v "$PWD/../tdoku:/tdoku" -v tdoku-build:/build \
    tdoku-debug "<puzzle>" 2

docker run --rm --platform linux/amd64 \
    -v "$PWD:/rdoku" -v rdoku-target:/rdoku/target -v cargo-registry:/usr/local/cargo/registry \
    rdoku-debug "<puzzle>" 2

# Wipe volume caches (force full rebuild):
docker volume rm tdoku-build rdoku-target cargo-registry
```
