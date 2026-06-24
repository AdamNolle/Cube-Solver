//! Stage 3 of reduction: once centres are solid and edges paired, the NxN cube
//! behaves exactly like a 3×3 under outer face turns (each face's centre block
//! rotates within itself and stays solid; a paired edge's wings move together).
//!
//! So we extract a 3×3 from the reduced cube — corner stickers, one wing per edge,
//! and one centre per face — solve it with the real two-phase ([`kociemba`]) solver,
//! and replay that 3×3 solution as outer turns on the big cube.
//!
//! Even cubes can present a 3×3 state a real 3×3 never can (a single flipped edge,
//! or two swapped edges) — OLL/PLL parity. That is detected and handled separately;
//! this module is the parity-free finish.

use crate::kociemba::cube3::solve_sticker;
use crate::kociemba::search::Solver;
use cube_core::{Color, CubeSize, CubeState, Face, Move, StickerCube};

const COLOR_NAMES: [&str; 6] = ["White", "Yellow", "Green", "Blue", "Orange", "Red"];

fn color_name(c: Color) -> &'static str {
    match c {
        Color::White => COLOR_NAMES[0],
        Color::Yellow => COLOR_NAMES[1],
        Color::Green => COLOR_NAMES[2],
        Color::Blue => COLOR_NAMES[3],
        Color::Orange => COLOR_NAMES[4],
        Color::Red => COLOR_NAMES[5],
    }
}

fn face_ord(f: Face) -> usize {
    Face::ALL.iter().position(|&x| x == f).unwrap()
}

/// Build the 3×3 sticker representation of a reduced NxN cube: 3×3 cell `(r,c)`
/// reads the big cube at `(map(r), map(c))` where `0→0`, `1→1` (any inner cell —
/// centres are solid, edges paired), `2→n-1`.
fn extract_3x3(cube: &StickerCube) -> StickerCube {
    let n = cube.size().get();
    let map = |i: usize| match i {
        0 => 0,
        1 => 1,
        _ => n - 1,
    };
    let mut names: Vec<&str> = vec![COLOR_NAMES[0]; 6 * 9];
    for f in Face::ALL {
        for r in 0..3 {
            for c in 0..3 {
                let col = cube.color_at(f, map(r), map(c)).unwrap();
                names[face_ord(f) * 9 + r * 3 + c] = color_name(col);
            }
        }
    }
    let snap: cube_core::CubeSnapshot =
        serde_json::from_value(serde_json::json!({ "size": 3, "stickers": names }))
            .expect("3x3 snapshot");
    StickerCube::from_snapshot(snap)
}

/// Recover `(face, quarters)` from a 3×3 outer-turn move so it can be rebuilt at
/// another size. `solve_sticker` emits moves as `Move::face(face, size3, q)`.
fn recover(m: &Move, size3: CubeSize) -> Option<(Face, i8)> {
    for f in Face::ALL {
        for q in [1i8, 2, 3] {
            if Move::face(f, size3, q) == *m {
                return Some((f, q));
            }
        }
    }
    None
}

/// Solve a reduced NxN cube's 3×3 stage. Returns the outer-turn moves (applied to
/// `cube`), or `None` if the extracted 3×3 is unsolvable (parity — handled elsewhere).
pub fn finish_3x3(cube: &mut StickerCube, solver: &Solver) -> Option<Vec<Move>> {
    let size = cube.size();
    let size3 = CubeSize::new(3).unwrap();
    let cube3 = extract_3x3(cube);
    let moves3 = solve_sticker(&cube3, solver)?;
    let mut out = Vec::with_capacity(moves3.len());
    for m in &moves3 {
        let (f, q) = recover(m, size3)?;
        let big = Move::face(f, size, q);
        cube.apply_move(big).ok()?;
        out.push(big);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reduction::{centers_solved, edges_paired, solve_edges};

    fn lcg(s: &mut u64) -> u64 {
        *s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *s >> 33
    }

    fn scramble(n: usize, seed: u64, depth: usize) -> StickerCube {
        let size = CubeSize::new(n).unwrap();
        let mut cube = StickerCube::solved(size);
        let mut rng = seed;
        for _ in 0..depth {
            let f = Face::ALL[(lcg(&mut rng) % 6) as usize];
            let width = 1 + (lcg(&mut rng) % (n as u64 - 1)) as usize;
            let turns = [1i8, -1, 2][(lcg(&mut rng) % 3) as usize];
            cube.apply_move(Move::wide(f, size, width, turns)).unwrap();
        }
        cube
    }

    /// End-to-end 4×4: centres → edges → 3×3 finish, then check fully solved.
    /// (Seeds with OLL/PLL parity will fail until the parity stage exists.)
    #[test]
    #[ignore = "full 4x4 pipeline probe"]
    fn full_solve_n4() {
        let solver = Solver::new();
        let mut solved = 0;
        let mut paritied = 0;
        for seed in 0..8u64 {
            let mut cube = scramble(4, 0x100 + seed, 40);
            let _ = super::super::centers_det::solve_centers(&mut cube);
            if !centers_solved(&cube) {
                println!("seed {seed}: centres failed");
                continue;
            }
            let _ = solve_edges(&mut cube);
            if !edges_paired(&cube) {
                println!("seed {seed}: edges not paired");
                continue;
            }
            match finish_3x3(&mut cube, &solver) {
                Some(_) if cube.is_solved() => {
                    solved += 1;
                    println!("seed {seed}: SOLVED");
                }
                Some(_) => println!("seed {seed}: finish ran but not solved"),
                None => {
                    paritied += 1;
                    println!("seed {seed}: parity (3x3 unsolvable)");
                }
            }
        }
        println!("n=4 full solve: {solved}/8 solved, {paritied} hit parity");
    }
}
