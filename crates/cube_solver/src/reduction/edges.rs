//! Stage 2 of reduction: pair the wing pieces of every edge into solved
//! composite edges, without disturbing the already-solved centers.
//!
//! WORK IN PROGRESS — geometry + exploration scaffolding first.

use super::*;
use cube_core::{Color, CubeSize, Face, Move, StickerCube};

/// The twelve edges, each identified 0..12. For edge `e` and wing index
/// `t` in `1..=n-2`, [`wing_cells`] returns the two stickers (one per adjacent
/// face) of that wing. `faceA`/`faceB` solved colors define the edge's identity.
pub(crate) const EDGE_FACES: [(Face, Face); 12] = [
    (Face::Up, Face::Front),    // 0  UF
    (Face::Up, Face::Back),     // 1  UB
    (Face::Down, Face::Front),  // 2  DF
    (Face::Down, Face::Back),   // 3  DB
    (Face::Front, Face::Right), // 4 FR
    (Face::Back, Face::Right),  // 5  BR
    (Face::Front, Face::Left),  // 6  FL
    (Face::Back, Face::Left),   // 7  BL
    (Face::Up, Face::Right),    // 8  UR
    (Face::Up, Face::Left),     // 9  UL
    (Face::Down, Face::Right),  // 10 DR
    (Face::Down, Face::Left),   // 11 DL
];

/// Sticker positions `((faceA,rowA,colA),(faceB,rowB,colB))` of wing `t` on edge
/// `e`. Derived from cube_core's `face_cell_to_coord` and checked against the 3×3
/// `EDGE_FACELET` table.
pub(crate) fn wing_cells(
    e: usize,
    t: usize,
    n: usize,
) -> ((Face, usize, usize), (Face, usize, usize)) {
    let l = n - 1;
    match e {
        0 => ((Face::Up, l, t), (Face::Front, 0, t)),      // UF
        1 => ((Face::Up, 0, t), (Face::Back, 0, l - t)),   // UB
        2 => ((Face::Down, 0, t), (Face::Front, l, t)),    // DF
        3 => ((Face::Down, l, t), (Face::Back, l, l - t)), // DB
        4 => ((Face::Front, l - t, l), (Face::Right, l - t, 0)), // FR
        5 => ((Face::Back, l - t, 0), (Face::Right, l - t, l)), // BR
        6 => ((Face::Front, l - t, 0), (Face::Left, l - t, l)), // FL
        7 => ((Face::Back, l - t, l), (Face::Left, l - t, 0)), // BL
        8 => ((Face::Up, t, l), (Face::Right, 0, l - t)),  // UR
        9 => ((Face::Up, t, 0), (Face::Left, 0, t)),       // UL
        10 => ((Face::Down, l - t, l), (Face::Right, l, l - t)), // DR
        11 => ((Face::Down, l - t, 0), (Face::Left, l, t)), // DL
        _ => unreachable!("edge id 0..12"),
    }
}

/// Colors `(colorA, colorB)` currently on wing `t` of edge `e`.
fn wing_colors(cube: &StickerCube, e: usize, t: usize) -> (Color, Color) {
    let ((fa, ra, ca), (fb, rb, cb)) = wing_cells(e, t, cube.size().get());
    (
        cube.color_at(fa, ra, ca).unwrap(),
        cube.color_at(fb, rb, cb).unwrap(),
    )
}

/// True if edge slot `e` is uniform: every wing shows the same `(colorA, colorB)`
/// pair in the same orientation. A uniform slot behaves as a single 3×3 edge.
pub(crate) fn edge_uniform(cube: &StickerCube, e: usize) -> bool {
    let n = cube.size().get();
    if n <= 2 {
        return true;
    }
    let first = wing_colors(cube, e, 1);
    (2..=n - 2).all(|t| wing_colors(cube, e, t) == first)
}

/// True if all twelve composite edges are uniform.
pub fn edges_paired(cube: &StickerCube) -> bool {
    (0..12).all(|e| edge_uniform(cube, e))
}

/// True if wing `t` of edge `e` shows that edge's solved colors, correctly
/// oriented (color A on face A, color B on face B).
fn wing_correct(cube: &StickerCube, e: usize, t: usize) -> bool {
    let (fa, fb) = EDGE_FACES[e];
    wing_colors(cube, e, t) == (fa.color(), fb.color())
}

fn count_correct_wings(cube: &StickerCube, n: usize) -> usize {
    let mut k = 0;
    for e in 0..12 {
        for t in 1..=n - 2 {
            if wing_correct(cube, e, t) {
                k += 1;
            }
        }
    }
    k
}

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state >> 33
}

/// Center-safe pure wing 3-cycles: `[Sy_d, f U f']` (an inner Y-slice commutated
/// with a face-shifted U turn). Found by exploration to move exactly three wings
/// while leaving every center solid. Conjugated by setups they reach all wings.
fn wing_base_cycles(n: usize) -> Vec<Vec<Move>> {
    let size = CubeSize::new(n).expect("size>=2");
    let mut out = Vec::new();
    for d in 1..=n - 2 {
        let a = slice_from(Face::Up, n, d, 1);
        for f in [Face::Front, Face::Back, Face::Left, Face::Right] {
            for ut in [1i8, -1] {
                let inner = conjugate(&[Move::face(f, size, 1)], &[Move::face(Face::Up, size, ut)]);
                out.push(commutator(&[a], &inner));
            }
        }
    }
    out
}

/// Setups (whole-cube rotations + short face/slice words) used to re-aim the wing
/// 3-cycles. Conjugating a center-safe base by any sequence stays center-safe.
fn edge_setups(n: usize) -> Vec<Vec<Move>> {
    let size = CubeSize::new(n).expect("size>=2");
    let mut setups = super::centers::cube_rotations(n);
    let mut singles: Vec<Move> = Face::ALL
        .iter()
        .flat_map(|&f| {
            [1i8, -1, 2]
                .into_iter()
                .map(move |t| Move::face(f, size, t))
        })
        .collect();
    for f in [Face::Up, Face::Right, Face::Front] {
        for d in 1..=n - 2 {
            for s in [1i8, -1] {
                singles.push(slice_from(f, n, d, s));
            }
        }
    }
    setups.push(Vec::new());
    for &a in &singles {
        setups.push(vec![a]);
    }
    let faces: Vec<Move> = Face::ALL
        .iter()
        .flat_map(|&f| [1i8, -1].into_iter().map(move |t| Move::face(f, size, t)))
        .collect();
    let slices: Vec<Move> = singles
        .iter()
        .copied()
        .filter(|m| m.layer_start != 0 && m.layer_end != n - 1)
        .collect();
    for &s in &slices {
        for &f in &faces {
            setups.push(vec![s, f]);
            setups.push(vec![f, s]);
        }
    }
    setups
}

fn wing_repertoire(n: usize) -> Vec<Vec<Move>> {
    let setups = edge_setups(n);
    let bases = wing_base_cycles(n);
    let mut out = Vec::with_capacity(setups.len() * bases.len());
    for s in &setups {
        for b in &bases {
            out.push(conjugate(s, b));
        }
    }
    out
}

/// Wing slot index `0..12*(n-2)`: `i = e*(n-2) + (t-1)`.
fn wing_decode(i: usize, n: usize) -> (usize, usize) {
    (i / (n - 2), i % (n - 2) + 1)
}

/// Oriented colour pair on every wing slot, in index order.
fn all_wing_colors(cube: &StickerCube, n: usize) -> Vec<(Color, Color)> {
    let total = 12 * (n - 2);
    (0..total)
        .map(|i| {
            let (e, t) = wing_decode(i, n);
            wing_colors(cube, e, t)
        })
        .collect()
}

/// True if wing slot `i` shows its solved colours in solved orientation.
fn wing_idx_correct(cube: &StickerCube, n: usize, i: usize) -> bool {
    let (e, t) = wing_decode(i, n);
    wing_correct(cube, e, t)
}

/// Stage 2: pair every edge's wings into solved (home + oriented) composite edges
/// without disturbing the solved centres. Center-safe wing 3-cycles are precomputed
/// with the wing slots they change (their support, orientation-aware), so each step
/// only tries cycles touching a still-wrong wing instead of scanning the whole
/// repertoire — the same support-filter that made the centre solver fast. Even cubes
/// can leave an OLL/PLL parity for the 3×3-finish stage to fix.
pub fn solve_edges(cube: &mut StickerCube) -> Vec<Move> {
    let n = cube.size().get();
    let mut moves = Vec::new();
    if n <= 3 {
        return moves;
    }
    let total = 12 * (n - 2);

    let solved = StickerCube::solved(cube.size());
    let solved_wings = all_wing_colors(&solved, n);
    // Center-safe cycles + the wing slots each changes on a solved cube.
    let cands: Vec<(Vec<Move>, Vec<usize>)> = wing_repertoire(n)
        .into_iter()
        .filter_map(|seq| {
            let mut c = solved.clone();
            apply_all(&mut c, &seq);
            if !centers_solved(&c) {
                return None;
            }
            let after = all_wing_colors(&c, n);
            let support: Vec<usize> = (0..total)
                .filter(|&i| after[i] != solved_wings[i])
                .collect();
            if support.is_empty() {
                None
            } else {
                Some((seq, support))
            }
        })
        .collect();

    let wrong_set = |cube: &StickerCube| -> std::collections::HashSet<usize> {
        (0..total)
            .filter(|&i| !wing_idx_correct(cube, n, i))
            .collect()
    };

    let escape_limit = n * n * 10 + 200;
    let mut escapes = 0usize;
    let mut rng = 0xD1B54A32D192ED03u64 ^ (n as u64);
    let mut iters = 0usize;
    let iter_cap = total * 60 + 2500; // hard bound so a hard scramble fails fast

    while count_correct_wings(cube, n) < total {
        iters += 1;
        if iters > iter_cap {
            return moves;
        }
        let baseline = count_correct_wings(cube, n);
        let wrong = wrong_set(cube);
        let touch: Vec<&(Vec<Move>, Vec<usize>)> = cands
            .iter()
            .filter(|(_, s)| s.iter().any(|i| wrong.contains(i)))
            .collect();

        // 1-ply: the FIRST support-touching cycle that raises the correct-wing
        // count (first-improvement — far fewer trial applications than best-of-all,
        // which dominated the runtime when many wings are still wrong).
        let mut found = None;
        for (seq, _) in &touch {
            let mut trial = cube.clone();
            apply_all(&mut trial, seq);
            if count_correct_wings(&trial, n) > baseline {
                found = Some(seq);
                break;
            }
        }
        if let Some(seq) = found {
            apply_all(cube, seq);
            moves.extend_from_slice(seq);
            escapes = 0;
            continue;
        }

        escapes += 1;
        if escapes > escape_limit {
            return moves; // give up; callers verify edges_paired
        }

        // 2-ply bridge over the (small) support-touching set. Bounded so a large
        // touch set (early, when many wings are wrong) can't make this quadratic.
        let mut bridged = false;
        'bridge: for (c1, _) in touch.iter().take(60) {
            let mut t1 = cube.clone();
            apply_all(&mut t1, c1);
            if count_correct_wings(&t1, n) < baseline {
                continue;
            }
            let w1 = wrong_set(&t1);
            for (c2, s2) in touch.iter().take(60) {
                if !s2.iter().any(|i| w1.contains(i)) {
                    continue;
                }
                let mut t2 = t1.clone();
                apply_all(&mut t2, c2);
                if count_correct_wings(&t2, n) > baseline {
                    apply_all(cube, c1);
                    moves.extend_from_slice(c1);
                    apply_all(cube, c2);
                    moves.extend_from_slice(c2);
                    bridged = true;
                    break 'bridge;
                }
            }
        }
        if bridged {
            escapes = 0;
            continue;
        }

        // Neutral escape: a non-regressing touching cycle to reshuffle.
        let neutral: Vec<&Vec<Move>> = touch
            .iter()
            .filter_map(|(seq, _)| {
                let mut t = cube.clone();
                apply_all(&mut t, seq);
                if count_correct_wings(&t, n) >= baseline {
                    Some(seq)
                } else {
                    None
                }
            })
            .collect();
        if neutral.is_empty() {
            return moves;
        }
        let pick = neutral[(lcg(&mut rng) as usize) % neutral.len()];
        apply_all(cube, pick);
        moves.extend_from_slice(pick);
    }
    moves
}

#[cfg(test)]
mod explore {
    use super::*;
    use cube_core::{Axis, CubeSize, Move};

    /// Build a "piece id" for every wing sticker location so we can track motion.
    /// Returns a vector of (colorA,colorB) for all 12*(n-2) wings in order.
    fn wing_signature(cube: &StickerCube) -> Vec<(Color, Color)> {
        let n = cube.size().get();
        let mut v = Vec::new();
        for e in 0..12 {
            for t in 1..=n - 2 {
                v.push(wing_colors(cube, e, t));
            }
        }
        v
    }

    fn solved(n: usize) -> StickerCube {
        StickerCube::solved(CubeSize::new(n).unwrap())
    }

    /// Geometry self-check: on a solved cube every wing shows its two face colors,
    /// and every edge is uniform.
    #[test]
    fn solved_cube_edges_uniform_and_colored() {
        for n in [4usize, 5, 6, 7] {
            let cube = solved(n);
            assert!(edges_paired(&cube), "solved n={n} must be paired");
            for (e, &(fa, fb)) in EDGE_FACES.iter().enumerate() {
                for t in 1..=n - 2 {
                    let (ca, cb) = wing_colors(&cube, e, t);
                    assert_eq!(ca, fa.color(), "edge {e} t{t} A color (n={n})");
                    assert_eq!(cb, fb.color(), "edge {e} t{t} B color (n={n})");
                }
            }
        }
    }

    /// Apply a move list and report how many wing slots changed signature, how
    /// many center cells broke, and whether corners moved. Used to hunt for
    /// center-safe pure wing 3-cycles.
    #[allow(dead_code)]
    fn analyze(n: usize, moves: &[Move]) -> (usize, bool) {
        let mut cube = solved(n);
        let before = wing_signature(&cube);
        apply_all(&mut cube, moves);
        let after = wing_signature(&cube);
        let changed = before
            .iter()
            .zip(after.iter())
            .filter(|(a, b)| a != b)
            .count();
        let centers_ok = centers_solved(&cube);
        (changed, centers_ok)
    }

    fn lcg(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state >> 33
    }

    fn scramble(n: usize, seed: u64, depth: usize) -> StickerCube {
        let size = CubeSize::new(n).unwrap();
        let mut cube = StickerCube::solved(size);
        let mut rng = seed;
        for _ in 0..depth {
            let f = [
                Face::Up,
                Face::Down,
                Face::Front,
                Face::Back,
                Face::Left,
                Face::Right,
            ][(lcg(&mut rng) % 6) as usize];
            let width = 1 + (lcg(&mut rng) % (n as u64 - 1)) as usize; // 1..=n-1
            let turns = [1i8, -1, 2][(lcg(&mut rng) % 3) as usize];
            cube.apply_move(Move::wide(f, size, width, turns)).unwrap();
        }
        cube
    }

    /// Count changed cells (total and center-only) for candidate center 3-cycle
    /// commutators, to confirm which are clean pure center 3-cycles.
    #[test]
    #[ignore = "debug; run manually with --ignored"]
    fn verify_center_base_cycles() {
        let n = 5;
        let count_changes = |moves: &[Move]| -> (usize, usize) {
            let base = solved(n);
            let mut c = base.clone();
            apply_all(&mut c, moves);
            let bs = base.clone_snapshot();
            let cs = c.clone_snapshot();
            let total = bs
                .stickers()
                .iter()
                .zip(cs.stickers())
                .filter(|(a, b)| a != b)
                .count();
            // center-only changes:
            let mut centers = 0;
            for f in [
                Face::Up,
                Face::Down,
                Face::Front,
                Face::Back,
                Face::Left,
                Face::Right,
            ] {
                for r in 0..n {
                    for cc in 0..n {
                        if super::super::is_center_cell(r, cc, n)
                            && base.color_at(f, r, cc) != c.color_at(f, r, cc)
                        {
                            centers += 1;
                        }
                    }
                }
            }
            (total, centers)
        };
        // [X-slice, Y-slice]
        let a = slice_from(Face::Right, n, 1, 1);
        let b = slice_from(Face::Up, n, 1, 1);
        let ss = commutator(&[a], &[b]);
        println!("[Xslice,Yslice] total/centers = {:?}", count_changes(&ss));
        // [X-slice, U face]
        let sf = commutator(&[a], &[Move::face(Face::Up, CubeSize::new(n).unwrap(), 1)]);
        println!("[Xslice,Uface] total/centers = {:?}", count_changes(&sf));
        // [X-slice depth1, Z-slice depth1]
        let zc = commutator(&[a], &[slice_from(Face::Front, n, 1, 1)]);
        println!("[Xslice,Zslice] total/centers = {:?}", count_changes(&zc));
    }

    /// Search for a pure 3-cell center-only cycle (commutator of an inner slice
    /// with a face-conjugated inner slice is the classic clean center 3-cycle).
    /// Collect distinct band-preserving (U,D-fixed) center 3-cycles and report
    /// how many distinct cell-triples they reach across the F/B/L/R band.
    #[test]
    #[ignore = "exploration; run manually with --ignored"]
    fn hunt_band_center_3cycles() {
        let n = 4;
        let size = CubeSize::new(n).unwrap();
        // Band-preserving generators: Y-slices + all six outer face turns.
        let mut gens: Vec<Move> = Vec::new();
        for s in [1i8, -1] {
            for d in 1..=n - 2 {
                gens.push(slice_from(Face::Up, n, d, s));
            }
            for f in [
                Face::Up,
                Face::Down,
                Face::Front,
                Face::Back,
                Face::Left,
                Face::Right,
            ] {
                gens.push(Move::face(f, size, s));
            }
        }
        // cell signature of the center-cell permutation: which (face,r,c) changed.
        let triple = |moves: &[Move]| -> Option<Vec<(usize, usize, usize)>> {
            let base = solved(n);
            let mut c = base.clone();
            apply_all(&mut c, moves);
            let mut cells = Vec::new();
            for (fi, f) in [
                Face::Up,
                Face::Down,
                Face::Front,
                Face::Back,
                Face::Left,
                Face::Right,
            ]
            .into_iter()
            .enumerate()
            {
                for r in 0..n {
                    for cc in 0..n {
                        if super::super::is_center_cell(r, cc, n)
                            && base.color_at(f, r, cc) != c.color_at(f, r, cc)
                        {
                            cells.push((fi, r, cc));
                        }
                    }
                }
            }
            if cells.len() == 3 {
                // verify order-3 on centers and U,D preserved
                let mut c3 = solved(n);
                for _ in 0..3 {
                    apply_all(&mut c3, moves);
                }
                let ud_ok = [Face::Up, Face::Down].iter().all(|&f| {
                    (0..n).all(|r| {
                        (0..n).all(|cc| {
                            !super::super::is_center_cell(r, cc, n)
                                || c.color_at(f, r, cc) == Some(f.color())
                        })
                    })
                });
                if ud_ok && c3.clone_snapshot().stickers() == solved(n).clone_snapshot().stickers()
                {
                    return Some(cells);
                }
            }
            None
        };
        use std::collections::HashSet;
        let mut triples: HashSet<Vec<(usize, usize, usize)>> = HashSet::new();
        let mut covered: HashSet<(usize, usize, usize)> = HashSet::new();
        // Base = nested form [Sy_a, f Sy_b f'] (the only band center 3-cycle).
        let mut bases: Vec<Vec<Move>> = Vec::new();
        for a in 1..=n - 2 {
            for b in 1..=n - 2 {
                if a == b {
                    continue;
                }
                for f in [Face::Front, Face::Back, Face::Left, Face::Right] {
                    for ct in [1i8, -1] {
                        let sa = slice_from(Face::Up, n, a, 1);
                        let sb = slice_from(Face::Up, n, b, 1);
                        let shifted = conjugate(&[Move::face(f, size, ct)], &[sb]);
                        bases.push(commutator(&[sa], &shifted));
                    }
                }
            }
        }
        // Conjugate each base by all band generators (and identity).
        let mut setups: Vec<Vec<Move>> = vec![Vec::new()];
        for g in &gens {
            setups.push(vec![*g]);
        }
        for g1 in &gens {
            for g2 in &gens {
                setups.push(vec![*g1, *g2]);
            }
        }
        for base in &bases {
            for s in &setups {
                let cand = conjugate(s, base);
                if let Some(mut t) = triple(&cand) {
                    t.sort();
                    for &cell in &t {
                        covered.insert(cell);
                    }
                    triples.insert(t);
                }
            }
        }
        println!("distinct band center 3-cycles: {}", triples.len());
        println!("band center cells covered: {} (of 16)", covered.len());
        let mut samples: Vec<_> = triples.iter().take(8).collect();
        samples.sort();
        for t in samples {
            println!("triple {t:?}");
        }
    }

    #[test]
    #[ignore = "exploration; run manually with --ignored"]
    fn hunt_center_3cycle() {
        let n = 6;
        let size = CubeSize::new(n).unwrap();
        let faces = [
            Face::Up,
            Face::Down,
            Face::Front,
            Face::Back,
            Face::Left,
            Face::Right,
        ];
        let changes = |moves: &[Move]| -> (usize, usize) {
            let base = solved(n);
            let mut c = base.clone();
            apply_all(&mut c, moves);
            let total = base
                .clone_snapshot()
                .stickers()
                .iter()
                .zip(c.clone_snapshot().stickers())
                .filter(|(a, b)| a != b)
                .count();
            let mut centers = 0;
            for f in faces {
                for r in 0..n {
                    for cc in 0..n {
                        if super::super::is_center_cell(r, cc, n)
                            && base.color_at(f, r, cc) != c.color_at(f, r, cc)
                        {
                            centers += 1;
                        }
                    }
                }
            }
            (total, centers)
        };
        let mut found = 0;
        // A = inner slice; B = f · (inner slice) · f'  (conjugated slice).
        let mut slices = Vec::new();
        for &f in &faces {
            for d in 1..=n - 2 {
                for s in [1i8, -1] {
                    slices.push(slice_from(f, n, d, s));
                }
            }
        }
        'outer: for a in &slices {
            for sb in &slices {
                for &cf in &faces {
                    for ct in [1i8, -1, 2] {
                        let bconj = conjugate(&[Move::face(cf, size, ct)], &[*sb]);
                        let comm = commutator(&[*a], &bconj);
                        let (total, centers) = changes(&comm);
                        if total == 3 && centers == 3 {
                            // verify order 3
                            let mut c3 = solved(n);
                            for _ in 0..3 {
                                apply_all(&mut c3, &comm);
                            }
                            if c3.clone_snapshot().stickers()
                                == solved(n).clone_snapshot().stickers()
                            {
                                let notation: Vec<String> =
                                    comm.iter().map(|m| m.notation(size)).collect();
                                println!("3CYCLE [{}]", notation.join(" "));
                                found += 1;
                                if found >= 6 {
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            }
        }
        println!("center 3-cycles found: {found}");
    }

    #[test]
    #[ignore = "debug; run manually with --ignored"]
    fn centers_debug_one() {
        let n = 4;
        let mut cube = scramble(n, 0x1234, 40);
        let _ = super::super::solve_centers(&mut cube);
        for f in [
            Face::Up,
            Face::Down,
            Face::Front,
            Face::Back,
            Face::Left,
            Face::Right,
        ] {
            let want = f.color();
            let mut correct = 0;
            for r in 0..n {
                for c in 0..n {
                    if super::super::is_center_cell(r, c, n) && cube.color_at(f, r, c) == Some(want)
                    {
                        correct += 1;
                    }
                }
            }
            println!(
                "face {f:?}: {correct}/{} center cells correct",
                (n - 2) * (n - 2)
            );
        }
        println!("centers_solved={}", centers_solved(&cube));
    }

    /// Centers THEN edges: scramble, solve centers, solve edges, assert paired.
    #[test]
    #[ignore = "probe; run manually with --ignored"]
    fn edges_probe() {
        for n in [4usize] {
            let trials = 3u64;
            let mut ok = 0;
            for seed in 0..trials {
                let mut cube = scramble(n, 0x55 + seed, 40);
                let _mc = super::super::solve_centers(&mut cube);
                if !centers_solved(&cube) {
                    println!("n={n} seed={seed}: centers FAILED");
                    continue;
                }
                let me = super::super::solve_edges(&mut cube);
                let paired = edges_paired(&cube);
                let centers_ok = centers_solved(&cube);
                println!(
                    "n={n} seed={seed}: paired={paired} centers_ok={centers_ok} (edge moves={})",
                    me.len()
                );
                if paired && centers_ok {
                    ok += 1;
                }
            }
            println!("n={n}: edges {ok}/{trials}");
        }
    }

    #[test]
    #[ignore = "probe; run manually with --ignored"]
    fn centers_reliability_probe() {
        for n in [4usize, 5] {
            let mut ok = 0;
            let trials = 10;
            for seed in 0..trials {
                let mut cube = scramble(n, 0x1234 + seed, 40);
                let _ = super::super::solve_centers(&mut cube);
                if centers_solved(&cube) {
                    ok += 1;
                }
            }
            println!("n={n}: centers solved {ok}/{trials}");
        }
    }

    /// Which wing slots changed signature, and is it order-3 on wings (a clean
    /// 3-cycle)? Centers must stay solved.
    fn wing_effect(n: usize, moves: &[Move]) -> Option<usize> {
        let mut c1 = solved(n);
        let before = wing_signature(&c1);
        apply_all(&mut c1, moves);
        if !centers_solved(&c1) {
            return None;
        }
        let after = wing_signature(&c1);
        let changed = before.iter().zip(&after).filter(|(a, b)| a != b).count();
        if changed != 3 {
            return None;
        }
        // order-3 on wings: applying 3× restores all wing signatures.
        let mut c3 = solved(n);
        for _ in 0..3 {
            apply_all(&mut c3, moves);
        }
        if wing_signature(&c3) == before {
            Some(changed)
        } else {
            None
        }
    }

    #[test]
    #[ignore = "exploration harness; run manually with --ignored"]
    fn hunt_center_safe_wing_cycles() {
        let n = 6;
        let size = CubeSize::new(n).unwrap();
        let faces = [
            Face::Up,
            Face::Down,
            Face::Front,
            Face::Back,
            Face::Left,
            Face::Right,
        ];
        // Generator pool: inner single slices, outer turns, wide moves width 2..3.
        let mut gens: Vec<Move> = Vec::new();
        for &f in &faces {
            for s in [1i8, -1] {
                for d in 1..=n - 2 {
                    gens.push(slice_from(f, n, d, s));
                }
                gens.push(Move::face(f, size, s));
                for w in 2..=3 {
                    gens.push(Move::wide(f, size, w, s));
                }
            }
        }

        let mut found = 0;
        let outers: Vec<Move> = faces
            .iter()
            .flat_map(|&f| [1i8, -1].into_iter().map(move |t| Move::face(f, size, t)))
            .collect();
        // Nested form [a, f b f'] (the shape that yields clean 3-cycles) plus the
        // plain commutator, optionally wrapped by an outer setup.
        'search: for a in &gens {
            for b in &gens {
                let mut cands: Vec<Vec<Move>> = vec![commutator(&[*a], &[*b])];
                for f in &outers {
                    let shifted = conjugate(&[*f], &[*b]);
                    cands.push(commutator(&[*a], &shifted));
                }
                for cand in &cands {
                    if wing_effect(n, cand).is_some() {
                        let notation: Vec<String> = cand.iter().map(|m| m.notation(size)).collect();
                        println!("PURE-3CYCLE [{}]", notation.join(" "));
                        found += 1;
                        if found >= 10 {
                            break 'search;
                        }
                    }
                }
            }
        }
        println!("total pure center-safe wing 3-cycles found: {found}");
        let _ = Axis::X;
    }
}
