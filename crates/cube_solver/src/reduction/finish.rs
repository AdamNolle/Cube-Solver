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
    // Even cubes ≥6 have the same two-orbit wing parity as odd cubes (n-2≥4 wings/edge),
    // but a face/wide turn flips corner parity too — acceptable here because the
    // deterministic dedge swap then re-fixes the corners. Appended *after* the slices so
    // 4×4 (which never needs them) stays fast and 6×6 only reaches them on a real stall.
    if n.is_multiple_of(2) && n >= 6 {
        for f in Face::ALL {
            for t in [1i8, -1] {
                out.push(vec![Move::face(f, size, t)]);
            }
            for w in 2..=n - 1 {
                out.push(vec![Move::wide(f, size, w, 1)]);
            }
        }
    }
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
    let home = home_targets(n);
    let even = n.is_multiple_of(2);
    // The centre solver isn't 100% reliable on big cubes (≈n=8 it stalls on some
    // scrambles). Don't give up if it does: the parity-disturbance loop below re-reduces
    // centres *and* edges after each disturbance, and a disturbed centre configuration
    // very often solves where the original stalled. So only solve edges (and try an
    // immediate finish) when the centres are already solid; otherwise fall straight into
    // the disturbance search.
    if centers_solved(cube) {
        moves.extend(solve_edges(cube));
    } else if dbg {
        eprintln!("[red] centres stalled on first pass; relying on disturbance recovery");
    }

    // Finish from the current state if edges are all-home: solve the reduced 3×3, and on an
    // even cube clear an odd-corner PLL by re-driving edges to a two-dedges-swapped target
    // (odd dedge perm to match the corners; reachable since n-2 is even). Extends `extra`
    // and returns true on success.
    let try_finish = |cube: &mut StickerCube, extra: &mut Vec<Move>| -> bool {
        if !at_target(cube, &home) {
            return false;
        }
        if let Some(fin) = finish_3x3(cube, solver) {
            extra.extend(fin);
            return true;
        }
        if even {
            let swapped = home_swapped_target(n, 0, 1);
            extra.extend(solve_to_target(cube, &swapped));
            if at_target(cube, &swapped) {
                if let Some(fin) = finish_3x3(cube, solver) {
                    extra.extend(fin);
                    return true;
                }
            }
        }
        false
    };

    // Snapshot the clean stalled state BEFORE attempting a finish: `try_finish` runs the
    // dedge swap, which mutates `cube` even when it ultimately fails (e.g. the swap target
    // is itself unreachable at this parity) — the parity search must start from the clean
    // stall, not that half-swapped state. (This was the 8×8 bug: a disturbance that solves
    // exists, but Phase 2 was applying it to the corrupted post-swap cube.)
    let base = cube.clone();
    let base_moves = moves.clone();

    if centers_solved(cube) && try_finish(cube, &mut moves) {
        return Some(moves);
    }

    // Parity search. The edges stalled — an odd wing permutation in one or more wing orbits
    // (n-2 wings/edge split into orbits with independent parities; larger cubes have more).

    // Phase 0 — multi-orbit parity via flipper subsets (the big-cube speedup). The wings
    // split into K=⌊(n-2)/2⌋ orbits {d, n-1-d}; one disturbance flips one orbit's parity —
    // a slice at depth d on EVEN cubes (slices don't move corners there), a wide for ODD
    // cubes (slices launder under their fixed centre). Trying all 2^K subsets of the K
    // orbit flippers lands on whatever combination of orbits is odd in one shot, instead of
    // walking long cumulative prefixes or scanning the whole repertoire. Bounded to 2^8.
    {
        let size = CubeSize::new(n).unwrap();
        let k = ((n - 2) / 2).min(8);
        let flippers: Vec<Move> = (1..=k)
            .map(|d| {
                if even {
                    super::slice_from(Face::Right, n, d, 1)
                } else {
                    Move::wide(Face::Right, size, d + 1, 1)
                }
            })
            .collect();
        for mask in 1u32..(1u32 << flippers.len()) {
            let mut c = base.clone();
            let mut m = base_moves.clone();
            for (i, fl) in flippers.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    c.apply_move(*fl).ok()?;
                    m.push(*fl);
                }
            }
            m.extend(solve_centers(&mut c));
            if !centers_solved(&c) {
                continue;
            }
            m.extend(solve_edges(&mut c));
            if try_finish(&mut c, &mut m) {
                *cube = c;
                return Some(m);
            }
        }
    }

    let rep = parity_repertoire(n);

    // Phase 1 — cumulative walk (fast, resolves n≤6 and easy n≥7): accumulate disturbances
    // and re-check after each, so prefixes of the walk cover multi-orbit combinations.
    {
        let mut c = base.clone();
        let mut m = base_moves.clone();
        for dist in &rep {
            for &mv in dist {
                c.apply_move(mv).ok()?;
                m.push(mv);
            }
            m.extend(solve_centers(&mut c));
            if !centers_solved(&c) {
                continue;
            }
            m.extend(solve_edges(&mut c));
            if try_finish(&mut c, &mut m) {
                *cube = c;
                return Some(m);
            }
        }
    }

    // Phase 2 — non-cumulative singles: try each disturbance from THIS SAME stalled state.
    // A single face/wide/slice flips a specific orbit's parity, but only relative to the
    // stall; stacking disturbances (Phase 1) never tests the lone flip a single odd orbit
    // needs. This catches the cases the cumulative walk's fixed path misses.
    for dist in &rep {
        let mut c = base.clone();
        let mut m = base_moves.clone();
        for &mv in dist {
            c.apply_move(mv).ok()?;
            m.push(mv);
        }
        m.extend(solve_centers(&mut c));
        if !centers_solved(&c) {
            continue;
        }
        m.extend(solve_edges(&mut c));
        if try_finish(&mut c, &mut m) {
            *cube = c;
            return Some(m);
        }
    }

    if dbg {
        eprintln!("[red] NOT resolved after {} disturbances", rep.len());
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

    /// Inspect the n=6 centre stall: which cells are wrong, of what type/chirality, and
    /// Which centre orbit stalls on a large cube? Prints wrong cells with their
    /// orbit signature `(min(rr,cc), max(rr,cc))` where rr=min(r,n-1-r), cc=min(c,n-1-c).
    #[test]
    #[ignore = "diagnostic"]
    fn big_centre_stall() {
        use super::super::{centers_solved, solve_centers};
        for n in [8usize, 7] {
            let mut cube = scramble(n, 0x900, n * 12);
            solve_centers(&mut cube);
            println!("n={n}: centres_solved={}", centers_solved(&cube));
            for f in Face::ALL {
                let want = f.color();
                let mut wrong = Vec::new();
                for r in 1..n - 1 {
                    for c in 1..n - 1 {
                        if cube.color_at(f, r, c) != Some(want) {
                            let rr = r.min(n - 1 - r);
                            let cc = c.min(n - 1 - c);
                            wrong.push((r, c, (rr.min(cc), rr.max(cc))));
                        }
                    }
                }
                if !wrong.is_empty() {
                    println!("  face {f:?}: {} wrong {:?}", wrong.len(), wrong);
                }
            }
        }
    }

    /// where the matching-colour reservoir pieces sit. Tests the chirality hypothesis.
    #[test]
    #[ignore = "diagnostic"]
    fn n6_centre_stall_inspect() {
        use super::super::solve_centers;
        let n = 6usize;
        // Cell classification on the n×n centre block.
        let classify = |r: usize, c: usize| -> &'static str {
            let on_d1 = r == c;
            let on_d2 = r + c == n - 1;
            if on_d1 || on_d2 {
                "X"
            } else if (r - 1) < (n - 1 - r) {
                // upper triangle; chirality by which side of the main diagonal
                if c > r {
                    "obl-A"
                } else {
                    "obl-B"
                }
            } else if c < r {
                "obl-A"
            } else {
                "obl-B"
            }
        };
        for seed in [0u64] {
            let mut cube = scramble(n, 0x700 + seed, 60);
            solve_centers(&mut cube);
            for f in Face::ALL {
                let want = f.color();
                let mut wrong = Vec::new();
                for r in 1..n - 1 {
                    for c in 1..n - 1 {
                        if cube.color_at(f, r, c) != Some(want) {
                            wrong.push((r, c, classify(r, c)));
                        }
                    }
                }
                if !wrong.is_empty() {
                    eprintln!("seed {seed} face {f:?}: {} wrong: {:?}", wrong.len(), wrong);
                    // For the first wrong cell, where are the `want`-coloured obliques?
                    if let Some(&(_, _, ty)) = wrong.first() {
                        let mut res = Vec::new();
                        for g in Face::ALL {
                            for r in 1..n - 1 {
                                for c in 1..n - 1 {
                                    if cube.color_at(g, r, c) == Some(want) && classify(r, c) != "X"
                                    {
                                        res.push((format!("{g:?}"), r, c, classify(r, c)));
                                    }
                                }
                            }
                        }
                        eprintln!(
                            "   want={want:?} ({ty}); {}-coloured obliques: {:?}",
                            want as u8, res
                        );
                    }
                }
            }
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

    /// End-to-end across sizes 4–8: even cubes exercise the deterministic parity path, odd
    /// cubes the parity-free path; both rely on disturbance recovery when the centre solver
    /// stalls on a big cube. Verified fully solved by replay; all of 4×4–8×8 are reliable.
    #[test]
    #[ignore = "slow; run explicitly"]
    fn full_solve_sizes() {
        let solver = Solver::new();
        for n in [4usize, 5, 6, 7, 8] {
            let mut solved = 0;
            let mut fails = Vec::new();
            let t0 = std::time::Instant::now();
            // Fewer trials for the slower big cubes.
            let trials: u64 = if n <= 6 { 20 } else { 10 };
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
