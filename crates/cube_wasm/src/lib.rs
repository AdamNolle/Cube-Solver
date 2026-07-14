//! WebAssembly bridge exposing the real `cube_core` model and `cube_solver`
//! engines to the web UI. Single-threaded (the `parallel` feature of
//! `cube_solver` is disabled), so the solver race runs sequentially in the
//! browser — same results, just serial.

use cube_core::{Axis, Color, CubeSize, CubeState, Face, Move, StickerCube};
use cube_solver::{run_solver_lab_tiered, wide_move_set, SolverBudget};
use std::time::Duration;
use wasm_bindgen::prelude::*;

/// Stable 0..6 index for a color (matches the JS palette order).
fn color_index(c: Color) -> u8 {
    match c {
        Color::White => 0,
        Color::Yellow => 1,
        Color::Green => 2,
        Color::Blue => 3,
        Color::Orange => 4,
        Color::Red => 5,
    }
}

fn color_from_index(index: u8) -> Option<Color> {
    Some(match index {
        0 => Color::White,
        1 => Color::Yellow,
        2 => Color::Green,
        3 => Color::Blue,
        4 => Color::Orange,
        5 => Color::Red,
        _ => return None,
    })
}

fn axis_index(a: Axis) -> u8 {
    match a {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    }
}

thread_local! {
    /// The two-phase 3×3 solver builds a few MB of lookup tables once; cache it on
    /// the (single) worker thread so only the first solve pays the build cost.
    static KOCIEMBA: std::cell::OnceCell<cube_solver::kociemba::search::Solver> =
        const { std::cell::OnceCell::new() };
}

fn with_kociemba<R>(f: impl FnOnce(&cube_solver::kociemba::search::Solver) -> R) -> R {
    KOCIEMBA.with(|c| f(c.get_or_init(cube_solver::kociemba::search::Solver::new)))
}

/// Build the 3×3 two-phase tables ahead of the first solve. The worker calls this
/// once at startup so the first real Solve isn't slowed by the ~one-time table build.
#[wasm_bindgen]
pub fn warm_solver() {
    with_kociemba(|_| {});
}

/// A tiny deterministic RNG (xorshift64*) so scrambles are reproducible by seed
/// without pulling OS entropy.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform integer in `0..n` via Lemire's debiased multiply-shift (no modulo
    /// bias). `n` is always >= 1 at every call site.
    fn below(&mut self, n: usize) -> usize {
        ((self.next() as u128 * n as u128) >> 64) as usize
    }
}

#[wasm_bindgen]
pub struct CubeLab {
    cube: StickerCube,
    n: usize,
    solution: Vec<Move>,
}

#[wasm_bindgen]
impl CubeLab {
    #[wasm_bindgen(constructor)]
    pub fn new(n: usize) -> CubeLab {
        #[cfg(target_arch = "wasm32")]
        console_error_panic_hook::set_once();
        let n = n.clamp(2, 2000);
        let size = CubeSize::new(n).expect("valid size");
        CubeLab {
            cube: StickerCube::solved(size),
            n,
            solution: Vec::new(),
        }
    }

    pub fn set_size(&mut self, n: usize) {
        let n = n.clamp(2, 2000);
        if n != self.n {
            self.n = n;
            self.cube = StickerCube::solved(CubeSize::new(n).expect("valid size"));
            self.solution.clear();
        }
    }

    pub fn size(&self) -> usize {
        self.n
    }

    pub fn reset(&mut self) {
        self.cube = StickerCube::solved(CubeSize::new(self.n).expect("valid size"));
        self.solution.clear();
    }

    pub fn is_solved(&self) -> bool {
        self.cube.is_solved()
    }

    pub fn solved_percent(&self) -> f64 {
        let total = (6 * self.n * self.n) as f64;
        let mism = self.cube.mismatch_count() as f64;
        (100.0 * (1.0 - mism / total)).clamp(0.0, 100.0)
    }

    /// Visible (surface) pieces: N³ minus the hidden inner core.
    pub fn piece_count(&self) -> usize {
        let n = self.n;
        if n < 2 {
            0
        } else {
            n * n * n - (n - 2) * (n - 2) * (n - 2)
        }
    }

    /// Span of wide moves used for this cube (1 for the 3×3, wider for big cubes).
    fn scramble_span(&self) -> usize {
        if self.n <= 3 {
            1
        } else {
            3.min(self.n - 1)
        }
    }

    /// Scramble from solved with `depth` random moves; returns the count applied.
    /// Reproducible for a given `seed`.
    pub fn scramble(&mut self, depth: usize, seed: u64) -> usize {
        let size = CubeSize::new(self.n).expect("valid size");
        self.cube = StickerCube::solved(size);
        self.solution.clear();
        let moves = wide_move_set(size, self.scramble_span());
        if moves.is_empty() {
            return 0;
        }
        let mut rng = Rng::new(seed ^ 0x9E37_79B9_7F4A_7C15);
        let mut applied = 0usize;
        let mut last: Option<Move> = None;
        let mut guard = 0usize;
        while applied < depth && guard < depth * 20 + 50 {
            guard += 1;
            let mv = moves[rng.below(moves.len())];
            if last
                .map(|p| p.axis == mv.axis && p.layer_start == mv.layer_start)
                .unwrap_or(false)
            {
                continue;
            }
            if self.cube.apply_move(mv).is_ok() {
                applied += 1;
                last = Some(mv);
            }
        }
        applied
    }

    /// Flat color indices (0..6) for rendering: 6 faces × dim × dim, in
    /// `Face::ALL` order (Up, Down, Front, Back, Left, Right), row-major. Faces
    /// larger than `sample` are down-sampled.
    pub fn face_colors(&self, sample: usize) -> Vec<u8> {
        let dim = self.render_dim(sample);
        let mut out = Vec::with_capacity(6 * dim * dim);
        for face in Face::ALL {
            let fs = self.cube.face_sample(face, dim);
            for row in &fs.cells {
                for c in row {
                    out.push(color_index(*c));
                }
            }
        }
        out
    }

    pub fn render_dim(&self, sample: usize) -> usize {
        sample.min(self.n).max(1)
    }

    /// Replace the model from a complete sticker-state buffer in `Face::ALL`
    /// order. The solver worker uses this boundary so it receives no scramble
    /// history to invert—only the same colors visible to the user.
    pub fn load_face_colors(&mut self, colors: &[u8]) -> bool {
        if colors.len() != 6 * self.n * self.n {
            return false;
        }
        let Some(stickers) = colors
            .iter()
            .copied()
            .map(color_from_index)
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        let Ok(snapshot) = serde_json::from_value::<cube_core::CubeSnapshot>(
            serde_json::json!({ "size": self.n, "stickers": stickers }),
        ) else {
            return false;
        };
        let cube = StickerCube::from_snapshot(snapshot);
        if cube.validate().is_err() {
            return false;
        }
        self.cube = cube;
        self.solution.clear();
        true
    }

    /// Apply one quarter turn in the web UI's `{axis, layer, dir}` convention,
    /// which is identical to `cube_core`'s (axes X/Y/Z = 0/1/2, layer index along
    /// the axis, dir ±1 = right-hand quarter turn). Used to mirror the on-screen
    /// scramble into the solver's cube so the returned solution matches exactly.
    pub fn apply_design_move(&mut self, axis: u8, layer: usize, dir: i32) {
        if layer >= self.n {
            return; // ignore out-of-range layers (e.g. a stale move after N shrank)
        }
        let ax = match axis {
            0 => Axis::X,
            1 => Axis::Y,
            _ => Axis::Z,
        };
        let turns: i8 = if dir >= 0 { 1 } else { -1 };
        let mv = Move::new(ax, layer, layer, turns);
        let _ = self.cube.apply_move(mv);
    }

    /// Solve a 3×3 with the two-phase (Kociemba) solver. Returns the same JSON shape
    /// as `solve`, or `None` if conversion/solve fails (then `solve` falls back).
    fn try_kociemba(&mut self) -> Option<String> {
        let solution =
            with_kociemba(|s| cube_solver::kociemba::cube3::solve_sticker(&self.cube, s))?;
        let size = cube_core::CubeSize::new(3).ok()?;
        let mut moves_json: Vec<serde_json::Value> = Vec::new();
        let mut notation: Vec<String> = Vec::new();
        for m in &solution {
            notation.push(m.notation(size));
            let axis = axis_index(m.axis);
            let (count, dir): (usize, i32) = match m.turns.rem_euclid(4) {
                1 => (1, 1),
                2 => (2, 1),
                3 => (1, -1),
                _ => (0, 1),
            };
            for layer in m.layer_start..=m.layer_end {
                for _ in 0..count {
                    moves_json
                        .push(serde_json::json!({ "axis": axis, "layer": layer, "dir": dir }));
                }
            }
        }
        self.solution = solution;
        // Report the standard face-turn (HTM) count — one per notation entry, where a
        // half-turn like U2 is a single move. `moves_json` stays half-turn-expanded
        // (U2 -> two quarter steps) purely so the animation plays each quarter; using
        // its length as the move count would inflate the figure (~34 vs ~22) and
        // disagree with both the README and the 2×2 path (which report HTM moves).
        let htm = notation.len();
        Some(
            serde_json::json!({
                "found": true,
                "winner": "kociemba",
                "moveCount": htm,
                "elapsedMs": 0,
                "moves": moves_json,
                "notation": notation,
                "lanes": [ { "id": "kociemba", "pct": 100, "moveCount": htm, "label": "two-phase", "solved": true } ],
            })
            .to_string(),
        )
    }

    /// Solve a 4×4+ cube with the NxN reduction method (centres → edges → 3×3 finish →
    /// parity). Same JSON shape as `try_kociemba`, or `None` if the reduction fails (then
    /// `solve` falls back to the legacy engine race). Solves a CLONE — `solve_reduction`
    /// mutates its cube to solved — so `self.cube` stays scrambled for the animation.
    fn try_reduction(&mut self) -> Option<String> {
        let original = self.cube.clone();
        let mut work = original.clone();
        // Finish before the browser's worker watchdog so timeout can unwind through
        // reduction loops cleanly; worker termination remains the hard-stop backstop.
        let internal_limit = if self.n <= 5 {
            std::time::Duration::from_secs(28)
        } else if self.n <= 8 {
            std::time::Duration::from_secs(115)
        } else {
            std::time::Duration::from_secs(290)
        };
        let control = cube_solver::reduction::ReductionControl::with_timeout(internal_limit);
        let solution = with_kociemba(|s| {
            cube_solver::reduction::solve_reduction_with_control(&mut work, s, &control)
        })
        .ok()?;
        if !work.is_solved() {
            return None;
        }
        let mut replay = original;
        for &mv in &solution {
            replay.apply_move(mv).ok()?;
        }
        if !replay.is_solved() {
            return None;
        }
        let size = CubeSize::new(self.n).ok()?;
        let mut moves_json: Vec<serde_json::Value> = Vec::new();
        let mut notation: Vec<String> = Vec::new();
        for m in &solution {
            notation.push(m.notation(size));
            let axis = axis_index(m.axis);
            let (count, dir): (usize, i32) = match m.turns.rem_euclid(4) {
                1 => (1, 1),
                2 => (2, 1),
                3 => (1, -1),
                _ => (0, 1),
            };
            for layer in m.layer_start..=m.layer_end {
                for _ in 0..count {
                    moves_json
                        .push(serde_json::json!({ "axis": axis, "layer": layer, "dir": dir }));
                }
            }
        }
        self.solution = solution;
        let htm = notation.len();
        Some(
            serde_json::json!({
                "found": true,
                "winner": "reduction",
                "moveCount": htm,
                "elapsedMs": 0,
                "moves": moves_json,
                "notation": notation,
                "lanes": [ { "id": "reduction", "pct": 100, "moveCount": htm, "label": "reduction", "solved": true } ],
            })
            .to_string(),
        )
    }

    /// Run the solver on the current state and store the winning (fewest-move,
    /// replay-verified) solution. 3×3 uses the two-phase solver; 4×4+ (up to
    /// `MAX_REDUCTION`) uses the reduction method; other sizes use the legacy engine
    /// race. Returns a JSON summary.
    pub fn solve(&mut self, max_depth: usize, time_ms: f64) -> String {
        let snapshot = self.cube.clone_snapshot();
        // 3×3: the two-phase (Kociemba) solver returns a verified solution for ANY
        // scramble — not just the <=9-move ones the legacy search can reach. Fall
        // through to the legacy search only if it somehow fails.
        if self.n == 3 {
            if let Some(json) = self.try_kociemba() {
                return json;
            }
        }
        // 4×4 and up: the reduction method returns a verified real solution. Capped at
        // 11×11 — beyond that the one-time library build + solve exceed a reasonable
        // interactive wait (12×12 ~5 min). This runs in the solver Web Worker, so even a
        // multi-second solve never blocks the UI; larger cubes stay on the visual path.
        const MAX_REDUCTION: usize = 11;
        if self.n > 3 && self.n <= MAX_REDUCTION {
            if let Some(json) = self.try_reduction() {
                return json;
            }
        }
        let depth = max_depth.clamp(1, 9);
        let mut budget = SolverBudget::for_depth(depth);
        budget.time_limit = Duration::from_millis(time_ms.max(50.0) as u64);
        // The web UI scrambles with outer-face turns only, so the solver searches
        // the same (outer) move set — guaranteeing it can invert the scramble.
        budget.max_wide = 1;
        // Scale the search budget with depth so deeper scrambles still solve, but
        // cap it so a hard scramble can never freeze the page for more than a
        // moment (the meet-in-the-middle frontier grows ~18^(depth/2)).
        let half = depth.div_ceil(2) as u32;
        budget.max_nodes = 18usize
            .saturating_pow(half)
            .saturating_mul(2)
            .clamp(400_000, 1_200_000);

        // The exact solver gets the full budget; the heuristic engines get a small
        // slice (just for the race display), so a deep solve never runs all three
        // at full budget.
        let mut secondary = budget;
        secondary.time_limit = Duration::from_millis(280);
        secondary.max_nodes = 60_000;
        let run = run_solver_lab_tiered(snapshot, budget, secondary);

        // Per-worker latest state for the race lanes.
        let lane_ids = ["deterministic", "beam", "evolution"];
        let mut lanes = Vec::new();
        for id in lane_ids {
            let last = run.events.iter().rev().find(|e| e.worker_id == id);
            let solved = run.events.iter().any(|e| {
                e.worker_id == id && e.candidate.as_ref().map(|c| c.solved).unwrap_or(false)
            });
            let (pct, mc, label) = match last {
                Some(e) => (
                    (e.best_fitness.clamp(0.0, 1.0) * 100.0) as i32,
                    if e.best_move_count == usize::MAX {
                        -1
                    } else {
                        e.best_move_count as i32
                    },
                    e.message.clone(),
                ),
                None => (0, -1, "idle".to_string()),
            };
            lanes.push(serde_json::json!({
                "id": id, "pct": pct, "moveCount": mc, "label": label, "solved": solved
            }));
        }

        match run.best {
            Some(best) => {
                self.solution = best.moves.clone();
                // Decompose into single-layer quarter turns {axis, layer, dir} —
                // the format the on-screen cube animates. Parallel layers of a
                // wide turn commute, so splitting is exact.
                let size = CubeSize::new(self.n).unwrap();
                let mut moves: Vec<serde_json::Value> = Vec::new();
                let mut notation: Vec<String> = Vec::new();
                for m in &best.moves {
                    notation.push(m.notation(size));
                    let axis = axis_index(m.axis);
                    let t = m.turns.rem_euclid(4);
                    let (count, dir) = match t {
                        1 => (1, 1),
                        2 => (2, 1),
                        3 => (1, -1),
                        _ => (0, 1),
                    };
                    for layer in m.layer_start..=m.layer_end {
                        for _ in 0..count {
                            moves.push(serde_json::json!({
                                "axis": axis, "layer": layer, "dir": dir
                            }));
                        }
                    }
                }
                serde_json::json!({
                    "found": true,
                    "winner": best.worker_id,
                    "moveCount": best.move_count,
                    "elapsedMs": best.elapsed_ms,
                    "moves": moves,
                    "notation": notation,
                    "lanes": lanes,
                })
                .to_string()
            }
            None => {
                self.solution.clear();
                serde_json::json!({ "found": false, "lanes": lanes }).to_string()
            }
        }
    }

    pub fn solution_len(&self) -> usize {
        self.solution.len()
    }

    /// Apply the `i`-th move of the stored solution to the live cube.
    pub fn apply_solution_step(&mut self, i: usize) -> bool {
        if i < self.solution.len() {
            self.cube.apply_move(self.solution[i]).is_ok()
        } else {
            false
        }
    }
}

// ===================== Evolutionary swarm =====================

/// One learning trial: a candidate move sequence and its current fitness.
struct Member {
    seq: Vec<Move>,
    mismatch: usize,
    flash: u32,
    /// Steps since this trial last improved — used to restart plateaued trials.
    stuck: u32,
    /// Last accepted learning operator: 0=seed, 1=mutation, 2=crossover, 3=restart.
    operator: u8,
}

/// A wall of independent trials all learning to solve the same scramble by
/// mutation/crossover hill-climbing. Each member improves over `step`s; when it
/// reaches solved it flashes, counts as converged, and reseeds with a fresh
/// trial — so the grid is a live picture of evolutionary search.
#[wasm_bindgen]
pub struct Swarm {
    base: StickerCube,
    n: usize,
    moves: Vec<Move>,
    members: Vec<Member>,
    rng: Rng,
    converged: usize,
    max_len: usize,
    scramble_depth: usize,
    seed: u64,
}

impl Swarm {
    fn outer_scramble(n: usize, moves: &[Move], rng: &mut Rng, depth: usize) -> StickerCube {
        let mut cube = StickerCube::solved(CubeSize::new(n).unwrap());
        let mut last: Option<Move> = None;
        let mut applied = 0;
        let mut guard = 0;
        while applied < depth && guard < depth * 12 + 40 {
            guard += 1;
            let mv = moves[rng.below(moves.len())];
            if last
                .map(|p| p.axis == mv.axis && p.layer_start == mv.layer_start)
                .unwrap_or(false)
            {
                continue;
            }
            if cube.apply_move(mv).is_ok() {
                applied += 1;
                last = Some(mv);
            }
        }
        cube
    }

    fn eval(&self, seq: &[Move]) -> usize {
        let mut cube = self.base.clone();
        for mv in seq {
            let _ = cube.apply_move(*mv);
        }
        cube.mismatch_count()
    }

    fn mutate(&mut self, seq: &[Move]) -> Vec<Move> {
        let mut out = seq.to_vec();
        match self.rng.below(4) {
            0 if out.len() < self.max_len => {
                let at = self.rng.below(out.len() + 1);
                out.insert(at, self.moves[self.rng.below(self.moves.len())]);
            }
            1 if out.len() > 1 => {
                let at = self.rng.below(out.len());
                out.remove(at);
            }
            2 if !out.is_empty() => {
                let at = self.rng.below(out.len());
                out[at] = self.moves[self.rng.below(self.moves.len())];
            }
            // Append, but never exceed max_len (replace a random move if full).
            _ => {
                let mv = self.moves[self.rng.below(self.moves.len())];
                if out.len() < self.max_len {
                    out.push(mv);
                } else if !out.is_empty() {
                    let at = self.rng.below(out.len());
                    out[at] = mv;
                }
            }
        }
        out
    }

    fn crossover(&mut self, a: &[Move], b: &[Move]) -> Vec<Move> {
        let i = if a.is_empty() {
            0
        } else {
            self.rng.below(a.len() + 1)
        };
        let j = if b.is_empty() {
            0
        } else {
            self.rng.below(b.len() + 1)
        };
        let mut out: Vec<Move> = a[..i].to_vec();
        out.extend_from_slice(&b[j..]);
        if out.len() > self.max_len {
            out.truncate(self.max_len);
        }
        out
    }
}

#[wasm_bindgen]
impl Swarm {
    #[wasm_bindgen(constructor)]
    pub fn new(count: usize, n: usize, scramble_depth: usize, seed: u64) -> Swarm {
        #[cfg(target_arch = "wasm32")]
        console_error_panic_hook::set_once();
        let n = n.clamp(2, 12);
        let count = count.clamp(1, 400);
        let scramble_depth = scramble_depth.clamp(3, 14);
        let moves = cube_solver::wide_move_set(CubeSize::new(n).unwrap(), 1);
        let mut rng = Rng::new(seed | 1);
        let base = Swarm::outer_scramble(n, &moves, &mut rng, scramble_depth);
        let mut swarm = Swarm {
            base,
            n,
            moves,
            members: Vec::new(),
            rng,
            converged: 0,
            max_len: scramble_depth * 2 + 6,
            scramble_depth,
            seed,
        };
        swarm.fill_population(count);
        swarm
    }

    /// Replace the population with `count` fresh random trials.
    fn fill_population(&mut self, count: usize) {
        self.members.clear();
        // Every trial starts AT the current cube (no moves applied yet), so the
        // whole wall begins as exact copies of the Studio cube and then diverges
        // as each trial learns. mismatch = the cube's own mismatch.
        let base_mismatch = self.base.mismatch_count();
        for _ in 0..count {
            self.members.push(Member {
                seq: Vec::new(),
                mismatch: base_mismatch,
                flash: 0,
                stuck: 0,
                operator: 0,
            });
        }
    }

    fn random_sequence(&mut self) -> Vec<Move> {
        let max = self
            .max_len
            .min(self.scramble_depth.saturating_add(4))
            .max(1);
        let len = 1 + self.rng.below(max);
        let mut seq = Vec::with_capacity(len);
        while seq.len() < len {
            let mv = self.moves[self.rng.below(self.moves.len())];
            let cancels = seq.last().is_some_and(|prev: &Move| {
                prev.axis == mv.axis
                    && prev.layer_start == mv.layer_start
                    && prev.layer_end == mv.layer_end
                    && (prev.turns + mv.turns).rem_euclid(4) == 0
            });
            if !cancels {
                seq.push(mv);
            }
        }
        seq
    }

    /// Restart a plateaued trial in a different part of the search space. Resetting
    /// every member to the same empty sequence recreates the same local optimum;
    /// a random legal genome gives mutation/crossover genuinely new material.
    fn restart_member(&mut self, i: usize, base_mismatch: usize) {
        // Preserve the reliable empty-sequence restart for half the population
        // while the other half explores a fresh genome. This maintains a stable
        // baseline and prevents the whole swarm from collapsing to one lineage.
        let (seq, mismatch) = if i.is_multiple_of(2) {
            (Vec::new(), base_mismatch)
        } else {
            let seq = self.random_sequence();
            let mismatch = self.eval(&seq);
            (seq, mismatch)
        };
        self.members[i].seq = seq;
        self.members[i].mismatch = mismatch;
        self.members[i].flash = u32::from(mismatch == 0) * 26;
        self.members[i].stuck = 0;
        self.members[i].operator = 3;
        if mismatch == 0 {
            self.converged += 1;
        }
    }

    /// Seed the swarm from the Studio's exact scramble (the same `{axis,layer,dir}`
    /// quarter-turns), so every trial is learning to solve the very cube on
    /// screen. The scramble *moves* are only used to build the start state — the
    /// trials never see them, they search from the resulting sticker state.
    pub fn set_scramble(&mut self, axes: &[u8], layers: &[u32], dirs: &[i32]) {
        let mut cube = StickerCube::solved(CubeSize::new(self.n).unwrap());
        let count = axes.len().min(layers.len()).min(dirs.len());
        for i in 0..count {
            let ax = match axes[i] {
                0 => Axis::X,
                1 => Axis::Y,
                _ => Axis::Z,
            };
            let layer = layers[i] as usize;
            if layer >= self.n {
                continue; // skip out-of-range layers rather than silently no-op'ing
            }
            let turns: i8 = if dirs[i] >= 0 { 1 } else { -1 };
            let _ = cube.apply_move(Move::new(ax, layer, layer, turns));
        }
        self.base = cube;
        let n_members = self.members.len().max(1);
        self.converged = 0;
        self.fill_population(n_members);
    }

    pub fn member_count(&self) -> usize {
        self.members.len()
    }
    /// Cumulative number of times a trial has reached solved since the last
    /// scramble (a running tally of convergence events, not the count of
    /// currently-solved members).
    pub fn converged(&self) -> usize {
        self.converged
    }

    /// Number of members not yet solved.
    pub fn solving_now(&self) -> usize {
        self.members.iter().filter(|m| m.mismatch > 0).count()
    }

    /// Mean progress (0..1) across the swarm.
    pub fn avg_progress(&self) -> f64 {
        if self.members.is_empty() {
            return 0.0;
        }
        let total = (6 * self.n * self.n) as f64;
        let sum: f64 = self
            .members
            .iter()
            .map(|m| 1.0 - m.mismatch as f64 / total)
            .sum();
        sum / self.members.len() as f64
    }

    /// Advance every trial one learning step.
    pub fn step(&mut self) {
        let len = self.members.len();
        let base_mismatch = self.base.mismatch_count();
        for i in 0..len {
            if self.members[i].mismatch == 0 {
                if base_mismatch == 0 {
                    // The Studio cube itself is solved — nothing to learn.
                    continue;
                }
                // Solved: hold briefly (green flash), then restart this trial FROM
                // the Studio cube so the wall keeps tracking the on-screen scramble.
                if self.members[i].flash > 0 {
                    self.members[i].flash -= 1;
                } else {
                    self.restart_member(i, base_mismatch);
                }
                continue;
            }
            // (1+λ) elitist step: spawn a few variants (mutation, occasionally a
            // peer crossover) and keep the best — converges far faster than a
            // single-mutation hill climb, so trials actually reach solved.
            let cur = self.members[i].mismatch;
            let mut best_m = cur;
            let mut best: Option<(Vec<Move>, u8)> = None;
            // members[i].seq is immutable across the inner loop (only replaced
            // after it), so clone the base sequence once instead of 8 times.
            let base_seq = self.members[i].seq.clone();
            for k in 0..8 {
                // The first couple of variants splice in a fitter peer's genes
                // (tournament-picked), the rest are plain mutations.
                let candidate = if k < 2 && len > 1 {
                    let mut j = self.rng.below(len);
                    let j2 = self.rng.below(len);
                    if self.members[j2].mismatch < self.members[j].mismatch {
                        j = j2;
                    }
                    let peer = self.members[j].seq.clone();
                    let child = self.crossover(&base_seq, &peer);
                    self.mutate(&child)
                } else {
                    self.mutate(&base_seq)
                };
                let m = self.eval(&candidate);
                if m < best_m {
                    best_m = m;
                    best = Some((candidate, if k < 2 { 2 } else { 1 }));
                }
            }
            match best {
                Some((seq, operator)) => {
                    self.members[i].seq = seq;
                    self.members[i].mismatch = best_m;
                    self.members[i].stuck = 0;
                    self.members[i].operator = operator;
                    if best_m == 0 {
                        self.members[i].flash = 26;
                        self.converged += 1;
                    }
                }
                None => {
                    self.members[i].stuck += 1;
                    // A plateaued trial restarts from the cube to try a fresh path.
                    if self.members[i].stuck > 24 {
                        self.restart_member(i, base_mismatch);
                    }
                }
            }
        }
    }

    /// Reseed the whole swarm onto a fresh scramble.
    pub fn reset(&mut self) {
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.rng = Rng::new(self.seed | 1);
        self.base = Swarm::outer_scramble(self.n, &self.moves, &mut self.rng, self.scramble_depth);
        self.converged = 0;
        let count = self.members.len().max(1);
        self.fill_population(count);
    }

    /// Render buffer: per member, 62 bytes — progress, exact mismatch (u16),
    /// genome length (u16), stagnation, solved-flash, last operator, then 54
    /// sampled color indices. This lets the Swarm UI visualize how each lineage
    /// is learning rather than showing only a cosmetic percentage.
    pub fn render(&self) -> Vec<u8> {
        const STRIDE: usize = 62;
        let total = (6 * self.n * self.n) as f32;
        let mut out = Vec::with_capacity(self.members.len() * STRIDE);
        for m in &self.members {
            let pct = (100.0 * (1.0 - m.mismatch as f32 / total)).round() as u8;
            out.push(pct.min(100));
            out.extend_from_slice(&(m.mismatch.min(u16::MAX as usize) as u16).to_le_bytes());
            out.extend_from_slice(&(m.seq.len().min(u16::MAX as usize) as u16).to_le_bytes());
            out.push(m.stuck.min(u8::MAX as u32) as u8);
            out.push(u8::from(m.flash > 0));
            out.push(m.operator);
            let mut cube = self.base.clone();
            for mv in &m.seq {
                let _ = cube.apply_move(*mv);
            }
            for face in Face::ALL {
                let fs = cube.face_sample(face, 3);
                // Always emit a fixed 3x3 (9 cells) per face so the JS swarm layout
                // (62 bytes/member) stays aligned even for 2x2 cubes, where
                // face_sample returns 2x2. Nearest-neighbour upsample.
                let d = fs.cells.len().max(1);
                for r in 0..3 {
                    let sr = r * d / 3;
                    for c in 0..3 {
                        let sc = c * d / 3;
                        out.push(color_index(fs.cells[sr][sc]));
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticker_state_boundary_round_trips_without_scramble_history() {
        let mut source = CubeLab::new(3);
        source.apply_design_move(0, 2, 1);
        source.apply_design_move(1, 0, -1);
        source.apply_design_move(2, 2, 1);
        let colors = source.face_colors(3);

        let mut worker = CubeLab::new(3);
        assert!(worker.load_face_colors(&colors));
        assert_eq!(worker.face_colors(3), colors);
        assert!(!worker.is_solved());

        let mut invalid = colors.clone();
        invalid[0] = 9;
        assert!(!worker.load_face_colors(&invalid));
        assert!(!worker.load_face_colors(&colors[..colors.len() - 1]));
    }

    /// Exactly what the web UI does: load only the scrambled sticker state, run a
    /// real solve, then apply the returned `{axis,layer,dir}` moves — must solve.
    #[test]
    fn outer_scramble_is_really_solved() {
        for n in [2usize, 3, 4, 5] {
            let mut lab = CubeLab::new(n);
            let mut rng = Rng::new(1234 + n as u64);
            for _ in 0..6 {
                let axis = rng.below(3) as u8;
                let layer = if rng.below(2) == 0 { 0 } else { n - 1 };
                let dir = if rng.below(2) == 0 { 1 } else { -1 };
                lab.apply_design_move(axis, layer, dir);
            }
            assert!(!lab.is_solved(), "n={n} should be scrambled");

            let json = lab.solve(8, 3000.0);
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert!(v["found"].as_bool().unwrap(), "no solution found for n={n}");

            for m in v["moves"].as_array().unwrap() {
                lab.apply_design_move(
                    m["axis"].as_u64().unwrap() as u8,
                    m["layer"].as_u64().unwrap() as usize,
                    m["dir"].as_i64().unwrap() as i32,
                );
            }
            assert!(lab.is_solved(), "returned solution did not solve n={n}");
        }
    }

    /// 4×4–6×6 route through the REAL reduction solver (winner == "reduction", not the
    /// legacy/visual fallback) under deep inner+outer scrambles, and the returned moves
    /// genuinely solve the cube — the end-to-end wiring of `try_reduction`.
    #[test]
    fn reduction_path_solves() {
        for n in [4usize, 5] {
            let mut lab = CubeLab::new(n);
            let mut rng = Rng::new(99 + n as u64);
            for _ in 0..(n * 6) {
                let axis = rng.below(3) as u8;
                let layer = rng.below(n); // inner slices too, not just outer
                let dir = if rng.below(2) == 0 { 1 } else { -1 };
                lab.apply_design_move(axis, layer, dir);
            }
            assert!(!lab.is_solved(), "n={n} should be scrambled");
            let v: serde_json::Value = serde_json::from_str(&lab.solve(8, 3000.0)).unwrap();
            assert!(v["found"].as_bool().unwrap(), "n={n} no solution");
            assert_eq!(
                v["winner"], "reduction",
                "n={n} did not use the reduction solver"
            );
            for m in v["moves"].as_array().unwrap() {
                lab.apply_design_move(
                    m["axis"].as_u64().unwrap() as u8,
                    m["layer"].as_u64().unwrap() as usize,
                    m["dir"].as_i64().unwrap() as i32,
                );
            }
            assert!(lab.is_solved(), "n={n} reduction solution did not solve");
        }
    }

    /// The depth-scaled budget must crack deeper 3×3 scrambles (the "auto-stronger
    /// on deep" behaviour), up to the depth-10 ceiling the UI promises.
    #[test]
    fn deep_scrambles_still_solve() {
        for depth in [8usize, 9] {
            let mut lab = CubeLab::new(3);
            let mut rng = Rng::new(7 + depth as u64);
            let mut last: i64 = -1;
            let mut applied = 0;
            while applied < depth {
                let axis = rng.below(3) as u8;
                let layer = if rng.below(2) == 0 { 0 } else { 2 };
                let key = axis as i64 * 3 + layer as i64;
                if key == last {
                    continue;
                }
                last = key;
                let dir = if rng.below(2) == 0 { 1 } else { -1 };
                lab.apply_design_move(axis, layer, dir);
                applied += 1;
            }
            let v: serde_json::Value = serde_json::from_str(&lab.solve(depth, 5000.0)).unwrap();
            assert!(v["found"].as_bool().unwrap(), "depth {depth} not solved");
            for m in v["moves"].as_array().unwrap() {
                lab.apply_design_move(
                    m["axis"].as_u64().unwrap() as u8,
                    m["layer"].as_u64().unwrap() as usize,
                    m["dir"].as_i64().unwrap() as i32,
                );
            }
            assert!(lab.is_solved(), "depth {depth} solution did not solve");
        }
    }

    /// The two-phase solver cracks ANY 3×3 scramble — far past the legacy ≤9-move
    /// limit — and `solve()` routes every 3×3 through it.
    #[test]
    fn kociemba_solves_arbitrarily_deep_3x3() {
        let mut lab = CubeLab::new(3);
        let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..40 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let axis = (s % 3) as u8;
            let layer: usize = if (s >> 3) & 1 == 0 { 0 } else { 2 };
            let dir: i32 = if (s >> 4) & 1 == 0 { 1 } else { -1 };
            lab.apply_design_move(axis, layer, dir);
        }
        assert!(
            !lab.is_solved(),
            "a 40-move scramble should not already be solved"
        );
        let v: serde_json::Value = serde_json::from_str(&lab.solve(20, 5000.0)).unwrap();
        assert!(
            v["found"].as_bool().unwrap(),
            "deep 3x3 reported no solution"
        );
        assert_eq!(
            v["winner"].as_str().unwrap(),
            "kociemba",
            "3x3 should use the two-phase solver"
        );
        for m in v["moves"].as_array().unwrap() {
            lab.apply_design_move(
                m["axis"].as_u64().unwrap() as u8,
                m["layer"].as_u64().unwrap() as usize,
                m["dir"].as_i64().unwrap() as i32,
            );
        }
        assert!(
            lab.is_solved(),
            "the two-phase solution did not solve the deep scramble"
        );
    }

    #[test]
    fn swarm_learns_and_converges() {
        let mut swarm = Swarm::new(40, 3, 5, 7);
        let start = swarm.avg_progress();
        for _ in 0..600 {
            swarm.step();
        }
        // Progress should improve and at least some trials should have solved.
        assert!(
            swarm.avg_progress() > start,
            "swarm did not improve ({} -> {})",
            start,
            swarm.avg_progress()
        );
        assert!(swarm.converged() > 0, "no trials converged");
        // Render buffer has the documented shape.
        let buf = swarm.render();
        assert_eq!(buf.len(), swarm.member_count() * 62);
    }

    #[test]
    fn swarm_restarts_keep_a_baseline_and_add_genetic_diversity() {
        let mut swarm = Swarm::new(12, 3, 6, 17);
        let base_mismatch = swarm.base.mismatch_count();
        for i in 0..swarm.members.len() {
            swarm.restart_member(i, base_mismatch);
        }
        assert!(swarm.members.iter().step_by(2).all(|m| m.seq.is_empty()));
        assert!(swarm
            .members
            .iter()
            .skip(1)
            .step_by(2)
            .all(|m| !m.seq.is_empty()));
        for member in &swarm.members {
            assert_eq!(member.mismatch, swarm.eval(&member.seq));
        }
    }

    #[test]
    fn swarm_solves_a_shared_scramble() {
        // Seed the swarm from an explicit (Studio-style) scramble and confirm it
        // learns to solve that exact cube.
        let mut swarm = Swarm::new(40, 3, 6, 11);
        // A 5-move outer scramble: axis/layer/dir triples.
        let axes: Vec<u8> = vec![0, 1, 2, 0, 1];
        let layers: Vec<u32> = vec![2, 0, 2, 0, 2];
        let dirs: Vec<i32> = vec![1, -1, 1, 1, -1];
        swarm.set_scramble(&axes, &layers, &dirs);
        assert!(swarm.avg_progress() < 1.0, "scramble should not be solved");
        for _ in 0..800 {
            swarm.step();
        }
        assert!(
            swarm.converged() > 0,
            "no trials solved the shared scramble"
        );
    }
}

#[cfg(test)]
mod reliability {
    use super::*;
    #[test]
    fn swarm_converges_across_seeds() {
        for seed in 1u64..=8 {
            let mut sw = Swarm::new(64, 3, 6, seed);
            for _ in 0..400 {
                sw.step();
            }
            assert!(
                sw.converged() >= 3,
                "seed {seed}: only {} converged in 400 steps",
                sw.converged()
            );
        }
    }
}
