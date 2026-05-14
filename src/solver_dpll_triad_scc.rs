//! DPLL solver with triad constraints and SCC variable-selection heuristic —
//! port of `tdoku/src/solver_dpll_triad_scc.cc`.
//!
//! # SAT encoding
//!
//! Sudoku is reduced to a SAT problem with 2592 Boolean literals representing
//! every possible (box, element, value) triple.  For each of the 9 boxes there
//! are 16 *elements* arranged in a 4×4 logical grid:
//!
//! ```text
//!   cells            triads
//! ┌─────┬─────┬─────┬─────┐
//! │ 0,0 │ 0,1 │ 0,2 │  H0 │  ← row 0: 3 cells + 1 horizontal triad
//! ├─────┼─────┼─────┼─────┤
//! │ 1,0 │ 1,1 │ 1,2 │  H1 │  ← row 1
//! ├─────┼─────┼─────┼─────┤
//! │ 2,0 │ 2,1 │ 2,2 │  H2 │  ← row 2
//! ├─────┼─────┼─────┼─────┤
//! │ V0  │ V1  │ V2  │  —  │  ← row 3: 3 vertical triads + 1 unused slot
//! └─────┴─────┴─────┴─────┘
//! ```
//!
//! This gives 9 cells + 3 horizontal triads + 3 vertical triads = 15 used
//! elements per box.  With 9 values per element we get 15 × 9 = 135 positive
//! literals per box, doubled to 270 with negations — hence 9 × 270 = **2430**
//! *used* literals (the remaining 162 are padding for the unused 16th slot).
//!
//! ## Constraints
//!
//! - **ExactlyN (cell, value)**: each cell must have exactly one value — one
//!   of the 9 literals is true, the other 8 are false.
//! - **ExactlyN (triad, value)**: each triad (horizontal or vertical) must
//!   have exactly one of a given value across its 3 cells.
//! - **ExactlyN (band-triad, value)**: across the 3 boxes in a band, the
//!   3 triads that span the same 3 columns/rows must contain exactly one of
//!   each value.
//!
//! ExactlyN constraints are encoded as one *positive clause* (at-least-n)
//! plus either pairwise mutual-exclusion implications (n = 1) or a single
//! *negative clause* (at-most-n via negations).
//!
//! ## Search
//!
//! - **BCP** (Boolean Constraint Propagation): when all but one literal in a
//!   clause are eliminated, the survivor must be asserted.  This cascades
//!   through the implication graph built from pairwise exclusions.
//! - **Path-based SCC** (Pearce's algorithm): the implication graph is
//!   analyzed for strongly-connected components.  Asserting any literal in a
//!   non-trivial SCC forces all others in that SCC, so the solver uses
//!   component size as a branching heuristic and infers forced literals.
//! - **Config flags**: `config` bits control SCC inference and SCC heuristic
//!   independently (config=3 enables both).

use std::cell::RefCell;

// ---------------------------------------------------------------------------
// Constants & types
// ---------------------------------------------------------------------------

const NUM_BOXES: usize = 9;
const NUM_POS_CLAUSES_PER_BOX: usize = 16; // 9 cells + 6 triads + 1 slack
const NUM_VALUES: usize = 9;

/// Total number of literals: each (box, element, value) yields one positive
/// literal and one negative literal.
const NUM_LITERALS: usize = NUM_BOXES * NUM_POS_CLAUSES_PER_BOX * NUM_VALUES * 2; // 2592

/// A puzzle is solved when this many positive literals are asserted (one per
/// cell/triad element per value, excluding the unused 16th slot per box).
const ALL_ASSERTED: u32 = (NUM_BOXES * (NUM_POS_CLAUSES_PER_BOX - 1) * NUM_VALUES) as u32; // 1215

/// Number of 64-bit words needed to cover `NUM_LITERALS` bits.
const NUM_BITSET_WORDS: usize = NUM_LITERALS / 64 + 1; // 41

type ClauseId = u32;
type LiteralId = u32;
const NO_LITERAL: LiteralId = u32::MAX;

// ---------------------------------------------------------------------------
// Literal helpers
// ---------------------------------------------------------------------------

/// Negate a literal (flip the low bit).
#[inline(always)]
fn lit_not(l: LiteralId) -> LiteralId {
    l ^ 1
}

/// Build a *positive* literal id for (box, element, value).
///
/// Element is an index into the 4×4 layout of the box:
///   - rows 0-2, cols 0-2 → the 9 actual cells
///   - rows 0-2, col 3     → horizontal triads
///   - row 3,   cols 0-2  → vertical triads
///   - row 3,   col 3     → unused (no literal)
#[inline(always)]
fn lit(box_idx: usize, elem: usize, value: usize) -> LiteralId {
    (2 * (elem + 16 * (value + 9 * box_idx))) as LiteralId
}

/// Returns true if the literal corresponds to a real (box, element, value)
/// rather than the unused filler slots at the end of each box's 4×4 grid.
#[inline(always)]
fn valid_literal(l: LiteralId) -> bool {
    (l % 32) & 0x1e != 0x1e
}

// ---------------------------------------------------------------------------
// FastBitset
// ---------------------------------------------------------------------------

/// Dense bitset covering `NUM_LITERALS` bits, backed by a fixed array.
#[derive(Clone)]
struct FastBitset {
    bits: [u64; NUM_BITSET_WORDS],
}

impl Default for FastBitset {
    fn default() -> Self {
        FastBitset {
            bits: [0u64; NUM_BITSET_WORDS],
        }
    }
}

impl FastBitset {
    #[inline(always)]
    fn set(&mut self, index: LiteralId) {
        self.bits[(index >> 6) as usize] |= 1u64 << (index & 63);
    }

    #[inline(always)]
    fn get(&self, index: LiteralId) -> bool {
        self.bits[(index >> 6) as usize] & (1u64 << (index & 63)) != 0
    }

    /// Returns true if either the positive or the negative version of the
    /// variable at `index` is asserted (i.e. if the variable is determined).
    #[inline(always)]
    fn pos_or_neg(&self, index: LiteralId) -> bool {
        // Round down to the positive literal then test both bits at once.
        let positive = index & !1u32;
        self.bits[(positive >> 6) as usize] & (3u64 << (positive & 63)) != 0
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// The mutable solver state that is cloned when the DPLL search branches.
#[derive(Clone)]
struct State {
    /// Bit is 1 iff the literal has been asserted.
    asserted: FastBitset,
    /// For each clause: number of literals that can still be eliminated before
    /// the clause becomes unit.  Decremented as literals are negated.
    clause_free_literals: Vec<u16>,
    /// For each literal: logical size of its implication list (stack pointer).
    /// The actual `literals_to_implications` vec can be longer; we use this
    /// count to track how many implications have been discovered so far.
    implication_counts: Vec<u16>,
    /// How many positive literals have been asserted so far.
    num_asserted: u32,
}

impl State {
    fn new(num_literals: usize) -> Self {
        State {
            asserted: FastBitset::default(),
            clause_free_literals: Vec::new(),
            implication_counts: vec![0u16; num_literals],
            num_asserted: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Setup helpers (free functions to avoid borrow conflicts)
// ---------------------------------------------------------------------------

/// Push an implication `from → to` into the global implication lists,
/// incrementing the logical size tracked in `implication_counts`.
#[allow(clippy::ptr_arg)]
fn setup_add_implication(
    from: LiteralId,
    to: LiteralId,
    impls: &mut Vec<Vec<LiteralId>>,
    counts: &mut Vec<u16>,
) {
    let count = counts[from as usize] as usize;
    let v = &mut impls[from as usize];
    if v.len() == count {
        v.push(to);
    } else {
        v[count] = to;
    }
    counts[from as usize] += 1;
}

/// Register a clause covering `literals` with `min` required true literals,
#[allow(clippy::ptr_arg)]
/// updating all the bookkeeping data structures.
fn setup_add_clause_with_min(
    literals: &[LiteralId],
    min: usize,
    clauses_to_lits: &mut Vec<Vec<LiteralId>>,
    lits_to_clauses: &mut Vec<Vec<ClauseId>>,
    clause_free_literals: &mut Vec<u16>,
    positive_cell_clauses: &mut Vec<ClauseId>,
) {
    let new_clause_id = clauses_to_lits.len() as ClauseId;
    for &l in literals {
        lits_to_clauses[l as usize].push(new_clause_id);
    }
    clauses_to_lits.push(literals.to_vec());
    // free slots = total slots minus the minimum required and minus one
    // (the last required literal cannot be "free"; it triggers unit prop).
    clause_free_literals.push((literals.len() - 1 - min) as u16);
    // Track 9-literal unit clauses as branching candidates.
    if min == 1 && literals.len() == 9 {
        positive_cell_clauses.push(new_clause_id);
    }
}

/// Build ExactlyN constraints for a group of literals.
#[allow(clippy::too_many_arguments)]
fn setup_add_exactly_n(
    literals: &[LiteralId],
    n: usize,
    clauses_to_lits: &mut Vec<Vec<LiteralId>>,
    lits_to_clauses: &mut Vec<Vec<ClauseId>>,
    clause_free_literals: &mut Vec<u16>,
    positive_cell_clauses: &mut Vec<ClauseId>,
    impls: &mut Vec<Vec<LiteralId>>,
    counts: &mut Vec<u16>,
) {
    // Positive clause: at least n of the literals must be true.
    setup_add_clause_with_min(
        literals,
        n,
        clauses_to_lits,
        lits_to_clauses,
        clause_free_literals,
        positive_cell_clauses,
    );

    if n == 1 {
        // Pairwise mutual-exclusion implications: if any one is true, all
        // others are false.
        for i in 0..literals.len() - 1 {
            for j in i + 1..literals.len() {
                setup_add_implication(literals[i], lit_not(literals[j]), impls, counts);
                setup_add_implication(literals[j], lit_not(literals[i]), impls, counts);
            }
        }
    } else {
        // Negative clause: at most n of the literals can be true, expressed
        // as "at least (size - n) of the negations must be true".
        let negations: Vec<LiteralId> = literals.iter().map(|&l| lit_not(l)).collect();
        setup_add_clause_with_min(
            &negations,
            negations.len() - n,
            clauses_to_lits,
            lits_to_clauses,
            clause_free_literals,
            positive_cell_clauses,
        );
    }
}

// ---------------------------------------------------------------------------
// Solver struct
// ---------------------------------------------------------------------------

struct SolverDpllTriadScc {
    // ── Immutable after construction ────────────────────────────────────────
    /// clauses_to_literals[clause_id] → list of literals in the clause.
    clauses_to_literals: Vec<Vec<LiteralId>>,
    /// literals_to_clauses[literal] → list of clauses containing that literal.
    literals_to_clauses: Vec<Vec<ClauseId>>,
    /// Cell-only clauses (9 literals, min=1) used as branching heuristic
    /// when SCC is disabled.
    positive_cell_clauses: Vec<ClauseId>,
    /// Initial constraint state cloned for each puzzle.
    initial_state: State,

    // ── Grows during search ─────────────────────────────────────────────────
    /// literals_to_implications[from] → implications discovered so far.
    /// The logical size is tracked by `State::implication_counts`.
    literals_to_implications: Vec<Vec<LiteralId>>,

    // ── Temporary SCC state (reset each run) ────────────────────────────────
    preorder_index: Vec<i32>,
    stack_p: Vec<LiteralId>,
    stack_s: Vec<LiteralId>,
    literal_to_component_id: Vec<i32>,
    preorder_counter: i32,
    next_component_id: i32,
    best_component_literal: LiteralId,
    best_component_size: i32,

    // ── Scratch buffer ──────────────────────────────────────────────────────
    noneliminated: Vec<LiteralId>,

    // ── Per-puzzle search results ───────────────────────────────────────────
    limit: usize,
    scc_heuristic: bool,
    scc_inference: bool,
    num_guesses: usize,
    num_solutions: usize,
    result: State,
}

impl SolverDpllTriadScc {
    fn new() -> Self {
        // Allocate all setup data here to avoid borrow-checker conflicts
        // between `self` and `self.initial_state`.
        let mut clauses_to_literals: Vec<Vec<LiteralId>> = Vec::new();
        let mut literals_to_clauses: Vec<Vec<ClauseId>> = vec![Vec::new(); NUM_LITERALS];
        let mut literals_to_implications: Vec<Vec<LiteralId>> = vec![Vec::new(); NUM_LITERALS];
        let mut positive_cell_clauses: Vec<ClauseId> = Vec::new();
        let mut clause_free_literals: Vec<u16> = Vec::new();
        let mut implication_counts: Vec<u16> = vec![0u16; NUM_LITERALS];

        Self::build_constraints(
            &mut clauses_to_literals,
            &mut literals_to_clauses,
            &mut literals_to_implications,
            &mut positive_cell_clauses,
            &mut clause_free_literals,
            &mut implication_counts,
        );

        let initial_state = State {
            asserted: FastBitset::default(),
            clause_free_literals,
            implication_counts,
            num_asserted: 0,
        };

        let result = State::new(NUM_LITERALS);

        SolverDpllTriadScc {
            clauses_to_literals,
            literals_to_clauses,
            literals_to_implications,
            positive_cell_clauses,
            initial_state,
            preorder_index: vec![-1i32; NUM_LITERALS],
            stack_p: Vec::new(),
            stack_s: Vec::new(),
            literal_to_component_id: vec![-1i32; NUM_LITERALS],
            preorder_counter: 0,
            next_component_id: 0,
            best_component_literal: NO_LITERAL,
            best_component_size: -1,
            noneliminated: Vec::new(),
            limit: 1,
            scc_heuristic: true,
            scc_inference: true,
            num_guesses: 0,
            num_solutions: 0,
            result,
        }
    }

    /// Build all Sudoku constraints (cell/triad ExactlyN + band triad
    /// ExactlyN) and populate the clause/implication tables.
    fn build_constraints(
        clauses_to_lits: &mut Vec<Vec<LiteralId>>,
        lits_to_clauses: &mut Vec<Vec<ClauseId>>,
        impls: &mut Vec<Vec<LiteralId>>,
        pos_clauses: &mut Vec<ClauseId>,
        cfl: &mut Vec<u16>,
        counts: &mut Vec<u16>,
    ) {
        for box_idx in 0..9usize {
            // ExactlyN over all 9 values for each of the 15 used elements
            // (9 cells + 6 triads; element 15 is unused).
            for elem in 0..15usize {
                let lits: Vec<LiteralId> = (0..9).map(|v| lit(box_idx, elem, v)).collect();
                // cells: elem/4 < 3 and elem%4 < 3 → ExactlyOne
                // triads: else → ExactlyThree
                let n = if elem / 4 < 3 && elem % 4 < 3 { 1 } else { 3 };
                setup_add_exactly_n(
                    &lits,
                    n,
                    clauses_to_lits,
                    lits_to_clauses,
                    cfl,
                    pos_clauses,
                    impls,
                    counts,
                );
            }

            // ExactlyN constraints that define each horizontal/vertical triad
            // in terms of its 3 constituent cells.
            for val in 0..9usize {
                for i in 0..3usize {
                    // Horizontal triad: cells (i,0),(i,1),(i,2) + neg of triad
                    let h_triad: Vec<LiteralId> = (0..3)
                        .map(|j| lit(box_idx, i * 4 + j, val))
                        .chain(std::iter::once(lit_not(lit(box_idx, i * 4 + 3, val))))
                        .collect();
                    // Vertical triad: cells (0,i),(1,i),(2,i) + neg of triad
                    let v_triad: Vec<LiteralId> = (0..3)
                        .map(|j| lit(box_idx, i + j * 4, val))
                        .chain(std::iter::once(lit_not(lit(box_idx, i + 12, val))))
                        .collect();
                    setup_add_exactly_n(
                        &h_triad,
                        1,
                        clauses_to_lits,
                        lits_to_clauses,
                        cfl,
                        pos_clauses,
                        impls,
                        counts,
                    );
                    setup_add_exactly_n(
                        &v_triad,
                        1,
                        clauses_to_lits,
                        lits_to_clauses,
                        cfl,
                        pos_clauses,
                        impls,
                        counts,
                    );
                }
            }
        }

        // ExactlyOne constraints over band triads: within a band, each value
        // appears in exactly one of the 3 horizontal (or vertical) triads.
        for val in 0..9usize {
            for band in 0..3usize {
                for i in 0..3usize {
                    let h_within: Vec<LiteralId> =
                        (0..3).map(|j| lit(band * 3 + i, j * 4 + 3, val)).collect();
                    let h_across: Vec<LiteralId> =
                        (0..3).map(|j| lit(band * 3 + j, i * 4 + 3, val)).collect();
                    let v_within: Vec<LiteralId> =
                        (0..3).map(|j| lit(i * 3 + band, j + 12, val)).collect();
                    let v_across: Vec<LiteralId> =
                        (0..3).map(|j| lit(j * 3 + band, i + 12, val)).collect();
                    for lits in [h_within, h_across, v_within, v_across] {
                        setup_add_exactly_n(
                            &lits,
                            1,
                            clauses_to_lits,
                            lits_to_clauses,
                            cfl,
                            pos_clauses,
                            impls,
                            counts,
                        );
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // BCP helpers
    // -----------------------------------------------------------------------

    /// Push `from → to` onto the implication list, using the logical-size
    /// stack-pointer pattern from the C++ original.
    #[inline]
    fn add_implication(&mut self, from: LiteralId, to: LiteralId, state: &mut State) {
        let count = state.implication_counts[from as usize] as usize;
        let v = &mut self.literals_to_implications[from as usize];
        if v.len() == count {
            v.push(to);
        } else {
            v[count] = to;
        }
        state.implication_counts[from as usize] += 1;
    }

    /// When a clause's free-literal count drops to zero, add binary
    /// implications among all non-eliminated literals so that eliminating any
    /// one of them implies the rest.
    fn add_binary_implications_among_non_eliminated(
        &mut self,
        clause_id: ClauseId,
        state: &mut State,
    ) {
        let num_clause_lits = self.clauses_to_literals[clause_id as usize].len();
        let initial_free = self.initial_state.clause_free_literals[clause_id as usize] as usize;
        // `expect` = how many non-eliminated literals remain = min + 1.
        let expect = num_clause_lits - initial_free;

        if expect == 2 {
            // Common fast path: exactly 2 non-eliminated literals remain.
            let mut first = NO_LITERAL;
            for k in 0..num_clause_lits {
                let l = self.clauses_to_literals[clause_id as usize][k];
                if !state.asserted.get(lit_not(l)) {
                    if first == NO_LITERAL {
                        first = l;
                    } else {
                        let second = l;
                        self.add_implication(lit_not(first), second, state);
                        self.add_implication(lit_not(second), first, state);
                        return;
                    }
                }
            }
            // Unreachable in a consistent state.
        } else {
            // General path: collect all non-eliminated literals then add
            // pairwise implications.  Use take/replace to allow calling
            // `add_implication` (which needs &mut self) while iterating.
            self.noneliminated.clear();
            for k in 0..num_clause_lits {
                let l = self.clauses_to_literals[clause_id as usize][k];
                if !state.asserted.get(lit_not(l)) {
                    self.noneliminated.push(l);
                }
            }
            let ne = std::mem::take(&mut self.noneliminated);
            for i in 0..ne.len() - 1 {
                for j in i + 1..ne.len() {
                    self.add_implication(lit_not(ne[i]), ne[j], state);
                    self.add_implication(lit_not(ne[j]), ne[i], state);
                }
            }
            self.noneliminated = ne;
        }
    }

    /// Assert `literal` (unit propagation).  Returns false if a contradiction
    /// is reached.
    fn assert_lit(&mut self, literal: LiteralId, state: &mut State) -> bool {
        if state.asserted.get(literal) {
            return true; // already asserted
        }
        if state.asserted.get(lit_not(literal)) {
            return false; // contradiction
        }
        state.asserted.set(literal);
        state.num_asserted += 1;

        // Decrement free-literal counts for clauses containing the negation.
        // If a clause drops to zero free literals, discover new implications.
        // We read the clause list by index to satisfy the borrow checker
        // (the length does not change during this loop).
        let neg_clause_count = self.literals_to_clauses[lit_not(literal) as usize].len();
        for ci in 0..neg_clause_count {
            let clause_id = self.literals_to_clauses[lit_not(literal) as usize][ci];
            // Use wrapping_sub to match C++ unsigned wrap-around semantics:
            // if a clause is already at 0 (implications already added) a
            // subsequent decrement wraps to u16::MAX, skipping the == 0 check.
            state.clause_free_literals[clause_id as usize] =
                state.clause_free_literals[clause_id as usize].wrapping_sub(1);
            if state.clause_free_literals[clause_id as usize] == 0 {
                self.add_binary_implications_among_non_eliminated(clause_id, state);
            }
        }

        // Unit propagation: assert all currently-known implications.
        // The comment in the original notes that new implications are NOT
        // added to this literal's list during this loop, so reading the count
        // upfront is correct.
        let num_implications = state.implication_counts[literal as usize];
        for i in 0..num_implications as usize {
            let impl_lit = self.literals_to_implications[literal as usize][i];
            if !self.assert_lit(impl_lit, state) {
                return false;
            }
        }
        true
    }

    // -----------------------------------------------------------------------
    // Path-based SCC (Pearce's algorithm)
    // -----------------------------------------------------------------------

    /// DFS visitor used by the SCC algorithm.  Returns false on contradiction.
    fn scc_visit(&mut self, literal: LiteralId, state: &mut State) -> bool {
        // Inference: if any ancestor on stack_p implies both `literal` and
        // its negation, we can eliminate that ancestor.
        if self.scc_inference {
            let neg_preorder = self.preorder_index[lit_not(literal) as usize];
            let mut common_ancestor = NO_LITERAL;
            for &ancestor in &self.stack_p {
                if self.preorder_index[ancestor as usize] <= neg_preorder {
                    common_ancestor = ancestor;
                } else {
                    break;
                }
            }
            if common_ancestor != NO_LITERAL {
                if !self.assert_lit(lit_not(common_ancestor), state) {
                    return false;
                }
                if state.asserted.get(literal) {
                    return true; // already handled
                }
            }
        }

        let my_preorder = self.preorder_counter;
        self.preorder_index[literal as usize] = my_preorder;
        self.preorder_counter += 1;
        self.stack_p.push(literal);
        self.stack_s.push(literal);

        // Iterate implications.  We re-read the count each iteration because
        // scc_inference may add new implications during recursive visits.
        let mut i = 0;
        loop {
            let num_implications = state.implication_counts[literal as usize] as usize;
            if i >= num_implications {
                break;
            }
            let implication = self.literals_to_implications[literal as usize][i];
            i += 1;

            if state.asserted.get(implication) {
                // Skip subsumed implications.
                continue;
            } else if self.preorder_index[implication as usize] == -1 {
                // Not yet visited: recurse.
                if !self.scc_visit(implication, state) {
                    return false;
                }
                if self.scc_inference && state.asserted.pos_or_neg(literal) {
                    // The recursive visit determined this literal; stop.
                    break;
                }
            } else if self.literal_to_component_id[implication as usize] == -1 {
                // Back/cross edge to a node on stack_p: merge into same SCC.
                let imp_preorder = self.preorder_index[implication as usize];
                while self.preorder_index[*self.stack_p.last().unwrap() as usize] > imp_preorder {
                    self.stack_p.pop();
                }
            }
        }

        // Check if `literal` is still the top of stack_p (SCC root).
        if self.stack_p.last() == Some(&literal) {
            self.stack_p.pop();

            // Find where this SCC starts in stack_s.
            let pos_in_s = self.stack_s.iter().rposition(|&x| x == literal).unwrap();
            let component_size = self.stack_s.len() - pos_in_s;

            if !state.asserted.pos_or_neg(literal) {
                let negation_has_component =
                    self.literal_to_component_id[lit_not(literal) as usize] >= 0;
                let id = self.next_component_id;
                for k in pos_in_s..self.stack_s.len() {
                    self.literal_to_component_id[self.stack_s[k] as usize] = id;
                }
                // Prefer literals whose negation has no prior component, and
                // among those prefer the largest component.
                if !negation_has_component && component_size as i32 > self.best_component_size {
                    self.best_component_size = component_size as i32;
                    self.best_component_literal = literal;
                }
                self.next_component_id += 1;
            }
            self.stack_s.truncate(pos_in_s);
        }
        true
    }

    /// Run the full SCC algorithm over all unasserted positive literals.
    /// Returns false if a contradiction is discovered.
    fn find_strongly_connected_components(&mut self, state: &mut State) -> bool {
        self.preorder_counter = 0;
        for x in &mut self.preorder_index {
            *x = -1;
        }
        self.stack_p.clear();
        self.stack_s.clear();
        for x in &mut self.literal_to_component_id {
            *x = -1;
        }
        self.next_component_id = 0;
        self.best_component_literal = NO_LITERAL;
        self.best_component_size = -1;

        // Explore positive literals as roots; their negative counterparts
        // will be visited via the implication graph.
        let mut l: LiteralId = 0;
        while l < NUM_LITERALS as LiteralId {
            if self.preorder_index[l as usize] == -1
                && valid_literal(l)
                && !state.asserted.pos_or_neg(l)
                && !self.scc_visit(l, state)
            {
                return false;
            }
            l += 2;
        }
        true
    }

    // -----------------------------------------------------------------------
    // Branching heuristics
    // -----------------------------------------------------------------------

    /// Return the literal in the largest SCC found during the last
    /// `find_strongly_connected_components` call.
    fn choose_literal_by_component(&self) -> LiteralId {
        self.best_component_literal
    }

    /// Return a literal from the clause with the fewest free (undetermined)
    /// literals.  Used as fallback when the SCC heuristic is disabled.
    fn choose_literal_by_clause(&self, state: &State) -> LiteralId {
        let mut min_free = i32::MAX;
        let mut which_clause = 0usize;
        for &clause_id in &self.positive_cell_clauses {
            let num_free = state.clause_free_literals[clause_id as usize] as i32;
            if num_free < min_free {
                min_free = num_free;
                which_clause = clause_id as usize;
            }
        }
        for &l in &self.clauses_to_literals[which_clause] {
            if !state.asserted.get(lit_not(l)) {
                return l;
            }
        }
        panic!("choose_literal_by_clause: no free literal found");
    }

    // -----------------------------------------------------------------------
    // DPLL search
    // -----------------------------------------------------------------------

    /// Try asserting `literal`; if consistent, recurse; then try its
    /// negation.
    fn branch_on_literal(&mut self, literal: LiteralId, state: &mut State) {
        self.num_guesses += 1;

        // Branch 1: assert the literal on a cloned state.
        let mut state_copy = state.clone();
        if self.assert_lit(literal, &mut state_copy) {
            self.count_solutions(&mut state_copy);
            if self.num_solutions == self.limit {
                return;
            }
        }

        // Branch 2: assert the negation in the current state.
        if self.assert_lit(lit_not(literal), state) {
            self.count_solutions(state);
        }
    }

    /// Core DPLL loop.  Applies SCC-based inference until quiescent, then
    /// either records a solution or branches.
    fn count_solutions(&mut self, state: &mut State) {
        // Run SCC in a loop until either done, inconsistent, or quiescent.
        if self.scc_heuristic || self.scc_inference {
            while state.num_asserted < ALL_ASSERTED {
                let prev_asserted = state.num_asserted;
                if !self.find_strongly_connected_components(state) {
                    return; // contradiction
                }
                if prev_asserted == state.num_asserted {
                    break; // quiescent
                }
            }
        }

        if state.num_asserted == ALL_ASSERTED {
            self.num_solutions += 1;
            if self.num_solutions == 1 {
                self.result = state.clone();
            }
        } else {
            let branch_literal = if self.scc_heuristic {
                let l = self.choose_literal_by_component();
                if l == NO_LITERAL {
                    // Fallback: SCC found no unambiguous branch literal.
                    self.choose_literal_by_clause(state)
                } else {
                    l
                }
            } else {
                self.choose_literal_by_clause(state)
            };
            self.branch_on_literal(branch_literal, state);
        }
    }

    // -----------------------------------------------------------------------
    // Puzzle initialization
    // -----------------------------------------------------------------------

    /// Parse the input string and assert the given clues.
    fn initialize_puzzle(&mut self, input: &[u8], pencilmark: bool, state: &mut State) -> bool {
        // Validate vanilla input: reject strings with invalid characters.
        // '0' is an alternate empty-cell marker used by some puzzle formats.
        if !pencilmark {
            let len = input.len().min(81);
            if input[..len]
                .iter()
                .any(|&b| !matches!(b, b'0'..=b'9' | b'.'))
            {
                return false;
            }
        }

        // For vanilla format, pad short inputs with '.' so we always have 81 bytes.
        let vanilla_buf;
        let vanilla_slice: &[u8] = if !pencilmark {
            vanilla_buf = {
                let mut b = [b'.'; 81];
                let copy_len = input.len().min(81);
                b[..copy_len].copy_from_slice(&input[..copy_len]);
                b
            };
            &vanilla_buf
        } else {
            input
        };

        for i in 0..81usize {
            let box_idx = i / 27 * 3 + (i % 9) / 3;
            let elem = ((i / 9) % 3) * 4 + (i % 3);
            if pencilmark {
                for j in 0..9usize {
                    if input[i * 9 + j] == b'.'
                        && !self.assert_lit(lit_not(lit(box_idx, elem, j)), state)
                    {
                        return false;
                    }
                }
            } else {
                let ch = vanilla_slice[i];
                // Only treat bytes '1'–'9' as clues; anything else is empty.
                if (b'1'..=b'9').contains(&ch) {
                    let val = (ch - b'1') as usize;
                    if !self.assert_lit(lit(box_idx, elem, val), state) {
                        return false;
                    }
                }
            }
        }
        true
    }

    // -----------------------------------------------------------------------
    // Entry point
    // -----------------------------------------------------------------------

    fn solve_sudoku(
        &mut self,
        input: &[u8],
        limit: usize,
        config: u32,
    ) -> (usize, [u8; 81], usize) {
        self.limit = limit;
        self.scc_inference = (config & 1) != 0;
        self.scc_heuristic = (config & 2) != 0;
        // Pencilmark format: exactly 729 bytes (81 cells × 9 candidates).
        let pencilmark = input.len() >= 729;
        self.num_solutions = 0;
        self.num_guesses = 0;

        // Clone the initial (puzzle-independent) constraint state.
        self.result = self.initial_state.clone();
        let mut state = self.initial_state.clone();

        if !self.initialize_puzzle(input, pencilmark, &mut state) {
            return (0, [0u8; 81], 0);
        }
        self.count_solutions(&mut state);

        // Extract the solution string from the result state.
        let mut solution = [0u8; 81];
        #[allow(clippy::needless_range_loop)]
        for i in 0..81usize {
            let box_idx = i / 27 * 3 + (i % 9) / 3;
            let elem = ((i / 9) % 3) * 4 + (i % 3);
            for val in 0..9usize {
                if self.result.asserted.get(lit(box_idx, elem, val)) {
                    solution[i] = b'1' + val as u8;
                }
            }
        }
        (self.num_solutions, solution, self.num_guesses)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Solve a Sudoku puzzle using the DPLL + triad + SCC solver.
///
/// `input` must be exactly 81 bytes: digits `'1'`–`'9'` for givens, `'.'` for
/// blanks.  Returns `(num_solutions, solution, num_guesses)`.
///
/// `config` bits:
///   - bit 0: enable SCC inference (assert ancestors that imply contradictions)
///   - bit 1: enable SCC heuristic for variable selection
///
/// The default configuration for best performance is `config = 3`.
pub fn solve(input: &[u8], limit: usize, config: u32) -> (usize, [u8; 81], usize) {
    thread_local! {
        static SOLVER: RefCell<SolverDpllTriadScc> =
            RefCell::new(SolverDpllTriadScc::new());
    }
    SOLVER.with(|cell| {
        let mut solver = cell.borrow_mut();
        solver.solve_sudoku(input, limit, config)
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const PUZZLE: &[u8] =
        b".5..83.17...1..4..3.4..56.8....3...9.9.8245....6....7...9....5...729..861.36.72.4";
    const SOLUTION: &[u8] =
        b"652483917978162435314975628825736149791824563436519872269348751547291386183657294";

    #[test]
    fn test_scc_both_heuristics() {
        let (count, sol, _guesses) = solve(PUZZLE, 1, 3);
        assert_eq!(count, 1);
        assert_eq!(&sol, SOLUTION);
    }

    #[test]
    fn test_scc_inference_only() {
        let (count, sol, _guesses) = solve(PUZZLE, 1, 1);
        assert_eq!(count, 1);
        assert_eq!(&sol, SOLUTION);
    }

    #[test]
    fn test_scc_heuristic_only() {
        let (count, sol, _guesses) = solve(PUZZLE, 1, 2);
        assert_eq!(count, 1);
        assert_eq!(&sol, SOLUTION);
    }

    #[test]
    fn test_scc_no_heuristics() {
        let (count, sol, _guesses) = solve(PUZZLE, 1, 0);
        assert_eq!(count, 1);
        assert_eq!(&sol, SOLUTION);
    }

    #[test]
    fn test_scc_invalid_puzzle() {
        let bad81 =
            b"11...............................................................................";
        assert_eq!(bad81.len(), 81);
        let (count, _sol, _guesses) = solve(bad81, 1, 3);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_scc_agrees_with_basic() {
        // SCC solver must return the same solution as the basic solver.
        let (scc_count, scc_sol, _) = solve(PUZZLE, 1, 3);
        let (basic_count, basic_sol, _) = crate::solver_basic::solve(PUZZLE, 1, 0);
        assert_eq!(scc_count, basic_count);
        assert_eq!(scc_sol, basic_sol);
    }
}
