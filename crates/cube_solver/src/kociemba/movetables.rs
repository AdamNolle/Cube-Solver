//! Stage 3 — move tables.
//!
//! Search must turn "apply move `m` to a state" into an array lookup, so we precompute,
//! for every coordinate value and every move, the resulting coordinate. Phase-1
//! coordinates (twist/flip/slice) get all 18 moves; the phase-2 permutation
//! coordinates are only meaningful once the cube is already in the subgroup, so they
//! get only the 10 subgroup moves.
//!
//! Moves are indexed `face*3 + (turn-1)`, face order `U,R,F,D,L,B`:
//! `U,U2,U' = 0,1,2`, `R,R2,R' = 3,4,5`, … `B,B2,B' = 15,16,17`.

use super::{coords, CubieCube, BASIC_MOVES};

pub const N_MOVES: usize = 18;

/// Indices (into the 18-move list) of the phase-2 generators ⟨U,D,R2,L2,F2,B2⟩.
pub const PHASE2_MOVES: [usize; 10] = [0, 1, 2, 9, 10, 11, 4, 13, 7, 16];

/// The 18 moves as cubie cubes (each face turned 1, 2, 3 quarters).
pub fn all_move_cubes() -> [CubieCube; N_MOVES] {
    let mut m = [CubieCube::SOLVED; N_MOVES];
    for face in 0..6 {
        let base = BASIC_MOVES[face];
        let mut c = base;
        m[face * 3] = c;
        c = c.multiply(&base);
        m[face * 3 + 1] = c;
        c = c.multiply(&base);
        m[face * 3 + 2] = c;
    }
    m
}

/// Precomputed transition tables for every coordinate.
pub struct MoveTables {
    pub twist: Vec<u16>,       // [2187 * 18]
    pub flip: Vec<u16>,        // [2048 * 18]
    pub slice: Vec<u16>,       // [495 * 18]
    pub corner_perm: Vec<u16>, // [40320 * 18]
    pub edge8_perm: Vec<u16>,  // [40320 * 10]  (phase-2 moves, indexed by PHASE2_MOVES order)
    pub slice_perm: Vec<u8>,   // [24 * 10]
}

impl MoveTables {
    pub fn build() -> MoveTables {
        let moves = all_move_cubes();

        let mut twist = vec![0u16; 2187 * N_MOVES];
        for t in 0..2187u16 {
            let mut c = CubieCube::SOLVED;
            coords::set_twist(&mut c, t);
            for (mi, mv) in moves.iter().enumerate() {
                twist[t as usize * N_MOVES + mi] = coords::twist(&c.multiply(mv));
            }
        }

        let mut flip = vec![0u16; 2048 * N_MOVES];
        for f in 0..2048u16 {
            let mut c = CubieCube::SOLVED;
            coords::set_flip(&mut c, f);
            for (mi, mv) in moves.iter().enumerate() {
                flip[f as usize * N_MOVES + mi] = coords::flip(&c.multiply(mv));
            }
        }

        let mut slice = vec![0u16; 495 * N_MOVES];
        for s in 0..495u16 {
            let mut c = CubieCube::SOLVED;
            coords::set_slice(&mut c, s);
            for (mi, mv) in moves.iter().enumerate() {
                slice[s as usize * N_MOVES + mi] = coords::slice(&c.multiply(mv));
            }
        }

        let mut corner_perm = vec![0u16; 40320 * N_MOVES];
        for p in 0..40320u16 {
            let mut c = CubieCube::SOLVED;
            coords::set_corner_perm(&mut c, p);
            for (mi, mv) in moves.iter().enumerate() {
                corner_perm[p as usize * N_MOVES + mi] = coords::corner_perm(&c.multiply(mv));
            }
        }

        let p2 = PHASE2_MOVES.len();
        let mut edge8_perm = vec![0u16; 40320 * p2];
        for e in 0..40320u16 {
            let mut c = CubieCube::SOLVED;
            coords::set_edge8_perm(&mut c, e);
            for (j, &mi) in PHASE2_MOVES.iter().enumerate() {
                edge8_perm[e as usize * p2 + j] = coords::edge8_perm(&c.multiply(&moves[mi]));
            }
        }

        let mut slice_perm = vec![0u8; 24 * p2];
        for s in 0..24u8 {
            let mut c = CubieCube::SOLVED;
            coords::set_slice_perm(&mut c, s);
            for (j, &mi) in PHASE2_MOVES.iter().enumerate() {
                slice_perm[s as usize * p2 + j] = coords::slice_perm(&c.multiply(&moves[mi]));
            }
        }

        MoveTables {
            twist,
            flip,
            slice,
            corner_perm,
            edge8_perm,
            slice_perm,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A tiny deterministic PRNG so the test needs no external crate.
    fn lcg(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state >> 33
    }

    #[test]
    fn phase1_tables_match_direct_cube() {
        let t = MoveTables::build();
        let mut rng = 0x1234_5678u64;
        for _ in 0..200 {
            // Walk a random 18-move sequence through both the tables and the cube.
            let mut cube = CubieCube::SOLVED;
            let (mut tw, mut fl, mut sl) = (0u16, 0u16, coords::slice(&cube));
            for _ in 0..30 {
                let m = (lcg(&mut rng) % N_MOVES as u64) as usize;
                cube = cube.multiply(&all_move_cubes()[m]);
                tw = t.twist[tw as usize * N_MOVES + m];
                fl = t.flip[fl as usize * N_MOVES + m];
                sl = t.slice[sl as usize * N_MOVES + m];
            }
            assert_eq!(tw, coords::twist(&cube), "twist table diverged");
            assert_eq!(fl, coords::flip(&cube), "flip table diverged");
            assert_eq!(sl, coords::slice(&cube), "slice table diverged");
        }
    }

    #[test]
    fn phase2_tables_match_direct_cube() {
        let t = MoveTables::build();
        let p2 = PHASE2_MOVES.len();
        let moves = all_move_cubes();
        let mut rng = 0x9e37_79b9u64;
        for _ in 0..200 {
            // Phase-2 moves only, so edge8/slice permutations stay well-defined.
            let mut cube = CubieCube::SOLVED;
            let (mut cp, mut e8, mut sp) = (0u16, 0u16, 0u8);
            for _ in 0..30 {
                let j = (lcg(&mut rng) % p2 as u64) as usize;
                let m = PHASE2_MOVES[j];
                cube = cube.multiply(&moves[m]);
                cp = t.corner_perm[cp as usize * N_MOVES + m];
                e8 = t.edge8_perm[e8 as usize * p2 + j];
                sp = t.slice_perm[sp as usize * p2 + j];
            }
            assert_eq!(cp, coords::corner_perm(&cube), "corner_perm table diverged");
            assert_eq!(e8, coords::edge8_perm(&cube), "edge8_perm table diverged");
            assert_eq!(sp, coords::slice_perm(&cube), "slice_perm table diverged");
        }
    }
}
