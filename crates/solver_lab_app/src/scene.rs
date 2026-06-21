//! Interactive 3D cube projection and animation.
//!
//! `build_projected_stickers` turns a `StickerCube` into a depth-sorted list of
//! flat quads (one per visible sticker) using an orthographic projection of the
//! three camera-facing faces. Large cubes are down-sampled for free via
//! `CubeState::face_sample`, so the cost is bounded by `sample_dim^2`, not `N^2`.
//!
//! The face/coordinate convention is replicated verbatim from
//! `cube_core::face_cell_to_coord` so that an animated slice turn rotates exactly
//! the stickers a real `Move` would move.

use cube_core::{CubeState, Face, Move, StickerCube};
use eframe::egui;

/// Camera orientation: yaw orbits about the vertical axis, pitch tilts up/down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewAngles {
    pub yaw: f32,
    pub pitch: f32,
}

impl ViewAngles {
    /// Apply a mouse drag (in points) as an orbit, clamping pitch so the cube
    /// never flips past the poles.
    pub fn orbit(&mut self, drag: egui::Vec2) {
        self.yaw += drag.x * 0.01;
        self.pitch = (self.pitch + drag.y * 0.01).clamp(-1.4, 1.4);
    }
}

/// A slice turn currently being animated. `progress` runs 0.0..=1.0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActiveTurn {
    pub mv: Move,
    pub progress: f32,
}

/// One projected sticker quad, ready to paint. `depth` is the camera-space z of
/// the quad centroid; the list is sorted ascending so callers paint back-to-front.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectedSticker {
    pub face: Face,
    pub depth: f32,
    pub points: [egui::Pos2; 4],
    pub color: egui::Color32,
}

type V3 = [f32; 3];

fn axis_index(mv: Move) -> usize {
    use cube_core::Axis;
    match mv.axis {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    }
}

/// Integer cube coordinate (0..m on each axis) for the sampled sticker at
/// (face, row, col), matching `cube_core::face_cell_to_coord`.
fn sample_coord(face: Face, row: usize, col: usize, m: usize) -> [usize; 3] {
    let last = m - 1;
    match face {
        Face::Up => [col, last, row],
        Face::Down => [col, 0, last - row],
        Face::Front => [col, last - row, last],
        Face::Back => [last - col, last - row, 0],
        Face::Left => [0, last - row, col],
        Face::Right => [last, last - row, last - col],
    }
}

/// Outward unit normal of a face in cube space.
fn face_normal(face: Face) -> V3 {
    match face {
        Face::Up => [0.0, 1.0, 0.0],
        Face::Down => [0.0, -1.0, 0.0],
        Face::Front => [0.0, 0.0, 1.0],
        Face::Back => [0.0, 0.0, -1.0],
        Face::Left => [-1.0, 0.0, 0.0],
        Face::Right => [1.0, 0.0, 0.0],
    }
}

/// The two in-plane axes (0=x,1=y,2=z) for a face, given its normal axis.
fn in_plane_axes(normal_axis: usize) -> (usize, usize) {
    match normal_axis {
        0 => (1, 2), // X normal -> vary Y, Z
        1 => (0, 2), // Y normal -> vary X, Z
        _ => (0, 1), // Z normal -> vary X, Y
    }
}

fn normal_axis_of(face: Face) -> usize {
    match face {
        Face::Left | Face::Right => 0,
        Face::Up | Face::Down => 1,
        Face::Front | Face::Back => 2,
    }
}

/// Center of cell `i` (of `m`) mapped into [-1, 1].
fn cell_center(i: usize, m: usize) -> f32 {
    -1.0 + (2.0 * i as f32 + 1.0) / m as f32
}

fn rotate_about_axis(p: V3, axis: usize, theta: f32) -> V3 {
    let (s, c) = theta.sin_cos();
    let [x, y, z] = p;
    match axis {
        0 => [x, y * c - z * s, y * s + z * c],
        1 => [x * c + z * s, y, -x * s + z * c],
        _ => [x * c - y * s, x * s + y * c, z],
    }
}

/// Apply camera yaw (about Y) then pitch (about X).
fn camera_transform(p: V3, angles: ViewAngles) -> V3 {
    let after_yaw = rotate_about_axis(p, 1, angles.yaw);
    rotate_about_axis(after_yaw, 0, angles.pitch)
}

/// Project a sampled cube into depth-sorted sticker quads.
///
/// Only the three faces whose outward normal points toward the camera are
/// emitted (≈ `3 * sample_dim^2` quads). `active_turn`, if present, rotates the
/// quads in its layer by `progress * turns * 90°` so a turn animates smoothly.
pub fn build_projected_stickers(
    cube: &StickerCube,
    dim: usize,
    angles: ViewAngles,
    active_turn: Option<ActiveTurn>,
    rect: egui::Rect,
) -> Vec<ProjectedSticker> {
    let n = cube.size().get();
    let m = dim.clamp(2, n);
    let half = 1.0 / m as f32;
    let center = rect.center();
    let scale = 0.42 * rect.height().min(rect.width()).max(1.0);

    // Map the move's original-cube layer range down to sample space.
    let turn_layer = active_turn.map(|turn| {
        let ax = axis_index(turn.mv);
        let scale_layer = |layer: usize| -> usize {
            if n <= 1 {
                0
            } else {
                (layer * (m - 1) + (n - 1) / 2) / (n - 1)
            }
        };
        let lo = scale_layer(turn.mv.layer_start);
        let hi = scale_layer(turn.mv.layer_end);
        let theta = turn.progress * turn.mv.turns as f32 * std::f32::consts::FRAC_PI_2;
        (ax, lo, hi, theta)
    });

    let mut out = Vec::with_capacity(3 * m * m);

    for face in Face::ALL {
        // Back-face culling: only emit camera-facing faces.
        let cam_normal = camera_transform(face_normal(face), angles);
        if cam_normal[2] <= 1e-4 {
            continue;
        }

        // Directional shading so the three visible faces read as distinct planes.
        let shade = face_shade(cam_normal);

        let normal_axis = normal_axis_of(face);
        let (a1, a2) = in_plane_axes(normal_axis);
        let sample = cube.face_sample(face, m);

        for (row, row_cells) in sample.cells.iter().enumerate() {
            for (col, color) in row_cells.iter().enumerate() {
                let coord = sample_coord(face, row, col, m);
                let cx = cell_center(coord[0], m);
                let cy = cell_center(coord[1], m);
                let cz = cell_center(coord[2], m);
                let center3: V3 = [cx, cy, cz];

                // Four corners: ±half along the two in-plane axes.
                let corners_dir = [(-1.0, -1.0), (-1.0, 1.0), (1.0, 1.0), (1.0, -1.0)];
                let mut points = [egui::Pos2::ZERO; 4];
                let mut depth_sum = 0.0;

                for (i, (s1, s2)) in corners_dir.iter().enumerate() {
                    let mut p = center3;
                    p[a1] += s1 * half;
                    p[a2] += s2 * half;

                    // Slice-turn animation.
                    if let Some((ax, lo, hi, theta)) = turn_layer {
                        let layer = coord[ax];
                        if layer >= lo && layer <= hi {
                            p = rotate_about_axis(p, ax, theta);
                        }
                    }

                    let cam = camera_transform(p, angles);
                    depth_sum += cam[2];
                    points[i] = egui::pos2(center.x + scale * cam[0], center.y - scale * cam[1]);
                }

                out.push(ProjectedSticker {
                    face,
                    depth: depth_sum / 4.0,
                    points,
                    color: shade_color(egui_color(*color), shade),
                });
            }
        }
    }

    out.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    out
}

/// Draw the cube into `ui`, returning the response so the caller can detect drag.
/// Mutates `angles` on drag (mouse orbit).
pub fn draw_scene(
    ui: &mut egui::Ui,
    cube: &StickerCube,
    sample_dim: usize,
    angles: &mut ViewAngles,
    active_turn: Option<ActiveTurn>,
) -> egui::Response {
    let size = ui.available_size();
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::drag());
    if response.dragged() {
        angles.orbit(response.drag_delta());
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, egui::Color32::from_gray(18));

    let stickers = build_projected_stickers(cube, sample_dim, *angles, active_turn, rect);
    let outline = egui::Stroke::new(1.0, egui::Color32::from_gray(8));
    for sticker in &stickers {
        painter.add(egui::Shape::convex_polygon(
            sticker.points.to_vec(),
            sticker.color,
            outline,
        ));
    }
    response
}

fn egui_color(color: cube_core::Color) -> egui::Color32 {
    let [r, g, b] = color.as_rgb();
    egui::Color32::from_rgb(r, g, b)
}

/// Lambert-ish brightness for a face given its camera-space normal, with a fixed
/// light from the upper-front. Keeps an ambient floor so no face goes black.
fn face_shade(cam_normal: V3) -> f32 {
    const LIGHT: V3 = [0.32, 0.55, 0.77];
    let dot = cam_normal[0] * LIGHT[0] + cam_normal[1] * LIGHT[1] + cam_normal[2] * LIGHT[2];
    (0.62 + 0.45 * dot.max(0.0)).clamp(0.45, 1.0)
}

fn shade_color(color: egui::Color32, factor: f32) -> egui::Color32 {
    let scale = |c: u8| (c as f32 * factor).round().clamp(0.0, 255.0) as u8;
    egui::Color32::from_rgb(scale(color.r()), scale(color.g()), scale(color.b()))
}
