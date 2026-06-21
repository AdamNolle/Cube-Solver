//! "Wall of cubes" grid: many cubes solving simultaneously in real time.
//!
//! Each cell is an independent agent that scrambles its cube, then animates the
//! guaranteed solution (the inverse of its own scramble) move by move, then
//! re-scrambles — so the whole wall is perpetually "solving", a mastermind /
//! evolution-in-motion effect. Rendering is decoupled from N via
//! `CubeState::face_sample` (each cell samples down to a small grid), and uses
//! level-of-detail by on-screen cell size, so hundreds of cells stay smooth.

use crate::scene::{self, ViewAngles};
use cube_core::{CubeState, Move, StickerCube};
use cube_solver::face_turn_move_set;
use eframe::egui;

/// Tiny deterministic RNG (xorshift64*) so the grid needs no extra dependency
/// and stays reproducible per seed.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// One solving cube in the wall.
struct Agent {
    cube: StickerCube,
    moves: Vec<Move>,
    moveset: Vec<Move>,
    cursor: usize,
    timer: f32,
    step: f32,
    rng: Rng,
    rounds: u32,
}

impl Agent {
    fn new(n: usize, seed: u64, scramble_len: usize) -> Self {
        let size = cube_core::CubeSize::new(n).expect("grid cube size >= 2");
        let moveset = face_turn_move_set(size);
        let mut agent = Agent {
            cube: StickerCube::solved(size),
            moves: Vec::new(),
            moveset,
            cursor: 0,
            timer: 0.0,
            // Vary the cadence a little so the wall does not pulse in lockstep.
            step: 0.16 + (seed % 7) as f32 * 0.02,
            rng: Rng::new(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1)),
            rounds: 0,
        };
        agent.scramble(scramble_len);
        agent
    }

    /// Scramble from solved and store the solution (the inverse sequence).
    fn scramble(&mut self, len: usize) {
        let mut cube = {
            let n = self.cube.size();
            StickerCube::solved(n)
        };
        let mut scramble = Vec::with_capacity(len);
        let mut last: Option<Move> = None;
        for _ in 0..len {
            let mv = self.moveset[self.rng.below(self.moveset.len())];
            // Avoid trivially cancelling the previous move.
            if last
                .map(|p| p.axis == mv.axis && p.layer_start == mv.layer_start)
                .unwrap_or(false)
            {
                continue;
            }
            cube.apply_move(mv).expect("valid scramble move");
            scramble.push(mv);
            last = Some(mv);
        }
        self.cube = cube;
        self.moves = scramble.iter().rev().map(|m| m.inverse()).collect();
        self.cursor = 0;
        self.timer = 0.0;
    }

    fn tick(&mut self, dt: f32, scramble_len: usize) {
        if self.cursor >= self.moves.len() {
            // Solved: pause a beat, then start a fresh round.
            self.timer += dt;
            if self.timer > 0.6 {
                self.rounds = self.rounds.wrapping_add(1);
                self.scramble(scramble_len);
            }
            return;
        }
        self.timer += dt;
        while self.timer >= self.step && self.cursor < self.moves.len() {
            self.timer -= self.step;
            let mv = self.moves[self.cursor];
            let _ = self.cube.apply_move(mv);
            self.cursor += 1;
        }
    }

    fn progress(&self) -> f32 {
        if self.moves.is_empty() {
            1.0
        } else {
            self.cursor as f32 / self.moves.len() as f32
        }
    }
}

/// The grid of solving cubes.
pub struct GridModel {
    agents: Vec<Agent>,
    pub n: usize,
    pub count: usize,
    pub scramble_len: usize,
}

impl GridModel {
    pub fn new(count: usize, n: usize, scramble_len: usize) -> Self {
        let agents = (0..count)
            .map(|i| {
                Agent::new(
                    n,
                    0xA1B2_C3D4 ^ (i as u64).wrapping_mul(0x100_0193),
                    scramble_len,
                )
            })
            .collect();
        GridModel {
            agents,
            n,
            count,
            scramble_len,
        }
    }

    /// Rebuild if the cube size, agent count, or scramble depth changed.
    pub fn reconfigure(&mut self, count: usize, n: usize, scramble_len: usize) {
        if self.n != n || self.count != count || self.scramble_len != scramble_len {
            *self = GridModel::new(count, n, scramble_len);
        }
    }

    pub fn tick(&mut self, dt: f32) {
        for agent in &mut self.agents {
            agent.tick(dt, self.scramble_len);
        }
    }
}

/// Render the wall. Virtualizes by painting only cells inside the visible
/// viewport, and degrades detail for small cells.
pub fn draw_grid(ui: &mut egui::Ui, model: &GridModel, sample_limit: usize, cell_px: f32) {
    let cols = ((ui.available_width() / cell_px).floor() as usize).max(1);
    let rows = model.agents.len().div_ceil(cols);
    let gap = 6.0;
    let total_h = rows as f32 * (cell_px + gap);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            ui.set_height(total_h);
            let origin = ui.min_rect().min;
            let painter = ui.painter();
            let visible_top = viewport.min.y;
            let visible_bottom = viewport.max.y;

            for (i, agent) in model.agents.iter().enumerate() {
                let r = i / cols;
                let c = i % cols;
                let y = r as f32 * (cell_px + gap);
                // Cull rows outside the viewport (virtualization).
                if y + cell_px < visible_top || y > visible_bottom {
                    continue;
                }
                let x = c as f32 * (cell_px + gap);
                let cell = egui::Rect::from_min_size(
                    origin + egui::vec2(x, y),
                    egui::vec2(cell_px, cell_px),
                );
                draw_cell(painter, cell, agent, sample_limit);
            }
        });
}

fn draw_cell(painter: &egui::Painter, rect: egui::Rect, agent: &Agent, sample_limit: usize) {
    let progress = agent.progress();
    // Background tint by progress: red (scrambled) -> green (solved).
    let bg = egui::Color32::from_rgb(
        (60.0 * (1.0 - progress) + 14.0) as u8,
        (20.0 + 50.0 * progress) as u8,
        24,
    );
    painter.rect_filled(rect, 3.0, bg);

    if rect.width() < 16.0 {
        // LOD: tiny cells become a single progress tile.
        let inner = rect.shrink(1.5);
        painter.rect_filled(inner, 1.0, progress_color(progress));
        return;
    }

    // Isometric cube via the shared projector; sample big cubes down for free.
    let dim = sample_limit.clamp(2, agent.cube.size().get()).min(8);
    let angles = ViewAngles {
        yaw: -0.6,
        pitch: 0.55,
    };
    let pad = rect.shrink(rect.width() * 0.08);
    let stickers = scene::build_projected_stickers(&agent.cube, dim, angles, None, pad);
    for s in &stickers {
        painter.add(egui::Shape::convex_polygon(
            s.points.to_vec(),
            s.color,
            egui::Stroke::NONE,
        ));
    }
}

fn progress_color(p: f32) -> egui::Color32 {
    egui::Color32::from_rgb((220.0 * (1.0 - p)) as u8, (200.0 * p) as u8, 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_solution_solves_its_scramble() {
        let mut agent = Agent::new(3, 42, 12);
        assert!(
            !agent.cube.is_solved(),
            "a fresh scramble should be unsolved"
        );
        while agent.cursor < agent.moves.len() {
            let mv = agent.moves[agent.cursor];
            agent.cube.apply_move(mv).unwrap();
            agent.cursor += 1;
        }
        assert!(agent.cube.is_solved(), "playing the solution must solve it");
    }

    #[test]
    fn grid_model_count_and_reconfigure() {
        let mut model = GridModel::new(16, 3, 12);
        assert_eq!(model.agents.len(), 16);
        model.reconfigure(9, 4, 16);
        assert_eq!(model.agents.len(), 9);
        assert_eq!(model.n, 4);
    }

    #[test]
    fn ticking_never_panics_and_keeps_cubes_legal() {
        let mut model = GridModel::new(6, 3, 8);
        for _ in 0..300 {
            model.tick(0.1);
        }
        // Every agent's cube must still be a legal cube (color counts preserved).
        for agent in &model.agents {
            assert!(agent.cube.validate().is_ok());
        }
    }
}
