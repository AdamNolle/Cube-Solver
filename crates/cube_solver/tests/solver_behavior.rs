use cube_core::{Challenge, ChallengeSpec, CubeSize, CubeState, Face, Move, StickerCube};
use cube_solver::{
    face_turn_move_set, run_solver_lab, simplify_moves, wide_move_set, DeterministicSolver,
    SolutionCandidate, SolverBudget,
};

fn replay_is_solved(snapshot: &cube_core::CubeSnapshot, moves: &[Move]) -> bool {
    let mut cube = StickerCube::from_snapshot(snapshot.clone());
    for mv in moves {
        cube.apply_move(*mv).unwrap();
    }
    cube.is_solved()
}

#[test]
fn deterministic_solver_returns_replay_verified_solution_without_scramble_history() {
    let challenge = Challenge::generate(
        CubeSize::new(3).unwrap(),
        ChallengeSpec {
            seed: 7,
            scramble_depth: 3,
            max_layer_span: 1,
        },
    )
    .unwrap();
    let snapshot = challenge.cube().clone_snapshot();

    let result = DeterministicSolver
        .solve(snapshot.clone(), SolverBudget::for_depth(6))
        .unwrap();

    assert!(result.solved);
    assert!(replay_is_solved(&snapshot, &result.moves));
}

#[test]
fn path_simplifier_preserves_final_state() {
    let size = CubeSize::new(4).unwrap();
    let moves = vec![
        Move::face(Face::Right, size, 1),
        Move::face(Face::Right, size, 1),
        Move::face(Face::Right, size, 1),
        Move::face(Face::Up, size, 1),
        Move::face(Face::Up, size, -1),
    ];

    let simplified = simplify_moves(&moves);
    assert_eq!(simplified, vec![Move::face(Face::Right, size, -1)]);

    let mut before = StickerCube::solved(size);
    for mv in &moves {
        before.apply_move(*mv).unwrap();
    }
    let mut after = StickerCube::solved(size);
    for mv in &simplified {
        after.apply_move(*mv).unwrap();
    }

    assert_eq!(before.clone_snapshot(), after.clone_snapshot());
}

#[test]
fn lab_coordinator_highlights_fewest_verified_solution() {
    let challenge = Challenge::generate(
        CubeSize::new(2).unwrap(),
        ChallengeSpec {
            seed: 11,
            scramble_depth: 2,
            max_layer_span: 1,
        },
    )
    .unwrap();
    let snapshot = challenge.cube().clone_snapshot();

    let run = run_solver_lab(snapshot.clone(), SolverBudget::for_depth(5));

    let best = run.best.expect("expected a verified solution");
    assert!(best.solved);
    assert!(replay_is_solved(&snapshot, &best.moves));
    assert!(run
        .events
        .iter()
        .any(|event| event.worker_id == "deterministic"));
}

#[test]
fn solved_candidates_are_replay_verified() {
    let size = CubeSize::new(3).unwrap();
    let snapshot = StickerCube::solved(size).clone_snapshot();
    let candidate =
        SolutionCandidate::new("test", vec![Move::face(Face::Front, size, 4)], true, 1.0);

    assert!(candidate.verify_against(&snapshot).unwrap());
}

#[test]
fn face_turn_move_set_contains_outer_turns_only() {
    let moves = face_turn_move_set(CubeSize::new(5).unwrap());

    assert_eq!(moves.len(), 18);
    assert!(moves.iter().all(|mv| mv.layer_count() == 1));
}

#[test]
fn wide_move_set_includes_inner_blocks_and_matches_outer_at_width_one() {
    let size = CubeSize::new(5).unwrap();
    assert_eq!(wide_move_set(size, 1), face_turn_move_set(size));

    let wide = wide_move_set(size, 3);
    assert!(wide.iter().any(|mv| mv.layer_count() == 2));
    assert!(wide.iter().any(|mv| mv.layer_count() == 3));
    // Every move must be replay-legal on the cube.
    let mut cube = StickerCube::solved(size);
    for mv in &wide {
        cube.apply_move(*mv).unwrap();
        cube.apply_move(mv.inverse()).unwrap();
    }
}

#[test]
fn deterministic_solver_handles_wide_layer_scrambles() {
    // A width-2 scramble is unsolvable with outer turns alone; the wide-aware
    // move set must invert it.
    let challenge = Challenge::generate(
        CubeSize::new(4).unwrap(),
        ChallengeSpec {
            seed: 99,
            scramble_depth: 3,
            max_layer_span: 2,
        },
    )
    .unwrap();
    let snapshot = challenge.cube().clone_snapshot();

    let budget = SolverBudget {
        max_wide: 2,
        ..SolverBudget::for_depth(6)
    };
    let result = DeterministicSolver
        .solve(snapshot.clone(), budget)
        .expect("wide-aware solver should solve a shallow wide scramble");

    assert!(result.solved);
    assert!(replay_is_solved(&snapshot, &result.moves));
}

#[test]
fn huge_n_smoke_challenges_are_replay_verified() {
    for n in [25, 100] {
        let challenge = Challenge::generate(
            CubeSize::new(n).unwrap(),
            ChallengeSpec {
                seed: n as u64,
                scramble_depth: 2,
                max_layer_span: 1,
            },
        )
        .unwrap();
        let snapshot = challenge.cube().clone_snapshot();

        let result = DeterministicSolver
            .solve(snapshot.clone(), SolverBudget::for_depth(4))
            .unwrap();

        assert!(result.solved, "expected solved result for {n}x{n}");
        assert!(
            replay_is_solved(&snapshot, &result.moves),
            "expected replay verification for {n}x{n}"
        );
    }
}
