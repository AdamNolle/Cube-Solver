// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use cube_wasm::NativeReductionControl;
use std::sync::Mutex;
use tauri::State;

struct ActiveSolve {
    request_token: String,
    control: NativeReductionControl,
}

#[derive(Default)]
struct SolveState {
    active: Mutex<Option<ActiveSolve>>,
}

impl SolveState {
    fn begin(&self, request_token: String, control: NativeReductionControl) -> Result<(), String> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| "native solver state is unavailable".to_string())?;
        if let Some(previous) = active.replace(ActiveSolve {
            request_token,
            control,
        }) {
            previous.control.cancel();
        }
        Ok(())
    }

    /// Clear only the request that actually completed. A stale task must never
    /// remove a newer request's cancellation handle after a page reload/remount.
    fn finish(&self, request_token: &str) -> Result<(), String> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| "native solver state is unavailable".to_string())?;
        if active
            .as_ref()
            .is_some_and(|solve| solve.request_token == request_token)
        {
            active.take();
        }
        Ok(())
    }

    fn cancel(&self, request_token: &str) -> Result<bool, String> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| "native solver state is unavailable".to_string())?;
        if active
            .as_ref()
            .is_some_and(|solve| solve.request_token == request_token)
        {
            if let Some(solve) = active.take() {
                solve.control.cancel();
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Run desktop reduction on a native blocking thread. The command receives only
/// visible sticker colours; scramble history never crosses the frontend boundary.
#[tauri::command(rename_all = "camelCase")]
async fn solve_stickers(
    state: State<'_, SolveState>,
    request_token: String,
    n: usize,
    colors: Vec<u8>,
) -> Result<String, String> {
    if request_token.len() < 16 || request_token.len() > 128 {
        return Err("invalid solve request token".to_string());
    }
    if !(4..=11).contains(&n) || colors.len() != 6 * n * n {
        return Err("invalid cube size or sticker buffer".to_string());
    }

    let control = cube_wasm::native_reduction_control(n);
    state.begin(request_token.clone(), control.clone())?;

    let worker_control = control.clone();
    let join_result = tauri::async_runtime::spawn_blocking(move || {
        cube_wasm::solve_reduction_sticker_state(n, &colors, &worker_control)
    })
    .await;

    // Cleanup is unconditional and token-aware, including task join failure.
    state.finish(&request_token)?;
    join_result.map_err(|error| format!("native solver task failed: {error}"))?
}

#[tauri::command(rename_all = "camelCase")]
fn cancel_solve(state: State<'_, SolveState>, request_token: String) -> Result<bool, String> {
    state.cancel(&request_token)
}

fn main() {
    tauri::Builder::default()
        .manage(SolveState::default())
        .invoke_handler(tauri::generate_handler![solve_stickers, cancel_solve])
        .run(tauri::generate_context!())
        .expect("error while running Cube Solver");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_completion_cannot_clear_or_cancel_new_request() {
        let state = SolveState::default();
        let old = cube_wasm::native_reduction_control(4);
        let new = cube_wasm::native_reduction_control(4);
        state
            .begin("old-request-token-0001".to_string(), old.clone())
            .unwrap();
        state
            .begin("new-request-token-0002".to_string(), new.clone())
            .unwrap();
        assert!(!old.should_continue(), "replacement must cancel old work");

        state.finish("old-request-token-0001").unwrap();
        assert!(
            new.should_continue(),
            "stale completion cleared new control"
        );
        assert!(!state.cancel("old-request-token-0001").unwrap());
        assert!(state.cancel("new-request-token-0002").unwrap());
        assert!(!new.should_continue());
    }
}
