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
//! STATUS: work in progress — feature-gated (`--features reduction`) and
//! deliberately **not** wired into `cube_wasm` or the app, so the shipped solver is
//! never flaky. The 3×3 two-phase (Kociemba) solver is the lab's real engine.
//!
//! DONE & verified:
//!   * Centres — `centers_det::solve_centers`: deterministic, exact permutation
//!     tracking via probe cubes + meta-commutators for the last two centres.
//!     **4×4 and 5×5 solved** (30/30 and 6/6 random wide scrambles, ~1 s build,
//!     ms solves). 6×6+ blocked on even-cube oblique-centre coverage.
//!   * 3×3 finish — `finish::finish_3x3`: extract a 3×3 from the reduced cube, solve
//!     with the real two-phase engine, replay as outer turns. Detects unsolvable
//!     (parity) states so the search never hangs.
//!   * Parity — `finish::solve_reduction` toggles wing-permutation parity with an
//!     inner slice and re-reduces; this resolves OLL/PLL parity.
//!   * **First complete end-to-end 4×4 solves work** (centres → edges → finish +
//!     parity → `is_solved()`).
//!
//! WIP: edge-pairing (`edges::solve_edges`) is a support-filtered greedy — fast but
//! only reliable on some scrambles (it stalls like the old centres greedy did).
//! Making it deterministic (perm-tracking, like `centers_det`) is the next step to a
//! fully reliable 4×4. Then: even-cube oblique centres for 6×6+, and generalising
//! edges/parity to all N.

// Old greedy centre solver, superseded by `centers_det`; kept for its
// `orient_fixed_centers`/`cube_rotations` helpers.
#[allow(dead_code)]
mod centers;
// Deterministic centre solver (4×4/5×5 solved). Some helpers read as dead code in a
// non-test build.
#[allow(dead_code)]
mod centers_det;
mod edges;
// Deterministic edge-pairing (WIP), reusing the centre solver's permutation
// framework over wing stickers.
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
