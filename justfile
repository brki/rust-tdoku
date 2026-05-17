# rdoku development tasks — https://github.com/casey/just
#
# Usage:
#   just              → list all recipes
#   just validate     → format + clippy + tests
#   just all          → full CI suite
#   just fuzz 60      → fuzz for 60s per target
#
# Install: brew install just   or   cargo install just

# ── helpers ───────────────────────────────────────────────────────────────────

# Ensure rustup-managed cargo takes priority over Homebrew cargo.
export PATH := env_var_or_default('HOME', '') + "/.cargo/bin:" + env_var_or_default('PATH', '')

# ── display help ──────────────────────────────────────────────────────────────

default:
    @just --list

# ── formatting ────────────────────────────────────────────────────────────────

# Check code formatting.
@format:
    cargo fmt --check

# Auto-fix code formatting.
@format-fix:
    cargo fmt

# ── linting ───────────────────────────────────────────────────────────────────

# Lint with strict warnings.
@lint:
    cargo clippy -- -D warnings

# Lint with auto-fix suggestions.
@lint-fix:
    cargo clippy --fix --allow-dirty

# ── checks ────────────────────────────────────────────────────────────────────

# Run format check + clippy.
@check: format lint

# ── testing ───────────────────────────────────────────────────────────────────

# Run unit tests only.
@test-unit:
    cargo test --lib

# Run integration + edge case + property tests.
@test-integration:
    cargo test --test integration --test edge_cases --test property_tests

# Run all tests (unit + integration).
@test: test-unit test-integration

# Run tests in release mode, skipping Docker comparison tests.
@test-release:
    cargo test --release -- --skip comparison

# ── combined ──────────────────────────────────────────────────────────────────

# Run checks + tests (pre-commit).
@validate: check test

# ── build ─────────────────────────────────────────────────────────────────────

# Debug build.
@build:
    cargo build

# Release build with LTO.
@build-release:
    cargo build --release

# ── dependencies ──────────────────────────────────────────────────────────────

# Regenerate Cargo.lock from scratch and verify the project builds.
# More thorough than `cargo update` — purges stale entries from removed deps.
@update-lock:
    rm -f Cargo.lock
    cargo generate-lockfile
    cargo check

# ── fuzzing ───────────────────────────────────────────────────────────────────

# Fuzz solver & generator (requires cargo-fuzz + nightly).
fuzz timeout="30" verbose="":
    #!/usr/bin/env bash
    # Usage: just fuzz [timeout_seconds] [verbose=1]
    #
    # A wall-clock timeout caps the run in addition to libFuzzer's
    # -max_total_time, because -max_total_time only counts time inside the
    # harness — a single slow puzzle can stall fuzzing indefinitely.
    set -euo pipefail

    # ── helpers ──────────────────────────────────────────────────────────
    red()    { echo -e "\033[0;31m$*\033[0m"; }
    green()  { echo -e "\033[0;32m$*\033[0m"; }
    yellow() { echo -e "\033[0;33m$*\033[0m"; }

    # Run a command with a wall-clock timeout.  On timeout the command is
    # killed and the function returns 143 (128 + SIGTERM).  Works on macOS
    # and Linux without external dependencies.
    run_with_timeout() {
        local timeout=$1
        shift
        "$@" &
        local pid=$!
        ( sleep "$timeout"; kill "$pid" 2>/dev/null ) &
        local watcher=$!
        wait "$pid" 2>/dev/null
        local rc=$?
        kill "$watcher" 2>/dev/null
        wait "$watcher" 2>/dev/null
        return $rc
    }

    # ── main ─────────────────────────────────────────────────────────────
    if ! command -v cargo-fuzz &>/dev/null; then
        yellow "  • cargo-fuzz not installed — skipping fuzz run"
        echo   "    install with: cargo install cargo-fuzz"
        exit 0
    fi

    if [ -n "{{verbose}}" ]; then
        verbose=1
    else
        verbose=""
    fi

    wall_cap=$(({{timeout}} + 10))
    failed=0

    for target in solve_fuzz generator_fuzz; do
        echo "── cargo fuzz run $target ({{timeout}}s, wall cap ${wall_cap}s)"
        set +e
        if [ -n "$verbose" ]; then
            run_with_timeout "$wall_cap" \
                cargo +nightly fuzz run "$target" \
                -- -max_total_time={{timeout}} -runs=10000
        else
            run_with_timeout "$wall_cap" \
                cargo +nightly fuzz run "$target" \
                -- -max_total_time={{timeout}} -runs=10000 \
                2>/dev/null
        fi
        rc=$?
        set -e
        if [ $rc -eq 0 ]; then
            green "  ✓ $target ({{timeout}}s, no crashes)"
        elif [ $rc -eq 143 ]; then
            yellow "  ⚠ $target timed out after ${wall_cap}s wall clock — no crash"
        else
            red "  ✗ $target — crash or setup issue (exit $rc)"
            failed=1
        fi
    done

    if [ $failed -ne 0 ]; then
        red "Fuzz run failed — see output above"
        exit 1
    fi

# ── comparison ────────────────────────────────────────────────────────────────

# Run Docker-based C++/Rust trace comparison tests.
@comparison verbose="":
    #!/usr/bin/env bash
    # Usage: just comparison [verbose=1]
    set -euo pipefail
    if [ -n "{{verbose}}" ]; then
        cargo test --release --test comparison -- --nocapture
    else
        cargo test --release --test comparison -- --nocapture 2>/dev/null
    fi

# ── generated puzzle verification ─────────────────────────────────────────────

# Generate and verify unique-solution puzzles.
@generated count="1000" verbose="":
    #!/usr/bin/env bash
    # Usage: just generated [count] [verbose=1]
    set -euo pipefail
    if [ -n "{{verbose}}" ]; then
        cargo run --release --bin generate_verify -- --count {{count}} --verbose
    else
        cargo run --release --bin generate_verify -- --count {{count}} 2>/dev/null
    fi

# ── benchmarks ────────────────────────────────────────────────────────────────

# Run criterion micro-benchmarks + legacy tdoku-style benchmark.
# Results saved to benchmark-results/<timestamp>/.
@bench:
    #!/usr/bin/env bash
    set -euo pipefail
    root="$(dirname "$0")"
    ts="$(date +%Y%m%d_%H%M%S)"
    outdir="$root/benchmark-results/$ts"
    mkdir -p "$outdir"
    echo "==> rdoku benchmarks"
    echo "    results: $outdir"
    echo ""

    # ── criterion ──
    echo "── cargo bench (criterion)"
    cargo bench 2>/dev/null | tee "$outdir/criterion.txt"
    echo ""

    # ── legacy tdoku-style ──
    echo "── cargo run --release --bin benchmark (tdoku-style)"
    tmp="$outdir/.bench_puzzles"
    echo "#ALLOWZERO" > "$tmp"
    cat tests/test_puzzles >> "$tmp"
    cargo run --release --bin benchmark -- -w 1 -t 5 -n 1000 "$tmp" \
        > "$outdir/legacy_benchmark.txt" \
        2> "$outdir/legacy_benchmark.txt.stderr"
    rm -f "$tmp"
    echo ""

    echo -e "\033[0;32m==> Benchmarks complete — results in $outdir\033[0m"

# ── full CI suite ─────────────────────────────────────────────────────────────

# Run everything: validate + fuzz + comparison + generated + bench.
all-tests fuzz-timeout="30" generated-count="1000" verbose="":
    # Usage: just all [fuzz-timeout] [generated-count]
    just validate
    just fuzz {{fuzz-timeout}} {{verbose}}
    just comparison {{verbose}}
    just generated {{generated-count}} {{verbose}}
    echo -e "\033[0;32m==> All checks run\033[0m"
