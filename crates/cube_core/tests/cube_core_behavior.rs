use cube_core::{Challenge, ChallengeSpec, CubeSize, CubeState, Face, Move, StickerCube};
use proptest::prelude::*;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

#[test]
fn face_turn_inverse_round_trips_many_sizes() {
    for n in [2, 3, 4, 7, 25] {
        let size = CubeSize::new(n).unwrap();
        let mut cube = StickerCube::solved(size);
        let original = cube.clone_snapshot();
        let mv = Move::face(Face::Right, size, 1);

        cube.apply_move(mv).unwrap();
        cube.apply_move(mv.inverse()).unwrap();

        assert_eq!(cube.clone_snapshot(), original, "failed for {n}x{n}");
    }
}

#[test]
fn four_quarter_turns_equal_identity() {
    let size = CubeSize::new(5).unwrap();
    for face in Face::ALL {
        let mut cube = StickerCube::solved(size);
        let original = cube.clone_snapshot();
        let mv = Move::face(face, size, 1);

        for _ in 0..4 {
            cube.apply_move(mv).unwrap();
        }

        assert_eq!(cube.clone_snapshot(), original, "failed for {face:?}");
    }
}

#[test]
fn generated_challenge_is_legal_and_does_not_expose_scramble_history() {
    let challenge = Challenge::generate(
        CubeSize::new(9).unwrap(),
        ChallengeSpec {
            seed: 4242,
            scramble_depth: 12,
            max_layer_span: 2,
        },
    )
    .unwrap();

    assert_eq!(challenge.size().get(), 9);
    assert_eq!(challenge.seed(), 4242);
    assert_eq!(challenge.scramble_depth(), 12);
    assert!(challenge.cube().validate().is_ok());
    assert_eq!(
        challenge.cube().color_histogram().values().sum::<usize>(),
        6 * 9 * 9
    );
}

#[test]
fn adaptive_face_sample_caps_large_faces() {
    let cube = StickerCube::solved(CubeSize::new(100).unwrap());
    let sample = cube.face_sample(Face::Front, 16);

    assert_eq!(sample.source_size, 100);
    assert_eq!(sample.cells.len(), 16);
    assert_eq!(sample.cells[0].len(), 16);
    assert!(sample.sampled);
}

proptest! {
    #[test]
    fn random_sequences_preserve_counts_and_inverse_to_solved(
        n in 2usize..8,
        seed in any::<u64>(),
        depth in 1usize..24,
    ) {
        let size = CubeSize::new(n).unwrap();
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut cube = StickerCube::solved(size);
        let expected_counts = cube.color_histogram();
        let mut moves = Vec::with_capacity(depth);

        for _ in 0..depth {
            let face = Face::ALL[rng.gen_range(0..Face::ALL.len())];
            let turns = [-1, 1, 2][rng.gen_range(0..3)];
            let mv = Move::face(face, size, turns);
            cube.apply_move(mv).unwrap();
            moves.push(mv);
        }

        prop_assert_eq!(cube.color_histogram(), expected_counts);

        for mv in moves.into_iter().rev() {
            cube.apply_move(mv.inverse()).unwrap();
        }

        prop_assert!(cube.is_solved());
    }
}

#[test]
fn challenge_is_reproducible_by_seed() {
    let spec = ChallengeSpec {
        seed: 12_345,
        scramble_depth: 12,
        max_layer_span: 3,
    };
    let size = CubeSize::new(6).unwrap();
    let a = Challenge::generate(size, spec).unwrap();
    let b = Challenge::generate(size, spec).unwrap();
    assert_eq!(
        a.cube().clone_snapshot(),
        b.cube().clone_snapshot(),
        "same seed must reproduce the same scramble"
    );

    // A different seed should (almost surely) differ.
    let c = Challenge::generate(size, ChallengeSpec { seed: 999, ..spec }).unwrap();
    assert_ne!(a.cube().clone_snapshot(), c.cube().clone_snapshot());
}
