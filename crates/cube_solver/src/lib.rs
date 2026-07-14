//! Parallel solver worker interfaces and implementations.

use crossbeam_channel::{unbounded, Sender};
use cube_core::{CubeError, CubeSize, CubeSnapshot, CubeState, Face, Move, StickerCube};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
#[cfg(feature = "parallel")]
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(feature = "parallel")]
use std::thread;
use std::time::Duration;
use web_time::Instant;

// Experimental N×N reduction solver. It is opt-in for direct cube_solver users;
// the cube_wasm frontend enables the feature for its replay-verified 4×4–11×11 path.
#[cfg(feature = "reduction")]
pub mod reduction;

// Two-phase (Kociemba-style) 3×3 solver. Built in tested stages.
pub mod kociemba;

#[derive(Clone, Copy, Debug)]
pub struct SolverBudget {
    pub max_depth: usize,
    pub max_nodes: usize,
    pub time_limit: Duration,
    pub beam_width: usize,
    pub population: usize,
    /// Number of independent evolutionary islands evolved in parallel.
    pub islands: usize,
    /// Generations between ring migrations of the best individual.
    pub migration_interval: usize,
    /// Probability that a freshly bred child is additionally mutated.
    pub mutation_rate: f32,
    /// Tournament size for parent selection (>= 2).
    pub tournament_k: usize,
    /// Generations without improvement before an island soft-restarts.
    pub stagnation_limit: usize,
    /// Widest layer block the solvers may turn. Must cover the scramble's
    /// `max_layer_span` or wide-layer scrambles are unsolvable (the move set
    /// would not contain the inverse of the scramble's wide turns).
    pub max_wide: usize,
}

impl SolverBudget {
    pub fn for_depth(max_depth: usize) -> Self {
        Self {
            max_depth,
            max_nodes: 250_000,
            time_limit: Duration::from_secs(8),
            beam_width: 64,
            population: 96,
            islands: 8,
            migration_interval: 8,
            mutation_rate: 0.35,
            tournament_k: 4,
            stagnation_limit: 14,
            max_wide: 1,
        }
    }

    /// Clamp fields to safe ranges so workers cannot panic or stall.
    pub fn sanitized(self) -> Self {
        Self {
            max_depth: self.max_depth.max(1),
            beam_width: self.beam_width.max(1),
            population: self.population.max(4),
            islands: self.islands.clamp(1, 64),
            migration_interval: self.migration_interval.max(1),
            mutation_rate: self.mutation_rate.clamp(0.0, 1.0),
            tournament_k: self.tournament_k.clamp(2, self.population.max(2)),
            stagnation_limit: self.stagnation_limit.max(2),
            max_wide: self.max_wide.max(1),
            ..self
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SolverError {
    Cube(CubeError),
    BudgetExhausted { nodes: usize },
}

impl From<CubeError> for SolverError {
    fn from(value: CubeError) -> Self {
        Self::Cube(value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolutionCandidate {
    pub worker_id: String,
    pub moves: Vec<Move>,
    pub solved: bool,
    pub move_count: usize,
    pub elapsed_ms: u128,
    pub fitness: f32,
}

impl SolutionCandidate {
    pub fn new(worker_id: impl Into<String>, moves: Vec<Move>, solved: bool, fitness: f32) -> Self {
        let moves = simplify_moves(&moves);
        let move_count = moves.iter().filter(|mv| !mv.is_noop()).count();
        Self {
            worker_id: worker_id.into(),
            moves,
            solved,
            move_count,
            elapsed_ms: 0,
            fitness,
        }
    }

    pub fn with_elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed_ms = elapsed.as_millis();
        self
    }

    pub fn verify_against(&self, snapshot: &CubeSnapshot) -> Result<bool, SolverError> {
        let mut cube = StickerCube::from_snapshot(snapshot.clone());
        for mv in &self.moves {
            cube.apply_move(*mv)?;
        }
        // Honestly report whether replaying the moves actually leaves the cube
        // solved, rather than vacuously returning true for a non-solution.
        Ok(cube.is_solved())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerEvent {
    pub worker_id: String,
    pub generation: usize,
    pub nodes: usize,
    pub best_fitness: f32,
    pub best_move_count: usize,
    pub candidate: Option<SolutionCandidate>,
    pub message: String,
}

impl WorkerEvent {
    fn progress(
        worker_id: impl Into<String>,
        generation: usize,
        nodes: usize,
        best_fitness: f32,
        best_move_count: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            worker_id: worker_id.into(),
            generation,
            nodes,
            best_fitness,
            best_move_count,
            candidate: None,
            message: message.into(),
        }
    }

    fn candidate(
        worker_id: impl Into<String>,
        generation: usize,
        nodes: usize,
        candidate: SolutionCandidate,
        message: impl Into<String>,
    ) -> Self {
        Self {
            worker_id: worker_id.into(),
            generation,
            nodes,
            best_fitness: candidate.fitness,
            best_move_count: candidate.move_count,
            candidate: Some(candidate),
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolverRun {
    pub best: Option<SolutionCandidate>,
    pub events: Vec<WorkerEvent>,
}

pub trait SolverWorker: Send + Sync + 'static {
    fn worker_id(&self) -> &'static str;
    fn start(&self, snapshot: CubeSnapshot, budget: SolverBudget, tx: Sender<WorkerEvent>);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DeterministicSolver;

impl DeterministicSolver {
    pub fn solve(
        &self,
        snapshot: CubeSnapshot,
        budget: SolverBudget,
    ) -> Result<SolutionCandidate, SolverError> {
        let started = Instant::now();
        let start_cube = StickerCube::from_snapshot(snapshot.clone());
        start_cube.validate()?;
        if start_cube.is_solved() {
            return Ok(SolutionCandidate::new("deterministic", vec![], true, 1.0)
                .with_elapsed(started.elapsed()));
        }

        let size = snapshot.size();
        let solved = StickerCube::solved(size).clone_snapshot();
        let moves = wide_move_set(size, budget.max_wide);
        let backward_depth = budget.max_depth / 2;
        let forward_depth = budget.max_depth - backward_depth;
        let mut nodes = 0;
        let backward = build_backward_map(
            solved,
            &moves,
            backward_depth,
            budget.max_nodes / 2,
            started,
            budget.time_limit,
            &mut nodes,
        )?;

        let start = snapshot.clone();
        let mut visited = HashSet::from([snapshot.clone()]);
        let mut queue = VecDeque::from([(snapshot, Vec::<Move>::new())]);

        while let Some((state, path)) = queue.pop_front() {
            if started.elapsed() > budget.time_limit || nodes > budget.max_nodes {
                return Err(SolverError::BudgetExhausted { nodes });
            }

            if let Some(tail) = backward.get(&state) {
                let mut solution = path.clone();
                solution.extend(tail.iter().copied());
                let candidate = SolutionCandidate::new("deterministic", solution, true, 1.0)
                    .with_elapsed(started.elapsed());
                // `solution` is the forward path (start -> state) followed by the
                // backward tail (state -> solved); replaying it from the original
                // scramble is the authoritative correctness check.
                if candidate.verify_against(&start)? {
                    return Ok(candidate);
                }
            }

            if path.len() >= forward_depth {
                continue;
            }

            for mv in &moves {
                if is_immediate_inverse(path.last().copied(), *mv) {
                    continue;
                }
                let mut cube = StickerCube::from_snapshot(state.clone());
                cube.apply_move(*mv)?;
                let next = cube.clone_snapshot();
                nodes += 1;
                if visited.insert(next.clone()) {
                    let mut next_path = path.clone();
                    next_path.push(*mv);
                    queue.push_back((next, next_path));
                }
            }
        }

        Err(SolverError::BudgetExhausted { nodes })
    }
}

impl SolverWorker for DeterministicSolver {
    fn worker_id(&self) -> &'static str {
        "deterministic"
    }

    fn start(&self, snapshot: CubeSnapshot, budget: SolverBudget, tx: Sender<WorkerEvent>) {
        let started = Instant::now();
        match self.solve(snapshot.clone(), budget) {
            Ok(candidate) => {
                let verified = candidate.verify_against(&snapshot).unwrap_or(false);
                if verified {
                    let _ = tx.send(WorkerEvent::candidate(
                        self.worker_id(),
                        0,
                        candidate.move_count,
                        candidate,
                        "verified shortest path within deterministic budget",
                    ));
                } else {
                    let _ = tx.send(WorkerEvent::progress(
                        self.worker_id(),
                        0,
                        0,
                        0.0,
                        usize::MAX,
                        "candidate failed replay verification",
                    ));
                }
            }
            Err(err) => {
                let _ = tx.send(WorkerEvent::progress(
                    self.worker_id(),
                    0,
                    0,
                    0.0,
                    usize::MAX,
                    format!(
                        "stopped after {} ms: {err:?}",
                        started.elapsed().as_millis()
                    ),
                ));
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BeamSearchWorker;

impl SolverWorker for BeamSearchWorker {
    fn worker_id(&self) -> &'static str {
        "beam"
    }

    fn start(&self, snapshot: CubeSnapshot, budget: SolverBudget, tx: Sender<WorkerEvent>) {
        let started = Instant::now();
        let size = snapshot.size();
        let moves = wide_move_set(size, budget.max_wide);
        let total_stickers = size.stickers() as f32;
        let mut beam = vec![ScoredPath {
            moves: vec![],
            mismatches: mismatch_after(&snapshot, &[]).unwrap_or(usize::MAX),
        }];
        let mut nodes = 0;

        for depth in 1..=budget.max_depth {
            if started.elapsed() > budget.time_limit || nodes > budget.max_nodes {
                break;
            }

            let mut expanded = Vec::with_capacity(beam.len() * moves.len());
            for item in &beam {
                for mv in &moves {
                    if is_immediate_inverse(item.moves.last().copied(), *mv) {
                        continue;
                    }
                    let mut next_moves = item.moves.clone();
                    next_moves.push(*mv);
                    let simplified = simplify_moves(&next_moves);
                    let mismatches = mismatch_after(&snapshot, &simplified).unwrap_or(usize::MAX);
                    nodes += 1;
                    expanded.push(ScoredPath {
                        moves: simplified,
                        mismatches,
                    });
                }
            }

            expanded.sort_by_key(|item| (item.mismatches, item.moves.len()));
            expanded.dedup_by(|a, b| a.moves == b.moves);
            beam = expanded.into_iter().take(budget.beam_width).collect();

            if let Some(best) = beam.first() {
                let fitness = 1.0 - (best.mismatches as f32 / total_stickers);
                let event = WorkerEvent::progress(
                    self.worker_id(),
                    depth,
                    nodes,
                    fitness,
                    best.moves.len(),
                    format!("beam depth {depth}, best mismatches {}", best.mismatches),
                );
                let _ = tx.send(event);

                if best.mismatches == 0 {
                    let candidate =
                        SolutionCandidate::new(self.worker_id(), best.moves.clone(), true, fitness)
                            .with_elapsed(started.elapsed());
                    let _ = tx.send(WorkerEvent::candidate(
                        self.worker_id(),
                        depth,
                        nodes,
                        candidate,
                        "beam found replay-verifiable solution",
                    ));
                    return;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EvolutionaryWorker {
    pub seed: u64,
}

/// One evolutionary island: an isolated population with its own deterministic
/// RNG and stagnation counter.
struct Island {
    rng: ChaCha8Rng,
    population: Vec<Vec<Move>>,
    best: Option<ScoredPath>,
    stagnation: usize,
}

fn score_key(path: &ScoredPath) -> (usize, usize) {
    (path.mismatches, path.moves.len())
}

/// Tournament selection: sample `k` individuals, return the fittest. The scored
/// slice is sorted ascending, so the fittest is the lowest index drawn.
fn tournament<'a>(rng: &mut ChaCha8Rng, scored: &'a [ScoredPath], k: usize) -> &'a [Move] {
    debug_assert!(
        !scored.is_empty(),
        "tournament requires a non-empty population (guaranteed by SolverBudget::sanitized)"
    );
    let mut best = rng.gen_range(0..scored.len());
    for _ in 1..k.max(1) {
        let idx = rng.gen_range(0..scored.len());
        if idx < best {
            best = idx;
        }
    }
    &scored[best].moves
}

/// Cut-and-splice crossover: a prefix of `a` followed by a suffix of `b`. The
/// child's length varies, letting the population explore different solution
/// lengths; the result is simplified to curb bloat.
fn crossover(rng: &mut ChaCha8Rng, a: &[Move], b: &[Move], cap: usize) -> Vec<Move> {
    let i = if a.is_empty() {
        0
    } else {
        rng.gen_range(0..=a.len())
    };
    let j = if b.is_empty() {
        0
    } else {
        rng.gen_range(0..=b.len())
    };
    let mut child = Vec::with_capacity(i + (b.len() - j));
    child.extend_from_slice(&a[..i]);
    child.extend_from_slice(&b[j..]);
    if child.len() > cap {
        child.truncate(cap);
    }
    simplify_moves(&child)
}

fn fitness_of(path: &ScoredPath, total_stickers: f32) -> f32 {
    1.0 - (path.mismatches as f32 / total_stickers) - (path.moves.len() as f32 * 0.001)
}

/// Evolve one island by a single generation, returning its best individual.
fn evolve_island(
    island: &mut Island,
    snapshot: &CubeSnapshot,
    moves: &[Move],
    budget: &SolverBudget,
) -> ScoredPath {
    let mut scored: Vec<ScoredPath> = island
        .population
        .iter()
        .map(|path| ScoredPath {
            moves: simplify_moves(path),
            mismatches: mismatch_after(snapshot, path).unwrap_or(usize::MAX),
        })
        .collect();
    scored.sort_by_key(score_key);

    let island_best = scored[0].clone();
    let improved = island
        .best
        .as_ref()
        .map(|old| score_key(&island_best) < score_key(old))
        .unwrap_or(true);
    if improved {
        island.best = Some(island_best.clone());
        island.stagnation = 0;
    } else {
        island.stagnation += 1;
    }

    let elite_count = (budget.population / 6).max(1);
    let cap = (budget.max_depth * 2).max(budget.max_depth + 1);
    let mut next: Vec<Vec<Move>> = scored
        .iter()
        .take(elite_count)
        .map(|item| item.moves.clone())
        .collect();

    if island.stagnation >= budget.stagnation_limit {
        // Soft restart: keep the elites, reseed the rest with fresh random paths.
        island.stagnation = 0;
        while next.len() < budget.population {
            next.push(random_path(&mut island.rng, moves, budget.max_depth));
        }
    } else {
        while next.len() < budget.population {
            let parent_a = tournament(&mut island.rng, &scored, budget.tournament_k).to_vec();
            let parent_b = tournament(&mut island.rng, &scored, budget.tournament_k).to_vec();
            let mut child = crossover(&mut island.rng, &parent_a, &parent_b, cap);
            if island.rng.gen::<f32>() < budget.mutation_rate {
                child = mutate_path(&mut island.rng, &child, moves, budget.max_depth);
            }
            next.push(child);
        }
    }
    island.population = next;
    island_best
}

impl SolverWorker for EvolutionaryWorker {
    fn worker_id(&self) -> &'static str {
        "evolution"
    }

    fn start(&self, snapshot: CubeSnapshot, budget: SolverBudget, tx: Sender<WorkerEvent>) {
        let budget = budget.sanitized();
        let started = Instant::now();
        let size = snapshot.size();
        let moves = wide_move_set(size, budget.max_wide);
        let total_stickers = size.stickers() as f32;
        let generations = (budget.max_depth * 50).max(200);

        // Independent islands, each with its own deterministically seeded RNG so
        // results never depend on thread scheduling.
        let mut islands: Vec<Island> = (0..budget.islands)
            .map(|i| {
                let seed = self
                    .seed
                    .wrapping_add((i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                let mut rng = ChaCha8Rng::seed_from_u64(seed);
                let population = (0..budget.population)
                    .map(|_| random_path(&mut rng, &moves, budget.max_depth))
                    .collect();
                Island {
                    rng,
                    population,
                    best: None,
                    stagnation: 0,
                }
            })
            .collect();

        let mut global_best: Option<ScoredPath> = None;
        let mut nodes = 0usize;

        for generation in 0..generations {
            if started.elapsed() > budget.time_limit || nodes > budget.max_nodes {
                break;
            }

            // Evolve all islands (in parallel when threads are available; each
            // island only touches its own state, so the sequential fallback is
            // identical in result).
            #[cfg(feature = "parallel")]
            let gen_bests: Vec<ScoredPath> = islands
                .par_iter_mut()
                .map(|island| evolve_island(island, &snapshot, &moves, &budget))
                .collect();
            #[cfg(not(feature = "parallel"))]
            let gen_bests: Vec<ScoredPath> = islands
                .iter_mut()
                .map(|island| evolve_island(island, &snapshot, &moves, &budget))
                .collect();
            nodes += budget.islands * budget.population;

            let gen_best = gen_bests
                .into_iter()
                .min_by_key(score_key)
                .expect("at least one island");
            if global_best
                .as_ref()
                .map(|old| score_key(&gen_best) < score_key(old))
                .unwrap_or(true)
            {
                global_best = Some(gen_best);
            }

            let current = global_best.as_ref().unwrap();
            let fitness = fitness_of(current, total_stickers);
            let _ = tx.send(WorkerEvent::progress(
                self.worker_id(),
                generation,
                nodes,
                fitness,
                current.moves.len(),
                format!(
                    "{} islands, gen {generation}, best mismatches {}",
                    budget.islands, current.mismatches
                ),
            ));

            if current.mismatches == 0 {
                let candidate =
                    SolutionCandidate::new(self.worker_id(), current.moves.clone(), true, fitness)
                        .with_elapsed(started.elapsed());
                let _ = tx.send(WorkerEvent::candidate(
                    self.worker_id(),
                    generation,
                    nodes,
                    candidate,
                    "evolutionary islands found solution",
                ));
                return;
            }

            // Ring migration: seed each island with the previous island's best.
            if budget.islands > 1 && generation % budget.migration_interval == 0 {
                let migrants: Vec<Vec<Move>> = islands
                    .iter()
                    .map(|isl| {
                        isl.best
                            .as_ref()
                            .map(|b| b.moves.clone())
                            .unwrap_or_default()
                    })
                    .collect();
                for (i, island) in islands.iter_mut().enumerate() {
                    let donor = &migrants[(i + budget.islands - 1) % budget.islands];
                    if !donor.is_empty() {
                        let last = island.population.len() - 1;
                        island.population[last] = donor.clone();
                    }
                }
            }
        }
    }
}

pub fn run_solver_lab(snapshot: CubeSnapshot, budget: SolverBudget) -> SolverRun {
    run_solver_lab_observed(snapshot, budget, |_| {})
}

fn lab_workers() -> Vec<Box<dyn SolverWorker>> {
    vec![
        Box::new(DeterministicSolver),
        Box::new(BeamSearchWorker),
        Box::new(EvolutionaryWorker { seed: 0xC0B3 }),
    ]
}

/// Fold one worker event into the running best (replay-verified) and event log.
fn ingest_event(
    event: WorkerEvent,
    snapshot: &CubeSnapshot,
    best: &mut Option<SolutionCandidate>,
    events: &mut Vec<WorkerEvent>,
) {
    if let Some(candidate) = &event.candidate {
        if candidate.solved && candidate.verify_against(snapshot).unwrap_or(false) {
            let better = best
                .as_ref()
                .map(|old| {
                    (candidate.move_count, candidate.elapsed_ms) < (old.move_count, old.elapsed_ms)
                })
                .unwrap_or(true);
            if better {
                *best = Some(candidate.clone());
            }
        }
    }
    events.push(event);
}

/// Race all workers, returning the fewest-move replay-verified solution.
///
/// With the `parallel` feature each worker runs on its own OS thread; without it
/// (e.g. wasm32) the workers run sequentially in the calling thread — the result
/// is identical, only the wall-clock differs.
#[cfg(feature = "parallel")]
pub fn run_solver_lab_observed<F>(
    snapshot: CubeSnapshot,
    budget: SolverBudget,
    mut observe: F,
) -> SolverRun
where
    F: FnMut(&WorkerEvent),
{
    let budget = budget.sanitized();
    let (tx, rx) = unbounded();
    let mut handles = Vec::new();
    for worker in lab_workers() {
        let worker_tx = tx.clone();
        let worker_snapshot = snapshot.clone();
        handles.push(thread::spawn(move || {
            worker.start(worker_snapshot, budget, worker_tx);
        }));
    }
    drop(tx);

    let mut events = Vec::new();
    let mut best: Option<SolutionCandidate> = None;
    for event in rx {
        observe(&event);
        ingest_event(event, &snapshot, &mut best, &mut events);
    }
    for handle in handles {
        let _ = handle.join();
    }
    SolverRun { best, events }
}

#[cfg(not(feature = "parallel"))]
pub fn run_solver_lab_observed<F>(
    snapshot: CubeSnapshot,
    budget: SolverBudget,
    mut observe: F,
) -> SolverRun
where
    F: FnMut(&WorkerEvent),
{
    let budget = budget.sanitized();
    let (tx, rx) = unbounded();
    for worker in lab_workers() {
        worker.start(snapshot.clone(), budget, tx.clone());
    }
    drop(tx);

    let mut events = Vec::new();
    let mut best: Option<SolutionCandidate> = None;
    // The sender (and every worker's clone) is dropped above, so this blocking
    // drain yields all buffered events then terminates — same result as the
    // threaded path, and robust to any future change in worker timing.
    for event in rx {
        observe(&event);
        ingest_event(event, &snapshot, &mut best, &mut events);
    }
    SolverRun { best, events }
}

/// Run the (reliable, exact) deterministic solver with a generous `primary`
/// budget and the two heuristic workers with a small `secondary` budget — so a
/// deep scramble can be cracked by meet-in-the-middle without the beam/GA also
/// running at full budget. Sequential orchestration (the deterministic engine
/// stops as soon as it finds the meet), intended for single-threaded callers
/// such as wasm. Returns the fewest-move, replay-verified solution.
pub fn run_solver_lab_tiered(
    snapshot: CubeSnapshot,
    primary: SolverBudget,
    secondary: SolverBudget,
) -> SolverRun {
    let primary = primary.sanitized();
    let secondary = secondary.sanitized();
    let (tx, rx) = unbounded();
    DeterministicSolver.start(snapshot.clone(), primary, tx.clone());
    BeamSearchWorker.start(snapshot.clone(), secondary, tx.clone());
    EvolutionaryWorker { seed: 0xC0B3 }.start(snapshot.clone(), secondary, tx.clone());
    drop(tx);

    let mut events = Vec::new();
    let mut best: Option<SolutionCandidate> = None;
    for event in rx {
        ingest_event(event, &snapshot, &mut best, &mut events);
    }
    SolverRun { best, events }
}

pub fn face_turn_move_set(size: CubeSize) -> Vec<Move> {
    let mut moves = Vec::with_capacity(18);
    for face in Face::ALL {
        for turns in [1, -1, 2] {
            moves.push(Move::face(face, size, turns));
        }
    }
    moves
}

/// Move set including wide turns of width `1..=max_wide`, matching the space a
/// scramble with `max_layer_span = max_wide` draws from. With `max_wide == 1`
/// this is exactly [`face_turn_move_set`]. Deduplicated so equivalent moves
/// (e.g. a full-width turn from opposite faces) appear once.
pub fn wide_move_set(size: CubeSize, max_wide: usize) -> Vec<Move> {
    let n = size.get();
    let max_wide = max_wide.clamp(1, n);
    let mut moves = Vec::new();
    let mut seen = HashSet::new();
    for face in Face::ALL {
        for width in 1..=max_wide {
            for turns in [1, -1, 2] {
                let mv = Move::wide(face, size, width, turns);
                if mv.is_noop() {
                    continue;
                }
                if seen.insert((mv.axis, mv.layer_start, mv.layer_end, mv.turns)) {
                    moves.push(mv);
                }
            }
        }
    }
    moves
}

pub fn simplify_moves(moves: &[Move]) -> Vec<Move> {
    let mut stack: Vec<Move> = Vec::with_capacity(moves.len());
    for mv in moves.iter().copied() {
        if mv.is_noop() {
            continue;
        }
        if let Some(last) = stack.last_mut() {
            if last.axis == mv.axis
                && last.layer_start == mv.layer_start
                && last.layer_end == mv.layer_end
            {
                *last = Move::new(
                    last.axis,
                    last.layer_start,
                    last.layer_end,
                    last.turns + mv.turns,
                );
                if last.is_noop() {
                    stack.pop();
                }
                continue;
            }
        }
        stack.push(mv);
    }
    stack
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScoredPath {
    moves: Vec<Move>,
    mismatches: usize,
}

fn build_backward_map(
    solved: CubeSnapshot,
    moves: &[Move],
    max_depth: usize,
    max_nodes: usize,
    started: Instant,
    time_limit: Duration,
    nodes: &mut usize,
) -> Result<HashMap<CubeSnapshot, Vec<Move>>, SolverError> {
    let mut map = HashMap::from([(solved.clone(), Vec::<Move>::new())]);
    let mut queue = VecDeque::from([(solved, Vec::<Move>::new())]);

    while let Some((state, path_to_goal)) = queue.pop_front() {
        if path_to_goal.len() >= max_depth {
            continue;
        }
        if started.elapsed() > time_limit || *nodes > max_nodes {
            return Err(SolverError::BudgetExhausted { nodes: *nodes });
        }

        for mv in moves {
            if is_immediate_inverse(path_to_goal.first().copied(), mv.inverse()) {
                continue;
            }
            let mut cube = StickerCube::from_snapshot(state.clone());
            cube.apply_move(*mv)?;
            let next = cube.clone_snapshot();
            if map.contains_key(&next) {
                continue;
            }

            let mut next_path = Vec::with_capacity(path_to_goal.len() + 1);
            next_path.push(mv.inverse());
            next_path.extend(path_to_goal.iter().copied());
            map.insert(next.clone(), next_path.clone());
            queue.push_back((next, next_path));
            *nodes += 1;
        }
    }

    Ok(map)
}

fn is_immediate_inverse(previous: Option<Move>, next: Move) -> bool {
    previous
        .map(|prev| {
            prev.axis == next.axis
                && prev.layer_start == next.layer_start
                && prev.layer_end == next.layer_end
                && (prev.turns + next.turns).rem_euclid(4) == 0
        })
        .unwrap_or(false)
}

fn mismatch_after(snapshot: &CubeSnapshot, moves: &[Move]) -> Result<usize, SolverError> {
    let mut cube = StickerCube::from_snapshot(snapshot.clone());
    for mv in moves {
        cube.apply_move(*mv)?;
    }
    Ok(cube.mismatch_count())
}

fn random_path(rng: &mut ChaCha8Rng, moves: &[Move], max_depth: usize) -> Vec<Move> {
    let len = rng.gen_range(0..=max_depth);
    let mut path = Vec::with_capacity(len);
    // Fill to the chosen length: redraw (rather than drop) when a draw would
    // immediately cancel the previous move, so paths are not biased short.
    while path.len() < len {
        let mv = moves[rng.gen_range(0..moves.len())];
        if !is_immediate_inverse(path.last().copied(), mv) {
            path.push(mv);
        }
    }
    path
}

fn mutate_path(
    rng: &mut ChaCha8Rng,
    parent: &[Move],
    moves: &[Move],
    max_depth: usize,
) -> Vec<Move> {
    let mut child = parent.to_vec();
    match rng.gen_range(0..3) {
        0 if !child.is_empty() => {
            let idx = rng.gen_range(0..child.len());
            child[idx] = moves[rng.gen_range(0..moves.len())];
        }
        1 if child.len() < max_depth => {
            let idx = rng.gen_range(0..=child.len());
            child.insert(idx, moves[rng.gen_range(0..moves.len())]);
        }
        _ if !child.is_empty() => {
            let idx = rng.gen_range(0..child.len());
            child.remove(idx);
        }
        _ => child.push(moves[rng.gen_range(0..moves.len())]),
    }
    simplify_moves(&child)
}

#[cfg(test)]
mod evolution_tests {
    use super::*;
    use cube_core::{Challenge, ChallengeSpec, CubeSize};

    fn run_evolution(scramble_seed: u64, n: usize, depth: usize) -> SolverRun {
        let cube = Challenge::generate(
            CubeSize::new(n).unwrap(),
            ChallengeSpec {
                seed: scramble_seed,
                scramble_depth: depth,
                max_layer_span: 1,
            },
        )
        .unwrap()
        .into_cube();
        let snapshot = cube.clone_snapshot();
        let budget = SolverBudget::for_depth(6);
        let (tx, rx) = unbounded();
        EvolutionaryWorker { seed: 0xC0B3 }.start(snapshot.clone(), budget, tx);
        let events: Vec<WorkerEvent> = rx.iter().collect();
        let best = events
            .iter()
            .rev()
            .find_map(|e| e.candidate.clone())
            .filter(|c| c.solved && c.verify_against(&snapshot).unwrap_or(false));
        SolverRun { best, events }
    }

    #[test]
    fn island_ga_is_deterministic_across_runs() {
        let a = run_evolution(7, 3, 3);
        let b = run_evolution(7, 3, 3);
        assert_eq!(a.events.len(), b.events.len());
        assert_eq!(
            a.events.last().map(|e| e.best_fitness),
            b.events.last().map(|e| e.best_fitness),
        );
        assert_eq!(
            a.best.map(|c| c.moves),
            b.best.map(|c| c.moves),
            "same seed must yield the same solution"
        );
    }

    #[test]
    fn island_ga_solves_shallow_scramble() {
        let run = run_evolution(11, 3, 2);
        let best = run.best.expect("evolution should solve a depth-2 scramble");
        assert!(best.solved);
        assert!(best.move_count <= 8);
    }

    #[test]
    fn run_solver_lab_survives_degenerate_budget() {
        // Every field at a degenerate value; sanitized() must keep workers safe.
        let cube = Challenge::generate(
            CubeSize::new(3).unwrap(),
            ChallengeSpec {
                seed: 5,
                scramble_depth: 2,
                max_layer_span: 1,
            },
        )
        .unwrap()
        .into_cube();
        let budget = SolverBudget {
            max_depth: 0,
            max_nodes: 0,
            time_limit: Duration::from_millis(1),
            beam_width: 0,
            population: 0,
            islands: 0,
            migration_interval: 0,
            mutation_rate: 9.0,
            tournament_k: 0,
            stagnation_limit: 0,
            max_wide: 0,
        };
        // Must not panic.
        let _ = run_solver_lab(cube.clone_snapshot(), budget);
    }
}
