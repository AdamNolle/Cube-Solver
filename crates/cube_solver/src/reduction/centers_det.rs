//! Deterministic center solver for arbitrary NxN cubes.
//!
//! Centre pieces are fungible (identified by colour only), so there is no
//! permutation parity to get stuck on — the only requirement is *reach*: for every
//! cell we must be able to drop the right-coloured piece in without disturbing
//! already-finalised work. We get reach from a precomputed library of centre-only
//! cycles and place pieces by *apply-and-check* (try a safe cycle, keep it if it
//! makes the target cell correct without regressing the working face).
//!
//! The hard part is the last two centres: once four faces are frozen, a centre
//! piece can only travel between the two remaining opposite faces by passing
//! through a frozen face and restoring it. The sequences that do this (net effect
//! confined to two opposite faces) are *meta-commutators* `[P, Q]` of two ordinary
//! centre cycles — we generate those, re-aim them with face turns, and add them to
//! the library. A frozen face is solid, so such a cycle may churn it freely as long
//! as it restores it (checked via the exact permutation).
//!
//! Exact directed permutations are recovered by labelling every centre cell with a
//! unique base-6 id across a few probe cubes (built by snapshot deserialisation,
//! which cube_core does not validate) and reading the id back after a move — robust
//! despite same-colour ambiguity. Each elementary move's permutation is decoded
//! once and cycles are composed from them.
//!
//! STATUS: **the 4×4 is solved and verified** (30 random wide scrambles →
//! centres-solved, ~5 s one-time library build, ~30 ms/solve). N ≥ 5 now completes
//! (the bridge predicts results by composing permutations instead of cloning the
//! cube, so it no longer grinds) but coverage is still partial — n=5 solves ~2/6
//! random scrambles; the bigger, multi-orbit last-two-centres band (X- and
//! +-centres on odd cubes) needs more confined-cycle coverage, and n≥6 still needs
//! a faster library build. Work in progress; not yet wired into the app.

use super::centers::{cube_rotations, orient_fixed_centers};
use super::{apply_all, commutator, conjugate, is_center_cell, slice_from};
use cube_core::{Color, CubeSize, CubeState, Face, Move, StickerCube};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

type Cell = (Face, usize, usize);

fn face_ord(f: Face) -> usize {
    Face::ALL.iter().position(|&x| x == f).unwrap()
}

fn face_center_cells(face: Face, n: usize) -> Vec<Cell> {
    let mut v = Vec::new();
    for r in 0..n {
        for c in 0..n {
            if is_center_cell(r, c, n) {
                v.push((face, r, c));
            }
        }
    }
    v
}

fn is_odd(n: usize) -> bool {
    n % 2 == 1
}

/// All centre cells across the six faces, in `Face::ALL` × row × col order.
fn all_center_cells(n: usize) -> Vec<Cell> {
    let mut v = Vec::new();
    for &f in &Face::ALL {
        for r in 0..n {
            for c in 0..n {
                if is_center_cell(r, c, n) {
                    v.push((f, r, c));
                }
            }
        }
    }
    v
}

fn color_at(cube: &StickerCube, x: Cell) -> Color {
    cube.color_at(x.0, x.1, x.2).unwrap()
}

const COLOR_NAMES: [&str; 6] = ["White", "Yellow", "Green", "Blue", "Orange", "Red"];

fn color_index(c: Color) -> usize {
    match c {
        Color::White => 0,
        Color::Yellow => 1,
        Color::Green => 2,
        Color::Blue => 3,
        Color::Orange => 4,
        Color::Red => 5,
    }
}

/// Probe cubes that label every centre cell with a unique base-6 id (one cube per
/// digit). Built once via snapshot deserialisation (cube_core does not validate),
/// then cloned per sequence to recover exact permutations cheaply.
fn build_probes(n: usize, centers: &[Cell]) -> Vec<StickerCube> {
    let ndigits = {
        let mut d = 1;
        let mut cap = 6usize;
        while cap < centers.len() {
            cap *= 6;
            d += 1;
        }
        d
    };
    (0..ndigits)
        .map(|k| {
            let mut names: Vec<&str> = vec![COLOR_NAMES[0]; 6 * n * n];
            for (id, &(f, r, c)) in centers.iter().enumerate() {
                let digit = (id / 6usize.pow(k as u32)) % 6;
                names[face_ord(f) * n * n + r * n + c] = COLOR_NAMES[digit];
            }
            let snap: cube_core::CubeSnapshot =
                serde_json::from_value(serde_json::json!({ "size": n, "stickers": names }))
                    .expect("probe snapshot");
            StickerCube::from_snapshot(snap)
        })
        .collect()
}

/// Exact centre-cell permutation of `seq` restricted to `support`: maps each
/// destination cell to the cell whose piece lands there.
fn center_perm(
    centers: &[Cell],
    probes: &[StickerCube],
    support: &[Cell],
    seq: &[Move],
) -> HashMap<Cell, Cell> {
    let applied: Vec<StickerCube> = probes
        .iter()
        .map(|p| {
            let mut c = p.clone();
            apply_all(&mut c, seq);
            c
        })
        .collect();
    let mut map = HashMap::new();
    for &dst in support {
        let mut id = 0usize;
        for (k, probe) in applied.iter().enumerate() {
            id += color_index(color_at(probe, dst)) * 6usize.pow(k as u32);
        }
        if id < centers.len() {
            map.insert(dst, centers[id]);
        }
    }
    map
}

/// Compact key identifying an elementary move.
type MoveKey = (u8, usize, usize, i8);
fn move_key(m: &Move) -> MoveKey {
    let axis = match m.axis {
        cube_core::Axis::X => 0,
        cube_core::Axis::Y => 1,
        cube_core::Axis::Z => 2,
    };
    (axis, m.layer_start, m.layer_end, m.turns)
}

/// Centre-cell permutation of a single move as an index map over `centers`:
/// `perm[dst] = src` (the centre cell whose piece lands at `dst`).
fn single_move_perm(centers: &[Cell], probes: &[StickerCube], m: Move) -> Vec<usize> {
    let applied: Vec<StickerCube> = probes
        .iter()
        .map(|p| {
            let mut c = p.clone();
            c.apply_move(m).expect("valid move");
            c
        })
        .collect();
    let mut perm = vec![0usize; centers.len()];
    for (i, &dst) in centers.iter().enumerate() {
        let mut id = 0usize;
        for (k, probe) in applied.iter().enumerate() {
            id += color_index(color_at(probe, dst)) * 6usize.pow(k as u32);
        }
        perm[i] = if id < centers.len() { id } else { i };
    }
    perm
}

/// Compose index permutations: `(a∘b)[i] = a[b[i]]`.
fn compose(a: &[usize], b: &[usize]) -> Vec<usize> {
    b.iter().map(|&bi| a[bi]).collect()
}

/// Permutation of a whole sequence by composing precomputed single-move perms.
fn seq_perm(
    move_perms: &HashMap<MoveKey, Vec<usize>>,
    ncenters: usize,
    seq: &[Move],
) -> Vec<usize> {
    let mut r: Vec<usize> = (0..ncenters).collect();
    for m in seq {
        r = compose(&r, &move_perms[&move_key(m)]);
    }
    r
}

/// A centre-only cycle with its exact directed permutation.
struct Cyc {
    moves: Vec<Move>,
    support: Vec<Cell>,
    /// destination cell -> source cell (piece at `src` moves to `dst`).
    perm: HashMap<Cell, Cell>,
}

impl Cyc {
    /// The cell whose piece moves into `t` when this cycle is applied.
    fn src_into(&self, t: Cell) -> Option<Cell> {
        self.perm.get(&t).copied()
    }
}

/// Centre cells whose colour differs from home after applying `seq` to a solved cube.
fn changed_center_cells(solved: &StickerCube, n: usize, seq: &[Move]) -> Vec<Cell> {
    let mut c = solved.clone();
    apply_all(&mut c, seq);
    let mut out = Vec::new();
    for &f in &Face::ALL {
        let home = f.color();
        for r in 0..n {
            for col in 0..n {
                if is_center_cell(r, col, n) && c.color_at(f, r, col) != Some(home) {
                    out.push((f, r, col));
                }
            }
        }
    }
    out
}

/// Base candidate sequences: `[mover, face]` and `[slice, slice]` commutators,
/// re-aimed by face turns and the 24 cube rotations.
fn base_candidates(n: usize) -> Vec<Vec<Move>> {
    let size = CubeSize::new(n).expect("size>=2");
    let mut movers: Vec<Move> = Vec::new();
    for f in Face::ALL {
        for d in 1..=n - 2 {
            for s in [1i8, -1] {
                movers.push(slice_from(f, n, d, s));
            }
        }
        for w in 2..=(n - 1).min(3) {
            for s in [1i8, -1] {
                movers.push(Move::wide(f, size, w, s));
            }
        }
    }
    let faces: Vec<Move> = Face::ALL
        .iter()
        .flat_map(|&f| [1i8, -1].into_iter().map(move |t| Move::face(f, size, t)))
        .collect();
    let rots = cube_rotations(n);

    let mut out: Vec<Vec<Move>> = Vec::new();
    for m in &movers {
        for f in &faces {
            let base = commutator(&[*m], &[*f]);
            out.push(base.clone());
            for s in &faces {
                out.push(conjugate(&[*s], &base));
            }
            for r in &rots {
                if !r.is_empty() {
                    out.push(conjugate(r, &base));
                }
            }
        }
    }
    for (i, a) in movers.iter().enumerate() {
        for b in &movers[i + 1..] {
            if a.axis == b.axis {
                continue;
            }
            let base = commutator(&[*a], &[*b]);
            out.push(base.clone());
            for f in &faces {
                out.push(conjugate(&[*f], &base));
            }
        }
    }
    out
}

/// Build the cycle library: base cycles (deduped by full effect) plus
/// meta-commutators `[P, Q]` whose net centre effect is confined to ≤2 faces — the
/// last-two-centres tools. Sorted shortest-first.
fn build_library(n: usize) -> Vec<Cyc> {
    let solved = StickerCube::solved(CubeSize::new(n).expect("size>=2"));
    let solved_keys = solved.clone_snapshot().stickers().to_vec();
    let mut by_effect: HashMap<Vec<Color>, Vec<Move>> = HashMap::new();

    let consider = |seq: Vec<Move>, by_effect: &mut HashMap<Vec<Color>, Vec<Move>>| {
        let mut c = solved.clone();
        apply_all(&mut c, &seq);
        let eff = c.clone_snapshot().stickers().to_vec();
        if eff == solved_keys {
            return;
        }
        match by_effect.get(&eff) {
            Some(prev) if prev.len() <= seq.len() => {}
            _ => {
                by_effect.insert(eff, seq);
            }
        }
    };

    // Base cycles.
    let base = base_candidates(n);
    for seq in &base {
        consider(seq.clone(), &mut by_effect);
    }

    // Meta-commutators for the last two centres. Use the shortest base cycles that
    // actually move centres; keep metas whose net centre effect spans ≤2 faces.
    let mut short: Vec<Vec<Move>> = base
        .iter()
        .filter(|s| !changed_center_cells(&solved, n, s).is_empty())
        .cloned()
        .collect();
    short.sort_by_key(|s| s.len());
    short.dedup();
    short.truncate(200);
    let size = CubeSize::new(n).expect("size>=2");
    let face_turns: Vec<Move> = Face::ALL
        .iter()
        .flat_map(|&f| {
            [1i8, -1, 2]
                .into_iter()
                .map(move |t| Move::face(f, size, t))
        })
        .collect();
    for p in &short {
        for q in &short {
            let meta = commutator(p, q);
            let cells = changed_center_cells(&solved, n, &meta);
            if cells.is_empty() {
                continue;
            }
            let faces: HashSet<Face> = cells.iter().map(|c| c.0).collect();
            if faces.len() <= 2 {
                // Re-aim each confined meta with a face turn so every target/source
                // cell pair within the last two faces is covered (a conjugate of a
                // confined cycle is still confined to the rotated faces).
                consider(meta.clone(), &mut by_effect);
                for ft in &face_turns {
                    consider(conjugate(&[*ft], &meta), &mut by_effect);
                }
            }
        }
    }

    let centers = all_center_cells(n);
    // Reduce each distinct effect to (moves, support) cheaply (no perm yet).
    let support_of = |eff: &[Color]| -> Vec<Cell> {
        let mut support = Vec::new();
        for &f in &Face::ALL {
            for r in 0..n {
                for col in 0..n {
                    if is_center_cell(r, col, n)
                        && eff[face_ord(f) * n * n + r * n + col] != f.color()
                    {
                        support.push((f, r, col));
                    }
                }
            }
        }
        support
    };
    let mut raw: Vec<(Vec<Move>, Vec<Cell>)> = by_effect
        .into_iter()
        .map(|(eff, moves)| {
            let s = support_of(&eff);
            (moves, s)
        })
        .filter(|(_, s)| !s.is_empty())
        .collect();
    // Keep a surgical, well-covering set: prefer few-cell, short cycles. "Confined"
    // cycles (≤2 faces) are the last-two-centres tools; keep generously. Perms are
    // computed only for the kept set (deterministic placement is a lookup, so a
    // larger library costs build time, not solve time).
    let n_faces = |s: &[Cell]| s.iter().map(|x| x.0).collect::<HashSet<_>>().len();
    // Fully deterministic order (HashMap iteration is not), so the library — and
    // therefore coverage — is identical every run.
    let size = CubeSize::new(n).expect("size>=2");
    let sup_key = |s: &[Cell]| -> Vec<(usize, usize, usize)> {
        let mut v: Vec<_> = s.iter().map(|c| (face_ord(c.0), c.1, c.2)).collect();
        v.sort();
        v
    };
    let mv_key = |m: &[Move]| -> Vec<String> { m.iter().map(|x| x.notation(size)).collect() };
    raw.sort_by(|(ma, sa), (mb, sb)| {
        (sa.len(), ma.len())
            .cmp(&(sb.len(), mb.len()))
            .then_with(|| sup_key(sa).cmp(&sup_key(sb)))
            .then_with(|| mv_key(ma).cmp(&mv_key(mb)))
    });
    // Keep EVERY confined (≤2-face) cycle — those are the scarce last-two-centres
    // tools — and a generous cap of general cycles for the first four faces.
    let cap_gen = 7000usize;
    let mut kept: Vec<(Vec<Move>, Vec<Cell>)> = Vec::new();
    let mut ngen = 0;
    for (m, s) in raw {
        if n_faces(&s) <= 2 {
            kept.push((m, s));
        } else if ngen < cap_gen {
            ngen += 1;
            kept.push((m, s));
        }
    }

    // Precompute each distinct elementary move's centre permutation once, then get
    // every cycle's permutation by composing them (array ops) — far cheaper than
    // re-applying each cycle to probe cubes.
    let probes = build_probes(n, &centers);
    let mut move_perms: HashMap<MoveKey, Vec<usize>> = HashMap::new();
    for (moves, _) in &kept {
        for m in moves {
            move_perms
                .entry(move_key(m))
                .or_insert_with(|| single_move_perm(&centers, &probes, *m));
        }
    }
    let ncenters = centers.len();
    let mut out: Vec<Cyc> = kept
        .into_iter()
        .map(|(moves, _approx_support)| {
            let p = seq_perm(&move_perms, ncenters, &moves);
            let mut support = Vec::new();
            let mut perm = HashMap::new();
            for (i, &src_i) in p.iter().enumerate() {
                if src_i != i {
                    support.push(centers[i]);
                    perm.insert(centers[i], centers[src_i]);
                }
            }
            Cyc {
                moves,
                support,
                perm,
            }
        })
        .filter(|c| !c.support.is_empty())
        .collect();
    out.sort_by_key(|c| (c.support.len(), c.moves.len()));
    out
}

thread_local! {
    static LIB_CACHE: RefCell<HashMap<usize, std::rc::Rc<Vec<Cyc>>>> = RefCell::new(HashMap::new());
}

fn library(n: usize) -> std::rc::Rc<Vec<Cyc>> {
    LIB_CACHE.with(|c| {
        c.borrow_mut()
            .entry(n)
            .or_insert_with(|| std::rc::Rc::new(build_library(n)))
            .clone()
    })
}

fn correct_count(cube: &StickerCube, w_cells: &[Cell], want: Color) -> usize {
    w_cells
        .iter()
        .filter(|&&x| color_at(cube, x) == want)
        .count()
}

/// Solve all six centres. Returns the moves; the cube is left centres-solved.
pub fn solve_centers(cube: &mut StickerCube) -> Vec<Move> {
    let n = cube.size().get();
    let mut moves = Vec::new();
    if n <= 2 {
        return moves;
    }
    moves.extend(orient_fixed_centers(cube));

    let lib = library(n);
    let dbg = std::env::var("RDBG").is_ok();
    if dbg {
        eprintln!("[cd] n={n} library={}", lib.len());
    }
    let order = [
        Face::Up,
        Face::Down,
        Face::Front,
        Face::Back,
        Face::Left,
        Face::Right,
    ];
    let fixed: Vec<Cell> = if is_odd(n) {
        let mid = n / 2;
        Face::ALL.iter().map(|&f| (f, mid, mid)).collect()
    } else {
        Vec::new()
    };

    for fi in 0..order.len() {
        let w = order[fi];
        let want = w.color();
        let mut frozen: HashSet<Cell> = HashSet::new();
        for &ff in &order[..fi] {
            for cell in face_center_cells(ff, n) {
                frozen.insert(cell);
            }
        }
        for &c in &fixed {
            frozen.insert(c);
        }
        let w_cells: Vec<Cell> = face_center_cells(w, n);
        let finalized: HashSet<Face> = order[..fi].iter().copied().collect();

        // A cycle is safe for this face iff every frozen cell it permutes ends up the
        // same colour. A finalised face is solid, so its cells may be churned *within
        // that face* and still come out correct — this is exactly how the
        // last-two-centres meta-commutators work (they disturb the solved band and
        // restore it). A rigid fixed centre (odd cubes) must not move at all.
        let safe: Vec<&Cyc> = lib
            .iter()
            .filter(|c| {
                c.perm.iter().all(|(&dst, &src)| {
                    if finalized.contains(&dst.0) {
                        src.0 == dst.0 // finalised-face cell stays on its face
                    } else {
                        !fixed.contains(&dst) // a rigid fixed centre must not move
                    }
                })
            })
            .collect();
        let _ = &frozen;
        if dbg {
            eprintln!("[cd] face {w:?} (#{fi}): safe={}", safe.len());
        }

        // Cycles touching a given cell, indexed for O(1) lookup.
        let mut touch: HashMap<Cell, Vec<&Cyc>> = HashMap::new();
        for cy in &safe {
            for &cell in &cy.support {
                touch.entry(cell).or_default().push(cy);
            }
        }
        let w_set: HashSet<Cell> = w_cells.iter().copied().collect();

        let cap = n * n * 8 + 100;
        let mut guard = 0usize;
        let mut iters = 0usize;
        loop {
            let cc = correct_count(cube, &w_cells, want);
            if cc == w_cells.len() {
                if dbg {
                    eprintln!("[cd]   face {w:?} solved in {iters} iters");
                }
                break;
            }
            iters += 1;
            if dbg && iters.is_multiple_of(50) {
                eprintln!(
                    "[cd]   face {w:?}: iter {iters} correct={cc}/{} guard={guard}",
                    w_cells.len()
                );
            }
            let t = *w_cells
                .iter()
                .find(|&&x| color_at(cube, x) != want)
                .unwrap();

            // Direct placement: a cycle that drops a `want` piece into `t` from the
            // reservoir without breaking any already-correct cell of W. Pure lookup.
            let ok_other = |cy: &Cyc, extra_t: Cell| {
                cy.support
                    .iter()
                    .all(|&x| x == extra_t || !(w_set.contains(&x) && color_at(cube, x) == want))
            };
            let empty = Vec::new();
            let mut placed = false;
            for cy in touch.get(&t).unwrap_or(&empty) {
                let Some(src) = cy.src_into(t) else { continue };
                if w_set.contains(&src) || color_at(cube, src) != want {
                    continue; // src must be a reservoir cell holding `want`
                }
                if !ok_other(cy, t) {
                    continue; // would break a correct W cell
                }
                apply_all(cube, &cy.moves);
                moves.extend_from_slice(&cy.moves);
                placed = true;
                break;
            }
            if placed {
                guard = 0;
                continue;
            }

            // Two-step: stage a `want` piece into the source slot of a `t`-filling
            // cycle, using a cycle that breaks nothing already correct. Predicted via
            // the permutations, then applied.
            if let Some(seq) = two_step(cube, &touch, &w_set, t, want) {
                apply_all(cube, &seq);
                moves.extend(seq);
                guard = 0;
                continue;
            }

            // Bridge for the last-cell case: a safe cycle `c1` (which may temporarily
            // disturb the working face — both legs keep every finalised face intact)
            // then a safe cycle `c2` touching `t`, netting `t` correct without losing
            // ground. This is the "break and restore" the single steps can't do. The
            // result is PREDICTED by composing the cycles' permutations (no cube
            // clone), so it stays fast even over the whole safe set.
            let base = correct_count(cube, &w_cells, want);
            let src_through = |cy: &Cyc, x: Cell| cy.perm.get(&x).copied().unwrap_or(x);
            let empty: Vec<&Cyc> = Vec::new();
            let mut bridged = false;
            'bridge: for c1 in &safe {
                for c2 in touch.get(&t).unwrap_or(&empty) {
                    // colour a cell shows after c1∘c2: source is c1_src(c2_src(cell)).
                    let comb = |w: Cell| src_through(c1, src_through(c2, w));
                    if color_at(cube, comb(t)) != want {
                        continue;
                    }
                    let after = w_cells
                        .iter()
                        .filter(|&&w| color_at(cube, comb(w)) == want)
                        .count();
                    if after > base {
                        apply_all(cube, &c1.moves);
                        moves.extend_from_slice(&c1.moves);
                        apply_all(cube, &c2.moves);
                        moves.extend_from_slice(&c2.moves);
                        bridged = true;
                        break 'bridge;
                    }
                }
            }
            if bridged {
                guard = 0;
                continue;
            }

            guard += 1;
            if guard > cap {
                if dbg {
                    eprintln!(
                        "[cd]   GIVE UP on {w:?} (#{fi}): correct={}/{} safe={}",
                        correct_count(cube, &w_cells, want),
                        w_cells.len(),
                        safe.len()
                    );
                }
                return moves; // give up; caller verifies centers_solved
            }
        }
    }

    moves
}

/// Stage a `want` piece so a direct placement into `t` becomes possible. We pick a
/// "filler" cycle whose source slot for `t` we want to load with `want`, then a
/// "stager" cycle that drops a `want` piece into that slot and breaks nothing
/// already correct. Both are validated by predicting colours through the cycles'
/// permutations (no cube clone).
fn two_step(
    cube: &StickerCube,
    touch: &HashMap<Cell, Vec<&Cyc>>,
    w_set: &HashSet<Cell>,
    t: Cell,
    want: Color,
) -> Option<Vec<Move>> {
    let breaks_correct = |cy: &Cyc, except: Option<Cell>| {
        cy.support
            .iter()
            .any(|&x| Some(x) != except && w_set.contains(&x) && color_at(cube, x) == want)
    };
    let empty = Vec::new();
    // The source slots that, if loaded with `want`, let a filler place into `t`.
    let mut fillers: Vec<(&Cyc, Cell)> = Vec::new();
    for cy in touch.get(&t).unwrap_or(&empty) {
        if let Some(src) = cy.src_into(t) {
            if !w_set.contains(&src) && !breaks_correct(cy, Some(t)) {
                fillers.push((cy, src));
            }
        }
    }
    // Predicted colour of `cell` after applying `stager`: new[cell] = old[src(cell)].
    let pred = |stager: &Cyc, cell: Cell| -> Color {
        match stager.perm.get(&cell) {
            Some(&src) => color_at(cube, src),
            None => color_at(cube, cell),
        }
    };
    for (filler, slot) in fillers.iter() {
        for stager in touch.get(slot).unwrap_or(&empty).iter() {
            if breaks_correct(stager, None) || pred(stager, *slot) != want {
                continue;
            }
            // After staging, `t` is still wrong and the filler's source for `t` holds
            // `want`, so the direct placement then succeeds.
            if pred(stager, t) == want {
                continue;
            }
            let fsrc = filler.src_into(t).unwrap();
            if pred(stager, fsrc) != want {
                continue;
            }
            let mut seq = stager.moves.clone();
            seq.extend_from_slice(&filler.moves);
            // Guard against prediction/regression errors: only commit if it really
            // increases the working face's correct count.
            let w_cells: Vec<Cell> = w_set.iter().copied().collect();
            let base = correct_count(cube, &w_cells, want);
            let mut trial = cube.clone();
            apply_all(&mut trial, &seq);
            if correct_count(&trial, &w_cells, want) > base {
                return Some(seq);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reduction::centers_solved;
    use cube_core::CubeState;

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

    // The 4×4 is the verified size. N ≥ 5 build fast (see below) but the solve is
    // WIP — kept here, ignored, to document the target and report build sizes.
    #[test]
    #[ignore = "WIP: only n=4 is currently reliable"]
    fn solves_centers_n_ge_5_wip() {
        for n in [5usize, 6, 7] {
            let t0 = std::time::Instant::now();
            let lib_n = library(n).len();
            let build = t0.elapsed();
            let mut ok = 0;
            for seed in 0..6u64 {
                let mut cube = scramble(n, 0x100 + seed, 40);
                let _ = solve_centers(&mut cube);
                if centers_solved(&cube) {
                    ok += 1;
                }
            }
            println!("n={n}: {ok}/6 | library={lib_n} (build {build:?})");
        }
    }

    #[test]
    fn perm_is_correct() {
        for n in [4usize, 5] {
            let centers = all_center_cells(n);
            let probes = build_probes(n, &centers);
            let solved = StickerCube::solved(CubeSize::new(n).unwrap());
            // pick some base sequences and verify perm matches actual behaviour.
            let cands = base_candidates(n);
            let mut checked = 0;
            for seq in cands.iter().take(400) {
                let map = center_perm(&centers, &probes, &centers, seq);
                let mut c = solved.clone();
                apply_all(&mut c, seq);
                // for each dst, the colour now there must equal the home colour of src.
                for (&dst, &src) in &map {
                    if dst == src {
                        continue;
                    }
                    assert_eq!(
                        color_at(&c, dst),
                        src.0.color(),
                        "perm wrong n={n}: dst={dst:?} claims src={src:?}"
                    );
                    checked += 1;
                }
            }
            assert!(checked > 0, "nothing checked n={n}");
            println!("n={n}: perm verified on {checked} (dst,src) pairs");
        }
    }

    #[test]
    fn solves_centers_4x4() {
        let mut fails = Vec::new();
        for seed in 0..30u64 {
            let mut cube = scramble(4, 0x100 + seed, 40);
            let _ = solve_centers(&mut cube);
            if !centers_solved(&cube) {
                fails.push(seed);
            }
        }
        assert!(fails.is_empty(), "n=4 failed seeds: {fails:?}");
    }

    #[test]
    fn noop_on_solved() {
        for n in [2usize, 3, 4, 5, 8] {
            let mut cube = StickerCube::solved(CubeSize::new(n).unwrap());
            let mv = solve_centers(&mut cube);
            assert!(centers_solved(&cube));
            assert!(mv.is_empty(), "no moves for solved n={n}");
        }
    }
}
