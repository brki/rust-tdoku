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
afl-build:
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

    echo "==> Building rdoku binaries (release, for fuzzing)"
    cargo build --release --bin solve --bin generate

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
    for target in afl_solve afl_generate; do
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

    # Resume from previous queue if it exists; otherwise start from corpus.
    # Use 'just afl-clean' to wipe the queue for a fresh start.
    if [ -d "$out_parent/{{target}}" ]; then
        # Archive crashes before resuming to preserve historical crash data
        if [ -d "$out_parent/{{target}}/crashes" ] && [ -n "$(ls -A "$out_parent/{{target}}/crashes" 2>/dev/null)" ]; then
            timestamp=$(date +%Y%m%d_%H%M%S)
            archive_dir="$afl_dir/crashes_archived/{{target}}_$timestamp"
            mkdir -p "$archive_dir"
            mv "$out_parent/{{target}}/crashes"/* "$archive_dir/"
            green "  • archived crashes to $archive_dir"
        fi
        fuzz_args=(-i-)
    else
        fuzz_args=(-i "$corp_dir")
    fi

    echo "── afl fuzz {{target}} ({{timeout}}s)"

    # macOS: bypass crash reporter check (requires cargo afl system-config for shm).
    export AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES=1

    if [ -n "{{verbose}}" ]; then
        timeout --foreground "$(({{timeout}} + 5))" \
            nice -n 19 cargo afl fuzz "${fuzz_args[@]}" -o "$out_parent/{{target}}" \
            -t "15000" -V "{{timeout}}" \
            -- "fuzz-afl/target/debug/{{target}}"
    else
        timeout --foreground "$(({{timeout}} + 5))" \
            nice -n 19 cargo afl fuzz "${fuzz_args[@]}" -o "$out_parent/{{target}}" \
            -t "15000" -V "{{timeout}}" \
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

# Decode an AFL++ fuzz input file to see what command it represents.
# Usage: just afl-decode fuzz-afl/output/afl_solve/default/crashes/id:000000,sig:06,...
afl-decode input:
    #!/usr/bin/env bash
    set -euo pipefail
    cd fuzz-afl
    cargo build --release --bin decode_fuzz_input -q 2>&1
    cd ..
    fuzz-afl/target/release/decode_fuzz_input "{{input}}"

# Remove all AFL++ output directories for a fresh start on next fuzz run.
@afl-clean:
    rm -rf fuzz-afl/output fuzz-afl/crashes_archived
    echo "  ✓ AFL++ output and archived crashes cleared"

# Show AFL++ fuzzing status for all active output directories.
afl-status:
    #!/usr/bin/env bash
    set -euo pipefail
    out_parent="fuzz-afl/output"
    if ! command -v afl-whatsup &>/dev/null; then
        echo "afl-whatsup not found — install AFL++: brew install afl++ (macOS)"
        exit 1
    fi
    if [ -z "$(ls -A "$out_parent" 2>/dev/null)" ]; then
        echo "No AFL++ output directories found in $out_parent"
        exit 0
    fi
    afl-whatsup "$out_parent"

# Minimize a crashing AFL test case.
# Usage: just afl-tmin <target> <input_file>
afl-tmin target input:
    #!/usr/bin/env bash
    set -euo pipefail
    afl_dir="fuzz-afl"
    cargo afl tmin -i "{{input}}" -o "{{input}}.min" -- "$afl_dir/target/debug/{{target}}"
    echo "Minimized to {{input}}.min"

# Inspect an AFL crash or hang by running the harness with a specific input.
# Logs detailed execution trace to a log file and displays it.
# The target (afl_generate or afl_solve) is deduced from the input file path.
# Usage: just afl-inspect fuzz-afl/output/afl_generate/default/crashes/id:000000,sig:06,...
#        just afl-inspect fuzz-afl/output/afl_solve/default/hangs/id:000001,... /path/to/custom.log
afl-inspect input logfile="/tmp/afl-inspect.log":
    #!/usr/bin/env bash
    set -euo pipefail

    # Extract target from input path: fuzz-afl/output/<target>/...
    target=$(echo "{{input}}" | sed 's|.*fuzz-afl/output/\([^/]*\).*|\1|')

    if [ -z "$target" ]; then
        echo "Error: cannot deduce target from input path {{input}}"
        echo "Expected path like: fuzz-afl/output/<target>/default/crashes/..."
        exit 1
    fi

    if ! [[ "$target" =~ ^afl_(generate|solve)$ ]]; then
        echo "Error: target must be afl_generate or afl_solve, got: $target"
        exit 1
    fi

    if [ ! -f "{{input}}" ]; then
        echo "Error: input file not found: {{input}}"
        exit 1
    fi

    log_file="{{logfile}}"
    echo "── Inspecting $target with input: {{input}}"
    echo "  Log: $log_file"
    echo ""

    # Truncate the log file so this run starts fresh.
    : > "$log_file"

    # AFL-instrumented binaries hang when run standalone — they try to handshake
    # with an afl-fuzz parent via a forkserver pipe that never comes.  Build the
    # non-instrumented replay binaries with plain cargo build instead.
    ( cd fuzz-afl && cargo build --release --bin "replay_${target}" -q 2>&1 )

    # Export log file for harness to write detailed trace
    export RDOKU_AFL_LOG="$log_file"

    # Run the non-instrumented replay harness with the input file
    set +e
    "fuzz-afl/target/release/replay_${target}" < "{{input}}"
    rc=$?
    set -e

    echo ""
    echo "── Output log:"
    if [ -f "$log_file" ]; then
        cat "$log_file"
    else
        echo "(no log file was created)"
    fi

    exit $rc

# ── comparison ────────────────────────────────────────────────────────────────

# Run Docker-based C++/Rust trace comparison tests.
comparison verbose="":
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
generated count="1000" verbose="":
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
    just validate
    just fuzz {{fuzz-timeout}} {{verbose}}
    just comparison {{verbose}}
    just generated {{generated-count}} {{verbose}}
    echo -e "\033[0;32m==> All checks run\033[0m"
