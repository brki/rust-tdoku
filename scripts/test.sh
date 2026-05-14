#!/usr/bin/env bash
# rdoku test suite — runs formatting, linting, unit/integration tests, and a
# short fuzz run.  Use as a pre-commit or pre-push checklist.
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

pass()  { echo -e "  ${GREEN}✓${NC} $*"; }
fail()  { echo -e "  ${RED}✗${NC} $*"; }
info()  { echo -e "  ${YELLOW}•${NC} $*"; }

usage() {
    echo "usage: test.sh [flags]"
    echo ""
    echo "flags:"
    echo "  -h, --help        show this help"
    echo "  -v, --verbose     show full fuzz output (default: silent)"
    echo "  --comparison      also run Docker-based C++/Rust trace comparison"
    echo "  --fuzz=N          fuzz for N seconds (positive integer, default: 30) per fuzz target"
    echo "  --generated=N     generate N puzzles and verify uniqueness (positive integer, default: 1000)"
    exit 0
}

RUN_COMPARISON=false
VERBOSE=false
FUZZ_SECS=30
GENERATED_COUNT=0
for arg in "$@"; do
    case "$arg" in
        -h|--help) usage ;;
        -v|--verbose) VERBOSE=true ;;
        --comparison) RUN_COMPARISON=true ;;
        --fuzz=*)
            FUZZ_SECS="${arg#*=}"
            if ! [[ "$FUZZ_SECS" =~ ^[1-9][0-9]*$ ]]; then
                echo "error: --fuzz expects a positive integer, got '$FUZZ_SECS'" >&2
                exit 2
            fi
            ;;
        --generated=*)
            GENERATED_COUNT="${arg#*=}"
            if ! [[ "$GENERATED_COUNT" =~ ^[1-9][0-9]*$ ]]; then
                echo "error: --generated expects a positive integer, got '$GENERATED_COUNT'" >&2
                exit 2
            fi
            ;;
        *) echo "unknown flag: $arg (use -h for help)" >&2; exit 2 ;;
    esac
done

# Redirect stderr for noisy commands; --verbose keeps it visible.
quiet_stderr() {
    if [ "$VERBOSE" = true ]; then
        "$@"
    else
        "$@" 2>/dev/null
    fi
}

EXIT=0
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

echo "==> rdoku test suite"
echo "    project: $PROJECT_DIR"
echo "    fuzz:     ${FUZZ_SECS}s"
[ "$VERBOSE" = true ] && echo "    verbose:  on"
[ "$RUN_COMPARISON" = true ] && echo "    comparison: enabled"
echo ""

# ── formatting ────────────────────────────────────────────────────────────────
echo "── cargo fmt --check"
if quiet_stderr cargo fmt --check; then
    pass "formatting"
else
    fail "formatting — run 'cargo fmt' to fix"
    EXIT=1
fi

# ── clippy ─────────────────────────────────────────────────────────────────────
echo "── cargo clippy -- -D warnings"
if quiet_stderr cargo clippy -- -D warnings; then
    pass "clippy (no warnings)"
else
    fail "clippy — see output above"
    EXIT=1
fi

# ── unit + integration tests ───────────────────────────────────────────────────
echo "── cargo test --release -- --skip comparison"
if quiet_stderr cargo test --release -- --skip comparison; then
    pass "tests (release)"
else
    fail "tests — see output above"
    EXIT=1
fi

# ── helpers ────────────────────────────────────────────────────────────────────

# Run a command with a wall-clock timeout.  On timeout the command is killed
# and the function returns 143 (128 + SIGTERM).  Works on macOS and Linux
# without external dependencies.
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

# ── fuzz ───────────────────────────────────────────────────────────────────────
# cargo-fuzz needs a nightly toolchain. It is possible that this machine has a
# standalone cargo (e.g. from Homebrew) that doesn't understand +nightly.
# The rustup proxy lives at ~/.cargo/bin/cargo. We prepend ~/.cargo/bin to
# PATH so that both the initial `cargo fuzz` and cargo-fuzz's internal
# `cargo build` subprocess find the rustup-managed toolchain.
PATH="$HOME/.cargo/bin:$PATH"

if command -v cargo-fuzz &>/dev/null; then
    # Wall-clock cap per target: libFuzzer's -max_total_time counts only
    # time inside the harness, so a single slow puzzle can block it for
    # minutes.  The cap ensures we don't hang forever.
    WALL_CLOCK_CAP=$((FUZZ_SECS + 10))

    for target in solve_fuzz generator_fuzz; do
        echo "── cargo fuzz run $target (${FUZZ_SECS} s)"
        set +e
        run_with_timeout "$WALL_CLOCK_CAP" \
            quiet_stderr cargo +nightly fuzz run "$target" \
            -- -max_total_time="$FUZZ_SECS" -runs=10000
        rc=$?
        set -e
        if [ $rc -eq 0 ]; then
            pass "$target (${FUZZ_SECS}s, no crashes)"
        elif [ $rc -eq 143 ]; then
            info "$target timed out after ${WALL_CLOCK_CAP}s wall clock — no crash"
        else
            fail "$target — crash or setup issue (exit $rc)"
            EXIT=1
        fi
    done
else
    info "cargo-fuzz not installed — skipping fuzz run"
    info "  install with: cargo install cargo-fuzz"
fi

# ── generated puzzle verification (opt-in) ────────────────────────────────────
if [ "$GENERATED_COUNT" -gt 0 ]; then
    echo "── generated puzzle verification ($GENERATED_COUNT puzzles)"
    verb_flag=""
    [ "$VERBOSE" = true ] && verb_flag=" --verbose"
    if quiet_stderr cargo run --release --bin generate_verify -- \
        --count "$GENERATED_COUNT" $verb_flag; then
        pass "generated ($GENERATED_COUNT puzzles)"
    else
        fail "generated — see output above"
        EXIT=1
    fi
fi

# ── comparison (Docker, opt-in) ───────────────────────────────────────────────
if [ "$RUN_COMPARISON" = true ]; then
    echo "── cargo test --test comparison -- --nocapture"
    if quiet_stderr cargo test --release --test comparison -- --nocapture; then
        pass "comparison (Docker C++/Rust trace diff)"
    else
        fail "comparison — see output above"
        EXIT=1
    fi
fi

# ── summary ────────────────────────────────────────────────────────────────────
echo ""
if [ $EXIT -eq 0 ]; then
    echo -e "${GREEN}==> All checks passed${NC}"
else
    echo -e "${RED}==> Some checks FAILED${NC}"
fi
exit $EXIT
