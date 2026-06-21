use cube_core::CubeSize;
use cube_solver::SolutionCandidate;
use solver_store::{SolveRecord, SolveStore};

#[test]
fn solve_records_round_trip_through_sqlite() {
    let store = SolveStore::open_in_memory().unwrap();
    let record = SolveRecord {
        cube_size: CubeSize::new(4).unwrap(),
        seed: 123,
        scramble_depth: 9,
        worker_stats_json: r#"{"deterministic":{"nodes":42}}"#.to_string(),
        best: SolutionCandidate::new("deterministic", vec![], true, 0.0),
        heuristic_weights_json: r#"{"mismatch":1.0}"#.to_string(),
    };

    let id = store.insert_record(&record).unwrap();
    let recent = store.list_recent(5).unwrap();

    assert_eq!(id, 1);
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].cube_size, CubeSize::new(4).unwrap());
    assert_eq!(recent[0].seed, 123);
    assert_eq!(recent[0].best.worker_id, "deterministic");
}
