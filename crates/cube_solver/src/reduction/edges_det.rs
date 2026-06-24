//! Deterministic edge-pairing for arbitrary NxN cubes.
//!
//! Edge pairing is a *fungible* placement problem just like centres: the `n-2`
//! wings of an edge are interchangeable (same colour pair), so a wing slot is
//! "correct" once it shows that edge's home oriented colour pair. So we reuse the
//! centre solver's framework — probe cubes give the exact permutation a cycle
//! performs, and we place pieces deterministically — but here a piece is a *wing*
//! and it carries an orientation (it can arrive flipped), so we track each wing
//! slot's two stickers and a flip bit.
//!
//! Parity (an odd wing permutation a 3×3 can't have — OLL/PLL) is handled one level
//! up by `finish::solve_reduction`, which toggles it with an inner slice.

use super::edges::wing_repertoire;
use super::{apply_all, centers_solved};
use cube_core::{Color, CubeSize, CubeState, Face, Move, StickerCube};

/// A sticker address.
type Cell = (Face, usize, usize);

fn face_ord(f: Face) -> usize {
    Face::ALL.iter().position(|&x| x == f).unwrap()
}

/// Number of wing slots: 12 edges × (n-2) wings.
fn n_slots(n: usize) -> usize {
    12 * (n - 2)
}

/// The two stickers of every wing slot, flattened: indices `2*i` and `2*i+1` are the
/// A- and B-face stickers of slot `i = e*(n-2) + (t-1)`. Reuses `wing_cells`.
fn wing_sticker_cells(n: usize) -> Vec<Cell> {
    let mut v = Vec::with_capacity(2 * n_slots(n));
    for e in 0..12 {
        for t in 1..=n - 2 {
            let ((fa, ra, ca), (fb, rb, cb)) = super::edges::wing_cells(e, t, n);
            v.push((fa, ra, ca));
            v.push((fb, rb, cb));
        }
    }
    v
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

/// Probe cubes labelling each wing sticker (in `cells`) with a unique base-6 id.
fn build_probes(n: usize, cells: &[Cell]) -> Vec<StickerCube> {
    let ndigits = {
        let mut d = 1;
        let mut cap = 6usize;
        while cap < cells.len() {
            cap *= 6;
            d += 1;
        }
        d
    };
    (0..ndigits)
        .map(|k| {
            let mut names: Vec<&str> = vec![COLOR_NAMES[0]; 6 * n * n];
            for (id, &(f, r, c)) in cells.iter().enumerate() {
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

type MoveKey = (u8, usize, usize, i8);
fn move_key(m: &Move) -> MoveKey {
    let axis = match m.axis {
        cube_core::Axis::X => 0,
        cube_core::Axis::Y => 1,
        cube_core::Axis::Z => 2,
    };
    (axis, m.layer_start, m.layer_end, m.turns)
}

/// Permutation of a single move over the sticker list: `perm[dst] = src`.
fn single_move_perm(cells: &[Cell], probes: &[StickerCube], m: Move) -> Vec<usize> {
    let applied: Vec<StickerCube> = probes
        .iter()
        .map(|p| {
            let mut c = p.clone();
            c.apply_move(m).expect("valid move");
            c
        })
        .collect();
    let mut perm = vec![0usize; cells.len()];
    for (i, &(f, r, c)) in cells.iter().enumerate() {
        let mut id = 0usize;
        for (k, probe) in applied.iter().enumerate() {
            id += color_index(probe.color_at(f, r, c).unwrap()) * 6usize.pow(k as u32);
        }
        perm[i] = if id < cells.len() { id } else { i };
    }
    perm
}

fn compose(a: &[usize], b: &[usize]) -> Vec<usize> {
    b.iter().map(|&bi| a[bi]).collect()
}

fn seq_perm(
    move_perms: &std::collections::HashMap<MoveKey, Vec<usize>>,
    n: usize,
    seq: &[Move],
) -> Vec<usize> {
    let mut r: Vec<usize> = (0..n).collect();
    for m in seq {
        r = compose(&r, &move_perms[&move_key(m)]);
    }
    r
}

/// A center-safe wing cycle reduced to its action on wing slots: for slot `dst`,
/// `(src, flip)` = the slot whose wing lands there and whether it arrives flipped.
struct WingCyc {
    moves: Vec<Move>,
    /// `slot_src[dst] = (src_slot, flipped)`; identity slots omitted.
    map: Vec<(usize, usize, bool)>, // (dst, src, flip)
    /// Slots the cycle touches (the destinations in `map`).
    support: Vec<usize>,
    /// `dst -> (src, flip)` for O(1) lookup.
    smap: std::collections::HashMap<usize, (usize, bool)>,
}

impl WingCyc {
    /// `(src, flip)` of the wing that lands in `t` when this cycle is applied.
    fn src_into(&self, t: usize) -> Option<(usize, bool)> {
        self.smap.get(&t).copied()
    }
}

/// Reverse a colour pair iff `flip`.
fn orient(p: (Color, Color), flip: bool) -> (Color, Color) {
    if flip {
        (p.1, p.0)
    } else {
        p
    }
}

/// From a sticker permutation over the 2·slots wing stickers, derive the per-slot
/// `(src, flip)`. Sticker `2*i` is slot i's A-face; its source `2*j (+1)` tells the
/// source slot `j` and whether the wing flipped (A came from a B sticker).
fn slot_map(sticker_perm: &[usize], slots: usize) -> Vec<(usize, usize, bool)> {
    let mut out = Vec::new();
    for dst in 0..slots {
        let src_sticker = sticker_perm[2 * dst];
        let src = src_sticker / 2;
        let flip = src_sticker % 2 == 1;
        if src != dst || flip {
            out.push((dst, src, flip));
        }
    }
    out
}

/// Build the wing-cycle library for size `n`: center-safe sequences (preserve solved
/// centres) reduced to their slot action, deduped by exact effect, and *enriched with
/// meta-commutators*. Commutators `[P,Q]` of two short center-safe wing cycles are
/// themselves center-safe and — when `P,Q` overlap — confine their net effect to a few
/// slots (often a fresh pure 3-cycle at a slot-triple the raw repertoire misses). This is
/// the same trick that cracked the last two centres, and it gives the last-edges coverage
/// the raw conjugated 3-cycles lack.
fn build_library(n: usize) -> Vec<WingCyc> {
    use super::{commutator, conjugate};
    let cells = wing_sticker_cells(n);
    let probes = build_probes(n, &cells);
    let size = CubeSize::new(n).unwrap();
    let solved = StickerCube::solved(size);
    let slots = n_slots(n);
    let nstick = cells.len();
    let rep = wing_repertoire(n);

    // Decode each elementary move's wing-sticker permutation once (with inverses); all
    // sequences (incl. commutators/conjugates) are composed from these — no cube clones.
    let mut move_perms: std::collections::HashMap<MoveKey, Vec<usize>> = Default::default();
    let decode = |m: Move, mp: &mut std::collections::HashMap<MoveKey, Vec<usize>>| {
        mp.entry(move_key(&m))
            .or_insert_with(|| single_move_perm(&cells, &probes, m));
    };
    for seq in &rep {
        for &m in seq {
            decode(m, &mut move_perms);
            decode(m.inverse(), &mut move_perms);
        }
    }
    // Face turns re-aim the metas for full coverage (they keep centres solid).
    let face_turns: Vec<Move> = Face::ALL
        .iter()
        .flat_map(|&f| {
            [1i8, -1, 2]
                .into_iter()
                .map(move |t| Move::face(f, size, t))
        })
        .collect();
    for &m in &face_turns {
        decode(m, &mut move_perms);
        decode(m.inverse(), &mut move_perms);
    }

    let ident: Vec<usize> = (0..nstick).collect();
    let mv_key = |mv: &[Move]| -> Vec<String> { mv.iter().map(|x| x.notation(size)).collect() };

    // Dedup by exact wing-sticker permutation, keeping the shortest move sequence.
    let mut by_perm: std::collections::HashMap<Vec<usize>, Vec<Move>> = Default::default();
    let consider =
        |seq: &[Move], by_perm: &mut std::collections::HashMap<Vec<usize>, Vec<Move>>| {
            let p = seq_perm(&move_perms, nstick, seq);
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

    // Raw repertoire (center-safe only).
    for seq in &rep {
        let mut c = solved.clone();
        apply_all(&mut c, seq);
        if centers_solved(&c) {
            consider(seq, &mut by_perm);
        }
    }

    // Meta-commutators from the shortest cycles, confined to ≤6 slots, re-aimed by face
    // turns. (Metas/face-conjugations of center-safe cycles stay center-safe.)
    let mut short: Vec<Vec<Move>> = by_perm.values().cloned().collect();
    short.sort_by_key(|s| (s.len(), mv_key(s)));
    short.truncate(120);
    for p_seq in &short {
        for q_seq in &short {
            let meta = commutator(p_seq, q_seq);
            let mp = seq_perm(&move_perms, nstick, &meta);
            let sup = slot_map(&mp, slots).len();
            if mp == ident || sup == 0 || sup > 6 {
                continue;
            }
            consider(&meta, &mut by_perm);
            for ft in &face_turns {
                consider(&conjugate(&[*ft], &meta), &mut by_perm);
            }
        }
    }

    // Smallest-support first; cap the large-support (general) cycles, keep small ones all.
    let mut raw: Vec<(usize, Vec<usize>, Vec<Move>)> = by_perm
        .into_iter()
        .map(|(p, m)| (slot_map(&p, slots).len(), p, m))
        .collect();
    raw.sort_by(|(sa, pa, ma), (sb, pb, mb)| {
        (sa, ma.len())
            .cmp(&(sb, mb.len()))
            .then_with(|| pa.cmp(pb))
            .then_with(|| mv_key(ma).cmp(&mv_key(mb)))
    });
    let cap_gen = 9000usize;
    let mut ngen = 0usize;
    let mut out = Vec::new();
    for (sup, p, moves) in raw {
        let map = slot_map(&p, slots);
        if map.is_empty() {
            continue;
        }
        if sup > 4 {
            if ngen >= cap_gen {
                continue;
            }
            ngen += 1;
        }
        let support: Vec<usize> = map.iter().map(|&(d, _, _)| d).collect();
        let smap = map.iter().map(|&(d, s, f)| (d, (s, f))).collect();
        out.push(WingCyc {
            moves,
            map,
            support,
            smap,
        });
    }
    out
}

thread_local! {
    static LIB_CACHE: std::cell::RefCell<std::collections::HashMap<usize, std::rc::Rc<Vec<WingCyc>>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}
fn library(n: usize) -> std::rc::Rc<Vec<WingCyc>> {
    LIB_CACHE.with(|c| {
        c.borrow_mut()
            .entry(n)
            .or_insert_with(|| std::rc::Rc::new(build_library(n)))
            .clone()
    })
}

/// Home oriented colour pair of wing slot `i = e*(n-2)+(t-1)`.
fn home_pair(n: usize, i: usize) -> (Color, Color) {
    let e = i / (n - 2);
    let (fa, fb) = super::edges::EDGE_FACES[e];
    (fa.color(), fb.color())
}

fn cur_pair(cube: &StickerCube, cells: &[Cell], i: usize) -> (Color, Color) {
    let a = cells[2 * i];
    let b = cells[2 * i + 1];
    (
        cube.color_at(a.0, a.1, a.2).unwrap(),
        cube.color_at(b.0, b.1, b.2).unwrap(),
    )
}

/// Stage a wing so a direct placement into `t` becomes possible: a "filler" cycle
/// places into `t` from source slot `slot` (needing oriented pair `need` there), and a
/// "stager" cycle loads `need` into `slot` without breaking anything already correct.
/// Both validated by predicting oriented pairs through the cycles' slot maps (the final
/// pick is replay-verified). Mirrors `centers_det::two_step`, with a flip bit.
fn two_step_e(
    cube: &StickerCube,
    cells: &[Cell],
    home: &[(Color, Color)],
    touch: &std::collections::HashMap<usize, Vec<&WingCyc>>,
    t: usize,
) -> Option<Vec<Move>> {
    let pair = |i: usize| cur_pair(cube, cells, i);
    let correct = |i: usize| pair(i) == home[i];
    let breaks_correct = |cy: &WingCyc, except: Option<usize>| {
        cy.support.iter().any(|&x| Some(x) != except && correct(x))
    };
    let empty = Vec::new();
    // Source slots that, loaded with `need`, let a filler place into `t`.
    let mut fillers: Vec<(&WingCyc, usize, (Color, Color))> = Vec::new();
    for cy in touch.get(&t).unwrap_or(&empty) {
        let (src, flip) = cy.src_into(t).unwrap();
        if correct(src) || breaks_correct(cy, Some(t)) {
            continue;
        }
        // orient(need, flip) == home[t]  ⇒  need = orient(home[t], flip) (orient is involutive).
        fillers.push((cy, src, orient(home[t], flip)));
    }
    // Oriented pair predicted at `cell` after applying `stager`.
    let pred = |stager: &WingCyc, cell: usize| -> (Color, Color) {
        let (s, f) = stager.src_into(cell).unwrap_or((cell, false));
        orient(pair(s), f)
    };
    for &(filler, slot, need) in &fillers {
        for stager in touch.get(&slot).unwrap_or(&empty) {
            if breaks_correct(stager, None) || pred(stager, slot) != need {
                continue;
            }
            if pred(stager, t) == home[t] {
                continue; // stager alone fixes t — direct placement will catch it
            }
            let mut seq = stager.moves.clone();
            seq.extend_from_slice(&filler.moves);
            let base = (0..home.len()).filter(|&i| correct(i)).count();
            let mut trial = cube.clone();
            apply_all(&mut trial, &seq);
            if (0..home.len())
                .filter(|&i| cur_pair(&trial, cells, i) == home[i])
                .count()
                > base
            {
                return Some(seq);
            }
        }
    }
    None
}

/// `(orig_slot, cumulative_flip)` currently shown at `w` under a composed sequence.
fn cur_under(cum: &std::collections::HashMap<usize, (usize, bool)>, w: usize) -> (usize, bool) {
    cum.get(&w).copied().unwrap_or((w, false))
}

/// Compose `cy` *after* the sequence represented by `cum`: returns the updated map.
fn compose_cum(
    cum: &std::collections::HashMap<usize, (usize, bool)>,
    cy: &WingCyc,
) -> std::collections::HashMap<usize, (usize, bool)> {
    let mut next = cum.clone();
    // For each slot the cycle fills, it shows whatever was at its source under `cum`.
    let updates: Vec<(usize, (usize, bool))> = cy
        .support
        .iter()
        .map(|&x| {
            let (src, f) = cy.smap[&x];
            let (s0, f0) = cur_under(cum, src);
            (x, (s0, f0 ^ f))
        })
        .collect();
    for (x, v) in updates {
        if v == (x, false) {
            next.remove(&x);
        } else {
            next.insert(x, v);
        }
    }
    next
}

/// Count slots whose predicted oriented pair (under `cum`) already equals home.
fn correct_under(
    cube: &StickerCube,
    cells: &[Cell],
    home: &[(Color, Color)],
    cum: &std::collections::HashMap<usize, (usize, bool)>,
    slots: usize,
) -> usize {
    (0..slots)
        .filter(|&w| {
            let (s, f) = cur_under(cum, w);
            orient(cur_pair(cube, cells, s), f) == home[w]
        })
        .count()
}

/// Depth-limited search for when direct/2-step/bridge can't place a wing: shuffle it
/// through several positions, composing slot maps (predicted, no cube clone), up to
/// `maxd` deep within a node budget. Returns the moves of a sequence that strictly
/// increases the paired-slot count. Mirrors `centers_det::search_bridge` with a flip
/// bit. The focus each level is the first slot still wrong under the accumulated
/// permutation; we only try cycles that touch it.
#[allow(clippy::too_many_arguments)]
fn search_bridge_e(
    cube: &StickerCube,
    cells: &[Cell],
    home: &[(Color, Color)],
    touch: &std::collections::HashMap<usize, Vec<&WingCyc>>,
    slots: usize,
    maxd: usize,
    budget: usize,
) -> Option<Vec<Move>> {
    let base = correct_under(cube, cells, home, &std::collections::HashMap::new(), slots);
    let mut budget = budget as i64;
    let mut path: Vec<Move> = Vec::new();
    fn dfs(
        cube: &StickerCube,
        cells: &[Cell],
        home: &[(Color, Color)],
        touch: &std::collections::HashMap<usize, Vec<&WingCyc>>,
        slots: usize,
        cum: &std::collections::HashMap<usize, (usize, bool)>,
        depth: usize,
        maxd: usize,
        base: usize,
        budget: &mut i64,
        path: &mut Vec<Move>,
    ) -> bool {
        if *budget <= 0 || depth >= maxd {
            return false;
        }
        let empty: Vec<&WingCyc> = Vec::new();
        let Some(t) = (0..slots).find(|&w| {
            let (s, f) = cur_under(cum, w);
            orient(cur_pair(cube, cells, s), f) != home[w]
        }) else {
            return false;
        };
        for cy in touch.get(&t).unwrap_or(&empty) {
            if *budget <= 0 {
                return false;
            }
            *budget -= 1;
            let next = compose_cum(cum, cy);
            path.extend_from_slice(&cy.moves);
            if correct_under(cube, cells, home, &next, slots) > base {
                return true;
            }
            if dfs(
                cube,
                cells,
                home,
                touch,
                slots,
                &next,
                depth + 1,
                maxd,
                base,
                budget,
                path,
            ) {
                return true;
            }
            path.truncate(path.len() - cy.moves.len());
        }
        false
    }
    let cum = std::collections::HashMap::new();
    if dfs(
        cube,
        cells,
        home,
        touch,
        slots,
        &cum,
        0,
        maxd,
        base,
        &mut budget,
        &mut path,
    ) {
        Some(path)
    } else {
        None
    }
}

/// Deterministically pair every edge (each wing slot shows its home oriented pair),
/// preserving the solved centres. Returns the moves applied. Reaches a stall only on
/// the genuine wing parity, which `solve_reduction` resolves with an inner slice.
///
/// Mirrors `centers_det::solve_centers`: a fungible placement over wing slots, with the
/// added flip bit a wing carries. Escalates direct placement → `two_step` staging →
/// a permutation-*predicted* two-cycle bridge (composes slot maps, no cube clone, so it
/// scans the whole library cheaply).
pub fn solve_edges(cube: &mut StickerCube) -> Vec<Move> {
    let n = cube.size().get();
    if n <= 3 {
        return Vec::new();
    }
    solve_to_target(cube, &home_targets(n))
}

/// The home oriented pair of every wing slot, in slot order.
pub(crate) fn home_targets(n: usize) -> Vec<(Color, Color)> {
    (0..n_slots(n)).map(|i| home_pair(n, i)).collect()
}

/// Home targets with edges `a` and `b` exchanged: every wing of group `a` is asked to
/// show edge `b`'s home pair and vice versa. Reaching this from all-home is an even wing
/// permutation for even `n` (`n-2` transpositions), so the placement can get there — and
/// it leaves the dedge permutation odd (one transposition), flipping the PLL parity bit.
pub(crate) fn home_swapped_target(n: usize, a: usize, b: usize) -> Vec<(Color, Color)> {
    let mut t = home_targets(n);
    let w = n - 2;
    let pa = home_pair(n, a * w);
    let pb = home_pair(n, b * w);
    for k in 0..w {
        t[a * w + k] = pb;
        t[b * w + k] = pa;
    }
    t
}

/// True once every wing slot shows its `target` oriented pair.
pub(crate) fn at_target(cube: &StickerCube, target: &[(Color, Color)]) -> bool {
    let cells = wing_sticker_cells(cube.size().get());
    (0..target.len()).all(|i| cur_pair(cube, &cells, i) == target[i])
}

/// Like `solve_edges`, but drives every wing slot to an arbitrary per-slot oriented pair
/// `target` (not necessarily home). Used to place two edges *swapped*, which flips the
/// dedge-permutation parity to clear the odd-corner PLL case on even cubes.
pub(crate) fn solve_to_target(cube: &mut StickerCube, target: &[(Color, Color)]) -> Vec<Move> {
    let n = cube.size().get();
    let mut moves = Vec::new();
    if n <= 3 {
        return moves;
    }
    let lib = library(n);
    let cells = wing_sticker_cells(n);
    let slots = n_slots(n);
    let home: &[(Color, Color)] = target;

    // Cycles touching each slot (as a destination of their action).
    let mut touch: std::collections::HashMap<usize, Vec<&WingCyc>> = Default::default();
    for cy in lib.iter() {
        for &d in &cy.support {
            touch.entry(d).or_default().push(cy);
        }
    }
    let empty: Vec<&WingCyc> = Vec::new();
    let pair = |cube: &StickerCube, i: usize| cur_pair(cube, &cells, i);
    let correct = |cube: &StickerCube, i: usize| pair(cube, i) == home[i];

    while let Some(t) = (0..slots).find(|&i| !correct(cube, i)) {
        // 1) Direct placement: a cycle that lands `home[t]` (accounting for its flip)
        // at `t` from a non-correct reservoir slot, disturbing nothing already correct.
        let mut placed = false;
        for cy in touch.get(&t).unwrap_or(&empty) {
            let (src, flip) = cy.src_into(t).unwrap();
            if orient(pair(cube, src), flip) != home[t] || correct(cube, src) {
                continue;
            }
            if cy.support.iter().any(|&x| x != t && correct(cube, x)) {
                continue;
            }
            apply_all(cube, &cy.moves);
            moves.extend_from_slice(&cy.moves);
            placed = true;
            break;
        }
        if placed {
            continue;
        }

        // 2) Two-step staging.
        if let Some(seq) = two_step_e(cube, &cells, home, &touch, t) {
            apply_all(cube, &seq);
            moves.extend(seq);
            continue;
        }

        // 3) Predicted bridge: a cycle `c1` (break-and-restore is allowed; net progress
        // is what matters) then a `t`-touching `c2`, with the composite result predicted
        // by chaining slot maps (XORing flips) — no cube clone, so the whole library is
        // affordable. Commit only if the correct count strictly rises.
        let base = (0..slots).filter(|&i| correct(cube, i)).count();
        let mut bridged = false;
        'bridge: for c1 in lib.iter() {
            for c2 in touch.get(&t).unwrap_or(&empty) {
                let pred = |w: usize| -> (Color, Color) {
                    let (s2, f2) = c2.src_into(w).unwrap_or((w, false));
                    let (s1, f1) = c1.src_into(s2).unwrap_or((s2, false));
                    orient(pair(cube, s1), f1 ^ f2)
                };
                if pred(t) != home[t] {
                    continue;
                }
                if (0..slots).filter(|&w| pred(w) == home[w]).count() > base {
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
            continue;
        }

        // 4) DFS backstop for the hard last-wings coverage gaps: shuffle a wing through
        // several positions, composing slot maps (predicted), bounded by a node budget.
        if let Some(seq) = search_bridge_e(cube, &cells, home, &touch, slots, 6, 60_000) {
            apply_all(cube, &seq);
            moves.extend(seq);
            continue;
        }

        // A genuine stall: odd wing parity (no even cycle sequence fixes it). The caller
        // clears it by toggling wing parity with an inner slice and re-reducing.
        return moves;
    }
    moves
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solved(n: usize) -> StickerCube {
        StickerCube::solved(CubeSize::new(n).unwrap())
    }

    /// The slot (src,flip) map must match what the cube actually does: applying a
    /// cycle to a solved cube, slot `dst` ends up showing the home colours of `src`
    /// (reversed iff flipped).
    #[test]
    fn slot_map_is_correct() {
        for n in [4usize, 5] {
            let lib = build_library(n);
            assert!(!lib.is_empty(), "empty library n={n}");
            let base = solved(n);
            for cy in lib.iter().take(150) {
                let mut c = base.clone();
                apply_all(&mut c, &cy.moves);
                for &(dst, src, flip) in &cy.map {
                    // colours now at dst's two stickers
                    let ((da, db), (sa, sb)) = (slot_pair_cells(n, dst), slot_pair_cells(n, src));
                    let now = (
                        c.color_at(da.0, da.1, da.2).unwrap(),
                        c.color_at(db.0, db.1, db.2).unwrap(),
                    );
                    // home colours of src (on a solved cube the stickers at src show src's home pair)
                    let home_src = (
                        base.color_at(sa.0, sa.1, sa.2).unwrap(),
                        base.color_at(sb.0, sb.1, sb.2).unwrap(),
                    );
                    let expect = if flip {
                        (home_src.1, home_src.0)
                    } else {
                        home_src
                    };
                    assert_eq!(now, expect, "n={n} dst={dst} src={src} flip={flip}");
                }
            }
            println!("n={n}: slot_map verified, library={}", lib.len());
        }
    }

    fn slot_pair_cells(n: usize, slot: usize) -> (Cell, Cell) {
        let cells = wing_sticker_cells(n);
        (cells[2 * slot], cells[2 * slot + 1])
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

    /// Scramble, solve centres, then deterministically pair edges; report how often
    /// `edges_paired` succeeds without disturbing the centres.
    #[test]
    #[ignore = "measurement"]
    fn pair_rate() {
        use super::super::{centers_solved, edges_paired, solve_centers};
        for n in [4usize, 5] {
            let mut paired = 0;
            let mut fails = Vec::new();
            let t0 = std::time::Instant::now();
            let trials = 16u64;
            for seed in 0..trials {
                let mut cube = scramble(n, 0x900 + seed, 40);
                solve_centers(&mut cube);
                if !centers_solved(&cube) {
                    fails.push((seed, "centres"));
                    continue;
                }
                solve_edges(&mut cube);
                if edges_paired(&cube) && centers_solved(&cube) {
                    paired += 1;
                } else {
                    fails.push((
                        seed,
                        if edges_paired(&cube) {
                            "centres-broken"
                        } else {
                            "unpaired"
                        },
                    ));
                }
            }
            println!(
                "n={n}: edges paired {paired}/{trials} ({:?}); fails {fails:?}",
                t0.elapsed()
            );
        }
    }

    /// For odd cubes (no 3×3 parity), how close does a single `solve_edges` pass get to
    /// all-home? If it reaches all-but-2 (a wing transposition), the stall is parity and
    /// one slice toggle fixes it; if it stalls far short, it is a coverage gap.
    #[test]
    #[ignore = "diagnostic"]
    fn n5_closeness() {
        use super::super::{centers_solved, edges_paired, solve_centers};
        let n = 5usize;
        let slots = n_slots(n);
        let cells = wing_sticker_cells(n);
        let home: Vec<(Color, Color)> = (0..slots).map(|i| home_pair(n, i)).collect();
        for seed in 0..12u64 {
            let mut cube = scramble(n, 0x500 + seed, n * 15);
            solve_centers(&mut cube);
            if !centers_solved(&cube) {
                println!("seed {seed}: centres FAILED");
                continue;
            }
            solve_edges(&mut cube);
            let at_home = (0..slots)
                .filter(|&i| cur_pair(&cube, &cells, i) == home[i])
                .count();
            println!(
                "seed {seed}: home {at_home}/{slots}, paired={}, centres_ok={}",
                edges_paired(&cube),
                centers_solved(&cube),
            );
        }
    }
}
