//! Core cube state and move interfaces.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CubeError {
    #[error("cube size must be at least 2, got {0}")]
    InvalidSize(usize),
    #[error("layer range {start}..={end} is outside cube size {size}")]
    LayerOutOfBounds {
        start: usize,
        end: usize,
        size: usize,
    },
    #[error("layer range start {start} is greater than end {end}")]
    InvalidLayerRange { start: usize, end: usize },
    #[error("snapshot has {actual} stickers, expected {expected}")]
    InvalidStickerCount { actual: usize, expected: usize },
    #[error("color {color:?} appears {actual} times, expected {expected}")]
    InvalidColorCount {
        color: Color,
        actual: usize,
        expected: usize,
    },
    #[error("challenge scramble depth must be greater than zero")]
    InvalidScrambleDepth,
    #[error("maximum layer span must be greater than zero")]
    InvalidLayerSpan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CubeSize(usize);

impl CubeSize {
    pub fn new(size: usize) -> Result<Self, CubeError> {
        if size < 2 {
            return Err(CubeError::InvalidSize(size));
        }
        Ok(Self(size))
    }

    pub const fn get(self) -> usize {
        self.0
    }

    pub const fn stickers(self) -> usize {
        6 * self.0 * self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Color {
    White,
    Yellow,
    Green,
    Blue,
    Orange,
    Red,
}

impl Color {
    pub const ALL: [Self; 6] = [
        Self::White,
        Self::Yellow,
        Self::Green,
        Self::Blue,
        Self::Orange,
        Self::Red,
    ];

    pub const fn as_rgb(self) -> [u8; 3] {
        match self {
            Self::White => [245, 245, 240],
            Self::Yellow => [244, 214, 66],
            Self::Green => [38, 155, 86],
            Self::Blue => [37, 92, 191],
            Self::Orange => [235, 126, 39],
            Self::Red => [204, 44, 56],
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::White => "white",
            Self::Yellow => "yellow",
            Self::Green => "green",
            Self::Blue => "blue",
            Self::Orange => "orange",
            Self::Red => "red",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Face {
    Up,
    Down,
    Front,
    Back,
    Left,
    Right,
}

impl Face {
    pub const ALL: [Self; 6] = [
        Self::Up,
        Self::Down,
        Self::Front,
        Self::Back,
        Self::Left,
        Self::Right,
    ];

    pub const fn color(self) -> Color {
        match self {
            Self::Up => Color::White,
            Self::Down => Color::Yellow,
            Self::Front => Color::Green,
            Self::Back => Color::Blue,
            Self::Left => Color::Orange,
            Self::Right => Color::Red,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Up => "U",
            Self::Down => "D",
            Self::Front => "F",
            Self::Back => "B",
            Self::Left => "L",
            Self::Right => "R",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Move {
    pub axis: Axis,
    pub layer_start: usize,
    pub layer_end: usize,
    pub turns: i8,
}

impl Move {
    pub fn new(axis: Axis, layer_start: usize, layer_end: usize, turns: i8) -> Self {
        Self {
            axis,
            layer_start,
            layer_end,
            turns: canonical_turns(turns),
        }
    }

    pub fn face(face: Face, size: CubeSize, turns: i8) -> Self {
        Self::wide(face, size, 1, turns)
    }

    pub fn wide(face: Face, size: CubeSize, width: usize, turns: i8) -> Self {
        let n = size.get();
        let width = width.clamp(1, n);
        match face {
            Face::Up => Self::new(Axis::Y, n - width, n - 1, turns),
            Face::Down => Self::new(Axis::Y, 0, width - 1, -turns),
            Face::Right => Self::new(Axis::X, n - width, n - 1, turns),
            Face::Left => Self::new(Axis::X, 0, width - 1, -turns),
            Face::Front => Self::new(Axis::Z, n - width, n - 1, turns),
            Face::Back => Self::new(Axis::Z, 0, width - 1, -turns),
        }
    }

    pub fn inverse(self) -> Self {
        Self::new(self.axis, self.layer_start, self.layer_end, -self.turns)
    }

    pub fn is_noop(self) -> bool {
        self.turns == 0
    }

    pub fn layer_count(self) -> usize {
        self.layer_end.saturating_sub(self.layer_start) + 1
    }

    pub fn validate(self, size: CubeSize) -> Result<(), CubeError> {
        let n = size.get();
        if self.layer_start > self.layer_end {
            return Err(CubeError::InvalidLayerRange {
                start: self.layer_start,
                end: self.layer_end,
            });
        }
        if self.layer_end >= n {
            return Err(CubeError::LayerOutOfBounds {
                start: self.layer_start,
                end: self.layer_end,
                size: n,
            });
        }
        Ok(())
    }

    pub fn notation(self, size: CubeSize) -> String {
        if let Some((face, width, face_turns)) = self.as_face_like(size) {
            let width_prefix = if width > 1 {
                format!("{width}")
            } else {
                String::new()
            };
            let suffix = match face_turns {
                -1 => "'",
                2 => "2",
                1 => "",
                _ => "",
            };
            format!("{width_prefix}{}{suffix}", face.label())
        } else {
            let suffix = match self.turns {
                -1 => "'",
                2 => "2",
                1 => "",
                _ => "",
            };
            format!(
                "{:?}[{}..={}]{}",
                self.axis, self.layer_start, self.layer_end, suffix
            )
        }
    }

    fn as_face_like(self, size: CubeSize) -> Option<(Face, usize, i8)> {
        let n = size.get();
        match self.axis {
            Axis::Y if self.layer_end == n - 1 => Some((Face::Up, self.layer_count(), self.turns)),
            Axis::Y if self.layer_start == 0 => {
                Some((Face::Down, self.layer_count(), canonical_turns(-self.turns)))
            }
            Axis::X if self.layer_end == n - 1 => {
                Some((Face::Right, self.layer_count(), self.turns))
            }
            Axis::X if self.layer_start == 0 => {
                Some((Face::Left, self.layer_count(), canonical_turns(-self.turns)))
            }
            Axis::Z if self.layer_end == n - 1 => {
                Some((Face::Front, self.layer_count(), self.turns))
            }
            Axis::Z if self.layer_start == 0 => {
                Some((Face::Back, self.layer_count(), canonical_turns(-self.turns)))
            }
            _ => None,
        }
    }
}

fn canonical_turns(turns: i8) -> i8 {
    match turns.rem_euclid(4) {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => -1,
        _ => unreachable!(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CubeSnapshot {
    size: CubeSize,
    stickers: Vec<Color>,
}

impl CubeSnapshot {
    pub fn size(&self) -> CubeSize {
        self.size
    }

    pub fn stickers(&self) -> &[Color] {
        &self.stickers
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StickerCube {
    size: CubeSize,
    stickers: Vec<Color>,
}

impl StickerCube {
    pub fn solved(size: CubeSize) -> Self {
        let n2 = size.get() * size.get();
        let mut stickers = Vec::with_capacity(size.stickers());
        for face in Face::ALL {
            stickers.extend(std::iter::repeat_n(face.color(), n2));
        }
        Self { size, stickers }
    }

    pub fn from_snapshot(snapshot: CubeSnapshot) -> Self {
        Self {
            size: snapshot.size,
            stickers: snapshot.stickers,
        }
    }

    pub fn size(&self) -> CubeSize {
        self.size
    }

    pub fn color_at(&self, face: Face, row: usize, col: usize) -> Option<Color> {
        if row >= self.size.get() || col >= self.size.get() {
            return None;
        }
        Some(self.stickers[self.index(face, row, col)])
    }

    pub fn color_histogram(&self) -> HashMap<Color, usize> {
        let mut counts = HashMap::new();
        for color in &self.stickers {
            *counts.entry(*color).or_insert(0) += 1;
        }
        counts
    }

    pub fn mismatch_count(&self) -> usize {
        let n = self.size.get();
        let mut mismatches = 0;
        for face in Face::ALL {
            let expected = face.color();
            for row in 0..n {
                for col in 0..n {
                    if self.stickers[self.index(face, row, col)] != expected {
                        mismatches += 1;
                    }
                }
            }
        }
        mismatches
    }

    fn index(&self, face: Face, row: usize, col: usize) -> usize {
        let n = self.size.get();
        face_index(face) * n * n + row * n + col
    }
}

pub trait CubeState {
    fn size(&self) -> CubeSize;
    fn apply_move(&mut self, mv: Move) -> Result<(), CubeError>;
    fn is_solved(&self) -> bool;
    fn face_sample(&self, face: Face, max_cells: usize) -> FaceSample;
    fn clone_snapshot(&self) -> CubeSnapshot;
    fn validate(&self) -> Result<(), CubeError>;
}

impl CubeState for StickerCube {
    fn size(&self) -> CubeSize {
        self.size
    }

    fn apply_move(&mut self, mv: Move) -> Result<(), CubeError> {
        mv.validate(self.size)?;
        if mv.is_noop() {
            return Ok(());
        }

        let repetitions = match mv.turns {
            1 => 1,
            2 => 2,
            -1 => 3,
            _ => 0,
        };

        for _ in 0..repetitions {
            self.apply_positive_quarter_turn(mv);
        }
        Ok(())
    }

    fn is_solved(&self) -> bool {
        self.mismatch_count() == 0
    }

    fn face_sample(&self, face: Face, max_cells: usize) -> FaceSample {
        let n = self.size.get();
        let dim = max_cells.max(1).min(n);
        let sampled = dim < n;
        let mut cells = vec![vec![face.color(); dim]; dim];

        for (sample_row, row_cells) in cells.iter_mut().enumerate() {
            let row = sample_index(sample_row, dim, n);
            for (sample_col, cell) in row_cells.iter_mut().enumerate() {
                let col = sample_index(sample_col, dim, n);
                *cell = self.stickers[self.index(face, row, col)];
            }
        }

        FaceSample {
            face,
            source_size: n,
            cells,
            sampled,
        }
    }

    fn clone_snapshot(&self) -> CubeSnapshot {
        CubeSnapshot {
            size: self.size,
            stickers: self.stickers.clone(),
        }
    }

    fn validate(&self) -> Result<(), CubeError> {
        let expected_stickers = self.size.stickers();
        if self.stickers.len() != expected_stickers {
            return Err(CubeError::InvalidStickerCount {
                actual: self.stickers.len(),
                expected: expected_stickers,
            });
        }

        let counts = self.color_histogram();
        let expected = self.size.get() * self.size.get();
        for color in Color::ALL {
            let actual = counts.get(&color).copied().unwrap_or(0);
            if actual != expected {
                return Err(CubeError::InvalidColorCount {
                    color,
                    actual,
                    expected,
                });
            }
        }
        Ok(())
    }
}

impl StickerCube {
    /// Apply a +90° turn of the layers `[layer_start, layer_end]` about `mv.axis`.
    ///
    /// Only stickers whose coordinate along the turn axis lies in the layer range
    /// move, and on each face those form a contiguous row/column band. We touch
    /// just those `O(affected)` stickers (≈ `O(N)` for an inner slice) instead of
    /// cloning and scanning all `6·N²` — essential for replaying the `O(N²)`-move
    /// solutions the reduction solver produces on huge cubes.
    fn apply_positive_quarter_turn(&mut self, mv: Move) {
        let n = self.size.get();
        let mut writes: Vec<(usize, Color)> =
            Vec::with_capacity(affected_estimate(mv.layer_count(), n));

        for face in Face::ALL {
            let Some((row_lo, row_hi, col_lo, col_hi)) =
                affected_band(face, mv.axis, mv.layer_start, mv.layer_end, n)
            else {
                continue;
            };
            for row in row_lo..=row_hi {
                for col in col_lo..=col_hi {
                    let idx = self.index(face, row, col);
                    let (coord, normal) = face_cell_to_coord(face, row, col, n);
                    let rotated_coord = rotate_coord_positive(coord, mv.axis, n);
                    let rotated_normal = rotate_normal_positive(normal, mv.axis);
                    let (new_face, new_row, new_col) =
                        coord_to_face_cell(rotated_coord, rotated_normal, n);
                    let new_idx = self.index(new_face, new_row, new_col);
                    writes.push((new_idx, self.stickers[idx]));
                }
            }
        }

        for (new_idx, color) in writes {
            self.stickers[new_idx] = color;
        }
    }

    /// Reference implementation kept for differential testing against the fast path.
    #[cfg(test)]
    fn apply_positive_quarter_turn_reference(&mut self, mv: Move) {
        let n = self.size.get();
        let old = self.stickers.clone();
        let mut next = old.clone();

        for face in Face::ALL {
            for row in 0..n {
                for col in 0..n {
                    let idx = self.index(face, row, col);
                    let (coord, normal) = face_cell_to_coord(face, row, col, n);
                    if layer_value(coord, mv.axis) < mv.layer_start
                        || layer_value(coord, mv.axis) > mv.layer_end
                    {
                        continue;
                    }

                    let rotated_coord = rotate_coord_positive(coord, mv.axis, n);
                    let rotated_normal = rotate_normal_positive(normal, mv.axis);
                    let (new_face, new_row, new_col) =
                        coord_to_face_cell(rotated_coord, rotated_normal, n);
                    let new_idx = self.index(new_face, new_row, new_col);
                    next[new_idx] = old[idx];
                }
            }
        }

        self.stickers = next;
    }
}

fn affected_estimate(layer_count: usize, n: usize) -> usize {
    // 4 side bands of width `layer_count` plus up to 2 full cap faces.
    4 * layer_count * n + 2 * n * n
}

/// How a face's along-axis layer value depends on its (row, col): a constant
/// plane, or a forward/reversed function of the row or the column.
enum BandAxis {
    Const(usize),
    RowFwd,
    RowRev,
    ColFwd,
    ColRev,
}

fn band_axis(face: Face, axis: Axis, n: usize) -> BandAxis {
    let last = n - 1;
    match (face, axis) {
        (Face::Up, Axis::X) => BandAxis::ColFwd,
        (Face::Up, Axis::Y) => BandAxis::Const(last),
        (Face::Up, Axis::Z) => BandAxis::RowFwd,
        (Face::Down, Axis::X) => BandAxis::ColFwd,
        (Face::Down, Axis::Y) => BandAxis::Const(0),
        (Face::Down, Axis::Z) => BandAxis::RowRev,
        (Face::Front, Axis::X) => BandAxis::ColFwd,
        (Face::Front, Axis::Y) => BandAxis::RowRev,
        (Face::Front, Axis::Z) => BandAxis::Const(last),
        (Face::Back, Axis::X) => BandAxis::ColRev,
        (Face::Back, Axis::Y) => BandAxis::RowRev,
        (Face::Back, Axis::Z) => BandAxis::Const(0),
        (Face::Left, Axis::X) => BandAxis::Const(0),
        (Face::Left, Axis::Y) => BandAxis::RowRev,
        (Face::Left, Axis::Z) => BandAxis::ColFwd,
        (Face::Right, Axis::X) => BandAxis::Const(last),
        (Face::Right, Axis::Y) => BandAxis::RowRev,
        (Face::Right, Axis::Z) => BandAxis::ColRev,
    }
}

/// Inclusive (row_lo, row_hi, col_lo, col_hi) rectangle of cells on `face` whose
/// along-`axis` layer value falls in `[start, end]`, or `None` if the face is
/// untouched.
fn affected_band(
    face: Face,
    axis: Axis,
    start: usize,
    end: usize,
    n: usize,
) -> Option<(usize, usize, usize, usize)> {
    let last = n - 1;
    // Map a forward layer range [start, end] to a cell-index range, reversing for
    // faces whose layer value decreases with the cell index.
    let fwd = (start, end);
    let rev = (last - end, last - start);
    match band_axis(face, axis, n) {
        BandAxis::Const(c) => {
            if c >= start && c <= end {
                Some((0, last, 0, last))
            } else {
                None
            }
        }
        BandAxis::RowFwd => Some((fwd.0, fwd.1, 0, last)),
        BandAxis::RowRev => Some((rev.0, rev.1, 0, last)),
        BandAxis::ColFwd => Some((0, last, fwd.0, fwd.1)),
        BandAxis::ColRev => Some((0, last, rev.0, rev.1)),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceSample {
    pub face: Face,
    pub source_size: usize,
    pub cells: Vec<Vec<Color>>,
    pub sampled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChallengeSpec {
    pub seed: u64,
    pub scramble_depth: usize,
    pub max_layer_span: usize,
}

#[derive(Clone, Debug)]
pub struct Challenge {
    size: CubeSize,
    seed: u64,
    scramble_depth: usize,
    cube: StickerCube,
}

impl Challenge {
    pub fn generate(size: CubeSize, spec: ChallengeSpec) -> Result<Self, CubeError> {
        if spec.scramble_depth == 0 {
            return Err(CubeError::InvalidScrambleDepth);
        }
        if spec.max_layer_span == 0 {
            return Err(CubeError::InvalidLayerSpan);
        }

        let mut rng = ChaCha8Rng::seed_from_u64(spec.seed);
        let mut cube = StickerCube::solved(size);
        let faces = Face::ALL;
        let turns = [-1, 1, 2];
        let max_width = spec.max_layer_span.min(size.get());
        let mut previous: Option<Move> = None;

        for _ in 0..spec.scramble_depth {
            let mv = loop {
                let face = faces[rng.gen_range(0..faces.len())];
                let width = rng.gen_range(1..=max_width);
                let turn = turns[rng.gen_range(0..turns.len())];
                let candidate = Move::wide(face, size, width, turn);
                if previous
                    .map(|old| {
                        old.axis != candidate.axis || old.layer_start != candidate.layer_start
                    })
                    .unwrap_or(true)
                {
                    break candidate;
                }
            };
            cube.apply_move(mv)?;
            previous = Some(mv);
        }

        Ok(Self {
            size,
            seed: spec.seed,
            scramble_depth: spec.scramble_depth,
            cube,
        })
    }

    pub fn size(&self) -> CubeSize {
        self.size
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn scramble_depth(&self) -> usize {
        self.scramble_depth
    }

    pub fn cube(&self) -> &StickerCube {
        &self.cube
    }

    pub fn into_cube(self) -> StickerCube {
        self.cube
    }
}

fn sample_index(sample: usize, sample_dim: usize, source_dim: usize) -> usize {
    if sample_dim <= 1 {
        0
    } else {
        sample * (source_dim - 1) / (sample_dim - 1)
    }
}

fn face_index(face: Face) -> usize {
    match face {
        Face::Up => 0,
        Face::Down => 1,
        Face::Front => 2,
        Face::Back => 3,
        Face::Left => 4,
        Face::Right => 5,
    }
}

type Coord = (usize, usize, usize);
type Normal = (i8, i8, i8);

fn face_cell_to_coord(face: Face, row: usize, col: usize, n: usize) -> (Coord, Normal) {
    match face {
        Face::Up => ((col, n - 1, row), (0, 1, 0)),
        Face::Down => ((col, 0, n - 1 - row), (0, -1, 0)),
        Face::Front => ((col, n - 1 - row, n - 1), (0, 0, 1)),
        Face::Back => ((n - 1 - col, n - 1 - row, 0), (0, 0, -1)),
        Face::Left => ((0, n - 1 - row, col), (-1, 0, 0)),
        Face::Right => ((n - 1, n - 1 - row, n - 1 - col), (1, 0, 0)),
    }
}

fn coord_to_face_cell(coord: Coord, normal: Normal, n: usize) -> (Face, usize, usize) {
    let (x, y, z) = coord;
    match normal {
        (0, 1, 0) => (Face::Up, z, x),
        (0, -1, 0) => (Face::Down, n - 1 - z, x),
        (0, 0, 1) => (Face::Front, n - 1 - y, x),
        (0, 0, -1) => (Face::Back, n - 1 - y, n - 1 - x),
        (-1, 0, 0) => (Face::Left, n - 1 - y, z),
        (1, 0, 0) => (Face::Right, n - 1 - y, n - 1 - z),
        _ => unreachable!("invalid sticker normal {normal:?}"),
    }
}

#[cfg(test)]
fn layer_value(coord: Coord, axis: Axis) -> usize {
    match axis {
        Axis::X => coord.0,
        Axis::Y => coord.1,
        Axis::Z => coord.2,
    }
}

fn rotate_coord_positive(coord: Coord, axis: Axis, n: usize) -> Coord {
    let (x, y, z) = coord;
    match axis {
        Axis::X => (x, n - 1 - z, y),
        Axis::Y => (z, y, n - 1 - x),
        Axis::Z => (n - 1 - y, x, z),
    }
}

fn rotate_normal_positive(normal: Normal, axis: Axis) -> Normal {
    let (x, y, z) = normal;
    match axis {
        Axis::X => (x, -z, y),
        Axis::Y => (z, y, -x),
        Axis::Z => (-y, x, z),
    }
}

#[cfg(test)]
mod fast_rotation_tests {
    use super::*;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    /// A non-uniform cube produced via the trusted reference turn, so differential
    /// comparison is sensitive (a solved cube hides most permutation errors).
    fn scrambled(n: usize, seed: u64) -> StickerCube {
        let size = CubeSize::new(n).unwrap();
        let mut cube = StickerCube::solved(size);
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        for _ in 0..(2 * n + 8) {
            let face = Face::ALL[rng.gen_range(0..Face::ALL.len())];
            let width = rng.gen_range(1..=n);
            cube.apply_positive_quarter_turn_reference(Move::wide(face, size, width, 1));
        }
        cube
    }

    fn assert_same(base: &StickerCube, mv: Move) {
        let mut fast = base.clone();
        let mut reference = base.clone();
        fast.apply_positive_quarter_turn(mv);
        reference.apply_positive_quarter_turn_reference(mv);
        assert_eq!(
            fast.stickers, reference.stickers,
            "fast != reference for {mv:?}"
        );
    }

    #[test]
    fn fast_quarter_turn_matches_reference_all_sizes_and_layers() {
        for n in 2..=8 {
            let size = CubeSize::new(n).unwrap();
            let base = scrambled(n, n as u64 * 2_654_435_761);

            // Every outer/wide face turn.
            for face in Face::ALL {
                for width in 1..=n {
                    assert_same(&base, Move::wide(face, size, width, 1));
                }
            }
            // Every axis and every contiguous layer range (inner slices included).
            for axis in [Axis::X, Axis::Y, Axis::Z] {
                for s in 0..n {
                    for e in s..n {
                        assert_same(&base, Move::new(axis, s, e, 1));
                    }
                }
            }
        }
    }
}
