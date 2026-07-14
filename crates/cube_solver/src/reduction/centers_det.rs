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

/// Canonical physical center-cubie orbit under the 24 proper cube rotations.
///
/// Sorting the two face-local depths is insufficient: a generic oblique pair
/// represents two chiral 24-piece orbits. Canonicalizing the centered 3-D cubie
/// coordinate under determinant-+1 signed axis permutations keeps those mirror
/// orbits separate while naturally merging diagonal/zero-coordinate cases.
fn center_orbit_key(cell: Cell, n: usize) -> (isize, isize, isize) {
    let last = n - 1;
    let (face, row, col) = cell;
    let (x, y, z) = match face {
        Face::Up => (col, last, row),
        Face::Down => (col, 0, last - row),
        Face::Front => (col, last - row, last),
        Face::Back => (last - col, last - row, 0),
        Face::Left => (0, last - row, col),
        Face::Right => (last, last - row, last - col),
    };
    let centered = [
        2 * x as isize - last as isize,
        2 * y as isize - last as isize,
        2 * z as isize - last as isize,
    ];
    let permutations = [
        ([0usize, 1, 2], 1isize),
        ([0usize, 2, 1], -1isize),
        ([1usize, 0, 2], -1isize),
        ([1usize, 2, 0], 1isize),
        ([2usize, 0, 1], 1isize),
        ([2usize, 1, 0], -1isize),
    ];
    let mut best = (isize::MAX, isize::MAX, isize::MAX);
    for (permutation, parity) in permutations {
        for sx in [-1isize, 1] {
            for sy in [-1isize, 1] {
                for sz in [-1isize, 1] {
                    if parity * sx * sy * sz != 1 {
                        continue;
                    }
                    let candidate = (
                        sx * centered[permutation[0]],
                        sy * centered[permutation[1]],
                        sz * centered[permutation[2]],
                    );
                    best = best.min(candidate);
                }
            }
        }
    }
    best
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

/// Exact destination→source permutation of `setup core setup⁻¹` from known
/// component permutations, avoiding a second walk over the long move word.
fn conjugated_perm(setup: &[usize], core: &[usize], inverse: &[usize]) -> Vec<usize> {
    inverse.iter().map(|&source| setup[core[source]]).collect()
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

/// A centre-only cycle with its exact directed permutation. One compact ordered
/// pair array serves both support iteration and destination→source lookup; the
/// former `support + HashMap<Cell, Cell>` duplicated every destination and paid a
/// separate hash allocation for each retained cycle.
struct Cyc {
    moves: Vec<Move>,
    effect: Box<[(Cell, Cell)]>,
}

impl Cyc {
    fn support_len(&self) -> usize {
        self.effect.len()
    }

    fn support(&self) -> impl Iterator<Item = Cell> + '_ {
        self.effect.iter().map(|&(destination, _)| destination)
    }

    fn permutation(&self) -> impl Iterator<Item = (Cell, Cell)> + '_ {
        self.effect.iter().copied()
    }

    /// The cell whose piece moves into `target` when this cycle is applied.
    fn src_into(&self, target: Cell) -> Option<Cell> {
        self.effect
            .iter()
            .find_map(|&(destination, source)| (destination == target).then_some(source))
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
    if !super::reduction_checkpoint() {
        return Vec::new();
    }
    let centers = all_center_cells(n);
    let ncenters = centers.len();
    if ncenters > u32::MAX as usize {
        return Vec::new();
    }
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
            if !super::reduction_checkpoint() {
                return Vec::new();
            }
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

    // Dedup by exact centre permutation, storing only moved dst→src pairs. Full
    // identity-padded Vec<usize> keys made N=12 library construction consume
    // hundreds of MiB and scaled quadratically in cells per retained cycle.
    type SparsePerm = Vec<(u32, u32)>;
    let compact = |p: &[usize]| -> SparsePerm {
        p.iter()
            .enumerate()
            .filter(|&(dst, src)| dst != *src)
            .map(|(dst, &src)| {
                (
                    u32::try_from(dst).expect("center index checked"),
                    u32::try_from(src).expect("center index checked"),
                )
            })
            .collect()
    };
    let mut by_perm: HashMap<SparsePerm, Vec<Move>> = HashMap::new();
    {
        let consider_known =
            |seq: &[Move], dense: &[usize], by_perm: &mut HashMap<SparsePerm, Vec<Move>>| {
                let p = compact(dense);
                if p.is_empty() {
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
        let conjugated_dense = |core: &[usize], setup: Move| -> Vec<usize> {
            let setup_perm = &move_perms[&move_key(&setup)];
            let inverse_perm = &move_perms[&move_key(&setup.inverse())];
            conjugated_perm(setup_perm, core, inverse_perm)
        };

        // Compute every base permutation once for both dedup and shortlist
        // eligibility. The old two-pass path repeated ~805k dense N=32
        // evaluations and cloned every candidate before retaining only 200.
        let mut short: Vec<&Vec<Move>> = Vec::new();
        for seq in &base {
            let dense = seq_perm(&move_perms, ncenters, seq);
            if dense != ident {
                short.push(seq);
            }
            consider_known(seq, &dense, &mut by_perm);
        }

        // Meta-commutators for the last two centres, seeded by the shortest raw base
        // cycles that move centres; keep those CONFINED to <=2 faces by colour and
        // re-aim each with a face turn for full target/source coverage.
        short.sort_by_key(|sequence| (sequence.len(), mv_key(sequence)));
        short.dedup();
        short.truncate(200);

        for p_seq in &short {
            if !super::reduction_checkpoint() {
                return Vec::new();
            }
            for q_seq in &short {
                let meta = commutator(p_seq, q_seq);
                let mp = seq_perm(&move_perms, ncenters, &meta);
                if mp == ident || color_faces(&mp) > 2 {
                    continue;
                }
                consider_known(&meta, &mp, &mut by_perm);
                for &ft in &face_turns {
                    let word = conjugate(&[ft], &meta);
                    let dense = conjugated_dense(&mp, ft);
                    consider_known(&word, &dense, &mut by_perm);
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
            if !super::reduction_checkpoint() {
                return Vec::new();
            }
            for b in &singles {
                let meta = commutator(p_seq, &[*b]);
                let mp = seq_perm(&move_perms, ncenters, &meta);
                // Keep the orbit-isolated pure 3-cycles (any faces) AND the ≤2-face
                // colour-confined cycles (the last-two-centres churns, incl. inner-X).
                let sup = (0..ncenters).filter(|&i| mp[i] != i).count();
                if mp == ident || (sup > 3 && color_faces(&mp) > 2) {
                    continue;
                }
                consider_known(&meta, &mp, &mut by_perm);
                for &ft in &face_turns {
                    let word = conjugate(&[ft], &meta);
                    let dense = conjugated_dense(&mp, ft);
                    consider_known(&word, &dense, &mut by_perm);
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
        let orbit_key = |permutation: &SparsePerm| {
            let index = permutation.first().map_or(0, |&(dst, _)| dst as usize);
            center_orbit_key(centers[index], n)
        };
        let mut by_orbit: HashMap<(isize, isize, isize), Vec<Vec<Move>>> = HashMap::new();
        for (p, m) in &by_perm {
            if p.len() == 3 {
                by_orbit.entry(orbit_key(p)).or_default().push(m.clone());
            }
        }
        // Pair pure 3-cycles WITHIN each orbit, so every centre orbit — including the
        // deeper ones that first appear on bigger cubes (depth-2/3 X-centres at n≥8) and
        // whose 3-cycles are longer — gets its own last-two-centres algs, instead of being
        // crowded out of a single global shortest-N list.
        let mut orbits: Vec<_> = by_orbit.keys().copied().collect();
        orbits.sort();
        for key in &orbits {
            if !super::reduction_checkpoint() {
                return Vec::new();
            }
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
                    consider_known(&meta, &mp, &mut by_perm);
                    for &ft in &face_turns {
                        let word = conjugate(&[ft], &meta);
                        let dense = conjugated_dense(&mp, ft);
                        consider_known(&word, &dense, &mut by_perm);
                    }
                }
            }
        }
    }

    // Keep every colour-confined (<=2-face) cycle plus a generous cap of general
    // cycles, in a fully deterministic order so coverage is identical every run.
    let sparse_color_faces = |p: &SparsePerm| -> usize {
        p.iter()
            .filter(|&&(dst, src)| centers[dst as usize].0 != centers[src as usize].0)
            .map(|&(dst, _)| centers[dst as usize].0)
            .collect::<HashSet<_>>()
            .len()
    };
    let mut raw: Vec<(SparsePerm, Vec<Move>)> = by_perm.into_iter().collect();
    raw.sort_by(|(pa, ma), (pb, mb)| {
        (pa.len(), ma.len())
            .cmp(&(pb.len(), mb.len()))
            .then_with(|| pa.cmp(pb))
            .then_with(|| mv_key(ma).cmp(&mv_key(mb)))
    });
    let cap_gen = 7000usize;
    let mut ngen = 0;
    let mut retained_pure = 0usize;
    let mut retained_confined = 0usize;
    let mut retained_general = 0usize;
    let mut out: Vec<Cyc> = Vec::new();
    for (p, moves) in raw {
        if !super::reduction_checkpoint() {
            return Vec::new();
        }
        // Pure ≤3-cell 3-cycles are the orbit-isolated movers (inner-X, obliques — needed
        // to place a face's last piece at n≥6); keep them all. Only larger many-face
        // "general" cycles are capped.
        let face_count = sparse_color_faces(&p);
        if face_count > 2 && p.len() > 3 {
            if ngen >= cap_gen {
                continue;
            }
            ngen += 1;
            retained_general += 1;
        } else if p.len() <= 3 {
            retained_pure += 1;
        } else {
            retained_confined += 1;
        }
        let effect: Vec<(Cell, Cell)> = p
            .into_iter()
            .map(|(destination, source)| (centers[destination as usize], centers[source as usize]))
            .collect();
        out.push(Cyc {
            moves,
            effect: effect.into_boxed_slice(),
        });
    }
    out.sort_by_key(|cycle| (cycle.support_len(), cycle.moves.len()));
    if std::env::var("RDBG").is_ok() {
        let cycle_headers = out.capacity() * std::mem::size_of::<Cyc>();
        let move_entries: usize = out.iter().map(|cycle| cycle.moves.len()).sum();
        let move_bytes: usize = out
            .iter()
            .map(|cycle| cycle.moves.capacity() * std::mem::size_of::<Move>())
            .sum();
        let effect_entries: usize = out.iter().map(|cycle| cycle.effect.len()).sum();
        let effect_bytes: usize = out
            .iter()
            .map(|cycle| cycle.effect.len() * std::mem::size_of::<(Cell, Cell)>())
            .sum();
        eprintln!(
            "[cd] retained n={n}: cycles={} pure={retained_pure} confined={retained_confined} general={retained_general} move_entries={move_entries} effect_entries={effect_entries} bytes(headers={cycle_headers}, moves={move_bytes}, effects={effect_bytes})",
            out.len()
        );
    }
    out
}

thread_local! {
    static LIB_CACHE: RefCell<HashMap<usize, std::rc::Rc<Vec<Cyc>>>> = RefCell::new(HashMap::new());
}

fn library(n: usize) -> std::rc::Rc<Vec<Cyc>> {
    LIB_CACHE.with(|cache| {
        if let Some(existing) = cache.borrow().get(&n).cloned() {
            return existing;
        }
        let built = std::rc::Rc::new(build_library(n));
        if super::reduction_checkpoint() {
            cache.borrow_mut().insert(n, built.clone());
        }
        built
    })
}

fn correct_count(cube: &StickerCube, w_cells: &[Cell], want: Color) -> usize {
    w_cells
        .iter()
        .filter(|&&x| color_at(cube, x) == want)
        .count()
}

/// Exact binary-color solver for the final two center faces, one geometric orbit
/// at a time. Earlier faces are solid and `safe` only churns within their colors.
fn solve_last_two_center_orbits(
    cube: &StickerCube,
    safe: &[&Cyc],
    target: Face,
    reservoirs: &[Face],
    n: usize,
    want: Color,
) -> Option<Vec<Move>> {
    use std::collections::{HashMap, VecDeque};

    let orbit_key = |cell: Cell| center_orbit_key(cell, n);
    let mut keys: Vec<_> = face_center_cells(target, n)
        .into_iter()
        .filter(|&cell| color_at(cube, cell) != want)
        .map(orbit_key)
        .collect();
    keys.sort_unstable();
    keys.dedup();

    let mut trial = cube.clone();
    let mut moves = Vec::new();
    let mut processed = HashSet::new();
    for key in keys {
        if !super::reduction_checkpoint() {
            return None;
        }
        let cells: Vec<Cell> = std::iter::once(target)
            .chain(reservoirs.iter().copied())
            .flat_map(|face| face_center_cells(face, n))
            .filter(|&cell| orbit_key(cell) == key)
            .collect();
        if cells.len() >= u64::BITS as usize {
            return None;
        }
        let index: HashMap<Cell, usize> = cells
            .iter()
            .copied()
            .enumerate()
            .map(|(i, cell)| (cell, i))
            .collect();
        let encode = |state_cube: &StickerCube| -> u64 {
            cells.iter().enumerate().fold(0u64, |bits, (i, &cell)| {
                bits | (u64::from(color_at(state_cube, cell) == want) << i)
            })
        };
        let goal_mask = cells
            .iter()
            .enumerate()
            .filter(|(_, cell)| cell.0 == target)
            .fold(0u64, |bits, (i, _)| bits | (1u64 << i));
        let start = encode(&trial);
        if start & goal_mask == goal_mask {
            continue;
        }

        // Dedup exact orbit actions, keeping the shortest move word.
        let mut by_action: HashMap<Vec<usize>, Vec<Move>> = HashMap::new();
        for (cycle_index, &cycle) in safe.iter().enumerate() {
            if cycle_index.is_multiple_of(128) && !super::reduction_checkpoint() {
                return None;
            }
            // Multi-orbit words may act in parallel. Outside the active orbit they
            // may churn within a face, but must never transfer target/reservoir
            // colors across faces or they could undo an earlier solved orbit.
            if cycle.permutation().any(|(dst, src)| {
                !index.contains_key(&dst)
                    && (dst.0 == target || reservoirs.contains(&dst.0))
                    && dst.0 != src.0
            }) {
                continue;
            }
            let mut action = Vec::with_capacity(cells.len());
            let mut valid = true;
            for &dst in &cells {
                let src = cycle.src_into(dst).unwrap_or(dst);
                let Some(&src_index) = index.get(&src) else {
                    valid = false;
                    break;
                };
                action.push(src_index);
            }
            if !valid || action.iter().enumerate().all(|(i, &src)| i == src) {
                continue;
            }
            let replace = by_action
                .get(&action)
                .is_none_or(|previous| cycle.moves.len() < previous.len());
            if replace {
                by_action.insert(action, cycle.moves.clone());
            }
        }
        // Explicit inverses make the state graph undirected and enable a much
        // smaller bidirectional search for generic 32-cell center orbits.
        let originals: Vec<_> = by_action
            .iter()
            .map(|(action, moves)| (action.clone(), moves.clone()))
            .collect();
        for (action, moves) in originals {
            let mut inverse_action = vec![0usize; action.len()];
            for (dst, src) in action.into_iter().enumerate() {
                inverse_action[src] = dst;
            }
            let inverse_moves: Vec<Move> = moves.iter().rev().map(|mv| mv.inverse()).collect();
            let replace = by_action
                .get(&inverse_action)
                .is_none_or(|previous| inverse_moves.len() < previous.len());
            if replace {
                by_action.insert(inverse_action, inverse_moves);
            }
        }
        let mut generators: Vec<(Vec<usize>, Vec<Move>)> = by_action.into_iter().collect();
        generators.sort_by_key(|(action, moves)| (moves.len(), action.clone()));
        if generators.is_empty() {
            if std::env::var("RDBG").is_ok() {
                eprintln!(
                    "[cd] exact orbit {key:?}: no closed generators (cells={})",
                    cells.len()
                );
            }
            return None;
        }

        let action_indices: HashMap<Vec<usize>, usize> = generators
            .iter()
            .enumerate()
            .map(|(index, (action, _))| (action.clone(), index))
            .collect();
        let inverse_indices: Vec<usize> = generators
            .iter()
            .map(|(action, _)| {
                let mut inverse = vec![0usize; action.len()];
                for (dst, &src) in action.iter().enumerate() {
                    inverse[src] = dst;
                }
                action_indices[&inverse]
            })
            .collect();
        let apply_action = |state: u64, action: &[usize]| -> u64 {
            action.iter().enumerate().fold(0u64, |bits, (dst, &src)| {
                bits | (((state >> src) & 1) << dst)
            })
        };

        let mut from_start: HashMap<u64, (u64, usize)> =
            HashMap::from([(start, (start, usize::MAX))]);
        let mut from_goal: HashMap<u64, (u64, usize)> =
            HashMap::from([(goal_mask, (goal_mask, usize::MAX))]);
        let mut start_queue = VecDeque::from([start]);
        let mut goal_queue = VecDeque::from([goal_mask]);
        let mut meeting = None;
        while !start_queue.is_empty() && !goal_queue.is_empty() && meeting.is_none() {
            if !super::reduction_checkpoint() {
                return None;
            }
            let forward = from_start.len() <= from_goal.len();
            let state = if forward {
                start_queue.pop_front().unwrap()
            } else {
                goal_queue.pop_front().unwrap()
            };
            for (generator_index, (action, _)) in generators.iter().enumerate() {
                if generator_index.is_multiple_of(256) && !super::reduction_checkpoint() {
                    return None;
                }
                let next = apply_action(state, action);
                let (own, other, queue) = if forward {
                    (&mut from_start, &from_goal, &mut start_queue)
                } else {
                    (&mut from_goal, &from_start, &mut goal_queue)
                };
                if own.contains_key(&next) {
                    continue;
                }
                let path_generator = if forward {
                    generator_index
                } else {
                    inverse_indices[generator_index]
                };
                own.insert(next, (state, path_generator));
                if other.contains_key(&next) {
                    meeting = Some(next);
                    break;
                }
                if own.len() + other.len() >= 500_000 {
                    if std::env::var("RDBG").is_ok() {
                        eprintln!(
                            "[cd] exact orbit {key:?}: bidirectional state cap (cells={}, generators={})",
                            cells.len(),
                            generators.len()
                        );
                    }
                    return None;
                }
                queue.push_back(next);
            }
        }
        let Some(meeting) = meeting else {
            if std::env::var("RDBG").is_ok() {
                eprintln!(
                    "[cd] exact orbit {key:?}: unreachable (cells={}, states={}, generators={})",
                    cells.len(),
                    from_start.len() + from_goal.len(),
                    generators.len()
                );
            }
            return None;
        };
        let mut prefix = Vec::new();
        let mut state = meeting;
        while state != start {
            let &(previous, generator_index) = from_start.get(&state)?;
            prefix.push(generator_index);
            state = previous;
        }
        prefix.reverse();
        let mut suffix = Vec::new();
        state = meeting;
        while state != goal_mask {
            let &(next, generator_index) = from_goal.get(&state)?;
            suffix.push(generator_index);
            state = next;
        }
        prefix.extend(suffix);
        for generator_index in prefix {
            let generator_moves = &generators[generator_index].1;
            apply_all(&mut trial, generator_moves);
            moves.extend_from_slice(generator_moves);
        }
        processed.insert(key);
        if face_center_cells(target, n)
            .into_iter()
            .any(|cell| processed.contains(&orbit_key(cell)) && color_at(&trial, cell) != want)
        {
            return None;
        }
    }
    Some(moves)
}

/// Constructively fill one center face, physical orbit by physical orbit, by
/// conjugating a verified exact 3-cycle through the existing safe action group.
/// The search space is ordered triples of at most 24 active cells, not color
/// masks; once built for an orbit it supplies every source→target→buffer cycle.
fn solve_center_orbits_constructively(
    cube: &StickerCube,
    safe: &[&Cyc],
    target: Face,
    reservoirs: &[Face],
    n: usize,
    want: Color,
) -> Option<Vec<Move>> {
    use std::collections::{HashMap, VecDeque};

    let active_faces: Vec<Face> = std::iter::once(target)
        .chain(reservoirs.iter().copied())
        .collect();
    if active_faces.len() < 3 {
        return None;
    }
    let mut keys: Vec<_> = face_center_cells(target, n)
        .into_iter()
        .filter(|&cell| color_at(cube, cell) != want)
        .map(|cell| center_orbit_key(cell, n))
        .collect();
    keys.sort_unstable();
    keys.dedup();

    // Index safe cycles by every active physical orbit they affect. Without this,
    // each of O(N²) orbits rescanned the entire O(N²) center library, producing
    // the measured N=32 cliff despite only hundreds of relevant actions per key.
    let mut cycles_by_orbit: HashMap<(isize, isize, isize), Vec<&Cyc>> = HashMap::new();
    for &cycle in safe {
        let mut touched = HashSet::new();
        for (destination, _) in cycle.permutation() {
            if active_faces.contains(&destination.0) {
                touched.insert(center_orbit_key(destination, n));
            }
        }
        for key in touched {
            cycles_by_orbit.entry(key).or_default().push(cycle);
        }
    }

    let mut trial = cube.clone();
    let mut moves = Vec::new();
    let mut solved_keys = HashSet::new();
    for key in keys {
        if !super::reduction_checkpoint() {
            return None;
        }
        let cells: Vec<Cell> = active_faces
            .iter()
            .copied()
            .flat_map(|face| face_center_cells(face, n))
            .filter(|&cell| center_orbit_key(cell, n) == key)
            .collect();
        if cells.len() < 3 || cells.len() > 24 {
            return None;
        }
        let indexes: HashMap<Cell, usize> = cells
            .iter()
            .copied()
            .enumerate()
            .map(|(index, cell)| (cell, index))
            .collect();

        let action_of = |cycle: &Cyc| -> Option<Vec<u8>> {
            let mut action: Vec<u8> = (0..cells.len())
                .map(|index| u8::try_from(index).ok())
                .collect::<Option<_>>()?;
            for (dst, src) in cycle.permutation() {
                let Some(&dst_index) = indexes.get(&dst) else {
                    continue;
                };
                let &src_index = indexes.get(&src)?;
                action[src_index] = u8::try_from(dst_index).ok()?;
            }
            let mut image = action.clone();
            image.sort_unstable();
            (image.iter().copied().eq(0..cells.len() as u8)).then_some(action)
        };

        let orbit_cycles = cycles_by_orbit.get(&key)?;
        let seed = orbit_cycles
            .iter()
            .copied()
            .filter(|cycle| {
                cycle.support_len() == 3 && cycle.support().all(|cell| indexes.contains_key(&cell))
            })
            .min_by_key(|cycle| cycle.moves.len())?;
        let seed_action = action_of(seed)?;
        let first = indexes[&seed.support().next()?];
        let second = seed_action[first] as usize;
        let third = seed_action[second] as usize;
        if seed_action[third] as usize != first
            || first == second
            || second == third
            || first == third
        {
            return None;
        }
        let root = [first as u8, second as u8, third as u8];

        // Deduplicate source→destination actions and add explicit inverses. The
        // retained cycle reference plus direction reconstructs legal move words.
        let mut by_action: HashMap<Vec<u8>, (&Cyc, bool)> = HashMap::new();
        for &cycle in orbit_cycles {
            let Some(action) = action_of(cycle) else {
                continue;
            };
            if action
                .iter()
                .enumerate()
                .all(|(source, &destination)| source == destination as usize)
            {
                continue;
            }
            let replace = by_action
                .get(&action)
                .is_none_or(|(previous, _)| cycle.moves.len() < previous.moves.len());
            if replace {
                by_action.insert(action.clone(), (cycle, false));
            }
            let mut inverse = vec![0u8; action.len()];
            for (source, destination) in action.into_iter().enumerate() {
                inverse[destination as usize] = source as u8;
            }
            let replace = by_action
                .get(&inverse)
                .is_none_or(|(previous, _)| cycle.moves.len() < previous.moves.len());
            if replace {
                by_action.insert(inverse, (cycle, true));
            }
        }
        let mut generators: Vec<(Vec<u8>, &Cyc, bool)> = by_action
            .into_iter()
            .map(|(action, (cycle, inverse))| (action, cycle, inverse))
            .collect();
        generators
            .sort_by_key(|(action, cycle, inverse)| (cycle.moves.len(), *inverse, action.clone()));

        let width = cells.len();
        let encode = |state: [u8; 3]| -> usize {
            (state[0] as usize * width + state[1] as usize) * width + state[2] as usize
        };
        let state_count = width * width * width;
        let mut parent: Vec<Option<(usize, usize)>> = vec![None; state_count];
        let root_index = encode(root);
        parent[root_index] = Some((root_index, usize::MAX));
        // Expand lazily. Most color states need only a shallow conjugator; filling
        // all k·(k-1)·(k-2) ordered triples up front caused a sharp N=32 cliff.
        // The frontier/parents remain available for later placements in this orbit.
        let mut queue = VecDeque::from([root]);

        let orbit_targets: Vec<Cell> = cells
            .iter()
            .copied()
            .filter(|cell| cell.0 == target)
            .collect();
        while let Some(destination) = orbit_targets
            .iter()
            .copied()
            .find(|&cell| color_at(&trial, cell) != want)
        {
            if !super::reduction_checkpoint() {
                return None;
            }
            let source = cells
                .iter()
                .copied()
                .find(|&cell| cell.0 != target && color_at(&trial, cell) == want)?;
            let source_index = indexes[&source] as u8;
            let destination_index = indexes[&destination] as u8;
            let mut goal = cells
                .iter()
                .copied()
                .filter(|cell| cell.0 != target && *cell != source)
                .find_map(|buffer| {
                    let state = [source_index, destination_index, indexes[&buffer] as u8];
                    parent[encode(state)].map(|_| state)
                });
            while goal.is_none() {
                if !super::reduction_checkpoint() {
                    return None;
                }
                let state = queue.pop_front()?;
                let state_index = encode(state);
                for (generator_index, (action, _, _)) in generators.iter().enumerate() {
                    if generator_index.is_multiple_of(256) && !super::reduction_checkpoint() {
                        return None;
                    }
                    let next = [
                        action[state[0] as usize],
                        action[state[1] as usize],
                        action[state[2] as usize],
                    ];
                    let next_index = encode(next);
                    if parent[next_index].is_some() {
                        continue;
                    }
                    parent[next_index] = Some((state_index, generator_index));
                    queue.push_back(next);
                    if next[0] == source_index
                        && next[1] == destination_index
                        && cells[next[2] as usize].0 != target
                    {
                        goal = Some(next);
                        break;
                    }
                }
            }
            let goal = goal?;

            let mut path = Vec::new();
            let mut state_index = encode(goal);
            while state_index != root_index {
                let (previous, generator_index) = parent[state_index]?;
                path.push(generator_index);
                state_index = previous;
            }
            path.reverse();
            let mut conjugator = Vec::new();
            for generator_index in path {
                let (_, cycle, inverse) = generators[generator_index];
                if inverse {
                    conjugator.extend(cycle.moves.iter().rev().map(|mv| mv.inverse()));
                } else {
                    conjugator.extend_from_slice(&cycle.moves);
                }
            }
            let mut primitive: Vec<Move> = conjugator.iter().rev().map(|mv| mv.inverse()).collect();
            primitive.extend_from_slice(&seed.moves);
            primitive.extend_from_slice(&conjugator);

            let before = correct_count(&trial, &orbit_targets, want);
            apply_all(&mut trial, &primitive);
            if color_at(&trial, destination) != want
                || correct_count(&trial, &orbit_targets, want) != before + 1
            {
                if std::env::var("RDBG").is_ok() {
                    eprintln!("[cd] constructive orbit {key:?}: conjugation replay mismatch");
                }
                return None;
            }
            moves.extend(primitive);
        }
        solved_keys.insert(key);
        if face_center_cells(target, n).into_iter().any(|cell| {
            solved_keys.contains(&center_orbit_key(cell, n)) && color_at(&trial, cell) != want
        }) {
            return None;
        }
    }
    Some(moves)
}

/// Solve all six centres. Returns the moves; the cube is left centres-solved.
pub fn solve_centers(cube: &mut StickerCube) -> Vec<Move> {
    let n = cube.size().get();
    let mut moves = Vec::new();
    if n <= 2 {
        return moves;
    }
    moves.extend(orient_fixed_centers(cube));

    let library_started = std::time::Instant::now();
    let lib = library(n);
    let library_elapsed = library_started.elapsed();
    let dbg = std::env::var("RDBG").is_ok();
    if dbg {
        eprintln!("[cd] n={n} library={} build={library_elapsed:?}", lib.len());
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
        if !super::reduction_checkpoint() {
            return moves;
        }
        let w = order[fi];
        let face_started = std::time::Instant::now();
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
                c.permutation().all(|(dst, src)| {
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
            for cell in cy.support() {
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
            if !super::reduction_checkpoint() {
                return moves;
            }
            let cc = correct_count(cube, &w_cells, want);
            if cc == w_cells.len() {
                if dbg {
                    eprintln!(
                        "[cd]   face {w:?} solved in {iters} iters ({:?})",
                        face_started.elapsed()
                    );
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
            let wrong_cells: Vec<Cell> = w_cells
                .iter()
                .copied()
                .filter(|&cell| color_at(cube, cell) != want)
                .collect();

            // Direct placement: try wrong cells in deterministic order until one has a
            // safe filler. The old first-wrong-only policy could declare a whole face
            // stalled while hundreds of other cells were still directly placeable.
            let empty = Vec::new();
            let mut best: Option<(&Cyc, usize)> = None;
            for &candidate in &wrong_cells {
                // The library is sorted by support/word length. Inspect a bounded
                // number of eligible fillers instead of rescanning every cycle that
                // touches one cell merely to prove no later cycle has one more gain.
                // Constructive transport remains the completeness fallback.
                let mut eligible = 0usize;
                for cy in touch.get(&candidate).unwrap_or(&empty) {
                    let Some(src) = cy.src_into(candidate) else {
                        continue;
                    };
                    if w_set.contains(&src) || color_at(cube, src) != want {
                        continue; // src must be a reservoir cell holding `want`
                    }
                    eligible += 1;

                    // Fuse preservation and gain scoring over exact target-face
                    // dst→src pairs. A correct target may churn only when it still
                    // receives `want`; wrong targets receiving `want` count as gain.
                    let mut gain = 0usize;
                    let mut regresses = false;
                    for (destination, source) in cy.permutation() {
                        if !w_set.contains(&destination) {
                            continue;
                        }
                        let destination_correct = color_at(cube, destination) == want;
                        let source_correct = color_at(cube, source) == want;
                        if destination_correct && !source_correct {
                            regresses = true;
                            break;
                        }
                        if !destination_correct && source_correct {
                            gain += 1;
                        }
                    }
                    if !regresses {
                        let replace = best.is_none_or(|(previous, previous_gain)| {
                            gain > previous_gain
                                || (gain == previous_gain && cy.moves.len() < previous.moves.len())
                        });
                        if replace {
                            best = Some((cy, gain));
                        }
                    }
                    if eligible >= 128 {
                        break;
                    }
                }
                if best.is_some() {
                    break;
                }
            }
            if let Some((cycle, _)) = best {
                apply_all(cube, &cycle.moves);
                moves.extend_from_slice(&cycle.moves);
                guard = 0;
                continue;
            }
            let t = wrong_cells[0];
            if let Some(seq) = two_step(cube, &touch, &w_set, t, want) {
                apply_all(cube, &seq);
                moves.extend(seq);
                guard = 0;
                continue;
            }

            // Prefer the polynomial exact construction over the legacy bounded
            // bridge searches. At N=24+ those searches consumed tens of seconds
            // proving failure before reaching the complete orbit transporter.
            if fi + 2 < order.len() {
                if let Some(seq) =
                    solve_center_orbits_constructively(cube, &safe, w, &order[fi + 1..], n, want)
                {
                    if !seq.is_empty() {
                        apply_all(cube, &seq);
                        moves.extend(seq);
                        guard = 0;
                        continue;
                    }
                }
            }

            // Bridge for the last-cell case: a safe cycle `c1` (which may temporarily
            // disturb the working face — both legs keep every finalised face intact)
            // then a safe cycle `c2` touching `t`, netting `t` correct without losing
            // ground. This is the "break and restore" the single steps can't do. The
            // result is PREDICTED by composing the cycles' permutations (no cube
            // clone), so it stays fast even over the whole safe set.
            let base = correct_count(cube, &w_cells, want);
            let src_through = |cy: &Cyc, x: Cell| cy.src_into(x).unwrap_or(x);
            let empty: Vec<&Cyc> = Vec::new();
            let mut bridged = false;
            'bridge: for c1 in &safe {
                if !super::reduction_checkpoint() {
                    return moves;
                }
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

            // Exact visible-orbit fallback for the final two center colors.
            if fi + 2 == order.len() {
                if let Some(seq) =
                    solve_last_two_center_orbits(cube, &safe, w, &order[fi + 1..], n, want)
                {
                    if !seq.is_empty() {
                        apply_all(cube, &seq);
                        moves.extend(seq);
                        guard = 0;
                        continue;
                    }
                }
            }

            guard += 1;
            if guard > cap {
                if dbg {
                    let wrong: Vec<_> = w_cells
                        .iter()
                        .copied()
                        .filter(|&cell| color_at(cube, cell) != want)
                        .map(|cell| (cell, color_at(cube, cell)))
                        .collect();
                    eprintln!(
                        "[cd]   GIVE UP on {w:?} (#{fi}): correct={}/{} safe={} wrong={wrong:?}",
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
        cy.support()
            .any(|x| Some(x) != except && w_set.contains(&x) && color_at(cube, x) == want)
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
        match stager.src_into(cell) {
            Some(source) => color_at(cube, source),
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
    if *budget <= 0 || !super::reduction_checkpoint() {
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
            for x in cy.support() {
                let s = cy.src_into(x).unwrap_or(x);
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

    #[test]
    fn cancelled_build_does_not_poison_center_cache() {
        let control = crate::reduction::ReductionControl::unlimited();
        control.cancel();
        crate::reduction::with_reduction_control(&control, || {
            assert!(library(127).is_empty());
        });
        LIB_CACHE.with(|cache| assert!(!cache.borrow().contains_key(&127)));
    }

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
    fn physical_center_orbit_keys_have_expected_sizes() {
        for n in [4usize, 5, 6, 7, 24] {
            let mut groups: HashMap<_, usize> = HashMap::new();
            for cell in all_center_cells(n) {
                *groups.entry(center_orbit_key(cell, n)).or_default() += 1;
            }
            let radial = if n.is_multiple_of(2) {
                n / 2 - 1
            } else {
                (n - 3) / 2
            };
            let expected = if n.is_multiple_of(2) {
                radial * radial
            } else {
                radial * radial + radial + 1
            };
            assert_eq!(groups.len(), expected, "orbit count at N={n}");
            assert!(
                groups.values().all(|&count| count == 24 || count == 6),
                "invalid physical orbit cardinality at N={n}: {groups:?}"
            );
            if n.is_multiple_of(2) {
                assert!(groups.values().all(|&count| count == 24));
            } else {
                assert_eq!(groups.values().filter(|&&count| count == 6).count(), 1);
            }
        }
    }

    #[test]
    #[ignore = "N=24 structural center-transport diagnostic; run in release mode"]
    fn n24_safe_center_action_is_orbit_connected() {
        let n = 24usize;
        let frozen = [Face::Up, Face::Down];
        let active = [Face::Front, Face::Back, Face::Left, Face::Right];
        let library = library(n);
        let mut cells_by_orbit: std::collections::BTreeMap<_, Vec<Cell>> = Default::default();
        for cell in all_center_cells(n) {
            if active.contains(&cell.0) {
                cells_by_orbit
                    .entry(center_orbit_key(cell, n))
                    .or_default()
                    .push(cell);
            }
        }
        assert_eq!(cells_by_orbit.len(), 121);
        assert!(cells_by_orbit.values().all(|cells| cells.len() == 16));

        let mut disconnected = Vec::new();
        let mut connected = 0usize;
        for (key, cells) in cells_by_orbit {
            let indexes: HashMap<Cell, usize> = cells
                .iter()
                .copied()
                .enumerate()
                .map(|(index, cell)| (cell, index))
                .collect();
            let mut parent: Vec<usize> = (0..cells.len()).collect();
            fn root(parent: &mut [usize], mut index: usize) -> usize {
                while parent[index] != index {
                    parent[index] = parent[parent[index]];
                    index = parent[index];
                }
                index
            }
            let mut generators = 0usize;
            let mut pure_cycles = 0usize;
            for cycle in library.iter() {
                let safe = cycle
                    .permutation()
                    .all(|(dst, src)| !frozen.contains(&dst.0) || dst.0 == src.0);
                if !safe {
                    continue;
                }
                let pairs: Vec<_> = cycle
                    .permutation()
                    .filter(|(destination, _)| indexes.contains_key(destination))
                    .collect();
                if pairs.is_empty() {
                    continue;
                }
                generators += 1;
                if cycle.support_len() == 3 {
                    pure_cycles += 1;
                }
                for (dst, src) in pairs {
                    let a = root(&mut parent, indexes[&dst]);
                    let b = root(&mut parent, indexes[&src]);
                    parent[a] = b;
                }
            }
            let components = (0..cells.len())
                .map(|index| root(&mut parent, index))
                .collect::<HashSet<_>>()
                .len();
            if components == 1 && pure_cycles > 0 {
                connected += 1;
            } else {
                disconnected.push((key, components, generators, pure_cycles));
            }
        }
        println!("connected physical center orbits: {connected}/121");
        assert!(
            disconnected.is_empty(),
            "safe center actions lack connectivity/3-cycle seeds: {disconnected:?}"
        );
    }

    #[test]
    fn known_conjugate_permutation_matches_full_composition() {
        for n in [4usize, 5, 8] {
            let centers = all_center_cells(n);
            let probes = build_probes(n, &centers);
            let core = base_candidates(n)
                .into_iter()
                .find(|sequence| !sequence.is_empty())
                .unwrap();
            let setup = Move::face(Face::Up, CubeSize::new(n).unwrap(), 1);
            let word = conjugate(&[setup], &core);
            let mut move_perms = HashMap::new();
            for &mv in word.iter().chain(core.iter()) {
                move_perms
                    .entry(move_key(&mv))
                    .or_insert_with(|| single_move_perm(&centers, &probes, mv));
                let inverse = mv.inverse();
                move_perms
                    .entry(move_key(&inverse))
                    .or_insert_with(|| single_move_perm(&centers, &probes, inverse));
            }
            let core_perm = seq_perm(&move_perms, centers.len(), &core);
            let derived = conjugated_perm(
                &move_perms[&move_key(&setup)],
                &core_perm,
                &move_perms[&move_key(&setup.inverse())],
            );
            let composed = seq_perm(&move_perms, centers.len(), &word);
            assert_eq!(derived, composed, "conjugate direction mismatch at N={n}");
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
    fn constructive_center_transport_replays_on_4x4() {
        let n = 4usize;
        let size = CubeSize::new(n).unwrap();
        let lib = library(n);
        let safe: Vec<&Cyc> = lib.iter().collect();
        let solved = StickerCube::solved(size);
        let (mut cube, disturbance) = lib
            .iter()
            .filter(|cycle| {
                cycle.support_len() == 3
                    && cycle.support().any(|cell| cell.0 == Face::Up)
                    && cycle.support().any(|cell| cell.0 != Face::Up)
            })
            .filter_map(|cycle| {
                let disturbance: Vec<Move> =
                    cycle.moves.iter().rev().map(|mv| mv.inverse()).collect();
                let mut cube = solved.clone();
                apply_all(&mut cube, &disturbance);
                (!face_center_cells(Face::Up, n)
                    .iter()
                    .all(|&cell| color_at(&cube, cell) == Face::Up.color()))
                .then_some((cube, disturbance))
            })
            .next()
            .expect("library has a cross-face pure center cycle");
        let reservoirs = [Face::Down, Face::Front, Face::Back, Face::Left, Face::Right];
        let solution = solve_center_orbits_constructively(
            &cube,
            &safe,
            Face::Up,
            &reservoirs,
            n,
            Face::Up.color(),
        )
        .expect("constructive transport path");
        assert!(!solution.is_empty());
        apply_all(&mut cube, &solution);
        assert!(
            face_center_cells(Face::Up, n)
                .iter()
                .all(|&cell| color_at(&cube, cell) == Face::Up.color()),
            "constructive transport did not restore the target center"
        );
        let mut replay = solved;
        apply_all(&mut replay, &disturbance);
        apply_all(&mut replay, &solution);
        assert_eq!(replay.clone_snapshot(), cube.clone_snapshot());
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
