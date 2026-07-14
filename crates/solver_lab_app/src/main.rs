use cube_core::{
    Challenge, ChallengeSpec, Color, CubeError, CubeSize, CubeState, Face, FaceSample, Move,
    StickerCube,
};
use cube_solver::{
    run_solver_lab_observed, SolutionCandidate, SolverBudget, SolverRun, WorkerEvent,
};
use eframe::egui;
use solver_store::{SolveRecord, SolveStore};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

mod grid;
mod scene;
use grid::GridModel;
use scene::*;

// 6·N² stickers; 24M allows N up to 2000 (~24 MB per cube at 1 byte/sticker).
const MAX_STICKERS: usize = 24_000_000;

/// eframe storage key for persisted UI preferences.
const SETTINGS_KEY: &str = "cube_lab_settings";

/// An OS-appropriate path for the history database (Application Support on macOS,
/// %APPDATA% on Windows, XDG data dir on Linux), creating the directory if needed.
/// Returns `None` to fall back to an in-memory store.
fn history_db_path() -> Option<PathBuf> {
    let proj = directories::ProjectDirs::from("dev", "CubeLab", "Cube Solver Lab")?;
    let dir = proj.data_dir();
    std::fs::create_dir_all(dir).ok()?;
    Some(dir.join("history.sqlite3"))
}

fn open_history_store() -> Option<SolveStore> {
    let store = history_db_path()
        .and_then(|path| SolveStore::open(path.to_string_lossy().as_ref()).ok())
        .or_else(|| SolveStore::open_in_memory().ok());
    store
}

/// Which cube renderer the central viewport uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewMode {
    Cube,
    Net,
    Grid,
}

/// A single replayed slice turn, animated over `duration`.
#[derive(Clone, Copy)]
struct ReplayTurn {
    mv: Move,
    started: Instant,
    duration: Duration,
}

impl ReplayTurn {
    fn progress(&self) -> f32 {
        let elapsed = self.started.elapsed().as_secs_f32();
        let total = self.duration.as_secs_f32().max(f32::EPSILON);
        (elapsed / total).clamp(0.0, 1.0)
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1380.0, 900.0])
            .with_min_inner_size([820.0, 560.0])
            .with_icon(std::sync::Arc::new(load_icon())),
        ..Default::default()
    };

    eframe::run_native(
        "Rust NxN Cube Solver Lab",
        options,
        Box::new(|cc| Ok(Box::new(SolverLabApp::new(cc)))),
    )
}

/// A small procedural window icon: the six cube colors as a tile grid (avoids
/// shipping an image asset / extra dependency).
fn load_icon() -> egui::IconData {
    let size = 32usize;
    let palette = [
        [245u8, 245, 245],
        [183, 18, 52],
        [0, 70, 173],
        [255, 213, 0],
        [0, 155, 72],
        [255, 88, 0],
    ];
    let mut rgba = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let tile = (x * 3 / size) + (y * 2 / size) * 3;
            let [r, g, b] = palette[tile.min(5)];
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    egui::IconData {
        rgba,
        width: size as u32,
        height: size as u32,
    }
}

enum LabMessage {
    Event(u64, WorkerEvent),
    Finished(u64, SolverRun),
}

struct SolverLabApp {
    cube_size: usize,
    scramble_depth: usize,
    max_layer_span: usize,
    seed: u64,
    solve_depth: usize,
    sample_limit: usize,
    challenge: Option<Challenge>,
    visible_cube: StickerCube,
    status: String,
    events: Vec<WorkerEvent>,
    best: Option<SolutionCandidate>,
    solving: bool,
    solve_job: u64,
    rx: Option<mpsc::Receiver<LabMessage>>,
    replaying: bool,
    replay_step: usize,
    active_replay_turn: Option<ReplayTurn>,
    view_angles: ViewAngles,
    view_mode: ViewMode,
    dark_mode: bool,
    ui_scale: f32,
    applied_scale: f32,
    grid_model: Option<GridModel>,
    grid_count: usize,
    grid_cube_n: usize,
    grid_speed: f32,
    generating: bool,
    gen_job: u64,
    gen_rx: Option<mpsc::Receiver<(u64, Result<Challenge, CubeError>)>>,
    store: Option<SolveStore>,
    history: Vec<SolveRecord>,
}

impl SolverLabApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let size = CubeSize::new(3).expect("default cube size is valid");
        let mut app = Self {
            cube_size: 3,
            scramble_depth: 3,
            max_layer_span: 1,
            seed: 42,
            solve_depth: 6,
            sample_limit: 24,
            challenge: None,
            visible_cube: StickerCube::solved(size),
            status: "Ready".to_string(),
            events: Vec::new(),
            best: None,
            solving: false,
            solve_job: 0,
            rx: None,
            replaying: false,
            replay_step: 0,
            active_replay_turn: None,
            view_angles: ViewAngles {
                yaw: -0.6,
                pitch: 0.55,
            },
            view_mode: ViewMode::Cube,
            dark_mode: true,
            ui_scale: 1.0,
            applied_scale: 1.0,
            grid_model: None,
            grid_count: 64,
            grid_cube_n: 3,
            grid_speed: 1.0,
            generating: false,
            gen_job: 0,
            gen_rx: None,
            store: open_history_store(),
            history: Vec::new(),
        };
        // Restore persisted preferences (theme, sizes, seed, …) if present.
        if let Some(raw) = cc.storage.and_then(|s| s.get_string(SETTINGS_KEY)) {
            app.apply_settings_json(&raw);
        }
        app.refresh_history();
        app.generate_challenge();
        app
    }

    fn apply_settings_json(&mut self, raw: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
            return;
        };
        let usize_at = |key: &str| v.get(key).and_then(|x| x.as_u64()).map(|n| n as usize);
        if let Some(b) = v.get("dark_mode").and_then(|x| x.as_bool()) {
            self.dark_mode = b;
        }
        if let Some(f) = v.get("ui_scale").and_then(|x| x.as_f64()) {
            self.ui_scale = (f as f32).clamp(0.75, 2.0);
        }
        if let Some(n) = usize_at("cube_size") {
            self.cube_size = n.clamp(2, 2000);
        }
        if let Some(n) = usize_at("scramble_depth") {
            self.scramble_depth = n.clamp(1, 200);
        }
        if let Some(n) = usize_at("max_layer_span") {
            self.max_layer_span = n.clamp(1, 256);
        }
        if let Some(n) = v.get("seed").and_then(|x| x.as_u64()) {
            self.seed = n;
        }
        if let Some(n) = usize_at("solve_depth") {
            self.solve_depth = n.clamp(1, 10);
        }
        if let Some(n) = usize_at("sample_limit") {
            self.sample_limit = n.clamp(4, 64);
        }
        if let Some(n) = usize_at("grid_count") {
            self.grid_count = n.clamp(4, 400);
        }
        if let Some(n) = usize_at("grid_cube_n") {
            self.grid_cube_n = n.clamp(2, 20);
        }
    }

    fn generate_challenge(&mut self) {
        self.replaying = false;
        self.active_replay_turn = None;
        // Invalidate and disconnect any solver for the previous challenge. The
        // worker may finish in the background, but its tagged result can no
        // longer be attached to or persisted against the replacement state.
        self.solve_job = self.solve_job.wrapping_add(1);
        self.solving = false;
        self.rx = None;
        let Ok(size) = CubeSize::new(self.cube_size) else {
            self.status = "Cube size must be at least 2".to_string();
            return;
        };
        if size.stickers() > MAX_STICKERS {
            self.status = format!(
                "Resource limit: {}x{} would allocate {} stickers; lower N or raise MAX_STICKERS",
                size.get(),
                size.get(),
                size.stickers()
            );
            return;
        }

        let spec = ChallengeSpec {
            seed: self.seed,
            scramble_depth: self.scramble_depth.max(1),
            max_layer_span: self.max_layer_span.max(1),
        };

        // Generate off the UI thread so huge N never freezes the app. A
        // monotonic job id lets us ignore results the user has superseded.
        self.gen_job += 1;
        let job = self.gen_job;
        let (tx, rx) = mpsc::channel();
        self.gen_rx = Some(rx);
        self.generating = true;
        self.status = format!(
            "Generating {}x{} challenge…",
            self.cube_size, self.cube_size
        );
        thread::spawn(move || {
            let _ = tx.send((job, Challenge::generate(size, spec)));
        });
    }

    fn poll_generation(&mut self) {
        let Some(rx) = self.gen_rx.take() else {
            return;
        };
        let mut keep_rx = true;
        while let Ok((job, result)) = rx.try_recv() {
            if job != self.gen_job {
                continue; // stale result the user already superseded
            }
            self.generating = false;
            keep_rx = false;
            match result {
                Ok(challenge) => {
                    self.visible_cube = challenge.cube().clone();
                    self.challenge = Some(challenge);
                    self.best = None;
                    self.events.clear();
                    self.status = format!(
                        "Challenge generated: {}x{}, seed {}, {} moves",
                        self.cube_size, self.cube_size, self.seed, self.scramble_depth
                    );
                }
                Err(err) => {
                    self.status = format!("Challenge error: {err}");
                }
            }
        }
        if keep_rx {
            self.gen_rx = Some(rx);
        }
    }

    fn start_solving(&mut self) {
        let Some(challenge) = &self.challenge else {
            self.status = "Generate a challenge first".to_string();
            return;
        };
        if self.solving {
            return;
        }

        let snapshot = challenge.cube().clone_snapshot();
        let budget = SolverBudget {
            max_depth: self.solve_depth,
            max_nodes: 300_000,
            time_limit: Duration::from_secs(12),
            beam_width: 80,
            population: 128,
            // Solvers must be able to invert the scramble's wide turns.
            max_wide: self.max_layer_span.max(1),
            ..SolverBudget::for_depth(self.solve_depth)
        };
        self.solve_job = self.solve_job.wrapping_add(1);
        let job = self.solve_job;
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.events.clear();
        self.best = None;
        self.replaying = false;
        self.active_replay_turn = None;
        self.solving = true;
        self.status = "Solvers running in parallel".to_string();

        thread::spawn(move || {
            let event_tx = tx.clone();
            let run = run_solver_lab_observed(snapshot, budget, |event| {
                let _ = event_tx.send(LabMessage::Event(job, event.clone()));
            });
            let _ = tx.send(LabMessage::Finished(job, run));
        });
    }

    fn poll_solver(&mut self) {
        let Some(rx) = self.rx.take() else {
            return;
        };
        let mut keep_rx = true;
        while let Ok(message) = rx.try_recv() {
            match message {
                LabMessage::Event(job, event) => {
                    if job != self.solve_job {
                        continue;
                    }
                    if let Some(candidate) = &event.candidate {
                        let better = self
                            .best
                            .as_ref()
                            .map(|old| {
                                (candidate.move_count, candidate.elapsed_ms)
                                    < (old.move_count, old.elapsed_ms)
                            })
                            .unwrap_or(true);
                        if candidate.solved && better {
                            self.best = Some(candidate.clone());
                        }
                    }
                    self.events.push(event);
                    if self.events.len() > 400 {
                        self.events.drain(0..100);
                    }
                }
                LabMessage::Finished(job, run) => {
                    if job != self.solve_job {
                        continue;
                    }
                    self.solving = false;
                    self.best = run.best.clone().or_else(|| self.best.clone());
                    self.status = if let Some(best) = &self.best {
                        format!(
                            "Solved: fewest verified path has {} moves from {}",
                            best.move_count, best.worker_id
                        )
                    } else {
                        "Solve budget exhausted without a verified path".to_string()
                    };
                    self.events = run.events;
                    self.persist_latest_run();
                    self.refresh_history();
                    keep_rx = false;
                }
            }
        }
        if keep_rx {
            self.rx = Some(rx);
        }
    }

    fn persist_latest_run(&mut self) {
        let (Some(store), Some(challenge), Some(best)) = (
            self.store.as_ref(),
            self.challenge.as_ref(),
            self.best.clone(),
        ) else {
            return;
        };
        let stats = serde_json::json!({
            "events": self.events.len(),
            "workers": worker_names(&self.events),
        });
        let record = SolveRecord {
            cube_size: challenge.size(),
            seed: challenge.seed(),
            scramble_depth: challenge.scramble_depth(),
            worker_stats_json: stats.to_string(),
            best,
            heuristic_weights_json: r#"{"mismatch":1.0,"move_penalty":0.001}"#.to_string(),
        };
        let _ = store.insert_record(&record);
    }

    fn refresh_history(&mut self) {
        self.history = self
            .store
            .as_ref()
            .and_then(|store| store.list_recent(8).ok())
            .unwrap_or_default();
    }

    fn start_replay(&mut self) {
        let Some(challenge) = &self.challenge else {
            return;
        };
        if self.best.is_none() {
            return;
        }
        self.visible_cube = challenge.cube().clone();
        self.replaying = true;
        self.replay_step = 0;
        self.active_replay_turn = None;
        self.status = "Replaying highlighted path".to_string();
    }

    fn tick_replay(&mut self, ctx: &egui::Context) {
        if let Some(turn) = self.active_replay_turn {
            ctx.request_repaint_after(Duration::from_millis(16));
            let progress = turn.progress();
            if progress < 1.0 {
                return;
            }
            let _ = self.visible_cube.apply_move(turn.mv);
            self.replay_step += 1;
            self.active_replay_turn = None;
            return;
        }

        if !self.replaying {
            return;
        }
        let Some(best) = &self.best else {
            self.replaying = false;
            return;
        };
        if self.replay_step >= best.moves.len() {
            self.replaying = false;
            self.status = "Replay complete".to_string();
            return;
        }
        if let Some(mv) = best.moves.get(self.replay_step).copied() {
            self.active_replay_turn = Some(ReplayTurn {
                mv,
                started: Instant::now(),
                duration: Duration::from_millis(520),
            });
            self.status = format!(
                "Replaying move {}/{}: {}",
                self.replay_step + 1,
                best.moves.len(),
                mv.notation(self.visible_cube.size())
            );
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn active_turn(&self) -> Option<ActiveTurn> {
        self.active_replay_turn.map(|turn| ActiveTurn {
            mv: turn.mv,
            progress: turn.progress(),
        })
    }
}

impl eframe::App for SolverLabApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let settings = serde_json::json!({
            "dark_mode": self.dark_mode,
            "ui_scale": self.ui_scale,
            "cube_size": self.cube_size,
            "scramble_depth": self.scramble_depth,
            "max_layer_span": self.max_layer_span,
            "seed": self.seed,
            "solve_depth": self.solve_depth,
            "sample_limit": self.sample_limit,
            "grid_count": self.grid_count,
            "grid_cube_n": self.grid_cube_n,
        });
        storage.set_string(SETTINGS_KEY, settings.to_string());
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Theme, and UI scale applied only when it changes (avoids DPI churn).
        ctx.set_visuals(if self.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        if (self.ui_scale - self.applied_scale).abs() > f32::EPSILON {
            ctx.set_pixels_per_point(self.ui_scale);
            self.applied_scale = self.ui_scale;
        }

        self.poll_generation();
        self.poll_solver();
        self.tick_replay(ctx);
        if self.generating {
            ctx.request_repaint();
        }
        let busy = self.generating;

        // Keyboard shortcuts (ignored while editing a field): Space=solve,
        // N=new challenge, R=replay best, G/C/V=switch view.
        let typing = ctx.memory(|m| m.focused().is_some());
        if !typing {
            let keys = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::Space),
                    i.key_pressed(egui::Key::N),
                    i.key_pressed(egui::Key::R),
                    i.key_pressed(egui::Key::C),
                    i.key_pressed(egui::Key::V),
                    i.key_pressed(egui::Key::G),
                )
            });
            if keys.0 && !self.solving && !busy {
                self.start_solving();
            }
            if keys.1 && !busy {
                self.generate_challenge();
            }
            if keys.2 && self.best.is_some() {
                self.start_replay();
            }
            if keys.3 {
                self.view_mode = ViewMode::Cube;
            }
            if keys.4 {
                self.view_mode = ViewMode::Net;
            }
            if keys.5 {
                self.view_mode = ViewMode::Grid;
            }
        }

        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("Rust NxN Cube Solver Lab");
                ui.separator();
                ui.label("N");
                ui.add(
                    egui::DragValue::new(&mut self.cube_size)
                        .range(2..=2000)
                        .speed(1),
                )
                .on_hover_text("Cube edge length N — the cube is N×N×N (2–2000)");
                ui.label("scramble");
                ui.add(egui::DragValue::new(&mut self.scramble_depth).range(1..=200))
                    .on_hover_text("Number of random moves used to scramble");
                ui.label("wide span");
                ui.add(egui::DragValue::new(&mut self.max_layer_span).range(1..=256))
                    .on_hover_text(
                        "Widest block the scramble may turn (and that the solver inverts)",
                    );
                ui.label("seed");
                ui.add(egui::DragValue::new(&mut self.seed).speed(1))
                    .on_hover_text("Deterministic RNG seed — same seed reproduces the scramble");
                ui.label("solve depth");
                ui.add(egui::DragValue::new(&mut self.solve_depth).range(1..=10))
                    .on_hover_text("Search depth budget for the solvers");
                ui.label("sample");
                ui.add(egui::DragValue::new(&mut self.sample_limit).range(4..=64))
                    .on_hover_text(
                        "Max face resolution drawn — large cubes are down-sampled to this",
                    );
                if ui
                    .add_enabled(!busy, egui::Button::new("New challenge"))
                    .clicked()
                {
                    self.generate_challenge();
                }
                if ui
                    .add_enabled(!self.solving && !busy, egui::Button::new("Solve"))
                    .clicked()
                {
                    self.start_solving();
                }
                if ui
                    .add_enabled(self.best.is_some(), egui::Button::new("Replay best"))
                    .clicked()
                {
                    self.start_replay();
                }
                ui.separator();
                let theme = if self.dark_mode {
                    "☀ Light"
                } else {
                    "🌙 Dark"
                };
                if ui.button(theme).clicked() {
                    self.dark_mode = !self.dark_mode;
                }
                ui.label("scale");
                ui.add(
                    egui::DragValue::new(&mut self.ui_scale)
                        .range(0.75..=2.0)
                        .speed(0.05),
                );
                if busy {
                    ui.add(egui::Spinner::new());
                }
            });
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak("shortcuts: Space solve · N new · R replay · C/V/G view");
                });
            });
        });

        egui::SidePanel::right("solver_lanes")
            .resizable(true)
            .min_width(220.0)
            .default_width(360.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Parallel workers");
                    ui.separator();
                    draw_worker_summary(
                        ui,
                        &self.events,
                        self.best.as_ref().map(|c| c.worker_id.as_str()),
                    );
                    ui.separator();
                    ui.heading("Best path");
                    draw_best_path(ui, self.best.as_ref(), self.visible_cube.size());
                    ui.separator();
                    ui.heading("History");
                    if self.history.is_empty() {
                        ui.weak("No solves yet.");
                    }
                    for record in &self.history {
                        ui.label(format!(
                            "{}x{} seed {}: {} moves by {}",
                            record.cube_size.get(),
                            record.cube_size.get(),
                            record.seed,
                            record.best.move_count,
                            record.best.worker_id
                        ));
                    }

                    ui.separator();
                    ui.collapsing("About / Help", |ui| {
                        ui.label("Generate an N×N scramble and race three solvers to a verified solution.");
                        ui.add_space(4.0);
                        ui.label("Solvers:");
                        ui.weak("• Meet-in-the-middle — exact, best for shallow scrambles");
                        ui.weak("• Beam search — mismatch-guided");
                        ui.weak("• Island GA — parallel evolutionary search");
                        ui.add_space(4.0);
                        ui.label("Views: 3D cube (drag to orbit) · 2D net · Wall (many cubes at once).");
                        ui.weak("Shortcuts: Space solve · N new · R replay · C/V/G views");
                        ui.add_space(4.0);
                        ui.weak(
                            "Huge cubes use O(N) rotation + face sampling, so generation and \
                             rendering stay fast. Solutions are verified by replay.",
                        );
                    });
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Adaptive cube viewport");
                ui.separator();
                ui.selectable_value(&mut self.view_mode, ViewMode::Cube, "3D cube");
                ui.selectable_value(&mut self.view_mode, ViewMode::Net, "2D net");
                ui.selectable_value(&mut self.view_mode, ViewMode::Grid, "Wall");
                ui.separator();
                if self.view_mode == ViewMode::Grid {
                    ui.label("cubes");
                    ui.add(egui::DragValue::new(&mut self.grid_count).range(4..=400))
                        .on_hover_text("How many cubes solve simultaneously on the wall");
                    ui.label("N");
                    ui.add(egui::DragValue::new(&mut self.grid_cube_n).range(2..=20));
                    ui.label("speed");
                    ui.add(
                        egui::Slider::new(&mut self.grid_speed, 0.25..=4.0)
                            .logarithmic(true)
                            .show_value(false),
                    )
                    .on_hover_text("Animation speed of the wall");
                } else {
                    let n = self.visible_cube.size().get();
                    if n > self.sample_limit {
                        ui.label(format!(
                            "sampled {}x{} from {}x{} faces",
                            self.sample_limit, self.sample_limit, n, n
                        ));
                    } else {
                        ui.label("exact stickers");
                    }
                    ui.separator();
                    // Solved badge — mismatch is O(N^2), so only compute it for
                    // moderate cubes to keep the frame cheap.
                    if n * n * 6 <= 60_000 {
                        let total = (n * n * 6) as f32;
                        let mismatched = self.visible_cube.mismatch_count();
                        if mismatched == 0 {
                            ui.colored_label(egui::Color32::from_rgb(60, 200, 90), "✔ solved");
                        } else {
                            let pct = 100.0 * (1.0 - mismatched as f32 / total);
                            ui.colored_label(
                                egui::Color32::from_rgb(230, 170, 60),
                                format!("{pct:.0}% solved · {mismatched} off"),
                            );
                        }
                    }
                }
            });
            ui.add_space(8.0);
            let log_height = 150.0;
            match self.view_mode {
                ViewMode::Net => {
                    draw_cube_net(ui, &self.visible_cube, self.sample_limit);
                }
                ViewMode::Cube => {
                    let active_turn = self.active_turn();
                    let avail = ui.available_size();
                    let scene_height = (avail.y - log_height - 24.0).max(160.0);
                    ui.allocate_ui(egui::vec2(avail.x, scene_height), |ui| {
                        let response = scene::draw_scene(
                            ui,
                            &self.visible_cube,
                            self.sample_limit,
                            &mut self.view_angles,
                            active_turn,
                        );
                        if response.dragged() {
                            ctx.request_repaint();
                        }
                    });
                    ui.label("drag to orbit");
                }
                ViewMode::Grid => {
                    // A wall of independent cubes perpetually solving in real time.
                    let scramble_len = (self.grid_cube_n * 6).max(8);
                    let model = self.grid_model.get_or_insert_with(|| {
                        GridModel::new(self.grid_count, self.grid_cube_n, scramble_len)
                    });
                    model.reconfigure(self.grid_count, self.grid_cube_n, scramble_len);
                    let dt = ctx.input(|i| i.stable_dt).min(0.05) * self.grid_speed;
                    model.tick(dt);
                    grid::draw_grid(ui, model, self.sample_limit, 72.0);
                    ctx.request_repaint(); // continuous animation
                }
            }
            ui.add_space(10.0);
            if self.view_mode != ViewMode::Grid {
                draw_event_log(ui, &self.events);
            }
        });
    }
}

fn draw_cube_net(ui: &mut egui::Ui, cube: &StickerCube, sample_limit: usize) {
    let n = cube.size().get();
    let dim = n.min(sample_limit.max(2));
    let available = ui.available_size();
    let cell = (available.x / ((dim * 4) as f32 + 6.0)).clamp(2.0, 18.0);
    let face_px = cell * dim as f32;
    let gap = cell * 0.8;
    let width = face_px * 4.0 + gap * 3.0;
    let height = face_px * 3.0 + gap * 2.0;
    let (rect, _response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let origin = rect.min;
    let samples = [
        (Face::Up, 1.0, 0.0),
        (Face::Left, 0.0, 1.0),
        (Face::Front, 1.0, 1.0),
        (Face::Right, 2.0, 1.0),
        (Face::Back, 3.0, 1.0),
        (Face::Down, 1.0, 2.0),
    ];

    for (face, x, y) in samples {
        let sample = cube.face_sample(face, dim);
        let face_origin = origin + egui::vec2(x * (face_px + gap), y * (face_px + gap));
        draw_face_sample(&painter, face_origin, cell, &sample);
    }
}

fn draw_face_sample(painter: &egui::Painter, origin: egui::Pos2, cell: f32, sample: &FaceSample) {
    let dim = sample.cells.len();
    let face_rect =
        egui::Rect::from_min_size(origin, egui::vec2(cell * dim as f32, cell * dim as f32));
    painter.rect_filled(face_rect.expand(2.0), 3.0, egui::Color32::from_gray(25));
    for (row, row_cells) in sample.cells.iter().enumerate() {
        for (col, color) in row_cells.iter().enumerate() {
            let min = origin + egui::vec2(col as f32 * cell, row as f32 * cell);
            let rect = egui::Rect::from_min_size(min, egui::vec2(cell - 0.6, cell - 0.6));
            painter.rect_filled(rect, 1.5, egui_color(*color));
        }
    }
    painter.text(
        origin + egui::vec2(4.0, 4.0),
        egui::Align2::LEFT_TOP,
        sample.face.label(),
        egui::FontId::monospace(12.0),
        egui::Color32::BLACK,
    );
}

fn draw_worker_summary(ui: &mut egui::Ui, events: &[WorkerEvent], best_worker: Option<&str>) {
    const GREEN: egui::Color32 = egui::Color32::from_rgb(60, 200, 90);
    const GOLD: egui::Color32 = egui::Color32::from_rgb(245, 200, 60);

    for (worker, label) in [
        ("deterministic", "Meet-in-the-middle"),
        ("beam", "Beam search"),
        ("evolution", "Island GA"),
    ] {
        let latest = events.iter().rev().find(|event| event.worker_id == worker);
        let solved = events.iter().any(|e| {
            e.worker_id == worker && e.candidate.as_ref().map(|c| c.solved).unwrap_or(false)
        });
        let is_winner = best_worker == Some(worker);

        ui.group(|ui| {
            ui.horizontal(|ui| {
                if solved {
                    ui.colored_label(GREEN, label);
                } else if latest.is_some() {
                    ui.strong(label);
                } else {
                    ui.weak(label);
                }
                if is_winner {
                    ui.colored_label(GOLD, "★ winner");
                } else if solved {
                    ui.colored_label(GREEN, "✔ solved");
                }
                if let Some(event) = latest {
                    ui.weak(format!("gen {} · {} nodes", event.generation, event.nodes));
                }
            });
            if let Some(event) = latest {
                let progress = event.best_fitness.clamp(0.0, 1.0);
                let bar = egui::ProgressBar::new(progress).show_percentage();
                ui.add(if solved { bar.fill(GREEN) } else { bar });
                ui.label(format!(
                    "best {} moves · {}",
                    if event.best_move_count == usize::MAX {
                        "—".to_string()
                    } else {
                        event.best_move_count.to_string()
                    },
                    event.message
                ));
            } else {
                ui.weak("waiting…");
            }
        });
    }
}

fn draw_best_path(ui: &mut egui::Ui, best: Option<&SolutionCandidate>, size: CubeSize) {
    let Some(best) = best else {
        ui.label("No verified path yet");
        return;
    };
    ui.horizontal(|ui| {
        ui.label(format!(
            "{} verified moves from {} in {} ms",
            best.move_count, best.worker_id, best.elapsed_ms
        ));
        let notation = best
            .moves
            .iter()
            .map(|mv| mv.notation(size))
            .collect::<Vec<_>>()
            .join(" ");
        if ui
            .add_enabled(!notation.is_empty(), egui::Button::new("⎘ copy"))
            .on_hover_text("Copy the solution in cube notation")
            .clicked()
        {
            ui.ctx().copy_text(notation);
        }
    });
    egui::ScrollArea::horizontal()
        .max_height(46.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for mv in &best.moves {
                    ui.colored_label(
                        egui::Color32::from_rgb(245, 190, 54),
                        format!(" {} ", mv.notation(size)),
                    );
                }
            });
        });
}

fn draw_event_log(ui: &mut egui::Ui, events: &[WorkerEvent]) {
    ui.heading("Live solver trace");
    egui::ScrollArea::vertical()
        .max_height(150.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for event in events.iter().rev().take(80).rev() {
                ui.label(format!(
                    "[{}] gen {} fitness {:.3}: {}",
                    event.worker_id, event.generation, event.best_fitness, event.message
                ));
            }
        });
}

fn worker_names(events: &[WorkerEvent]) -> Vec<String> {
    let mut names = Vec::new();
    for event in events {
        if !names.contains(&event.worker_id) {
            names.push(event.worker_id.clone());
        }
    }
    names
}

fn egui_color(color: Color) -> egui::Color32 {
    let [r, g, b] = color.as_rgb();
    egui::Color32::from_rgb(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cube_core::Move;

    #[test]
    fn cube_scene_builds_depth_sorted_3d_stickers() {
        let cube = StickerCube::solved(CubeSize::new(3).unwrap());
        let scene = build_projected_stickers(
            &cube,
            3,
            ViewAngles {
                yaw: -0.6,
                pitch: 0.55,
            },
            None,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 420.0)),
        );

        assert!(scene.len() >= 27);
        assert!(scene.windows(2).all(|pair| pair[0].depth <= pair[1].depth));
        assert!(scene.iter().any(|patch| patch.face == Face::Front));
        assert!(scene.iter().any(|patch| patch.face == Face::Up));
        assert!(scene.iter().any(|patch| patch.face == Face::Right));
    }

    #[test]
    fn active_turn_changes_projected_sticker_geometry() {
        let size = CubeSize::new(3).unwrap();
        let cube = StickerCube::solved(size);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 420.0));
        let angles = ViewAngles {
            yaw: -0.6,
            pitch: 0.55,
        };

        let still = build_projected_stickers(&cube, 3, angles, None, rect);
        let moving = build_projected_stickers(
            &cube,
            3,
            angles,
            Some(ActiveTurn {
                mv: Move::face(Face::Right, size, 1),
                progress: 0.5,
            }),
            rect,
        );

        assert_ne!(still[0].points, moving[0].points);
    }
}
