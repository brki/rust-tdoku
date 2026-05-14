# rdoku Generator Stack Overflow — Debug Log

## Context

`rdoku` is a Rust port of the `tdoku` C++ Sudoku solver/generator.
Workspace: `tdoku-to-rust/`

The generator binary (`rdoku/src/bin/generate.rs`) calls `minimize()` which
calls the SIMD solver (`SolverDpllTriadSimd`) with `limit=2` on sparse puzzles.
This causes a stack overflow via deep mutual recursion.

**Git branch for this work**: `debug/simd-compare` (in `rdoku/`)

---

## Bug Description

Stack overflow in:
```
rdoku::solver_dpll_triad_simd::SolverDpllTriadSimd<_>::count_solutions_consistent_with_partial_assignment
  → branch_on_band_and_value
    → count_solutions_consistent_with_partial_assignment  (unbounded recursion)
```

Triggered by: `minimize()` calling SIMD solver on a sparse (~25–30 clue) puzzle.

The equivalent C++ code handles these puzzles without overflow. Either:
- The Rust recursion goes much deeper (algorithmic divergence), or
- Rust stack frames are larger (State clone on stack?), or
- Both.

**Safety guard added**: `CSPA_DEPTH` atomic counter with `panic!` at depth > 5000
(prevents actual stack overflow during debugging; the panic is visible in tests).

---

## Bugs Fixed So Far

### 1. `'0'`-format puzzle handling (fixed in both C++ and Rust)

**Symptom**: Rust panicked with `index out of bounds: len=9, index=255` in `init_clue`
when given a puzzle using `'0'` for empty cells (some puzzle files use this).

**Cause**: `b'0' - b'1' = 255u8` used as array index. C++ had UB (array index -1,
accidentally read zeros).

**Fix** (`init_vanilla_by_band` in both implementations):
Added `>= '1' && <= '9'` guard before calling `InitClue`/`init_clue` in all three
loop bodies (64-bit block, 16-bit block, cell-80).

### 2. Depth counter alignment (C++ now matches Rust)

**Symptom**: C++ emitted `DT:C d=0` while Rust emitted `DT:C d=1` for first call.

**Fix**: Changed C++ `DT_C(state); DT_IN;` → `DT_IN; DT_C(state);`

### 3. NONE value format mismatch (fixed in Rust)

**Symptom**: C++ emitted `best=4294967295` (UINT32_MAX) for "no branch needed",
Rust incorrectly emitted `best=6`.

**Fix**: Changed Rust `dt::emit_t` to emit `best_band` directly as `u32`
(u32::MAX = 4294967295 matches C++ UINT32_MAX).

---

## Debugging Infrastructure

### Docker Setup

- **Context**: `colima-amd64` (default Docker context, linux/amd64)
- **Why Docker**: C++ uses x86-only SIMD intrinsics, can't compile on ARM macOS

#### Images

| Image | Dockerfile | Build context |
|-------|-----------|---------------|
| `tdoku-debug` | `rdoku/debug/Dockerfile.tdoku` | `tdoku/` |
| `rdoku-debug` | `rdoku/debug/Dockerfile.rdoku` | `rdoku/` |

Both Dockerfiles now use **entrypoint-time compilation** (not image-build-time)
with **named volumes** for caching:

| Volume | Mount | Purpose |
|--------|-------|---------|
| `tdoku-build` | `/build` | C++ compiled binary |
| `rdoku-target` | `/rdoku/target` | Rust build artifacts |
| `cargo-registry` | `/usr/local/cargo/registry` | Downloaded crates |

Source is **bind-mounted** at runtime (`/tdoku`, `/rdoku`) so source changes
don't require rebuilding images.

#### Key files

- `tdoku/src/debug_driver.cc` — C++ main() accepting puzzle + limit args
- `rdoku/src/bin/debug_solver.rs` — Rust equivalent
- `rdoku/debug/compare.sh` — automated comparison script
- `rdoku/debug/docker-compose.yml` — compose alternative

### Trace Format

Both implementations emit to stderr (filtered to `DT:` prefix by compare.sh):

```
DT:INIT ok=<0|1> pcs=<p00>,<p01>,<p02>,<p10>,<p11>,<p12>
DT:C d=<depth> pcs=<p00>,<p01>,<p02>,<p10>,<p11>,<p12>
DT:T d=<depth> best=<0-5 | 4294967295=NONE>
DT:S d=<depth> n=<total>
```

- `pcs` = `configurations.popcount()` for each of 6 bands
- `best=4294967295` = no branch needed (DPLL unit propagation solved it)
- Max 2000 events (guarded by `MAX_EVENTS`)
- C++ guard: `DT_MAX` macro
- Noise lines (`CSPA_DEPTH:`, `BE_DEPTH:`) are non-DT, filtered by compare.sh

### `Cargo.toml` additions

```toml
[[bin]]
name = "debug_solver"
path = "src/bin/debug_solver.rs"

[features]
debug-trace = []
```

---

## Test Results

### Reference puzzle (`.5..83.17...1..4..3.4..56.8....3...9.9.8245....6....7...9....5...729..861.36.72.4`)

**Status**: ✅ IDENTICAL traces

```
DT:INIT ok=1 pcs=9,9,9,9,9,9
DT:C d=1 pcs=9,9,9,9,9,9
DT:T d=1 best=4294967295
DT:S d=1 n=1
```

This puzzle doesn't branch — fully determined by unit propagation.
Does **not** exercise the branching logic where the divergence likely is.

### Al Escargot (`800000000003600000...` in `'0'` format)

**Status**: Both return `count=0` after '0'-format fix (puzzle is invalid/unsolvable
when `'0'` chars are misinterpreted — likely the '0' puzzle needs dots for empties)

Both C++ and Rust now correctly skip '0' chars as empty cells.

**TODO**: Test with `'.'`-format version of the same puzzle, or use a hard puzzle
from a known-good test set to exercise actual branching.

---

## Current State / Next Steps

### Immediate next step

Run comparison on a puzzle that requires actual DPLL branching:

```bash
cd tdoku-to-rust/rdoku

# Al Escargot in '.' format:
./debug/compare.sh "8..........3.6.....7..9.2...6..5...3...4..6..5..7...9...3..4..5.5...3...1...8.67.." 2

# Or try a puzzle from the tdoku test suite:
./debug/compare.sh "$(head -1 ../tdoku/test/test_puzzles)" 2
```

### If traces diverge on the branching puzzle

1. Examine `rdoku/debug/trace_diff.txt` for first differing `DT:` line
2. Identify whether `DT:T best=` differs (branch choice) or `DT:C pcs=` differs (propagation result)
3. Trace back to the specific operation:
   - Different `best`: compare `choose_band_and_value_to_branch`
   - Different `pcs`: compare `band_eliminate` / SIMD operations

### If traces are IDENTICAL on branching puzzles

The bug is likely a **stack frame size** issue, not algorithmic:
- Rust may allocate `State` (480 bytes) on stack in hot path where C++ uses heap/register
- Check: does `branch_on_band_and_value` allocate `state_copy` on the stack?
  - In Rust: `let mut state_copy = state.clone()` — yes, 480 bytes on stack per recursion
  - In C++: `State state_copy = *state;` — also stack, but compiler may optimize differently
- Mitigation: Box the state or increase thread stack size
  ```rust
  // In generate.rs, wrap minimize in a thread with larger stack:
  std::thread::Builder::new()
      .stack_size(64 * 1024 * 1024)  // 64 MB
      .spawn(|| minimize(...))
      .unwrap().join().unwrap()
  ```

### Candidate root causes (priority order)

1. **`choose_band_and_value_to_branch`** returns different `best_band` → different search path → much deeper recursion in Rust
2. **`band_eliminate`** propagates differently → different state → different branching
3. **SIMD operations** (`Bitvec08x16::shuffle`, `min_epu8`, etc.) produce different results
4. **Stack frame size** — algorithms identical but Rust frames are larger
5. **State initialization** — `Default` for `State` may leave fields uninitialized differently

---

## File Change Summary (on `debug/simd-compare` branch)

### New files
- `tdoku/src/debug_driver.cc`
- `rdoku/src/bin/debug_solver.rs`
- `rdoku/debug/Dockerfile.tdoku`
- `rdoku/debug/Dockerfile.rdoku`
- `rdoku/debug/docker-compose.yml`
- `rdoku/debug/compare.sh`

### Modified files
- `tdoku/src/solver_dpll_triad_simd.cc`
  - Added `DEBUG_TRACE` instrumentation (`DT_INIT`, `DT_C`, `DT_T`, `DT_S` macros)
  - Fixed `'0'`-format handling in `InitVanillaByBand`
  - Fixed depth alignment (`DT_IN` before `DT_C`)
- `rdoku/src/solver_dpll_triad_simd.rs`
  - Added `debug-trace` feature with `mod dt` (emit_init, emit_c, emit_t, emit_s)
  - Added `CSPA_DEPTH` tracking with panic at depth > 5000
  - Fixed `'0'`-format handling in `init_vanilla_by_band`
  - Fixed NONE value format in `emit_t`
- `rdoku/Cargo.toml`
  - Added `[[bin]] debug_solver`
  - Added `[features] debug-trace = []`
