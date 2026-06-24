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

use crate::kociemba::cube3::{solve_sticker, sticker_to_cubie};
use crate::kociemba::search::Solver;
use crate::kociemba::CubieCube;
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

/// Parity (true = odd) of a permutation given as `p[i] = where i goes`.
fn perm_parity(p: &[u8]) -> bool {
    let mut seen = vec![false; p.len()];
    let mut odd = false;
    for i in 0..p.len() {
        if seen[i] {
            continue;
        }
        let mut j = i;
        let mut len = 0usize;
        while !seen[j] {
            seen[j] = true;
            j = p[j] as usize;
            len += 1;
        }
        if len.is_multiple_of(2) {
            odd = !odd; // an even-length cycle is an odd permutation
        }
    }
    odd
}

/// Whether a 3×3 cubie state is reachable on a real 3×3 (the three standard
/// invariants). A reduced even cube can violate the edge invariants — that is
/// OLL/PLL parity, which `solve_reduction` resolves before finishing.
fn is_solvable(cc: &CubieCube) -> bool {
    let co: u32 = cc.co.iter().map(|&x| x as u32).sum();
    let eo: u32 = cc.eo.iter().map(|&x| x as u32).sum();
    co.is_multiple_of(3) && eo.is_multiple_of(2) && perm_parity(&cc.cp) == perm_parity(&cc.ep)
}

/// Solve a reduced NxN cube's 3×3 stage. Returns the outer-turn moves (applied to
/// `cube`), or `None` if the extracted 3×3 is unsolvable (parity — handled by
/// [`solve_reduction`]). The parity check is essential: feeding an impossible state
/// to the two-phase search would never terminate.
pub fn finish_3x3(cube: &mut StickerCube, solver: &Solver) -> Option<Vec<Move>> {
    let size = cube.size();
    let size3 = CubeSize::new(3).unwrap();
    let cube3 = extract_3x3(cube);
    if !is_solvable(&sticker_to_cubie(&cube3)) {
        return None; // OLL/PLL parity
    }
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

/// Varied parity-disturbance sequences for the toggle loop, of two complementary kinds.
/// Face quarter turns are *center-safe* odd permutations of the wings (each quarter turn
/// 4-cycles the wings of its layer): the edge solver's library is built from 3-cycles and
/// commutators, all *even*, so it can never flip wing parity on its own; a face turn
/// supplies the missing odd operation, and because it leaves every centre solid the
/// re-reduction doesn't launder the flip away (these are tried first, odd cubes only).
/// Inner slices (every face/depth) plus two-axis combos disturb the wings and centres to
/// explore the four OLL×PLL parity classes a single fixed slice can't. Cycling these (each
/// followed by a deterministic re-reduction) reaches a solvable class.
fn parity_repertoire(n: usize) -> Vec<Vec<Move>> {
    use super::slice_from;
    let size = cube_core::CubeSize::new(n).expect("size >= 2");
    let mut out: Vec<Vec<Move>> = Vec::new();
    // Center-safe odd wing flips: a face quarter turn (both directions) on every face.
    // Only odd cubes need these — even cubes resolve parity deterministically (the dedge
    // swap), and a face turn also perturbs corner parity, complicating that path. Odd
    // cubes have no reduction parity, so the face turn cleanly flips the wing parity that
    // the all-even cycle library cannot.
    if !n.is_multiple_of(2) {
        for f in Face::ALL {
            for t in [1i8, -1] {
                out.push(vec![Move::face(f, size, t)]);
            }
            // Wide turns (face + inner slices) flip *one* off-middle wing orbit
            // independently — the two-orbit parity a lone face turn or slice can't
            // separate (an odd cube's t=1 and t=3 wings can carry different parities).
            for w in 2..=n - 1 {
                out.push(vec![Move::wide(f, size, w, 1)]);
            }
        }
    }
    for f in [
        Face::Right,
        Face::Up,
        Face::Front,
        Face::Left,
        Face::Down,
        Face::Back,
    ] {
        for d in 1..=n - 2 {
            out.push(vec![slice_from(f, n, d, 1)]);
        }
    }
    // Two-axis combos flip the *other* combined parity bit, reaching classes a single
    // slice cannot from some starts.
    out.push(vec![
        slice_from(Face::Right, n, 1, 1),
        slice_from(Face::Up, n, 1, 1),
    ]);
    out.push(vec![
        slice_from(Face::Right, n, 1, 1),
        slice_from(Face::Front, n, 1, 1),
    ]);
    out.push(vec![
        slice_from(Face::Up, n, 1, 1),
        slice_from(Face::Front, n, 1, 1),
    ]);
    out
}

/// Full NxN reduction solve: centres → edges → 3×3 finish. On even cubes the reduced
/// 3×3 can carry OLL/PLL parity (an impossible 3×3 state); we disturb the wing
/// permutation with a varied repertoire of inner slices and re-reduce, which makes it
/// solvable. Returns the complete move list (applied to `cube`), or `None` if a stage
/// fails.
pub fn solve_reduction(cube: &mut StickerCube, solver: &Solver) -> Option<Vec<Move>> {
    use super::edges_det::{at_target, home_swapped_target, home_targets, solve_to_target};
    use super::{centers_solved, solve_centers, solve_edges};
    let dbg = std::env::var("RDBG").is_ok();
    let n = cube.size().get();
    let mut moves = Vec::new();
    moves.extend(solve_centers(cube));
    if !centers_solved(cube) {
        if dbg {
            eprintln!("[red] centres FAILED");
        }
        return None;
    }

    // One unified loop. Each pass drives edges toward *all-home*; if reached, the 3×3 is
    // finished — and if that all-home 3×3 is unsolvable (only possible cause: odd corners,
    // since all-home means no dedge flips and an even dedge permutation) we re-drive edges
    // to a target with two edges *swapped*, flipping the dedge-permutation parity to match
    // the corners (an even wing permutation for even n, hence reachable). When a pass
    // can't reach all-home (an odd wing permutation, or a rare coverage stall), we disturb
    // the wings with the next entry of a *varied* slice repertoire and re-reduce: varying
    // the disturbance both flips wing parity and explores re-reductions that sidestep a
    // coverage stall. Odd cubes have no reduction parity, so they finish on the first pass.
    let home = home_targets(n);
    let rep = parity_repertoire(n);
    let even = n.is_multiple_of(2);
    for attempt in 0..=rep.len() {
        if attempt > 0 {
            for &m in &rep[attempt - 1] {
                cube.apply_move(m).ok()?;
                moves.push(m);
            }
            moves.extend(solve_centers(cube));
            if !centers_solved(cube) {
                if dbg {
                    eprintln!("[red] centres FAILED after disturbance (attempt {attempt})");
                }
                return None;
            }
        }
        moves.extend(solve_edges(cube));
        if !at_target(cube, &home) {
            if dbg {
                eprintln!("[red] not all-home at attempt {attempt}; disturbing");
            }
            continue;
        }
        if let Some(fin) = finish_3x3(cube, solver) {
            moves.extend(fin);
            return Some(moves);
        }
        if !even {
            continue; // odd cubes never carry parity; a re-reduction will solve
        }
        // All-home but unsolvable ⇒ odd corners. Swap two dedges to flip PLL parity.
        if dbg {
            eprintln!("[red] all-home odd corners at attempt {attempt}; swapping two dedges");
        }
        let swapped = home_swapped_target(n, 0, 1);
        moves.extend(solve_to_target(cube, &swapped));
        if at_target(cube, &swapped) {
            if let Some(fin) = finish_3x3(cube, solver) {
                moves.extend(fin);
                return Some(moves);
            }
        }
        // Swap didn't resolve it (shouldn't happen); the next disturbance will retry.
    }
    if dbg {
        eprintln!("[red] NOT resolved after {} attempts", rep.len() + 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Diagnostic: after pairing edges to home (with a wing-parity toggle to reach a
    /// paired state), print the reduced 3×3's invariants per seed. If unsolvable cases
    /// are dominated by `cp_par != ep_par` with `ep_par == even`, the failure is odd
    /// corners — the home-targeting parity trap.
    #[test]
    #[ignore = "diagnostic"]
    fn parity_structure_n4() {
        use super::super::{centers_solved, edges_paired, slice_from, solve_centers, solve_edges};
        for seed in 0..16u64 {
            let mut cube = scramble(4, 0x100 + seed, 40);
            solve_centers(&mut cube);
            // reach a paired state, toggling wing parity if the solver stalls
            for _ in 0..6 {
                solve_edges(&mut cube);
                if edges_paired(&cube) {
                    break;
                }
                let _ = cube.apply_move(slice_from(Face::Right, 4, 1, 1));
                solve_centers(&mut cube);
            }
            let paired = edges_paired(&cube) && centers_solved(&cube);
            if !paired {
                println!("seed {seed}: NOT paired");
                continue;
            }
            let cc = sticker_to_cubie(&extract_3x3(&cube));
            let co: u32 = cc.co.iter().map(|&x| x as u32).sum();
            let eo: u32 = cc.eo.iter().map(|&x| x as u32).sum();
            println!(
                "seed {seed}: co%3={} eo%2={} cp_par={} ep_par={} solvable={}",
                co % 3,
                eo % 2,
                perm_parity(&cc.cp) as u8,
                perm_parity(&cc.ep) as u8,
                is_solvable(&cc)
            );
        }
    }

    /// Does the n=6 centre solver terminate (even if it can't fully solve the obliques)?
    /// A hanging solver is a defect; it must give up via its cap and return.
    #[test]
    #[ignore = "diagnostic"]
    fn n6_centres_terminate() {
        use super::super::{centers_solved, solve_centers};
        for seed in 0..3u64 {
            let mut cube = scramble(6, 0x700 + seed, 60);
            let t0 = std::time::Instant::now();
            solve_centers(&mut cube);
            println!(
                "seed {seed}: centres returned in {:?}, solved={}",
                t0.elapsed(),
                centers_solved(&cube)
            );
        }
    }

    /// Probe a stalled n=5 edge state: which single disturbance, applied then re-reduced,
    /// reaches all-home? Identifies the operation the two-orbit wing parity needs.
    #[test]
    #[ignore = "diagnostic"]
    fn n5_stall_probe() {
        use super::super::edges_det::{at_target, home_targets};
        use super::super::{centers_solved, slice_from, solve_centers, solve_edges};
        let n = 5usize;
        let size = CubeSize::new(n).unwrap();
        let home = home_targets(n);
        for seed in [5u64, 8] {
            let mut cube = scramble(n, 0x500 + seed, n * 15);
            solve_centers(&mut cube);
            solve_edges(&mut cube);
            let base = cube.clone();
            let at_home0 = at_target(&base, &home);
            println!("seed {seed}: initial all-home={at_home0}");
            // Candidate disturbances.
            let mut cands: Vec<(String, Vec<Move>)> = Vec::new();
            for f in Face::ALL {
                for t in [1i8, -1, 2] {
                    cands.push((format!("face {f:?} {t}"), vec![Move::face(f, size, t)]));
                }
                for d in 1..=n - 2 {
                    cands.push((format!("slice {f:?} d{d}"), vec![slice_from(f, n, d, 1)]));
                }
                for w in 2..=n - 1 {
                    cands.push((format!("wide {f:?} w{w}"), vec![Move::wide(f, size, w, 1)]));
                }
            }
            // A few face+slice combos (flip one orbit independently?).
            for f in [Face::Right, Face::Up, Face::Front] {
                for d in 1..=n - 2 {
                    cands.push((
                        format!("face {f:?}+slice d{d}"),
                        vec![Move::face(f, size, 1), slice_from(f, n, d, 1)],
                    ));
                }
            }
            let mut wins = Vec::new();
            for (name, dist) in &cands {
                let mut c = base.clone();
                for &m in dist {
                    c.apply_move(m).unwrap();
                }
                solve_centers(&mut c);
                if !centers_solved(&c) {
                    continue;
                }
                solve_edges(&mut c);
                if at_target(&c, &home) {
                    wins.push(name.clone());
                }
            }
            println!(
                "seed {seed}: {} disturbances reach all-home: {:?}",
                wins.len(),
                wins
            );
        }
    }

    /// End-to-end 4×4: full reduction (centres → edges → finish + parity), verified
    /// fully solved by replay over many scrambles.
    #[test]
    #[ignore = "slow; run explicitly"]
    fn full_solve_n4() {
        let solver = Solver::new();
        let mut solved = 0;
        let mut fails = Vec::new();
        let t0 = std::time::Instant::now();
        let trials = 50u64;
        for seed in 0..trials {
            let mut cube = scramble(4, 0x100 + seed, 60);
            match solve_reduction(&mut cube, &solver) {
                Some(_) if cube.is_solved() => solved += 1,
                _ => fails.push(seed),
            }
        }
        println!(
            "n=4 full solve: {solved}/{trials} ({:?}); fails {fails:?}",
            t0.elapsed()
        );
        assert_eq!(solved, trials, "n=4 not fully reliable: fails {fails:?}");
    }

    /// End-to-end across sizes: even cubes exercise the deterministic parity path, odd
    /// cubes the parity-free path. Verified fully solved by replay. 4×4 and 5×5 are fully
    /// reliable; n=6 centres (even obliques) and n≥7 are not yet covered.
    #[test]
    #[ignore = "slow; run explicitly"]
    fn full_solve_sizes() {
        let solver = Solver::new();
        for n in [4usize, 5] {
            let mut solved = 0;
            let mut fails = Vec::new();
            let t0 = std::time::Instant::now();
            let trials = 30u64;
            for seed in 0..trials {
                let mut cube = scramble(n, 0x500 + seed, n * 15);
                match solve_reduction(&mut cube, &solver) {
                    Some(_) if cube.is_solved() => solved += 1,
                    _ => fails.push(seed),
                }
            }
            // stderr so per-size results stream even if a later size hangs.
            eprintln!(
                "n={n} full solve: {solved}/{trials} ({:?}); fails {fails:?}",
                t0.elapsed()
            );
            assert_eq!(solved, trials, "n={n} not fully reliable: fails {fails:?}");
        }
    }
}
