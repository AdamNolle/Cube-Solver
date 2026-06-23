//! Stage 5 — integration with cube_core's `StickerCube`.
//!
//! Reads a real 3×3 `StickerCube` into a [`CubieCube`] (the facelet→cubie tables,
//! mapped to cube_core's verified geometry), solves it with the two-phase [`Solver`],
//! and maps the solution back to cube_core `Move`s. The decisive test scrambles a
//! real cube, solves it, replays the solution, and asserts it is solved.

use super::search::Solver;
use super::{CubieCube, FaceTurn};
use cube_core::{Color, CubeSize, Face, Move, StickerCube};

/// Read facelet number `n` (1..=9, reading order) on `face`.
fn facelet(s: &StickerCube, face: Face, n: usize) -> Color {
    s.color_at(face, (n - 1) / 3, (n - 1) % 3).unwrap()
}

// Facelet positions of each corner/edge slot, in Kociemba's canonical order (the
// U/D facelet first for corners; the reference facelet first for edges).
const CORNER_FACELET: [[(Face, usize); 3]; 8] = [
    [(Face::Up, 9), (Face::Right, 1), (Face::Front, 3)], // URF
    [(Face::Up, 7), (Face::Front, 1), (Face::Left, 3)],  // UFL
    [(Face::Up, 1), (Face::Left, 1), (Face::Back, 3)],   // ULB
    [(Face::Up, 3), (Face::Back, 1), (Face::Right, 3)],  // UBR
    [(Face::Down, 3), (Face::Front, 9), (Face::Right, 7)], // DFR
    [(Face::Down, 1), (Face::Left, 9), (Face::Front, 7)], // DLF
    [(Face::Down, 7), (Face::Back, 9), (Face::Left, 7)], // DBL
    [(Face::Down, 9), (Face::Right, 9), (Face::Back, 7)], // DRB
];
const CORNER_COLOR: [[Color; 3]; 8] = [
    [Color::White, Color::Red, Color::Green],     // URF
    [Color::White, Color::Green, Color::Orange],  // UFL
    [Color::White, Color::Orange, Color::Blue],   // ULB
    [Color::White, Color::Blue, Color::Red],      // UBR
    [Color::Yellow, Color::Green, Color::Red],    // DFR
    [Color::Yellow, Color::Orange, Color::Green], // DLF
    [Color::Yellow, Color::Blue, Color::Orange],  // DBL
    [Color::Yellow, Color::Red, Color::Blue],     // DRB
];
const EDGE_FACELET: [[(Face, usize); 2]; 12] = [
    [(Face::Up, 6), (Face::Right, 2)],    // UR
    [(Face::Up, 8), (Face::Front, 2)],    // UF
    [(Face::Up, 4), (Face::Left, 2)],     // UL
    [(Face::Up, 2), (Face::Back, 2)],     // UB
    [(Face::Down, 6), (Face::Right, 8)],  // DR
    [(Face::Down, 2), (Face::Front, 8)],  // DF
    [(Face::Down, 4), (Face::Left, 8)],   // DL
    [(Face::Down, 8), (Face::Back, 8)],   // DB
    [(Face::Front, 6), (Face::Right, 4)], // FR
    [(Face::Front, 4), (Face::Left, 6)],  // FL
    [(Face::Back, 6), (Face::Left, 4)],   // BL
    [(Face::Back, 4), (Face::Right, 6)],  // BR
];
const EDGE_COLOR: [[Color; 2]; 12] = [
    [Color::White, Color::Red],     // UR
    [Color::White, Color::Green],   // UF
    [Color::White, Color::Orange],  // UL
    [Color::White, Color::Blue],    // UB
    [Color::Yellow, Color::Red],    // DR
    [Color::Yellow, Color::Green],  // DF
    [Color::Yellow, Color::Orange], // DL
    [Color::Yellow, Color::Blue],   // DB
    [Color::Green, Color::Red],     // FR
    [Color::Green, Color::Orange],  // FL
    [Color::Blue, Color::Orange],   // BL
    [Color::Blue, Color::Red],      // BR
];

fn is_ud(c: Color) -> bool {
    c == Color::White || c == Color::Yellow
}

/// Convert a solved-or-scrambled 3×3 `StickerCube` into a [`CubieCube`].
pub fn sticker_to_cubie(s: &StickerCube) -> CubieCube {
    let mut cube = CubieCube::SOLVED;
    for (i, slot) in CORNER_FACELET.iter().enumerate() {
        let cols: [Color; 3] = std::array::from_fn(|k| facelet(s, slot[k].0, slot[k].1));
        let ori = (0..3)
            .find(|&o| is_ud(cols[o]))
            .expect("a corner facelet must be U/D");
        let (c0, c1) = (cols[ori], cols[(ori + 1) % 3]);
        let j = (0..8)
            .find(|&j| CORNER_COLOR[j][0] == c0 && CORNER_COLOR[j][1] == c1)
            .expect("unknown corner colours");
        cube.cp[i] = j as u8;
        cube.co[i] = ori as u8;
    }
    for (i, slot) in EDGE_FACELET.iter().enumerate() {
        let c0 = facelet(s, slot[0].0, slot[0].1);
        let c1 = facelet(s, slot[1].0, slot[1].1);
        let j = (0..12)
            .find(|&j| {
                let e = EDGE_COLOR[j];
                (e[0] == c0 && e[1] == c1) || (e[0] == c1 && e[1] == c0)
            })
            .expect("unknown edge colours");
        cube.ep[i] = j as u8;
        cube.eo[i] = if c0 == EDGE_COLOR[j][0] { 0 } else { 1 };
    }
    cube
}

/// cube_core face for each cubie face id (U,R,F,D,L,B order).
const FACE_TO_CORE: [Face; 6] = [
    Face::Up,
    Face::Right,
    Face::Front,
    Face::Down,
    Face::Left,
    Face::Back,
];

/// Map a cubie face-turn to a cube_core `Move`. cube_core's `Move::face(_, +1)` turns
/// a face the *opposite* way from our cubie generator (verified by the homomorphism
/// test below), so our `quarter` quarter-turns become `4 - quarter` cube_core turns
/// (1→3, 2→2, 3→1). Verified end-to-end by replaying solutions on real cubes.
fn turn_to_move(t: FaceTurn, size: CubeSize) -> Move {
    Move::face(FACE_TO_CORE[t.face as usize], size, 4 - t.quarter as i8)
}

/// Solve a 3×3 `StickerCube`, returning cube_core moves (or `None` if not a 3×3 /
/// no solution).
pub fn solve_sticker(s: &StickerCube, solver: &Solver) -> Option<Vec<Move>> {
    let size = CubeSize::new(3).ok()?;
    let cube = sticker_to_cubie(s);
    let turns = solver.solve(&cube)?;
    Some(turns.into_iter().map(|t| turn_to_move(t, size)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cube_core::CubeState;

    fn sz() -> CubeSize {
        CubeSize::new(3).unwrap()
    }

    fn lcg(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state >> 33
    }

    #[test]
    fn solved_sticker_maps_to_solved_cubie() {
        let c = sticker_to_cubie(&StickerCube::solved(sz()));
        assert!(
            c.is_solved(),
            "solved sticker cube must map to the identity cubie cube"
        );
    }

    #[test]
    fn each_basic_move_is_a_valid_cubie_move() {
        // Applying any single face quarter-turn and converting must land on a state
        // one move from solved (proves the facelet tables + geometry line up).
        let solver_moves = super::super::movetables::all_move_cubes();
        for (fi, &face) in FACE_TO_CORE.iter().enumerate() {
            let mut s = StickerCube::solved(sz());
            s.apply_move(Move::face(face, sz(), 1)).unwrap();
            let c = sticker_to_cubie(&s);
            // It must equal exactly one quarter/3-quarter of that face's cubie move.
            // cube_core's +1 quarter is our inverse (3-quarter) move, uniformly.
            let q3 = solver_moves[fi * 3 + 2];
            assert_eq!(
                c, q3,
                "face {face:?}: cube_core +1 quarter must equal our inverse move"
            );
        }
    }

    #[test]
    fn solves_real_scrambled_sticker_cubes() {
        let solver = Solver::new();
        let mut rng = 0xBEEF_F00Du64;
        for _ in 0..30 {
            let mut s = StickerCube::solved(sz());
            for _ in 0..25 {
                let face = FACE_TO_CORE[(lcg(&mut rng) % 6) as usize];
                let turns = (lcg(&mut rng) % 3) as i8 + 1;
                s.apply_move(Move::face(face, sz(), turns)).unwrap();
            }
            let sol = solve_sticker(&s, &solver).expect("no solution for a real scramble");
            let mut check = s.clone();
            for m in &sol {
                check.apply_move(*m).unwrap();
            }
            assert!(
                check.is_solved(),
                "solution did not solve the real cube ({} moves)",
                sol.len()
            );
        }
    }
}
