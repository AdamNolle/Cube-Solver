//! Persistent solve history.

use cube_core::CubeSize;
use cube_solver::SolutionCandidate;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid stored cube size {0}")]
    InvalidCubeSize(usize),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolveRecord {
    pub cube_size: CubeSize,
    pub seed: u64,
    pub scramble_depth: usize,
    pub worker_stats_json: String,
    pub best: SolutionCandidate,
    pub heuristic_weights_json: String,
}

pub struct SolveStore {
    conn: Connection,
}

impl SolveStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn insert_record(&self, record: &SolveRecord) -> Result<i64, StoreError> {
        let best_json = serde_json::to_string(&record.best)?;
        self.conn.execute(
            "INSERT INTO solve_records
                (cube_size, seed, scramble_depth, worker_stats_json, best_json, heuristic_weights_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                record.cube_size.get() as i64,
                record.seed as i64,
                record.scramble_depth as i64,
                record.worker_stats_json,
                best_json,
                record.heuristic_weights_json,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_recent(&self, limit: usize) -> Result<Vec<SolveRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT cube_size, seed, scramble_depth, worker_stats_json, best_json, heuristic_weights_json
             FROM solve_records
             ORDER BY id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let cube_size: i64 = row.get(0)?;
            let seed: i64 = row.get(1)?;
            let scramble_depth: i64 = row.get(2)?;
            let worker_stats_json: String = row.get(3)?;
            let best_json: String = row.get(4)?;
            let heuristic_weights_json: String = row.get(5)?;
            Ok((
                cube_size,
                seed,
                scramble_depth,
                worker_stats_json,
                best_json,
                heuristic_weights_json,
            ))
        })?;

        let mut records = Vec::new();
        for row in rows {
            let (
                cube_size,
                seed,
                scramble_depth,
                worker_stats_json,
                best_json,
                heuristic_weights_json,
            ) = row?;
            let cube_size = CubeSize::new(cube_size as usize)
                .map_err(|_| StoreError::InvalidCubeSize(cube_size as usize))?;
            let best = serde_json::from_str(&best_json)?;
            records.push(SolveRecord {
                cube_size,
                seed: seed as u64,
                scramble_depth: scramble_depth as usize,
                worker_stats_json,
                best,
                heuristic_weights_json,
            });
        }

        Ok(records)
    }

    fn migrate(&self) -> Result<(), StoreError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS solve_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                cube_size INTEGER NOT NULL,
                seed INTEGER NOT NULL,
                scramble_depth INTEGER NOT NULL,
                worker_stats_json TEXT NOT NULL,
                best_json TEXT NOT NULL,
                heuristic_weights_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_solve_records_created_at
                ON solve_records(created_at DESC);",
        )?;
        Ok(())
    }
}
