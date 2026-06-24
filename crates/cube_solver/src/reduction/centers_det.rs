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
//! centres-solved). **4×4 and 5×5 are solved** (30/30 and 6/6 random wide
//! scrambles), each with a ~1 s one-time cached library build and millisecond
//! solves. The library is built by composing precomputed single-move permutations
//! (no cube clones) and deduped by exact centre permutation; "confined" last-two-
//! centres cycles are classified by COLOUR effect (≤2 faces) — not positional
//! support, which also churns the restored band — and kept uncapped. n≥6 build is
//! still being made fast; once it is, the same solver should cover them. WIP; not
//! yet wired into the app.

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
    let centers = all_center_cells(n);
    let ncenters = centers.len();
    let probes = build_probes(n, &centers);
    let size = CubeSize::new(n).expect("size>=2");

    let base = base_candidates(n);
    let face_turns: Vec<Move> = Face::ALL
        .iter()
        .flat_map(|&f| {
            [1i8, -1, 2]
                .into_iter()
                .map(move |t| Move::face(f, size, t))
        })
        .collect();

    // Decode each elementary move's centre permutation once (with inverses, used by
    // commutators/conjugates); sequences are composed from these — no cube clones, so
    // the library build is fast at every N.
    let mut move_perms: HashMap<MoveKey, Vec<usize>> = HashMap::new();
    {
        let add = |m: Move, mp: &mut HashMap<MoveKey, Vec<usize>>| {
            mp.entry(move_key(&m))
                .or_insert_with(|| single_move_perm(&centers, &probes, m));
        };
        for seq in &base {
            for &m in seq {
                add(m, &mut move_perms);
                add(m.inverse(), &mut move_perms);
            }
        }
        for &m in &face_turns {
            add(m, &mut move_perms);
            add(m.inverse(), &mut move_perms);
        }
    }

    let ident: Vec<usize> = (0..ncenters).collect();
    let mv_key = |m: &[Move]| -> Vec<String> { m.iter().map(|x| x.notation(size)).collect() };
    // Faces whose COLOUR changes under a permutation (a cell receives a piece from a
    // different face). This — not the positional support, which also churns the
    // restored frozen band — is how "confined" (<=2-face) last-two-centres cycles are
    // identified and kept uncapped.
    let color_faces = |p: &[usize]| -> usize {
        p.iter()
            .enumerate()
            .filter(|(i, &src)| centers[src].0 != centers[*i].0)
            .map(|(i, _)| centers[i].0)
            .collect::<HashSet<_>>()
            .len()
    };

    // Dedup by exact centre permutation (composed, no snapshots).
    let mut by_perm: HashMap<Vec<usize>, Vec<Move>> = HashMap::new();
    {
        let consider = |seq: &[Move], by_perm: &mut HashMap<Vec<usize>, Vec<Move>>| {
            let p = seq_perm(&move_perms, ncenters, seq);
            if p == ident {
                return;
            }
            let better = match by_perm.get(&p) {
                Some(prev) => {
                    seq.len() < prev.len()
                        || (seq.len() == prev.len() && mv_key(seq) < mv_key(prev))
                }
                None => true,
            };
            if better {
                by_perm.insert(p, seq.to_vec());
            }
        };
        for seq in &base {
            consider(seq, &mut by_perm);
        }

        // Meta-commutators for the last two centres, seeded by the shortest raw base
        // cycles that move centres; keep those CONFINED to <=2 faces by colour and
        // re-aim each with a face turn for full target/source coverage.
        let mut short: Vec<Vec<Move>> = base
            .iter()
            .filter(|s| seq_perm(&move_perms, ncenters, s) != ident)
            .cloned()
            .collect();
        short.sort_by_key(|s| (s.len(), mv_key(s)));
        short.dedup();
        short.truncate(200);

        for p_seq in &short {
            for q_seq in &short {
                let meta = commutator(p_seq, q_seq);
                let mp = seq_perm(&move_perms, ncenters, &meta);
                if mp == ident || color_faces(&mp) > 2 {
                    continue;
                }
                consider(&meta, &mut by_perm);
                for ft in &face_turns {
                    consider(&conjugate(&[*ft], &meta), &mut by_perm);
                }
            }
        }

        // Single-orbit pure 3-cycles: a short base cycle commutated with a single face turn
        // or inner slice. Unlike the [slice,face] candidates (which mix orbits) and the
        // [short,short] metas above (kept only when ≤2-face colour-confined), these isolate
        // ONE centre orbit — inner-X and obliques, first needed at n≥6 to place a face's
        // last piece. Keep the pure (≤3-cell) ones regardless of how many faces they span;
        // re-aim each with a face turn for full coverage. All elementary moves here are
        // already decoded (face turns, and every slice appears as a base mover).
        let mut singles: Vec<Move> = face_turns.clone();
        for f in Face::ALL {
            for d in 1..=n - 2 {
                for s in [1i8, -1] {
                    singles.push(slice_from(f, n, d, s));
                }
            }
        }
        for p_seq in &short {
            for b in &singles {
                let meta = commutator(p_seq, &[*b]);
                let mp = seq_perm(&move_perms, ncenters, &meta);
                // Keep the orbit-isolated pure 3-cycles (any faces) AND the ≤2-face
                // colour-confined cycles (the last-two-centres churns, incl. inner-X).
                let sup = (0..ncenters).filter(|&i| mp[i] != i).count();
                if mp == ident || (sup > 3 && color_faces(&mp) > 2) {
                    continue;
                }
                consider(&meta, &mut by_perm);
                for ft in &face_turns {
                    consider(&conjugate(&[*ft], &meta), &mut by_perm);
                }
            }
        }

        // Last-two-centres algs: a commutator of two orbit-isolated 3-cycles that share a
        // buffer nets out to a single orbit's churn-restore confined to 2 faces — exactly
        // what the final two (opposite) centres need (no reservoir is left, so a 3-face
        // 3-cycle is unsafe there). Pair the shortest pure 3-cycles just generated and keep
        // the ≤2-face results; re-aim each by a face turn.
        // Group pure 3-cycles by the cell-orbit they move (by the block coords of one
        // moved cell), and keep the shortest few PER orbit, so every orbit — including
        // inner-X, whose 3-cycles are longer and would be truncated out of a global
        // shortest-N list — gets paired into last-two algs.
        let orbit_key = |p: &[usize]| -> (usize, usize) {
            let i = (0..p.len()).find(|&i| p[i] != i).unwrap_or(0);
            let (_, r, c) = centers[i];
            let rr = r.min(n - 1 - r);
            let cc = c.min(n - 1 - c);
            (rr.min(cc), rr.max(cc)) // orbit signature, mirror-invariant
        };
        let mut by_orbit: HashMap<(usize, usize), Vec<Vec<Move>>> = HashMap::new();
        for (p, m) in by_perm.iter() {
            if p.iter().enumerate().filter(|(i, &s)| *i != s).count() == 3 {
                by_orbit.entry(orbit_key(p)).or_default().push(m.clone());
            }
        }
        // Pair pure 3-cycles WITHIN each orbit, so every centre orbit — including the
        // deeper ones that first appear on bigger cubes (depth-2/3 X-centres at n≥8) and
        // whose 3-cycles are longer — gets its own last-two-centres algs, instead of being
        // crowded out of a single global shortest-N list.
        let mut orbits: Vec<(usize, usize)> = by_orbit.keys().copied().collect();
        orbits.sort();
        for key in &orbits {
            let cycles = by_orbit.get_mut(key).unwrap();
            cycles.sort_by_key(|s| (s.len(), mv_key(s)));
            cycles.truncate(48);
            let cycles = cycles.clone();
            for p in &cycles {
                for q in &cycles {
                    let meta = commutator(p, q);
                    let mp = seq_perm(&move_perms, ncenters, &meta);
                    if mp == ident || color_faces(&mp) > 2 {
                        continue;
                    }
                    consider(&meta, &mut by_perm);
                    for ft in &face_turns {
                        consider(&conjugate(&[*ft], &meta), &mut by_perm);
                    }
                }
            }
        }
    }

    // Keep every colour-confined (<=2-face) cycle plus a generous cap of general
    // cycles, in a fully deterministic order so coverage is identical every run.
    let support_len = |p: &[usize]| p.iter().enumerate().filter(|(i, &s)| *i != s).count();
    let mut raw: Vec<(Vec<usize>, Vec<Move>)> = by_perm.into_iter().collect();
    raw.sort_by(|(pa, ma), (pb, mb)| {
        (support_len(pa), ma.len())
            .cmp(&(support_len(pb), mb.len()))
            .then_with(|| pa.cmp(pb))
            .then_with(|| mv_key(ma).cmp(&mv_key(mb)))
    });
    let cap_gen = 7000usize;
    let mut ngen = 0;
    let mut out: Vec<Cyc> = Vec::new();
    for (p, moves) in raw {
        // Pure ≤3-cell 3-cycles are the orbit-isolated movers (inner-X, obliques — needed
        // to place a face's last piece at n≥6); keep them all. Only larger many-face
        // "general" cycles are capped.
        if color_faces(&p) > 2 && support_len(&p) > 3 {
            if ngen >= cap_gen {
                continue;
            }
            ngen += 1;
        }
        let mut support = Vec::new();
        let mut perm = HashMap::new();
        for (i, &src_i) in p.iter().enumerate() {
            if src_i != i {
                support.push(centers[i]);
                perm.insert(centers[i], centers[src_i]);
            }
        }
        out.push(Cyc {
            moves,
            support,
            perm,
        });
    }
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

        // No-progress is deterministic and unchanged, so a stalled iteration would fail
        // identically forever; give up at once rather than re-running the expensive search
        // backstop to a large cap (this is what made a failed 6×6 centre solve churn ~90 s).
        let cap = 0usize;
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

            // Multi-cycle search backstop: shuffle the piece through several positions
            // when no 1- or 2-cycle places it (even-cube obliques). Bounded, only
            // reached at a genuine stall.
            if let Some(seq) = search_bridge(cube, &touch, &w_cells, want, 5, 40_000) {
                apply_all(cube, &seq);
                moves.extend(seq);
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

/// Depth-limited search for when the direct/2-ply steps can't place a piece: the
/// wanted piece has to be shuffled through several positions (e.g. even-cube oblique
/// centres, where no single cycle brings it from the reservoir to the target). Tries
/// sequences of safe cycles — each touching a currently-wrong working cell —
/// predicted by composing permutations, up to `maxd` deep within a node budget.
/// Returns the moves of a sequence that strictly increases the working face's
/// correct count, preserving every finalised face (all cycles are `safe`).
fn search_bridge(
    cube: &StickerCube,
    touch: &HashMap<Cell, Vec<&Cyc>>,
    w_cells: &[Cell],
    want: Color,
    maxd: usize,
    budget: i64,
) -> Option<Vec<Move>> {
    let base = w_cells
        .iter()
        .filter(|&&w| color_at(cube, w) == want)
        .count();
    let mut budget = budget;
    let mut path: Vec<Move> = Vec::new();
    if dfs_fill(
        cube,
        touch,
        w_cells,
        want,
        base,
        &HashMap::new(),
        0,
        maxd,
        &mut budget,
        &mut path,
    ) {
        Some(path)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn dfs_fill(
    cube: &StickerCube,
    touch: &HashMap<Cell, Vec<&Cyc>>,
    w_cells: &[Cell],
    want: Color,
    base: usize,
    cum: &HashMap<Cell, Cell>,
    depth: usize,
    maxd: usize,
    budget: &mut i64,
    path: &mut Vec<Move>,
) -> bool {
    if *budget <= 0 {
        return false;
    }
    *budget -= 1;
    let cur = |w: Cell| cum.get(&w).copied().unwrap_or(w);
    let correct = w_cells
        .iter()
        .filter(|&&w| color_at(cube, cur(w)) == want)
        .count();
    if correct > base {
        return true;
    }
    if depth >= maxd {
        return false;
    }
    let wrong: Vec<Cell> = w_cells
        .iter()
        .copied()
        .filter(|&w| color_at(cube, cur(w)) != want)
        .collect();
    let empty: Vec<&Cyc> = Vec::new();
    let mut seen: HashSet<*const Cyc> = HashSet::new();
    for &wc in &wrong {
        for &cy in touch.get(&wc).unwrap_or(&empty) {
            if !seen.insert(cy as *const Cyc) {
                continue;
            }
            // After applying `cy` next, cell x holds the piece from cum(cy.src(x)).
            let mut new_cum = cum.clone();
            for &x in &cy.support {
                let s = cy.perm.get(&x).copied().unwrap_or(x);
                let c = cum.get(&s).copied().unwrap_or(s);
                if c == x {
                    new_cum.remove(&x);
                } else {
                    new_cum.insert(x, c);
                }
            }
            let prev = path.len();
            path.extend_from_slice(&cy.moves);
            if dfs_fill(
                cube,
                touch,
                w_cells,
                want,
                base,
                &new_cum,
                depth + 1,
                maxd,
                budget,
                path,
            ) {
                return true;
            }
            path.truncate(prev);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reduction::{centers_solved, edges_paired, solve_edges};
    use cube_core::CubeState;

    /// Baseline: centres (deterministic) then the existing greedy edge-pairing.
    #[test]
    #[ignore = "edges baseline probe"]
    fn centres_then_edges_n4() {
        let mut ok = 0;
        for seed in 0..6u64 {
            let mut cube = scramble(4, 0x100 + seed, 40);
            let _ = solve_centers(&mut cube);
            if !centers_solved(&cube) {
                println!("seed {seed}: centres FAILED");
                continue;
            }
            let t0 = std::time::Instant::now();
            let _ = solve_edges(&mut cube);
            let paired = edges_paired(&cube);
            let centres_still = centers_solved(&cube);
            println!(
                "seed {seed}: edges_paired={paired} centres_intact={centres_still} ({:?})",
                t0.elapsed()
            );
            if paired && centres_still {
                ok += 1;
            }
        }
        println!("n=4 centres+edges: {ok}/6");
    }

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

    /// Search for a clean orbit-isolated inner-X 3-cycle on 6×6 — the n=6 centre blocker.
    /// If found, its construction can be added to the library to place the last inner-X.
    #[test]
    #[ignore = "diagnostic search"]
    fn find_innerx_3cycle() {
        let n = 6usize;
        let size = CubeSize::new(n).unwrap();
        let centers = all_center_cells(n);
        let probes = build_probes(n, &centers);
        let nc = centers.len();
        let is_innerx = |i: usize| {
            let (_, r, c) = centers[i];
            (r == 2 || r == 3) && (c == 2 || c == 3)
        };
        let base = base_candidates(n);
        let mut bcands: Vec<Vec<Move>> = Vec::new();
        for f in Face::ALL {
            for t in [1i8, -1, 2] {
                bcands.push(vec![Move::face(f, size, t)]);
            }
            for d in 1..=n - 2 {
                for t in [1i8, -1] {
                    bcands.push(vec![slice_from(f, n, d, t)]);
                }
            }
        }
        let mut mp: HashMap<MoveKey, Vec<usize>> = HashMap::new();
        for seq in base.iter().chain(bcands.iter()) {
            for &m in seq {
                mp.entry(move_key(&m))
                    .or_insert_with(|| single_move_perm(&centers, &probes, m));
                let inv = m.inverse();
                mp.entry(move_key(&inv))
                    .or_insert_with(|| single_move_perm(&centers, &probes, inv));
            }
        }
        // 2-face-confined cycles touching inner-X (the last-two-centres churn-restore algs).
        let faces_of = |support: &[usize]| -> std::collections::BTreeSet<usize> {
            support
                .iter()
                .map(|&i| Face::ALL.iter().position(|&x| x == centers[i].0).unwrap())
                .collect()
        };
        let opposite = |fa: usize, fb: usize| -> bool {
            // Face::ALL order: Up,Down,Front,Back,Left,Right → opposite pairs (0,1)(2,3)(4,5)
            fa / 2 == fb / 2
        };
        let mut found3 = 0usize;
        let mut found2 = 0usize;
        let mut samp3: Vec<String> = Vec::new();
        let mut samp2: Vec<String> = Vec::new();
        let mut min2_a = usize::MAX;
        for a in &base {
            for b in &bcands {
                let meta = commutator(a, b);
                let perm = seq_perm(&mp, nc, &meta);
                let support: Vec<usize> = (0..nc).filter(|&i| perm[i] != i).collect();
                if support.is_empty() {
                    continue;
                }
                let has_innerx = support.iter().any(|&i| is_innerx(i));
                if support.len() == 3 && support.iter().all(|&i| is_innerx(i)) {
                    found3 += 1;
                    if samp3.len() < 2 {
                        samp3.push(
                            meta.iter()
                                .map(|m| m.notation(size))
                                .collect::<Vec<_>>()
                                .join(" "),
                        );
                    }
                }
                let fs = faces_of(&support);
                if has_innerx && support.len() <= 8 && fs.len() == 2 {
                    let v: Vec<usize> = fs.iter().copied().collect();
                    if opposite(v[0], v[1]) {
                        found2 += 1;
                        min2_a = min2_a.min(a.len());
                        if samp2.len() < 3 {
                            samp2.push(format!(
                                "sup={} a.len={}: {}",
                                support.len(),
                                a.len(),
                                meta.iter()
                                    .map(|m| m.notation(size))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            ));
                        }
                    }
                }
            }
        }
        println!("n=6 pure inner-X 3-cycles: {found3}; 2-opposite-face inner-X cycles: {found2} (min a.len={min2_a})");
        for s in &samp3 {
            println!("  3cyc: {s}");
        }
        for s in &samp2 {
            println!("  2face: {s}");
        }
    }
}
