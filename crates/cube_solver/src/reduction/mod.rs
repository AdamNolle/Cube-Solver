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
//! STATUS: **every size 4×4 through 10×10 solves end-to-end, reliably and fast**, and the
//! method generalises to any N. Asserted over many random wide scrambles in
//! `finish::tests::{full_solve_sizes, stress_reliability}` (the latter replays the returned
//! move list to solved). Per-solve is fast (≈0.17 s at 4×4 … ≈3.4 s at 7×7), plus a
//! one-time per-size library build (≈5–6 s at 9×9/10×10, cheap below). Still feature-gated
//! (`--features reduction`) and not yet wired into `cube_wasm`/the app — the 3×3 two-phase
//! (Kociemba) solver remains the shipped engine; wiring N>3 in is the next integration step.
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
//!   * Parity — `finish::solve_reduction`: even cubes carry OLL/PLL parity and the wings
//!     split into ⌊(n-2)/2⌋ orbits with independent parities. Handled by driving edges to
//!     all-home, a deterministic dedge swap for odd corners, and a *bitmask* over orbit-
//!     flipper subsets (slices on even cubes, wides on odd) that lands on the odd-orbit set
//!     in one shot; a centre-stall recovery and cumulative/non-cumulative disturbance walks
//!     are the fallbacks. The returned move list is `simplify`-ed.
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

// The deterministic centre solver is the real one; the old greedy `centers` module
// is kept only for `orient_fixed_centers`/`cube_rotations` that `centers_det` reuses.
pub use centers_det::solve_centers;
pub use edges::edges_paired;
// The deterministic edge solver (perm-tracking, like `centers_det`) is the real one;
// the greedy `edges::solve_edges` is kept behind `--features reduction` as a fallback.
pub use edges_det::solve_edges;
pub use finish::{finish_3x3, solve_reduction};

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
