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
//! STATUS: work in progress.
//!
//! DONE & verified: odd-cube fixed-center orientation (`orient_fixed_centers`) —
//! wide scrambles crossing the middle slice rotate the frame; this restores it,
//! and the centers commutators exclude the middle slice to preserve it.
//!
//! WIP: the greedy propose-and-verify centers repertoire does not always finish a
//! face (it returns partial progress rather than panicking). The next step is
//! deterministic commutator targeting instead of a greedy search.
//!
//! NOT STARTED: even-cube last-center parity, edge-wing pairing, the reduced-3x3
//! finish, and 3x3 OLL/PLL parity for even N. See the project plan for the staged
//! design. Intentionally **not** wired into `run_solver_lab_observed` yet.

mod centers;

use cube_core::{Axis, CubeState, Face, Move, StickerCube};

pub use centers::solve_centers;

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

/// Outer face quarter/half turn.
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
