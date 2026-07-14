//! Reduction solver for arbitrary-size NxN cubes.
//!
//! Brute-force search and the genetic worker only reach small cubes. The
//! reduction method scales to any N in polynomial time (move count ~O(N^2)):
//!   1. solve the six centers (each becomes one solid color),
//!   2. pair the edge wings into solved composite edges,
//!   3. solve the reduced cube as a 3x3 (with parity fixes for even N).
//!
//! Correctness is guaranteed by *propose-and-verify*: every transformation is
//! simulated on a clone and only committed if it makes progress without
//! disturbing already-finalized work. The full solve is checked by replay
//! (`StickerCube::is_solved`).
//!
//! STATUS: the advertised 4×4–11×11 range passes the release-mode replay corpus and CI.
//! `cube_wasm` enables reduction and routes only that measured range through this pipeline.
//! Cooperative deadlines and commit-on-success cancellation are implemented, with full
//! legal-move replay corpora measured through research-only N=44. Isolated noncanonical
//! orbit transport also replays at N=66/N=132 with lazy orbit-local edge libraries. Larger-N
//! reliability and resource economics remain active research, so these results do not expand
//! the product ceiling.
//!
//! How each stage works:
//!   * Centres — `centers_det::solve_centers`: deterministic, exact centre-cell permutation
//!     tracking via base-6 id probe cubes; fungible colour placement; last-two-centres and
//!     deeper-orbit (inner-X, obliques) cases via meta-commutators built from orbit-isolated
//!     3-cycles paired *within each orbit*.
//!   * Edges — `edges_det::solve_edges`: the same permutation framework over wing stickers
//!     with a flip bit; library enriched with meta-commutators for last-edges coverage.
//!   * 3×3 finish — `finish::finish_3x3`: extract a 3×3 from the reduced cube, solve with
//!     the two-phase engine, replay as outer turns; an `is_solvable` guard means the search
//!     never hangs on an impossible (parity) state.
//!   * Parity — `finish::solve_reduction`: paired wings are normalized per physical orbit
//!     to one of two exact sticker-visible forms (`E_d` home or canonical `D_d` defect).
//!     A depth-parameterized, machine-verified orbit-local template maps `D_d` to `E_d`;
//!     bounded search failure remains a coverage error, not a parity certificate. Legacy
//!     center-stall/disturbance recovery remains only as a bounded fallback. The returned
//!     move list is `simplify`-ed and replay-verified at the WASM boundary.
//!
//! Known non-goals: solutions are long (one commutator per piece + re-reductions); a much
//! shorter solver would be a major rewrite (batch placements / explicit parity algs).

// Old greedy centre solver, superseded by `centers_det`; kept for its
// `orient_fixed_centers`/`cube_rotations` helpers.
#[allow(dead_code)]
mod centers;
// Deterministic centre solver (all sizes). Some helpers read as dead code in a
// non-test build.
#[allow(dead_code)]
mod centers_det;
// Old greedy edge-pairing, superseded by `edges_det`; kept behind the feature as a fallback.
mod edges;
// Deterministic edge-pairing, reusing the centre solver's permutation framework over wing
// stickers (with a flip bit). The real edge solver.
#[allow(dead_code)]
mod edges_det;
#[allow(dead_code)]
mod finish;

use cube_core::{Axis, CubeState, Face, Move, StickerCube};
use std::cell::RefCell;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use web_time::Instant;

/// Cooperative operational control for long reduction solves. Worker termination
/// remains the browser's hard-stop backstop; this control lets native callers and
/// internal loops stop before starting more expensive work.
#[derive(Clone, Debug)]
pub struct ReductionControl {
    deadline: Option<Instant>,
    cancelled: Arc<AtomicBool>,
}

impl ReductionControl {
    pub fn unlimited() -> Self {
        Self {
            deadline: None,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            deadline: Some(Instant::now() + timeout),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn should_continue(&self) -> bool {
        !self.cancelled.load(Ordering::Relaxed)
            && self
                .deadline
                .is_none_or(|deadline| Instant::now() < deadline)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReductionError {
    CancelledOrTimedOut,
    Unsolved,
}

thread_local! {
    static ACTIVE_CONTROL: RefCell<Option<ReductionControl>> = const { RefCell::new(None) };
}

pub(crate) fn reduction_checkpoint() -> bool {
    ACTIVE_CONTROL.with(|active| {
        active
            .borrow()
            .as_ref()
            .is_none_or(ReductionControl::should_continue)
    })
}

struct ControlGuard(Option<ReductionControl>);

impl Drop for ControlGuard {
    fn drop(&mut self) {
        ACTIVE_CONTROL.with(|active| {
            active.replace(self.0.take());
        });
    }
}

pub(crate) fn with_reduction_control<T>(control: &ReductionControl, run: impl FnOnce() -> T) -> T {
    let previous = ACTIVE_CONTROL.with(|active| active.replace(Some(control.clone())));
    let _guard = ControlGuard(previous);
    run()
}

// The deterministic centre solver is the real one; the old greedy `centers` module
// is kept only for `orient_fixed_centers`/`cube_rotations` that `centers_det` reuses.
pub use centers_det::solve_centers;
pub use edges::edges_paired;
// The deterministic edge solver (perm-tracking, like `centers_det`) is the real one;
// the greedy `edges::solve_edges` is kept behind `--features reduction` as a fallback.
pub use edges_det::solve_edges;
pub use finish::{finish_3x3, solve_reduction, solve_reduction_with_control};

/// The single inner layer `depth` layers in from `face` (depth 0 = the outer
/// face layer). Sign matches `Move::wide`, so `slice_from(f, n, 0, t) ==
/// Move::wide(f, n, 1, t)` restricted to that one layer.
pub(crate) fn slice_from(face: Face, n: usize, depth: usize, turns: i8) -> Move {
    match face {
        Face::Up => Move::new(Axis::Y, n - 1 - depth, n - 1 - depth, turns),
        Face::Down => Move::new(Axis::Y, depth, depth, -turns),
        Face::Right => Move::new(Axis::X, n - 1 - depth, n - 1 - depth, turns),
        Face::Left => Move::new(Axis::X, depth, depth, -turns),
        Face::Front => Move::new(Axis::Z, n - 1 - depth, n - 1 - depth, turns),
        Face::Back => Move::new(Axis::Z, depth, depth, -turns),
    }
}

/// Outer face quarter/half turn. Part of the reduction move toolkit; kept for the
/// edge-pairing/parity stages still under construction.
#[allow(dead_code)]
pub(crate) fn turn(face: Face, n: usize, turns: i8) -> Move {
    let size = cube_core::CubeSize::new(n).expect("size >= 2");
    Move::face(face, size, turns)
}

pub(crate) fn invert(moves: &[Move]) -> Vec<Move> {
    moves.iter().rev().map(|m| m.inverse()).collect()
}

/// `a b a' b'`.
pub(crate) fn commutator(a: &[Move], b: &[Move]) -> Vec<Move> {
    let mut out = Vec::with_capacity(a.len() * 2 + b.len() * 2);
    out.extend_from_slice(a);
    out.extend_from_slice(b);
    out.extend(invert(a));
    out.extend(invert(b));
    out
}

/// `setup core setup'`.
pub(crate) fn conjugate(setup: &[Move], core: &[Move]) -> Vec<Move> {
    let mut out = Vec::with_capacity(setup.len() * 2 + core.len());
    out.extend_from_slice(setup);
    out.extend_from_slice(core);
    out.extend(invert(setup));
    out
}

/// Apply a move list to a cube (moves are pre-validated by construction).
pub(crate) fn apply_all(cube: &mut StickerCube, moves: &[Move]) {
    for mv in moves {
        cube.apply_move(*mv).expect("reduction move is valid");
    }
}

/// True if `(row, col)` is a center cell on an `n`x`n` face (not on any border).
pub(crate) fn is_center_cell(row: usize, col: usize, n: usize) -> bool {
    row > 0 && row + 1 < n && col > 0 && col + 1 < n
}

/// Count of center cells already showing `face`'s solved color.
pub(crate) fn face_center_correct(cube: &StickerCube, face: Face) -> usize {
    let n = cube.size().get();
    let want = face.color();
    let mut count = 0;
    for row in 0..n {
        for col in 0..n {
            if is_center_cell(row, col, n) && cube.color_at(face, row, col) == Some(want) {
                count += 1;
            }
        }
    }
    count
}

/// True if every center cell of `face` shows its solved color.
pub(crate) fn face_center_solved(cube: &StickerCube, face: Face) -> bool {
    let n = cube.size().get();
    if n <= 2 {
        return true; // 2x2 has no center cells
    }
    let inner = n - 2;
    face_center_correct(cube, face) == inner * inner
}

/// True if all six centers are solid.
pub fn centers_solved(cube: &StickerCube) -> bool {
    Face::ALL.iter().all(|&f| face_center_solved(cube, f))
}
