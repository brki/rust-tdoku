#!/usr/bin/env bash
# rdoku test suite — runs formatting, linting, unit/integration tests, and a
# short fuzz run.  Use as a pre-commit or pre-push checklist.
#
#   --comparison    Also run the Docker-based C++/Rust trace comparison test
#                   (requires Docker with amd64 support; skipped otherwise).
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

pass()  { echo -e "  ${GREEN}✓${NC} $*"; }
fail()  { echo -e "  ${RED}✗${NC} $*"; }
info()  { echo -e "  ${YELLOW}•${NC} $*"; }

RUN_COMPARISON=false
for arg in "$@"; do
    case "$arg" in
        --comparison) RUN_COMPARISON=true ;;
        *) echo "unknown flag: $arg" >&2; exit 2 ;;
    esac
done

EXIT=0
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

echo "==> rdoku test suite"
echo "    project: $PROJECT_DIR"
[ "$RUN_COMPARISON" = true ] && echo "    comparison: enabled"
echo ""

# ── formatting ────────────────────────────────────────────────────────────────
echo "── cargo fmt --check"
if cargo fmt --check 2>/dev/null; then
    pass "formatting"
else
    fail "formatting — run 'cargo fmt' to fix"
    EXIT=1
fi

# ── clippy ─────────────────────────────────────────────────────────────────────
echo "── cargo clippy -- -D warnings"
if cargo clippy -- -D warnings 2>/dev/null; then
    pass "clippy (no warnings)"
else
    fail "clippy — see output above"
    EXIT=1
fi

# ── unit + integration tests ───────────────────────────────────────────────────
echo "── cargo test --release -- --skip comparison"
if cargo test --release -- --skip comparison 2>/dev/null; then
    pass "tests (release)"
else
    fail "tests — see output above"
    EXIT=1
fi

# ── fuzz (short run, 30 s) ─────────────────────────────────────────────────────
if command -v cargo-fuzz &>/dev/null || cargo fuzz --help &>/dev/null 2>&1; then
    echo "── cargo fuzz run solve_fuzz (30 s)"
    if cargo fuzz run solve_fuzz -- -max_total_time=30 -runs=10000 2>/dev/null; then
        pass "fuzz (30 s, no crashes)"
    else
        fail "fuzz — crash detected or setup issue"
        EXIT=1
    fi
else
    info "cargo-fuzz not installed — skipping fuzz run"
    info "  install with: cargo install cargo-fuzz"
fi

# ── comparison (Docker, opt-in) ───────────────────────────────────────────────
if [ "$RUN_COMPARISON" = true ]; then
    echo "── cargo test --test comparison -- --nocapture"
    if cargo test --release --test comparison -- --nocapture 2>/dev/null; then
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
