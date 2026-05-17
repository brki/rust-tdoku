# rdoku development tasks — https://github.com/casey/just
#
# Usage:
#   just              → list all recipes
#   just validate     → format + clippy + tests
#   just all          → full CI suite
#   just fuzz 60      → fuzz all targets for 60s each
#   just fuzz-one solve_fuzz 120 true  → fuzz single target
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
    set -euo pipefail
    failed=0
    for target in solve_fuzz generator_fuzz; do
        just fuzz-one "$target" "{{timeout}}" "{{verbose}}" || failed=1
    done
    exit $failed

# Fuzz a single target (requires cargo-fuzz + nightly + GNU timeout).
# Install GNU timeout on macOS:  brew install coreutils
# Usage: just fuzz-one solve_fuzz [timeout_seconds] [verbose=1]
fuzz-one target timeout="30" verbose="":
    #!/usr/bin/env bash
    set -euo pipefail

    red()    { echo -e "\033[0;31m$*\033[0m"; }
    green()  { echo -e "\033[0;32m$*\033[0m"; }
    yellow() { echo -e "\033[0;33m$*\033[0m"; }

    if ! command -v cargo-fuzz &>/dev/null; then
        yellow "  • cargo-fuzz not installed — skipping fuzz run"
        echo   "    install with: cargo install cargo-fuzz"
        exit 0
    fi

    if ! command -v timeout &>/dev/null; then
        red "  ✗ GNU timeout not found — install with: brew install coreutils"
        exit 1
    fi

    # Wall-clock cap: fuzz timeout + 10 s grace period.
    wall_cap=$(({{timeout}} + 10))

    echo "── cargo fuzz run {{target}} ({{timeout}}s, wall cap ${wall_cap}s)"
    set +e
    if [ -n "{{verbose}}" ]; then
        timeout --foreground "$wall_cap" \
            cargo +nightly fuzz run "{{target}}" \
            -- -max_total_time={{timeout}} -runs=10000
    else
        timeout --foreground "$wall_cap" \
            cargo +nightly fuzz run "{{target}}" \
            -- -max_total_time={{timeout}} -runs=10000 \
            2>/dev/null
    fi
    rc=$?
    set -e
    if [ $rc -eq 0 ]; then
        green "  ✓ {{target}} ({{timeout}}s, no crashes)"
    elif [ $rc -eq 124 ] || [ $rc -eq 143 ]; then
        yellow "  ⚠ {{target}} timed out after ${wall_cap}s wall clock — no crash"
    else
        red "  ✗ {{target}} — crash or setup issue (exit $rc)"
        exit 1
    fi

# ── AFL++ fuzzing (binary-level) ──────────────────────────────────────────────

# Build AFL-instrumented harnesses (requires cargo-afl + AFL++).
@afl-build:
    #!/usr/bin/env bash
    set -euo pipefail

    yellow() { echo -e "\033[0;33m$*\033[0m"; }

    if ! command -v cargo-afl &>/dev/null; then
        yellow "  • cargo-afl not installed — install with: cargo install cargo-afl"
        yellow "  • also install AFL++: brew install afl++ (macOS) or apt install afl++ (Linux)"
        exit 1
    fi

    if ! command -v afl-fuzz &>/dev/null; then
        yellow "  • afl-fuzz not found — install AFL++: brew install afl++ (macOS)"
        exit 1
    fi

    # One-time system setup for shared memory (requires sudo).
    if [ "$(sysctl -n kern.sysv.shmmax 2>/dev/null || echo 0)" -lt 67108864 ]; then
        yellow "  • System V shared memory limit too low for AFL++."
        yellow "    Run: cargo afl system-config"
        yellow "    (requires sudo to set kern.sysv.shmmax)"
        exit 1
    fi

    echo "==> Building rdoku binaries (debug, for fuzzing)"
    cargo build --bin solve --bin generate --bin benchmark --bin debug_solver

    echo "==> Building AFL harnesses"
    cd fuzz-afl
    cargo afl config --build --force
    cargo afl build
    cd ..
    echo "  ✓ AFL harnesses built"

# Fuzz all AFL targets for N seconds each.
# Usage: just afl-fuzz [timeout=30] [log=1]  (set log=1 to log invocations to afl-logs/)
afl-fuzz timeout="30" verbose="" log="":
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-afl &>/dev/null; then
        echo -e "\033[0;33m  • cargo-afl not installed — install with: cargo install cargo-afl\033[0m"
        exit 1
    fi
    failed=0
    for target in afl_solve afl_generate afl_benchmark afl_debug_solver; do
        just afl-fuzz-one "$target" "{{timeout}}" "{{verbose}}" "{{log}}" || failed=1
    done
    exit $failed

# Fuzz a single AFL target (requires cargo-afl + AFL++).
# Usage: just afl-fuzz-one afl_solve [timeout_seconds] [verbose=1] [log=1]
afl-fuzz-one target timeout="30" verbose="" log="":
    #!/usr/bin/env bash
    set -euo pipefail

    red()    { echo -e "\033[0;31m$*\033[0m"; }
    green()  { echo -e "\033[0;32m$*\033[0m"; }
    yellow() { echo -e "\033[0;33m$*\033[0m"; }

    if ! command -v cargo-afl &>/dev/null; then
        yellow "  • cargo-afl not installed — skipping AFL fuzz run"
        echo   "    install with: cargo install cargo-afl"
        exit 0
    fi

    if ! command -v afl-fuzz &>/dev/null; then
        yellow "  • afl-fuzz not found — install AFL++: brew install afl++ (macOS)"
        exit 1
    fi

    # ── logging setup ──────────────────────────────────────────────────
    if [ -n "{{log}}" ]; then
        log_dir="afl-logs"
        mkdir -p "$log_dir"
        log_file="$log_dir/{{target}}.log"
        # Truncate this target's log (fresh start for this run).
        :> "$log_file"
        export RDOKU_AFL_LOG="$log_file"
        green "  • logging invocations to $log_file"
    fi

    # Directories
    afl_dir="fuzz-afl"
    corp_dir="$afl_dir/corpus/{{target}}"
    out_parent="$afl_dir/output"

    # Ensure corpus directory exists with at least one seed.
    mkdir -p "$corp_dir"
    if [ -z "$(ls -A "$corp_dir" 2>/dev/null)" ]; then
        # afl_generate is slow — use a minimal seed that maps to a fast profile.
        if [ "{{target}}" = "afl_generate" ]; then
            printf '\x00\x00\x01\x00\x00\x00\x00\x00' > "$corp_dir/seed_fast"
        elif [ -f tests/test_puzzles ]; then
            cp tests/test_puzzles "$corp_dir/seed_puzzles"
        else
            printf '\x00\x00\x00' > "$corp_dir/seed_min"
        fi
        yellow "  • seeded corpus: $corp_dir"
    fi

    # Ensure output parent directory exists (AFL++ creates the target subdir).
    mkdir -p "$out_parent"

    # Clean previous output for this target to avoid conflicts.
    rm -rf "$out_parent/{{target}}"

    echo "── afl fuzz {{target}} ({{timeout}}s)"

    # macOS: bypass crash reporter check (requires cargo afl system-config for shm).
    export AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES=1

    if [ -n "{{verbose}}" ]; then
        timeout --foreground "$(({{timeout}} + 5))" \
            nice -n 19 cargo afl fuzz -i "$corp_dir" -o "$out_parent" \
            -t "5000" -V "{{timeout}}" \
            -- "fuzz-afl/target/debug/{{target}}"
    else
        timeout --foreground "$(({{timeout}} + 5))" \
            nice -n 19 cargo afl fuzz -i "$corp_dir" -o "$out_parent" \
            -t "5000" -V "{{timeout}}" \
            -- "fuzz-afl/target/debug/{{target}}" \
            2>/dev/null
    fi
    rc=$?

    if [ $rc -eq 0 ]; then
        green "  ✓ {{target}} ({{timeout}}s, no crashes)"
    elif [ $rc -eq 124 ] || [ $rc -eq 143 ]; then
        yellow "  ⚠ {{target}} timed out — no crash"
    else
        red "  ✗ {{target}} — crash or setup issue (exit $rc)"
        exit 1
    fi

# Minimize a crashing AFL test case.
# Usage: just afl-tmin <target> <input_file>
afl-tmin target input:
    #!/usr/bin/env bash
    set -euo pipefail
    afl_dir="fuzz-afl"
    cargo afl tmin -i "{{input}}" -o "{{input}}.min" -- "$afl_dir/target/debug/{{target}}"
    echo "Minimized to {{input}}.min"

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
