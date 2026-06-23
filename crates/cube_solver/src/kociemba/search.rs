//! Stage 4 — pruning tables + IDA* two-phase search.
//!
//! Phase 1 drives the cube into the ⟨U,D,R2,L2,F2,B2⟩ subgroup (twist=0, flip=0, slice
//! edges in the slice). Phase 2 solves within the subgroup. Each phase is an IDA*
//! search guided by an admissible pruning table (a BFS distance over a pair of
//! coordinates, taken as `max` of two projections).

use super::coords;
use super::movetables::{MoveTables, N_MOVES, PHASE2_MOVES};
use super::{CubieCube, FaceTurn};

const SLICE_GOAL: usize = 494; // slice edges occupying the four slice slots

/// Opposite face of `f` (U↔D, R↔L, F↔B) given face order U,R,F,D,L,B.
fn opposite(f: u8) -> u8 {
    (f + 3) % 6
}

/// Move ordering pruning: never turn the same face twice in a row, and for the two
/// faces of an axis (which commute) only allow one order.
fn allowed(prev_face: Option<u8>, cur_face: u8) -> bool {
    match prev_face {
        None => true,
        Some(pf) => pf != cur_face && !(opposite(pf) == cur_face && pf > cur_face),
    }
}

/// A built solver: move tables plus the four pruning tables. Build once, reuse.
pub struct Solver {
    mt: MoveTables,
    p1_twist_slice: Vec<u8>, // [2187 * 495]
    p1_flip_slice: Vec<u8>,  // [2048 * 495]
    p2_cperm_sperm: Vec<u8>, // [40320 * 24]
    p2_eperm_sperm: Vec<u8>, // [40320 * 24]
}

impl Solver {
    pub fn new() -> Solver {
        let mt = MoveTables::build();
        let p1_twist_slice = build_phase1(&mt, &mt.twist, 2187);
        let p1_flip_slice = build_phase1(&mt, &mt.flip, 2048);
        let p2_cperm_sperm = build_phase2(&mt, &mt.corner_perm, true, 40320);
        let p2_eperm_sperm = build_phase2(&mt, &mt.edge8_perm, false, 40320);
        Solver {
            mt,
            p1_twist_slice,
            p1_flip_slice,
            p2_cperm_sperm,
            p2_eperm_sperm,
        }
    }

    fn h1(&self, twist: usize, flip: usize, slice: usize) -> u8 {
        self.p1_twist_slice[twist * 495 + slice].max(self.p1_flip_slice[flip * 495 + slice])
    }

    fn h2(&self, cperm: usize, eperm: usize, sperm: usize) -> u8 {
        self.p2_cperm_sperm[cperm * 24 + sperm].max(self.p2_eperm_sperm[eperm * 24 + sperm])
    }

    /// Solve a 3×3 cube; returns the move sequence (face turns).
    pub fn solve(&self, cube: &CubieCube) -> Option<Vec<FaceTurn>> {
        // Phase 1: shortest sequence into the subgroup.
        let p1 = self.phase1(cube)?;
        let mut mid = *cube;
        let all = super::movetables::all_move_cubes();
        for &m in &p1 {
            mid = mid.multiply(&all[m]);
        }
        // Phase 2: solve within the subgroup.
        let p2 = self.phase2(&mid)?;
        let mut moves = p1;
        moves.extend(p2);
        Some(merge_to_turns(&moves))
    }

    fn phase1(&self, cube: &CubieCube) -> Option<Vec<usize>> {
        let tw = coords::twist(cube) as usize;
        let fl = coords::flip(cube) as usize;
        let sl = coords::slice(cube) as usize;
        for bound in self.h1(tw, fl, sl)..=20 {
            let mut sol = Vec::new();
            if self.p1_dfs(tw, fl, sl, bound, None, &mut sol) {
                return Some(sol);
            }
        }
        None
    }

    fn p1_dfs(
        &self,
        tw: usize,
        fl: usize,
        sl: usize,
        depth: u8,
        last: Option<u8>,
        sol: &mut Vec<usize>,
    ) -> bool {
        if depth == 0 {
            return tw == 0 && fl == 0 && sl == SLICE_GOAL;
        }
        if self.h1(tw, fl, sl) > depth {
            return false;
        }
        for m in 0..N_MOVES {
            let face = (m / 3) as u8;
            if !allowed(last, face) {
                continue;
            }
            let ntw = self.mt.twist[tw * N_MOVES + m] as usize;
            let nfl = self.mt.flip[fl * N_MOVES + m] as usize;
            let nsl = self.mt.slice[sl * N_MOVES + m] as usize;
            sol.push(m);
            if self.p1_dfs(ntw, nfl, nsl, depth - 1, Some(face), sol) {
                return true;
            }
            sol.pop();
        }
        false
    }

    fn phase2(&self, cube: &CubieCube) -> Option<Vec<usize>> {
        let cp = coords::corner_perm(cube) as usize;
        let ep = coords::edge8_perm(cube) as usize;
        let sp = coords::slice_perm(cube) as usize;
        for bound in self.h2(cp, ep, sp)..=20 {
            let mut sol = Vec::new();
            if self.p2_dfs(cp, ep, sp, bound, None, &mut sol) {
                return Some(sol);
            }
        }
        None
    }

    fn p2_dfs(
        &self,
        cp: usize,
        ep: usize,
        sp: usize,
        depth: u8,
        last: Option<u8>,
        sol: &mut Vec<usize>,
    ) -> bool {
        if depth == 0 {
            return cp == 0 && ep == 0 && sp == 0;
        }
        if self.h2(cp, ep, sp) > depth {
            return false;
        }
        for (j, &m) in PHASE2_MOVES.iter().enumerate() {
            let face = (m / 3) as u8;
            if !allowed(last, face) {
                continue;
            }
            let ncp = self.mt.corner_perm[cp * N_MOVES + m] as usize;
            let nep = self.mt.edge8_perm[ep * PHASE2_MOVES.len() + j] as usize;
            let nsp = self.mt.slice_perm[sp * PHASE2_MOVES.len() + j] as usize;
            sol.push(m);
            if self.p2_dfs(ncp, nep, nsp, depth - 1, Some(face), sol) {
                return true;
            }
            sol.pop();
        }
        false
    }
}

impl Default for Solver {
    fn default() -> Self {
        Self::new()
    }
}

/// BFS from the phase-1 goal over (orientation, slice), giving a distance lower bound.
fn build_phase1(mt: &MoveTables, orient_table: &[u16], n_orient: usize) -> Vec<u8> {
    let size = n_orient * 495;
    let mut dist = vec![255u8; size];
    let goal = SLICE_GOAL; // orientation 0, slice at goal → 0*495 + SLICE_GOAL
    dist[goal] = 0;
    let mut frontier = vec![goal];
    let mut depth = 0u8;
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for &idx in &frontier {
            let o = idx / 495;
            let s = idx % 495;
            for m in 0..N_MOVES {
                let no = orient_table[o * N_MOVES + m] as usize;
                let ns = mt.slice[s * N_MOVES + m] as usize;
                let nidx = no * 495 + ns;
                if dist[nidx] == 255 {
                    dist[nidx] = depth + 1;
                    next.push(nidx);
                }
            }
        }
        frontier = next;
        depth += 1;
    }
    dist
}

/// BFS from the phase-2 goal over (permutation, slice_perm).
fn build_phase2(mt: &MoveTables, perm_table: &[u16], is_corner: bool, n_perm: usize) -> Vec<u8> {
    let p2 = PHASE2_MOVES.len();
    let size = n_perm * 24;
    let mut dist = vec![255u8; size];
    dist[0] = 0;
    let mut frontier = vec![0usize];
    let mut depth = 0u8;
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for &idx in &frontier {
            let p = idx / 24;
            let s = idx % 24;
            for (j, &m) in PHASE2_MOVES.iter().enumerate() {
                // corner_perm is stored over all 18 moves; edge8_perm over the 10.
                let np = if is_corner {
                    perm_table[p * N_MOVES + m] as usize
                } else {
                    perm_table[p * p2 + j] as usize
                };
                let ns = mt.slice_perm[s * p2 + j] as usize;
                let nidx = np * 24 + ns;
                if dist[nidx] == 255 {
                    dist[nidx] = depth + 1;
                    next.push(nidx);
                }
            }
        }
        frontier = next;
        depth += 1;
    }
    dist
}

/// Collapse consecutive same-face quarter turns into `FaceTurn`s (1/2/3 quarters).
fn merge_to_turns(moves: &[usize]) -> Vec<FaceTurn> {
    moves
        .iter()
        .map(|&m| FaceTurn {
            face: (m / 3) as u8,
            quarter: (m % 3) as u8 + 1,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state >> 33
    }

    #[test]
    fn solves_random_scrambles() {
        let solver = Solver::new();
        let all = super::super::movetables::all_move_cubes();
        let mut rng = 0xC0FFEEu64;
        let mut max_moves = 0usize;
        for _ in 0..50 {
            // Build a random scramble.
            let mut cube = CubieCube::SOLVED;
            let mut nq = 0usize;
            for _ in 0..25 {
                let m = (lcg(&mut rng) % N_MOVES as u64) as usize;
                cube = cube.multiply(&all[m]);
                nq += 1;
            }
            let _ = nq;
            let sol = solver.solve(&cube).expect("solver found no solution");
            // Apply the solution and confirm the cube is solved.
            let mut check = cube;
            for t in &sol {
                check = check.apply_turn(*t);
            }
            assert!(check.is_solved(), "solution did not solve the cube");
            max_moves = max_moves.max(sol.len());
            assert!(sol.len() <= 32, "solution unexpectedly long: {}", sol.len());
        }
        // Sanity: real two-phase solutions are well under 32 moves.
        assert!(max_moves <= 32, "worst solution {max_moves} moves");
    }

    #[test]
    fn solves_the_solved_cube_trivially() {
        let solver = Solver::new();
        let sol = solver.solve(&CubieCube::SOLVED).unwrap();
        assert!(
            sol.is_empty(),
            "solved cube should need no moves, got {}",
            sol.len()
        );
    }
}
