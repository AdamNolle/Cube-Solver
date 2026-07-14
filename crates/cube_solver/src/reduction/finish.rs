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

use crate::kociemba::cube3::{solve_sticker, sticker_to_cubie_unchecked};
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

fn wing_orbit_depth(slot: usize, n: usize) -> Option<usize> {
    let wings_per_edge = n.checked_sub(2)?;
    if wings_per_edge == 0 {
        return None;
    }
    let wing = slot % wings_per_edge + 1;
    if wing * 2 == n - 1 {
        return None; // fixed middle edge on odd cubes, not a paired-wing orbit
    }
    let depth = wing.min(n - 1 - wing);
    (depth > 0).then_some(depth)
}

/// RCube-style depth-parameterized visible wing-defect toggle translated into
/// cube_core's turn convention. The normalizer applies its inverse only after
/// reaching the exact canonical staged defect. Tests below verify its orbit-local
/// support independently of scramble history.
fn orbit_parity_template(n: usize, depth: usize) -> Option<Vec<Move>> {
    let orbit_count = (n.checked_sub(2)?) / 2;
    if depth == 0 || depth > orbit_count {
        return None;
    }
    let size = CubeSize::new(n).ok()?;
    // RCube's q is face-clockwise. cube_core's face-like positive quarter uses
    // the inverse convention, so indexed U/D slice quarters are negated.
    let d = |turns: i8| super::slice_from(Face::Down, n, depth, -turns);
    let u = |turns: i8| super::slice_from(Face::Up, n, depth, -turns);
    let face = |f, turns| Move::face(f, size, turns);
    Some(vec![
        d(-1),
        face(Face::Right, 2),
        u(1),
        face(Face::Front, 2),
        u(-1),
        face(Face::Front, 2),
        d(2),
        face(Face::Right, 2),
        d(1),
        face(Face::Right, 2),
        d(-1),
        face(Face::Right, 2),
        face(Face::Front, 2),
        d(2),
        face(Face::Front, 2),
    ])
}

/// The depth-local template has the same edge-major 24-pair visible signature
/// at every N/depth (machine-checked below), so build it once on a 4×4 instead
/// of constructing and turning an O(N²) reference cube for every orbit.
fn canonical_defect_pairs() -> &'static [(Color, Color)] {
    static PAIRS: std::sync::OnceLock<Vec<(Color, Color)>> = std::sync::OnceLock::new();
    PAIRS.get_or_init(|| {
        let mut defect = StickerCube::solved(CubeSize::new(4).unwrap());
        for mv in orbit_parity_template(4, 1).unwrap() {
            defect.apply_move(mv).unwrap();
        }
        super::edges_det::orbit_pairs(&defect, 1)
    })
}

/// Put each paired 24-wing orbit into one of two sticker-visible normal forms:
/// all-home (`E_d`) or the exact local defect produced by `orbit_parity_template`
/// (`D_d`). A bounded coverage failure is kept distinct from parity. When `D_d`
/// is reached, the verified inverse template maps it to `E_d` without touching
/// centers, corners, the odd-cube midge, or any other wing orbit.
fn normalize_and_correct_wing_orbits(cube: &mut StickerCube) -> Option<Vec<Move>> {
    use super::centers_solved;
    use super::edges_det::{home_orbit_pairs, orbit_matches_pairs, solve_orbit_to_pairs};

    let n = cube.size().get();
    if n <= 3 {
        return Some(Vec::new());
    }
    if !centers_solved(cube) {
        return None;
    }
    let home = home_orbit_pairs(n);
    let defect = canonical_defect_pairs();
    let mut moves = Vec::new();
    for depth in 1..=(n - 2) / 2 {
        if !super::reduction_checkpoint() {
            return None;
        }
        if orbit_matches_pairs(cube, depth, &home) {
            continue;
        }
        let template = orbit_parity_template(n, depth)?;

        // Exact canonical forms need no search. Each comparison reads 24 slots,
        // making classification O(N) across all paired wing orbits.
        if orbit_matches_pairs(cube, depth, defect) {
            for mv in template.iter().rev() {
                let inverse = mv.inverse();
                cube.apply_move(inverse).ok()?;
                moves.push(inverse);
            }
            if !orbit_matches_pairs(cube, depth, &home) {
                return None;
            }
            continue;
        }

        let mut even_trial = cube.clone();
        let even = solve_orbit_to_pairs(&mut even_trial, depth, &home);
        if std::env::var("RDBG").is_ok() {
            eprintln!(
                "[red] orbit normalizer depth {depth}: E stall {:?}",
                even.stalled_slot
            );
        }
        if even.stalled_slot.is_none() {
            *cube = even_trial;
            moves.extend(even.moves);
            continue;
        }

        let mut defect_trial = cube.clone();
        let defect_outcome = solve_orbit_to_pairs(&mut defect_trial, depth, defect);
        if std::env::var("RDBG").is_ok() {
            eprintln!(
                "[red] orbit normalizer depth {depth}: D stall {:?}",
                defect_outcome.stalled_slot
            );
        }
        if defect_outcome.stalled_slot.is_some() {
            return None;
        }
        moves.extend(defect_outcome.moves);
        for mv in template.iter().rev() {
            let inverse = mv.inverse();
            defect_trial.apply_move(inverse).ok()?;
            moves.push(inverse);
        }
        // Recompute the visible form rather than trusting the defect bookkeeping.
        if !orbit_matches_pairs(&defect_trial, depth, &home) {
            return None;
        }
        *cube = defect_trial;
    }
    centers_solved(cube).then_some(moves)
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
    let cubie = sticker_to_cubie_unchecked(&cube3)?;
    if !is_solvable(&cubie) {
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

/// Shorten a move list by collapsing maximal runs of same-axis moves — which all commute
/// (parallel layers don't interfere) — summing turns per layer and dropping zeros. Iterated
/// to a fixed point so cancellations cascade (e.g. `R U U' R' → ∅`). Effect-preserving:
/// reordering within a same-axis run and merging same-layer turns leaves the permutation
/// unchanged. Solutions built from many concatenated commutators accumulate exactly these
/// boundary cancellations, so this safely cuts the move count.
fn simplify(moves: Vec<Move>) -> Vec<Move> {
    let mut cur = moves;
    loop {
        let mut out: Vec<Move> = Vec::with_capacity(cur.len());
        let mut i = 0;
        while i < cur.len() {
            let axis = cur[i].axis;
            let mut acc: Vec<((usize, usize), i32)> = Vec::new();
            let mut j = i;
            while j < cur.len() && cur[j].axis == axis {
                let key = (cur[j].layer_start, cur[j].layer_end);
                match acc.iter_mut().find(|(k, _)| *k == key) {
                    Some(e) => e.1 += cur[j].turns as i32,
                    None => acc.push((key, cur[j].turns as i32)),
                }
                j += 1;
            }
            for ((ls, le), t) in acc {
                let tt = t.rem_euclid(4);
                if tt != 0 {
                    let turns = if tt == 3 { -1 } else { tt as i8 };
                    out.push(Move::new(axis, ls, le, turns));
                }
            }
            i = j;
        }
        if out.len() == cur.len() {
            return out;
        }
        cur = out;
    }
}

/// Full NxN reduction solve: centres → edges → 3×3 finish. On even cubes the reduced
/// 3×3 can carry OLL/PLL parity (an impossible 3×3 state); we disturb the wing
/// permutation with a varied repertoire of inner slices and re-reduce, which makes it
/// solvable. Returns the complete move list (applied to `cube`), or `None` if a stage
/// fails. The returned list is `simplify`-ed (adjacent same-axis cancellations).
pub fn solve_reduction(cube: &mut StickerCube, solver: &Solver) -> Option<Vec<Move>> {
    solve_reduction_core(cube, solver).map(simplify)
}

pub fn solve_reduction_with_control(
    cube: &mut StickerCube,
    solver: &Solver,
    control: &super::ReductionControl,
) -> Result<Vec<Move>, super::ReductionError> {
    let mut work = cube.clone();
    let result = super::with_reduction_control(control, || solve_reduction(&mut work, solver));
    if !control.should_continue() {
        Err(super::ReductionError::CancelledOrTimedOut)
    } else if let Some(solution) = result {
        *cube = work;
        Ok(solution)
    } else {
        Err(super::ReductionError::Unsolved)
    }
}

fn solve_reduction_core(cube: &mut StickerCube, solver: &Solver) -> Option<Vec<Move>> {
    if !super::reduction_checkpoint() {
        return None;
    }
    use super::edges_det::{
        at_target, home_swapped_target, home_targets, solve_edges_with_stall, solve_to_target,
    };
    use super::{centers_solved, solve_centers, solve_edges};
    let dbg = std::env::var("RDBG").is_ok();
    let n = cube.size().get();
    let mut moves = Vec::new();
    moves.extend(solve_centers(cube));
    if !super::reduction_checkpoint() {
        return None;
    }
    let center_base = cube.clone();
    let center_moves = moves.clone();
    let home = home_targets(n);
    let even = n.is_multiple_of(2);
    // The centre solver isn't 100% reliable on big cubes (≈n=8 it stalls on some
    // scrambles). Don't give up if it does: the parity-disturbance loop below re-reduces
    // centres *and* edges after each disturbance, and a disturbed centre configuration
    // very often solves where the original stalled. So only solve edges (and try an
    // immediate finish) when the centres are already solid; otherwise fall straight into
    // the disturbance search.
    let mut stalled_slot = None;
    if centers_solved(cube) {
        let outcome = solve_edges_with_stall(cube);
        moves.extend(outcome.moves);
        stalled_slot = outcome.stalled_slot;
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

    // Polynomial sticker-only orbit normalization. Unlike the legacy path below,
    // this never treats a first stalled slot as parity: each orbit must reach the
    // exact visible E_d or D_d form, and D_d is corrected with a signature-tested
    // center-safe template local to that orbit. Try the untouched center-solved
    // state first; a global greedy edge pass may turn a simple orbit residual into
    // a harder (though equivalent) normalizer state before it stalls.
    for (candidate, prefix) in [(&center_base, &center_moves), (&base, &base_moves)] {
        if !super::reduction_checkpoint() {
            return None;
        }
        if !centers_solved(candidate) {
            continue;
        }
        let mut c = candidate.clone();
        if let Some(normalized) = normalize_and_correct_wing_orbits(&mut c) {
            let mut m = prefix.clone();
            m.extend(normalized);
            if try_finish(&mut c, &mut m) {
                *cube = c;
                return Some(m);
            }
        } else if dbg {
            eprintln!("[red] visible orbit normalizer hit a coverage failure; trying fallback");
        }
    }

    // Greedy polynomial parity correction. The edge solver returns the first slot its
    // even-cycle library cannot place; use that depth to learn a batched candidate mask
    // from a clean base in at most K attempts. This resolves single high orbits and common
    // multi-orbit cases without powerset enumeration. It is not yet a completeness proof;
    // the bounded legacy recovery below remains the conservative fallback.
    if centers_solved(&base) {
        let size = CubeSize::new(n).ok()?;
        let orbit_count = (n - 2) / 2;
        let mut selected = Vec::<usize>::new();
        for _ in 0..orbit_count {
            if !super::reduction_checkpoint() {
                return None;
            }
            let Some(slot) = stalled_slot else {
                break;
            };
            let Some(depth) = wing_orbit_depth(slot, n) else {
                break;
            };
            if dbg {
                eprintln!("[red] direct parity: stalled slot {slot}, orbit depth {depth}");
            }
            if selected.contains(&depth) {
                if dbg {
                    eprintln!("[red] direct parity repeated orbit {depth}; using fallback");
                }
                break;
            }
            selected.push(depth);

            // Reapply the accumulated candidate mask to the same clean stalled
            // state. Re-reducing after each individual flip can redistribute wing
            // parity; batching the learned depths before one reduction preserves
            // the intended independent-orbit toggles without powerset enumeration.
            let mut c = base.clone();
            let mut m = base_moves.clone();
            for &selected_depth in &selected {
                let flipper = if even {
                    super::slice_from(Face::Right, n, selected_depth, 1)
                } else {
                    Move::wide(Face::Right, size, selected_depth + 1, 1)
                };
                c.apply_move(flipper).ok()?;
                m.push(flipper);
            }
            m.extend(solve_centers(&mut c));
            if !centers_solved(&c) {
                break;
            }
            let outcome = solve_edges_with_stall(&mut c);
            m.extend(outcome.moves);
            stalled_slot = outcome.stalled_slot;
            if dbg {
                eprintln!(
                    "[red] direct parity: selected {selected:?}, next stall {stalled_slot:?}"
                );
            }
            if try_finish(&mut c, &mut m) {
                *cube = c;
                return Some(m);
            }
        }
    }

    // Legacy recovery remains as a conservative fallback while the direct orbit path
    // accumulates larger-N evidence. It is bounded below and must not be widened into
    // an unbounded powerset search.
    let rep = parity_repertoire(n);

    // Orbit flippers: the wings split into K=⌊(n-2)/2⌋ orbits {d, n-1-d}, and one
    // disturbance flips one orbit's parity — a depth-d slice on EVEN cubes (slices don't
    // move corners there), a width-(d+1) wide on ODD cubes (slices launder under the fixed
    // centre). Bounded to K≤8.
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

    // Bitmask parity search from a base state: try every NON-EMPTY subset of the orbit
    // flippers, re-reduce, finish. Lands on whatever combination of odd orbits exists in one
    // shot. (The no-flip case is the caller's direct finish, not repeated here.)
    let bitmask_search = |b: &StickerCube, bm: &[Move]| -> Option<(StickerCube, Vec<Move>)> {
        // Try subsets in order of increasing size (popcount): a parity is usually a few odd
        // orbits, so flipping the FEWEST first finds the minimal odd set in the fewest slow
        // re-reductions (a single high-index orbit needed up to 2^K tries in binary order;
        // by popcount it's ≤K). Big win at large even sizes, and yields a shorter solution.
        let mut masks: Vec<u32> = (1u32..(1u32 << flippers.len())).collect();
        masks.sort_by_key(|m| (m.count_ones(), *m));
        for mask in masks {
            if !super::reduction_checkpoint() {
                return None;
            }
            let mut c = b.clone();
            let mut m = bm.to_vec();
            let mut ok = true;
            for (i, fl) in flippers.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    if c.apply_move(*fl).is_err() {
                        ok = false;
                        break;
                    }
                    m.push(*fl);
                }
            }
            if !ok {
                continue;
            }
            m.extend(solve_centers(&mut c));
            if !centers_solved(&c) {
                continue;
            }
            m.extend(solve_edges(&mut c));
            if try_finish(&mut c, &mut m) {
                return Some((c, m));
            }
        }
        None
    };

    // Phase 0 — bitmask parity. Run it only when the centres are SOLVED: if they stalled, each
    // mask's full re-reduction (centres + edges) is slow and mostly can't recover the centres,
    // so skip straight to the leaner centre-recovery in Phase 0b (which uses centres-only
    // checks). For a solved-centre cube this is the big-cube parity speedup, unchanged. (Big
    // win at large sizes where a stalled cube otherwise grinds through up to 2^K slow masks
    // before reaching Phase 0b.)
    if centers_solved(&base) {
        if let Some((c, m)) = bitmask_search(&base, &base_moves) {
            *cube = c;
            return Some(m);
        }
    }

    // Phase 0b — centre-stall recovery + bitmask: if the centres are still stalled (no
    // flipper subset from the stalled state recovered them), find a disturbance that just
    // re-solves the centres, then run the bitmask parity search from that centre-solid
    // state. This keeps the rare "stubborn centre stall" seeds fast instead of dropping to
    // the slow cumulative walk below.
    if !centers_solved(&base) {
        for dist in &rep {
            if !super::reduction_checkpoint() {
                return None;
            }
            let mut rb = base.clone();
            let mut rm = base_moves.clone();
            let mut ok = true;
            for &mv in dist {
                if rb.apply_move(mv).is_err() {
                    ok = false;
                    break;
                }
                rm.push(mv);
            }
            if !ok {
                continue;
            }
            rm.extend(solve_centers(&mut rb));
            if !centers_solved(&rb) {
                continue;
            }
            // Direct finish from the recovered state (the recovery may have left the edges
            // pairable to all-home), then the bitmask for any residual wing parity.
            let mut rb_e = rb.clone();
            let mut rm_e = rm.clone();
            rm_e.extend(solve_edges(&mut rb_e));
            if try_finish(&mut rb_e, &mut rm_e) {
                *cube = rb_e;
                return Some(rm_e);
            }
            if let Some((c, m)) = bitmask_search(&rb, &rm) {
                *cube = c;
                return Some(m);
            }
        }
    }

    // Phase 1 — cumulative walk: accumulate disturbances and re-check after each, so
    // prefixes cover multi-orbit/centre-recovery combinations no single disturbance and no
    // flipper subset reach. Kept FIRST because for odd cubes the walk reaches the needed
    // parity in far fewer re-reductions than scanning singles does — i.e. it's the *fast*
    // path here, which the directive prioritises. (It can yield long solutions on the rare
    // hardest seeds; `simplify` trims them, and length is a non-goal vs reliability+speed.)
    {
        let mut c = base.clone();
        let mut m = base_moves.clone();
        for dist in &rep {
            if !super::reduction_checkpoint() {
                return None;
            }
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

    // Phase 2 — non-cumulative singles: try each disturbance from THIS SAME stalled state,
    // catching the lone flip a single odd orbit needs that the cumulative walk's fixed path
    // missed.
    for dist in &rep {
        if !super::reduction_checkpoint() {
            return None;
        }
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

    fn wing_pair(cube: &StickerCube, edge: usize, wing: usize) -> (Color, Color) {
        let (a, b) = super::super::edges::wing_cells(edge, wing, cube.size().get());
        (
            cube.color_at(a.0, a.1, a.2).unwrap(),
            cube.color_at(b.0, b.1, b.2).unwrap(),
        )
    }

    #[test]
    fn controlled_reduction_cancels_without_mutating_input() {
        use std::time::Duration;

        let solver = Solver::new();
        let size = CubeSize::new(4).unwrap();
        let mut input = StickerCube::solved(size);
        input.apply_move(Move::face(Face::Right, size, 1)).unwrap();
        let before = input.clone_snapshot();

        let cancelled = super::super::ReductionControl::unlimited();
        cancelled.cancel();
        assert_eq!(
            solve_reduction_with_control(&mut input, &solver, &cancelled),
            Err(super::super::ReductionError::CancelledOrTimedOut)
        );
        assert_eq!(input.clone_snapshot(), before);

        let timed_out = super::super::ReductionControl::with_timeout(Duration::ZERO);
        assert_eq!(
            solve_reduction_with_control(&mut input, &solver, &timed_out),
            Err(super::super::ReductionError::CancelledOrTimedOut)
        );
        assert_eq!(input.clone_snapshot(), before);

        let mut solved = StickerCube::solved(size);
        let solution = solve_reduction_with_control(
            &mut solved,
            &solver,
            &super::super::ReductionControl::unlimited(),
        )
        .expect("unlimited control should preserve normal solving");
        assert!(solution.is_empty());
        assert!(solved.is_solved());
    }

    #[test]
    fn parity_template_is_center_safe_and_orbit_local() {
        for n in [4usize, 5, 6, 7, 8, 20, 66, 132] {
            let size = CubeSize::new(n).unwrap();
            let solved = StickerCube::solved(size);
            for depth in 1..=(n - 2) / 2 {
                let sequence = orbit_parity_template(n, depth).expect("valid orbit template");
                let mut cube = solved.clone();
                for &mv in &sequence {
                    cube.apply_move(mv).unwrap();
                }
                assert!(
                    super::super::centers_solved(&cube),
                    "n={n} d={depth}: centers changed"
                );

                // Corner stickers and every non-target wing orbit, including the
                // odd-cube midge, are exact sticker-state invariants of T_d.
                for face in Face::ALL {
                    for (row, col) in [(0, 0), (0, n - 1), (n - 1, 0), (n - 1, n - 1)] {
                        assert_eq!(
                            cube.color_at(face, row, col),
                            solved.color_at(face, row, col),
                            "n={n} d={depth}: corner sticker changed"
                        );
                    }
                }
                let mut target_changed = false;
                for edge in 0..12 {
                    for wing in 1..=n - 2 {
                        let slot = edge * (n - 2) + wing - 1;
                        let changed =
                            wing_pair(&cube, edge, wing) != wing_pair(&solved, edge, wing);
                        if wing_orbit_depth(slot, n) == Some(depth) {
                            target_changed |= changed;
                        } else {
                            assert!(!changed, "n={n} d={depth}: changed non-target slot {slot}");
                        }
                    }
                }
                assert!(
                    target_changed,
                    "n={n} d={depth}: template had no target effect"
                );
                assert_eq!(
                    super::super::edges_det::orbit_pairs(&cube, depth),
                    canonical_defect_pairs(),
                    "n={n} d={depth}: non-canonical visible defect signature"
                );

                for mv in sequence.iter().rev() {
                    cube.apply_move(mv.inverse()).unwrap();
                }
                assert!(cube.is_solved(), "n={n} d={depth}: inverse replay failed");
            }
        }
    }

    #[test]
    fn canonical_visible_forms_scale_beyond_word_width() {
        for n in [66usize, 132] {
            let k = (n - 2) / 2;
            let masks = [
                vec![1usize, k],
                (1..=k).step_by(2).collect::<Vec<_>>(),
                (1..=k).collect::<Vec<_>>(),
            ];
            for depths in masks {
                let mut cube = StickerCube::solved(CubeSize::new(n).unwrap());
                for &depth in &depths {
                    for mv in orbit_parity_template(n, depth).unwrap() {
                        cube.apply_move(mv).unwrap();
                    }
                }
                let start = cube.clone();
                let correction = normalize_and_correct_wing_orbits(&mut cube)
                    .unwrap_or_else(|| panic!("canonical normalizer failed n={n} {depths:?}"));
                assert!(cube.is_solved(), "canonical normalizer unsolved n={n}");
                let mut replay = start;
                for mv in correction {
                    replay.apply_move(mv).unwrap();
                }
                assert!(replay.is_solved(), "canonical replay failed n={n}");
            }
        }
    }

    #[test]
    #[ignore = "large noncanonical orbit-local transport/resource gate; run in release mode"]
    fn noncanonical_n66_n132_orbit_transport_replays() {
        use super::super::edges::wing_base_cycles_for_depths;
        use super::super::{apply_all, centers_solved};

        for n in [66usize, 132] {
            let k = (n - 2) / 2;
            let mut cube = StickerCube::solved(CubeSize::new(n).unwrap());
            for depth in [1usize, k] {
                let cycles = wing_base_cycles_for_depths(n, &[depth]);
                apply_all(&mut cube, &cycles[depth % cycles.len()]);
            }
            assert!(centers_solved(&cube));
            assert!(!cube.is_solved());
            let start = cube.clone();
            let correction = normalize_and_correct_wing_orbits(&mut cube)
                .unwrap_or_else(|| panic!("N={n} noncanonical orbit transport coverage"));
            assert!(cube.is_solved());
            let mut replay = start;
            apply_all(&mut replay, &correction);
            assert!(replay.is_solved());
        }
    }

    #[test]
    fn direct_stall_identifies_orbit_and_reduction_recovers() {
        use super::super::edges_det::solve_edges_with_stall;
        use super::super::{centers_solved, slice_from, solve_centers};

        let n = 4;
        let mut parity = StickerCube::solved(CubeSize::new(n).unwrap());
        parity.apply_move(slice_from(Face::Right, n, 1, 1)).unwrap();
        solve_centers(&mut parity);
        assert!(centers_solved(&parity));
        let outcome = solve_edges_with_stall(&mut parity);
        let slot = outcome
            .stalled_slot
            .expect("inner slice should create wing parity");
        assert_eq!(wing_orbit_depth(slot, n), Some(1));

        let solver = Solver::new();
        let mut original = StickerCube::solved(CubeSize::new(n).unwrap());
        original
            .apply_move(slice_from(Face::Right, n, 1, 1))
            .unwrap();
        let start = original.clone();
        let solution = solve_reduction(&mut original, &solver).expect("direct orbit recovery");
        assert!(original.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());

        // Two independent even-cube wing orbits: correction must iterate by
        // reported stall depth rather than enumerate a two-bit powerset.
        let n = 6;
        let mut multi = StickerCube::solved(CubeSize::new(n).unwrap());
        for depth in [1usize, 2] {
            multi
                .apply_move(slice_from(Face::Right, n, depth, 1))
                .unwrap();
        }
        let start = multi.clone();
        let solution = solve_reduction(&mut multi, &solver).expect("multi-orbit recovery");
        assert!(multi.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    #[test]
    #[ignore = "large-N research gate; run explicitly in release mode"]
    fn direct_orbit_nine_recovery() {
        use super::super::slice_from;
        let n = 20;
        let mut cube = StickerCube::solved(CubeSize::new(n).unwrap());
        cube.apply_move(slice_from(Face::Right, n, 9, 1)).unwrap();
        let start = cube.clone();
        let solver = Solver::new();
        let solution = solve_reduction(&mut cube, &solver).expect("orbit-nine recovery");
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    #[test]
    #[ignore = "large-N sparse multi-orbit research gate; run explicitly in release mode"]
    fn sparse_orbits_one_and_nine_recovery() {
        use super::super::slice_from;
        let n = 20;
        let mut cube = StickerCube::solved(CubeSize::new(n).unwrap());
        for depth in [1usize, 9] {
            cube.apply_move(slice_from(Face::Right, n, depth, 1))
                .unwrap();
        }
        let start = cube.clone();
        let solver = Solver::new();
        let solution = solve_reduction(&mut cube, &solver).expect("sparse orbit recovery");
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    #[test]
    #[ignore = "large-N dense/alternating research gate; run explicitly in release mode"]
    fn dense_and_alternating_n20_recovery() {
        use super::super::slice_from;
        let n = 20;
        let solver = Solver::new();
        for depths in [
            (1usize..=9).step_by(2).collect::<Vec<_>>(),
            (1usize..=9).collect(),
        ] {
            let mut cube = StickerCube::solved(CubeSize::new(n).unwrap());
            for &depth in &depths {
                cube.apply_move(slice_from(Face::Right, n, depth, 1))
                    .unwrap();
            }
            let start = cube.clone();
            let solution = solve_reduction(&mut cube, &solver)
                .unwrap_or_else(|| panic!("N=20 orbit recovery failed for {depths:?}"));
            assert!(cube.is_solved(), "N=20 not solved for {depths:?}");
            let mut replay = start;
            for mv in solution {
                replay.apply_move(mv).unwrap();
            }
            assert!(replay.is_solved(), "N=20 replay failed for {depths:?}");
        }
    }

    #[test]
    fn stalled_slots_map_to_every_dynamic_wing_orbit() {
        for n in [4usize, 5, 20, 66, 132] {
            let k = (n - 2) / 2;
            let depths: std::collections::HashSet<usize> = (0..12 * (n - 2))
                .filter_map(|slot| wing_orbit_depth(slot, n))
                .collect();
            assert_eq!(depths.len(), k, "n={n}");
            assert!(depths.contains(&1));
            assert!(depths.contains(&k));
        }
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
            let Some(cc) = sticker_to_cubie_unchecked(&extract_3x3(&cube)) else {
                println!("seed {seed}: malformed reduced piece set");
                continue;
            };
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

    /// Robustness stress test: a fresh, unseen seed range and DEEPER scrambles across all
    /// sizes, validated by replaying the returned move list to solved. Confirms the solver
    /// is reliable beyond the asserted `full_solve_sizes` seeds.
    #[test]
    #[ignore = "slow; run explicitly"]
    fn stress_reliability() {
        let solver = Solver::new();
        let mut total_fail = 0;
        for n in [4usize, 5, 6, 7, 8] {
            let mut warm = scramble(n, 0x9000, n * 20);
            let _ = solve_reduction(&mut warm, &solver);
            let (mut ok, mut fails) = (0u64, Vec::new());
            let trials: u64 = if n <= 6 { 30 } else { 15 };
            for seed in 0..trials {
                let mut cube = scramble(n, 0x9000 + seed, n * 20);
                let fresh = cube.clone();
                let solved = match solve_reduction(&mut cube, &solver) {
                    Some(moves) => {
                        // replay-validate the returned (simplified) move list
                        let mut chk = fresh.clone();
                        for &m in &moves {
                            chk.apply_move(m).unwrap();
                        }
                        cube.is_solved() && chk.is_solved()
                    }
                    None => false,
                };
                if solved {
                    ok += 1;
                } else {
                    fails.push(seed);
                }
            }
            total_fail += fails.len();
            eprintln!("n={n}: {ok}/{trials} (depth {}); fails {fails:?}", n * 20);
        }
        assert_eq!(total_fail, 0, "stress test found reliability gaps");
    }

    /// Bounded first size above the product ceiling: full centers→edges→3×3 reduction,
    /// controlled by the same 290-second internal deadline as the largest WASM route.
    #[test]
    #[ignore = "large end-to-end research/resource gate; run explicitly in release mode"]
    fn controlled_n12_end_to_end_replays() {
        use std::time::Duration;

        let n = 12usize;
        let solver = Solver::new();
        let mut cube = scramble(n, 0xC000, n * 12);
        let start = cube.clone();
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(290));
        let solution = solve_reduction_with_control(&mut cube, &solver, &control)
            .expect("N=12 controlled end-to-end reduction");
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    /// Larger bounded full-reduction research gate after sparse parity and isolated
    /// edge transport: randomized wide turns disturb both centers and wing orbits.
    #[test]
    #[ignore = "N=20 end-to-end research/resource gate; run explicitly in release mode"]
    fn controlled_n20_end_to_end_replays() {
        use std::time::Duration;

        let n = 20usize;
        let solver = Solver::new();
        let mut cube = scramble(n, 0xD000, n * 4);
        let start = cube.clone();
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(290));
        let solution = solve_reduction_with_control(&mut cube, &solver, &control)
            .expect("N=20 controlled end-to-end reduction");
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    /// Additional full N=20 seeds share the expensive per-size libraries within one
    /// process and replay every returned path. This is the reliability companion to
    /// the single measured resource gate above.
    #[test]
    #[ignore = "multi-seed N=20 end-to-end reliability gate; run in release mode"]
    fn controlled_n20_additional_seeds_replay() {
        use std::time::Duration;

        let n = 20usize;
        let solver = Solver::new();
        for seed in [0xD001u64, 0xD002] {
            let mut cube = scramble(n, seed, n * 4);
            let start = cube.clone();
            let control = super::super::ReductionControl::with_timeout(Duration::from_secs(290));
            let solution = solve_reduction_with_control(&mut cube, &solver, &control)
                .unwrap_or_else(|error| panic!("N=20 seed {seed:#x}: {error:?}"));
            assert!(cube.is_solved(), "N=20 seed {seed:#x} not solved");
            let mut replay = start;
            for mv in solution {
                replay.apply_move(mv).unwrap();
            }
            assert!(replay.is_solved(), "N=20 seed {seed:#x} replay failed");
        }
    }

    /// Isolate and replay-verify the N=24 center stage before edge/parity recovery.
    #[test]
    #[ignore = "N=24 center-stage research gate; run in release mode"]
    fn controlled_n24_centers_solve() {
        use super::super::{centers_solved, solve_centers};
        use std::time::Duration;

        let n = 24usize;
        let mut cube = scramble(n, 0xE000, n * 3);
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(300));
        let moves = super::super::with_reduction_control(&control, || solve_centers(&mut cube));
        assert!(control.should_continue(), "N=24 center stage timed out");
        let mut replay = scramble(n, 0xE000, n * 3);
        for mv in moves {
            replay.apply_move(mv).unwrap();
        }
        assert_eq!(replay.clone_snapshot(), cube.clone_snapshot());
        assert!(centers_solved(&cube), "N=24 center stage stalled");
    }

    /// First full center+edge replay gate beyond N=20. Kept shallower to bound
    /// research cost while still mixing randomized outer and wide turns.
    #[test]
    #[ignore = "N=24 end-to-end research/resource gate; run in release mode"]
    fn controlled_n24_end_to_end_replays() {
        use std::time::Duration;

        let n = 24usize;
        let solver = Solver::new();
        let mut cube = scramble(n, 0xE000, n * 3);
        let start = cube.clone();
        // N=24 is research-only and not constrained by the app's 300-second
        // watchdog; callers can supply a resource budget appropriate to their hardware.
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(600));
        let solution = solve_reduction_with_control(&mut cube, &solver, &control)
            .unwrap_or_else(|error| panic!("N=24 end-to-end reduction: {error:?}"));
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    /// Independent N=24 reliability seeds share the expensive center/edge
    /// libraries and strictly replay every returned legal-move path.
    #[test]
    #[ignore = "multi-seed N=24 end-to-end reliability gate; run in release mode"]
    fn controlled_n24_additional_seeds_replay() {
        use std::time::Duration;

        let n = 24usize;
        let solver = Solver::new();
        for seed in [0xE001u64, 0xE002] {
            let mut cube = scramble(n, seed, n * 3);
            let start = cube.clone();
            let control = super::super::ReductionControl::with_timeout(Duration::from_secs(600));
            let solution = solve_reduction_with_control(&mut cube, &solver, &control)
                .unwrap_or_else(|error| panic!("N=24 seed {seed:#x}: {error:?}"));
            assert!(cube.is_solved(), "N=24 seed {seed:#x} not solved");
            let mut replay = start;
            for mv in solution {
                replay.apply_move(mv).unwrap();
            }
            assert!(replay.is_solved(), "N=24 seed {seed:#x} replay failed");
        }
    }

    /// First strict full replay/resource gate beyond the established N=24 corpus.
    #[test]
    #[ignore = "N=28 end-to-end research/resource gate; run in release mode"]
    fn controlled_n28_end_to_end_replays() {
        use std::time::Duration;

        let n = 28usize;
        let solver = Solver::new();
        let mut cube = scramble(n, 0xF000, n * 3);
        let start = cube.clone();
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(900));
        let solution = solve_reduction_with_control(&mut cube, &solver, &control)
            .unwrap_or_else(|error| panic!("N=28 end-to-end reduction: {error:?}"));
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    /// One independent N=28 reliability seed, kept separate from the measured
    /// resource gate so either command remains externally rerunnable.
    #[test]
    #[ignore = "additional N=28 end-to-end reliability gate; run in release mode"]
    fn controlled_n28_additional_seed_replays() {
        use std::time::Duration;

        let n = 28usize;
        let seed = 0xF001u64;
        let solver = Solver::new();
        let mut cube = scramble(n, seed, n * 3);
        let start = cube.clone();
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(900));
        let solution = solve_reduction_with_control(&mut cube, &solver, &control)
            .unwrap_or_else(|error| panic!("N=28 seed {seed:#x}: {error:?}"));
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    /// Isolate and replay-verify N=32 centers before later edge/parity work.
    #[test]
    #[ignore = "N=32 center-stage research gate; run in release mode"]
    fn controlled_n32_centers_solve() {
        use super::super::{centers_solved, solve_centers};
        use std::time::Duration;

        let n = 32usize;
        let mut cube = scramble(n, 0xF100, n * 3);
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(600));
        let moves = super::super::with_reduction_control(&control, || solve_centers(&mut cube));
        let mut replay = scramble(n, 0xF100, n * 3);
        for mv in moves {
            replay.apply_move(mv).unwrap();
        }
        assert_eq!(replay.clone_snapshot(), cube.clone_snapshot());
        assert!(control.should_continue(), "N=32 center stage timed out");
        assert!(centers_solved(&cube), "N=32 center stage stalled");
    }

    /// Strict full N=32 replay/resource gate beyond the N=28 corpus.
    #[test]
    #[ignore = "N=32 end-to-end research/resource gate; run in release mode"]
    fn controlled_n32_end_to_end_replays() {
        use std::time::Duration;

        let n = 32usize;
        let solver = Solver::new();
        let mut cube = scramble(n, 0xF100, n * 3);
        let start = cube.clone();
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(1200));
        let solution = solve_reduction_with_control(&mut cube, &solver, &control)
            .unwrap_or_else(|error| panic!("N=32 end-to-end reduction: {error:?}"));
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    /// Independent N=32 reliability seed after center-build optimizations.
    #[test]
    #[ignore = "additional N=32 end-to-end reliability gate; run in release mode"]
    fn controlled_n32_additional_seed_replays() {
        use std::time::Duration;

        let n = 32usize;
        let seed = 0xF101u64;
        let solver = Solver::new();
        let mut cube = scramble(n, seed, n * 3);
        let start = cube.clone();
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(1200));
        let solution = solve_reduction_with_control(&mut cube, &solver, &control)
            .unwrap_or_else(|error| panic!("N=32 seed {seed:#x}: {error:?}"));
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    /// Strict full replay/resource gate beyond N=32.
    #[test]
    #[ignore = "N=36 end-to-end research/resource gate; run in release mode"]
    fn controlled_n36_end_to_end_replays() {
        use std::time::Duration;

        let n = 36usize;
        let solver = Solver::new();
        let mut cube = scramble(n, 0xF200, n * 3);
        let start = cube.clone();
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(1500));
        let solution = solve_reduction_with_control(&mut cube, &solver, &control)
            .unwrap_or_else(|error| panic!("N=36 end-to-end reduction: {error:?}"));
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    /// Independent N=36 reliability seed under the compact center representation.
    #[test]
    #[ignore = "additional N=36 end-to-end reliability gate; run in release mode"]
    fn controlled_n36_additional_seed_replays() {
        use std::time::Duration;

        let n = 36usize;
        let seed = 0xF201u64;
        let solver = Solver::new();
        let mut cube = scramble(n, seed, n * 3);
        let start = cube.clone();
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(1500));
        let solution = solve_reduction_with_control(&mut cube, &solver, &control)
            .unwrap_or_else(|error| panic!("N=36 seed {seed:#x}: {error:?}"));
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    /// Strict full replay/resource gate beyond N=36.
    #[test]
    #[ignore = "N=40 end-to-end research/resource gate; run in release mode"]
    fn controlled_n40_end_to_end_replays() {
        use std::time::Duration;

        let n = 40usize;
        let solver = Solver::new();
        let mut cube = scramble(n, 0xF300, n * 3);
        let start = cube.clone();
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(1800));
        let solution = solve_reduction_with_control(&mut cube, &solver, &control)
            .unwrap_or_else(|error| panic!("N=40 end-to-end reduction: {error:?}"));
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    /// Independent N=40 reliability seed under compact center effects.
    #[test]
    #[ignore = "additional N=40 end-to-end reliability gate; run in release mode"]
    fn controlled_n40_additional_seed_replays() {
        use std::time::Duration;

        let n = 40usize;
        let seed = 0xF301u64;
        let solver = Solver::new();
        let mut cube = scramble(n, seed, n * 3);
        let start = cube.clone();
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(1800));
        let solution = solve_reduction_with_control(&mut cube, &solver, &control)
            .unwrap_or_else(|error| panic!("N=40 seed {seed:#x}: {error:?}"));
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    /// Strict full replay/resource gate beyond N=40.
    #[test]
    #[ignore = "N=44 end-to-end research/resource gate; run in release mode"]
    fn controlled_n44_end_to_end_replays() {
        use std::time::Duration;

        let n = 44usize;
        let solver = Solver::new();
        let mut cube = scramble(n, 0xF400, n * 3);
        let start = cube.clone();
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(2100));
        let solution = solve_reduction_with_control(&mut cube, &solver, &control)
            .unwrap_or_else(|error| panic!("N=44 end-to-end reduction: {error:?}"));
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    /// Independent N=44 reliability seed at the current full-solve frontier.
    #[test]
    #[ignore = "additional N=44 end-to-end reliability gate; run in release mode"]
    fn controlled_n44_additional_seed_replays() {
        use std::time::Duration;

        let n = 44usize;
        let seed = 0xF401u64;
        let solver = Solver::new();
        let mut cube = scramble(n, seed, n * 3);
        let start = cube.clone();
        let control = super::super::ReductionControl::with_timeout(Duration::from_secs(2100));
        let solution = solve_reduction_with_control(&mut cube, &solver, &control)
            .unwrap_or_else(|error| panic!("N=44 seed {seed:#x}: {error:?}"));
        assert!(cube.is_solved());
        let mut replay = start;
        for mv in solution {
            replay.apply_move(mv).unwrap();
        }
        assert!(replay.is_solved());
    }

    /// "Solve ANYTHING": the large sizes 9–11 (both parities), replay-validated — proving
    /// the method keeps generalising past the asserted 8×8. (12×12 was also verified to
    /// solve reliably, but a hard seed takes ~5 min — the parity search runs 2^⌊(n-2)/2⌋
    /// masks, each a full re-reduction, which is costly at large even sizes — so it's left
    /// out of the routine assert.) Few seeds each because every size pays a one-time build.
    #[test]
    #[ignore = "very slow; run explicitly"]
    fn big_sizes() {
        let solver = Solver::new();
        for n in [9usize, 10, 11] {
            let (mut ok, mut fails) = (0u64, Vec::new());
            for seed in 0..3u64 {
                let mut cube = scramble(n, 0xB000 + seed, n * 18);
                let fresh = cube.clone();
                let solved = match solve_reduction(&mut cube, &solver) {
                    Some(moves) => {
                        let mut chk = fresh.clone();
                        for &m in &moves {
                            chk.apply_move(m).unwrap();
                        }
                        cube.is_solved() && chk.is_solved()
                    }
                    None => false,
                };
                if solved {
                    ok += 1;
                } else {
                    fails.push(seed);
                }
            }
            eprintln!("n={n}: {ok}/3 solved; fails {fails:?}");
            assert!(fails.is_empty(), "n={n} failed seeds {fails:?}");
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

    /// Solution length (move count) per size — a quality dimension. The reduction method
    /// isn't optimal, and disturbances/re-reductions inflate it; this checks it's sane.
    #[test]
    #[ignore = "diagnostic"]
    fn solution_length() {
        let solver = Solver::new();
        for n in [4usize, 5, 6, 7, 8] {
            let mut warm = scramble(n, 0x4ff, n * 15);
            let _ = solve_reduction(&mut warm, &solver);
            let (mut sum, mut max, mut cnt) = (0usize, 0usize, 0usize);
            for seed in 0..8u64 {
                let mut cube = scramble(n, 0x500 + seed, n * 15);
                let fresh = cube.clone();
                if let Some(moves) = solve_reduction(&mut cube, &solver) {
                    // Validate the (simplified) move list actually solves a fresh scramble.
                    let mut check = fresh.clone();
                    for &m in &moves {
                        check.apply_move(m).unwrap();
                    }
                    assert!(
                        check.is_solved(),
                        "n={n} seed {seed}: returned moves don't solve"
                    );
                    sum += moves.len();
                    max = max.max(moves.len());
                    cnt += 1;
                }
            }
            eprintln!(
                "n={n}: avg {} moves, max {max} (over {cnt} solves)",
                sum / cnt.max(1)
            );
        }
    }

    /// Per-seed timing for n=8 and whether the centres stalled on the first pass (the
    /// remaining slow path). Shows the distribution after the bitmask parity search.
    #[test]
    #[ignore = "timing"]
    fn timing_n8() {
        use super::super::{centers_solved, solve_centers};
        let solver = Solver::new();
        let n = 8usize;
        for seed in 0..15u64 {
            let mut probe = scramble(n, 0x500 + seed, n * 15);
            solve_centers(&mut probe);
            let centre_stall = !centers_solved(&probe);
            let mut cube = scramble(n, 0x500 + seed, n * 15);
            let t0 = std::time::Instant::now();
            let ok = solve_reduction(&mut cube, &solver).is_some() && cube.is_solved();
            eprintln!(
                "seed {seed}: {:?} solved={ok} centre_stall={centre_stall}",
                t0.elapsed()
            );
        }
    }

    /// End-to-end across sizes 4–8: even cubes exercise the deterministic parity path, odd
    /// cubes the parity-free path; both rely on disturbance recovery when the centre solver
    /// stalls on a big cube. Verified fully solved by replay; all of 4×4–8×8 are reliable.
    #[test]
    #[ignore = "slow; run explicitly"]
    fn full_solve_sizes() {
        let solver = Solver::new();
        for n in [4usize, 5, 6, 7, 8] {
            // Warm the per-size libraries (one-time ~5–6 s build for big cubes) so the
            // reported time below is honest *per-solve*, not build-inflated.
            let mut warm = scramble(n, 0x4ff, n * 15);
            let _ = solve_reduction(&mut warm, &solver);

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
                "n={n} full solve: {solved}/{trials} in {:?} ({:?}/solve, libs warm); fails {fails:?}",
                t0.elapsed(),
                t0.elapsed() / trials as u32
            );
            assert_eq!(solved, trials, "n={n} not fully reliable: fails {fails:?}");
        }
    }
}
