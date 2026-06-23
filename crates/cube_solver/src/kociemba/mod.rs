//! Two-phase (Kociemba-style) solver for the 3×3×3 cube.
//!
//! Built in tested stages. **Stage 1 (this commit): the cubie-level cube and the six
//! face moves.** The cube is represented at the cubie level — corner/edge permutation
//! and orientation — which is what the two-phase coordinates are computed from.
//!
//! Conventions (Herbert Kociemba's): corners are ordered
//! `URF, UFL, ULB, UBR, DFR, DLF, DBL, DRB` and edges
//! `UR, UF, UL, UB, DR, DF, DL, DB, FR, FL, BL, BR`. A move stores, for each slot, the
//! slot its piece comes *from* plus the orientation change, so cubes compose by
//! `new[i] = a[b[i]] (+ orientation)`.

pub mod coords;
pub mod movetables;
pub mod search;

/// A cube at the cubie level: corner/edge permutation + orientation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CubieCube {
    /// Corner permutation: `cp[i]` is the corner slot whose piece now sits at slot `i`.
    pub cp: [u8; 8],
    /// Corner orientation (twist) in `{0,1,2}` per slot.
    pub co: [u8; 8],
    /// Edge permutation: `ep[i]` is the edge slot whose piece now sits at slot `i`.
    pub ep: [u8; 12],
    /// Edge orientation (flip) in `{0,1}` per slot.
    pub eo: [u8; 12],
}

impl CubieCube {
    /// The solved cube (identity).
    pub const SOLVED: CubieCube = CubieCube {
        cp: [0, 1, 2, 3, 4, 5, 6, 7],
        co: [0; 8],
        ep: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        eo: [0; 12],
    };

    pub fn is_solved(&self) -> bool {
        *self == CubieCube::SOLVED
    }

    /// Compose: the cube that results from doing `self`, then `m` (`self * m`).
    pub fn multiply(&self, m: &CubieCube) -> CubieCube {
        let mut r = CubieCube::SOLVED;
        for i in 0..8 {
            let from = m.cp[i] as usize;
            r.cp[i] = self.cp[from];
            r.co[i] = (self.co[from] + m.co[i]) % 3;
        }
        for i in 0..12 {
            let from = m.ep[i] as usize;
            r.ep[i] = self.ep[from];
            r.eo[i] = (self.eo[from] + m.eo[i]) % 2;
        }
        r
    }
}

// ---- The six basic face moves (clockwise quarter turns) ----------------------
// Corner slots: URF=0 UFL=1 ULB=2 UBR=3 DFR=4 DLF=5 DBL=6 DRB=7
// Edge slots:   UR=0 UF=1 UL=2 UB=3 DR=4 DF=5 DL=6 DB=7 FR=8 FL=9 BL=10 BR=11

pub const MOVE_U: CubieCube = CubieCube {
    cp: [3, 0, 1, 2, 4, 5, 6, 7],
    co: [0; 8],
    ep: [3, 0, 1, 2, 4, 5, 6, 7, 8, 9, 10, 11],
    eo: [0; 12],
};
pub const MOVE_R: CubieCube = CubieCube {
    cp: [4, 1, 2, 0, 7, 5, 6, 3],
    co: [2, 0, 0, 1, 1, 0, 0, 2],
    ep: [8, 1, 2, 3, 11, 5, 6, 7, 4, 9, 10, 0],
    eo: [0; 12],
};
pub const MOVE_F: CubieCube = CubieCube {
    cp: [1, 5, 2, 3, 0, 4, 6, 7],
    co: [1, 2, 0, 0, 2, 1, 0, 0],
    ep: [0, 9, 2, 3, 4, 8, 6, 7, 1, 5, 10, 11],
    eo: [0, 1, 0, 0, 0, 1, 0, 0, 1, 1, 0, 0],
};
pub const MOVE_D: CubieCube = CubieCube {
    cp: [0, 1, 2, 3, 5, 6, 7, 4],
    co: [0; 8],
    ep: [0, 1, 2, 3, 5, 6, 7, 4, 8, 9, 10, 11],
    eo: [0; 12],
};
pub const MOVE_L: CubieCube = CubieCube {
    cp: [0, 2, 6, 3, 4, 1, 5, 7],
    co: [0, 1, 2, 0, 0, 2, 1, 0],
    ep: [0, 1, 10, 3, 4, 5, 9, 7, 8, 2, 6, 11],
    eo: [0; 12],
};
pub const MOVE_B: CubieCube = CubieCube {
    cp: [0, 1, 3, 7, 4, 5, 2, 6],
    co: [0, 0, 1, 2, 0, 0, 2, 1],
    ep: [0, 1, 2, 11, 4, 5, 6, 10, 8, 9, 3, 7],
    eo: [0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 1],
};

/// The six face generators, in order U, R, F, D, L, B.
pub const BASIC_MOVES: [CubieCube; 6] = [MOVE_U, MOVE_R, MOVE_F, MOVE_D, MOVE_L, MOVE_B];

/// A face turn: which face (0..6 = U,R,F,D,L,B) and how many quarter turns (1,2,3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FaceTurn {
    pub face: u8,
    pub quarter: u8,
}

impl CubieCube {
    /// Apply a face turn (1, 2, or 3 quarter turns of one face).
    pub fn apply_turn(&self, t: FaceTurn) -> CubieCube {
        let m = &BASIC_MOVES[t.face as usize];
        let mut c = *self;
        for _ in 0..t.quarter {
            c = c.multiply(m);
        }
        c
    }

    /// Apply a sequence of face turns.
    pub fn apply_sequence(&self, seq: &[FaceTurn]) -> CubieCube {
        let mut c = *self;
        for &t in seq {
            c = c.apply_turn(t);
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(face: u8, q: u8) -> FaceTurn {
        FaceTurn { face, quarter: q }
    }

    #[test]
    fn each_move_has_order_four() {
        // A quarter turn applied four times returns to solved — catches gross errors
        // in any of the six permutation/orientation tables.
        for face in 0..6u8 {
            let mut c = CubieCube::SOLVED;
            for _ in 0..4 {
                c = c.apply_turn(turn(face, 1));
            }
            assert!(c.is_solved(), "move {face} did not have order 4");
        }
    }

    #[test]
    fn a_move_and_its_inverse_cancel() {
        for face in 0..6u8 {
            let c = CubieCube::SOLVED
                .apply_turn(turn(face, 1))
                .apply_turn(turn(face, 3));
            assert!(
                c.is_solved(),
                "move {face} then its inverse was not identity"
            );
        }
    }

    #[test]
    fn sexy_move_has_order_six() {
        // (R U R' U')^6 = solved — a classic check that mixes corners + edges + twist.
        let seq = [turn(1, 1), turn(0, 1), turn(1, 3), turn(0, 3)];
        let mut c = CubieCube::SOLVED;
        for _ in 0..6 {
            c = c.apply_sequence(&seq);
        }
        assert!(c.is_solved(), "(R U R' U')^6 was not solved");
        // ...and it is NOT solved before the sixth repetition.
        let mut c = CubieCube::SOLVED;
        for _ in 0..5 {
            c = c.apply_sequence(&seq);
        }
        assert!(!c.is_solved(), "(R U R' U')^5 unexpectedly solved");
    }

    #[test]
    fn r_then_u_has_order_105() {
        // The well-known order of the R·U sequence is 105 — a strong correctness check.
        let seq = [turn(1, 1), turn(0, 1)];
        let mut c = CubieCube::SOLVED;
        for i in 1..=105 {
            c = c.apply_sequence(&seq);
            if i < 105 {
                assert!(!c.is_solved(), "(R U)^{i} solved too early");
            }
        }
        assert!(c.is_solved(), "(R U)^105 was not solved");
    }

    #[test]
    fn orientation_sums_are_valid() {
        // Corner twists sum to 0 mod 3 and edge flips to 0 mod 2 for any reachable state.
        let seq = [
            turn(1, 1),
            turn(2, 1),
            turn(0, 3),
            turn(4, 2),
            turn(5, 1),
            turn(3, 3),
        ];
        let c = CubieCube::SOLVED.apply_sequence(&seq);
        assert_eq!(c.co.iter().map(|&x| x as u32).sum::<u32>() % 3, 0);
        assert_eq!(c.eo.iter().map(|&x| x as u32).sum::<u32>() % 2, 0);
    }
}
