use cube_core::{Axis, CubeSize, CubeState, Move, StickerCube};
use std::time::Instant;

fn parse_sizes() -> Vec<usize> {
    let parsed: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|value| value.parse().ok())
        .filter(|&n| n >= 4)
        .collect();
    if parsed.is_empty() {
        vec![64, 256, 1_024, 2_000]
    } else {
        parsed
    }
}

fn benchmark(n: usize) {
    let size = CubeSize::new(n).expect("N must be at least 2");
    let mut cube = StickerCube::solved(size);
    let inner = Move::new(Axis::X, n / 2, n / 2, 1);
    let outer = Move::new(Axis::X, 0, 0, 1);
    // Keep total touched side-band stickers roughly comparable while retaining
    // enough samples for stable timing. Four turns restore the original state.
    let cycles = (2_000_000usize / (4 * n)).clamp(4, 2_000);

    let started = Instant::now();
    for _ in 0..cycles {
        for _ in 0..4 {
            cube.apply_move(inner).expect("valid inner turn");
        }
    }
    let inner_elapsed = started.elapsed();
    assert!(cube.is_solved());

    let started = Instant::now();
    for _ in 0..cycles {
        for _ in 0..4 {
            cube.apply_move(outer).expect("valid outer turn");
        }
    }
    let outer_elapsed = started.elapsed();
    assert!(cube.is_solved());

    let turns = (cycles * 4) as f64;
    println!(
        "{n},{cycles},{:.3},{:.3}",
        inner_elapsed.as_secs_f64() * 1_000_000.0 / turns,
        outer_elapsed.as_secs_f64() * 1_000_000.0 / turns
    );
}

fn main() {
    println!("n,cycles,inner_us_per_quarter,outer_us_per_quarter");
    for n in parse_sizes() {
        benchmark(n);
    }
}
