#!/usr/bin/env bash
# rdoku benchmark suite — runs criterion micro-benchmarks and the legacy
# tdoku-style benchmark binary.  Results are saved to benchmark-results/ with
# a timestamp.
set -euo pipefail

GREEN='\033[0;32m'
NC='\033[0m'

usage() {
    echo "usage: bench.sh [flags]"
    echo ""
    echo "flags:"
    echo "  -h, --help   show this help"
    exit 0
}

for arg in "$@"; do
    case "$arg" in
        -h|--help) usage ;;
        *) echo "unknown flag: $arg (use -h for help)" >&2; exit 2 ;;
    esac
done

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
OUTDIR="$PROJECT_DIR/benchmark-results/$TIMESTAMP"
mkdir -p "$OUTDIR"

echo "==> rdoku benchmarks"
echo "    results: $OUTDIR"
echo ""

# ── criterion benchmarks ──────────────────────────────────────────────────────
echo "── cargo bench (criterion)"
cargo bench 2>&1 | tee "$OUTDIR/criterion.txt"
echo ""

# ── legacy tdoku-style benchmark ───────────────────────────────────────────────
# The test_puzzles file contains an unsolvable puzzle (count=0).  The
# benchmark rejects these unless the file declares #ALLOWZERO.  We create a
# temporary file with that header so the benchmark handles them gracefully.
echo "── cargo run --release --bin benchmark (tdoku-style)"
BENCH_PUZZLES="$(mktemp)"
trap 'rm -f "$BENCH_PUZZLES"' EXIT
{ echo '#ALLOWZERO'; cat "$PROJECT_DIR/tests/test_puzzles"; } > "$BENCH_PUZZLES"

cargo run --release --bin benchmark -- \
    -w 1 -t 5 -n 1000 \
    "$BENCH_PUZZLES" \
    2>&1 | tee "$OUTDIR/legacy_benchmark.txt"
echo ""

echo -e "${GREEN}==> Benchmarks complete — results in $OUTDIR${NC}"
