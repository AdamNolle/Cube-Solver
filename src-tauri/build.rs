use std::path::Path;

/// Guard against shipping a stale or missing frontend.
///
/// This repo has lived in an iCloud-synced folder, where sync has silently
/// deleted `web/index.html` and scattered `" 2"` conflict duplicates. Because
/// Tauri embeds whatever is on disk at build time, that produced apps with a
/// broken/empty UI and no error. We fail the build loudly instead — a stale
/// frontend should never make it into a bundle.
fn assert_frontend_is_fresh() {
    let web = Path::new("../web");

    let index = web.join("index.html");
    let html = std::fs::read_to_string(&index).unwrap_or_else(|e| {
        panic!(
            "Cube Solver build aborted: cannot read web/index.html ({e}).\n\
             The frontend is missing (likely an iCloud sync conflict). Regenerate it \
             with `python3 tools/gen-index.py` before building."
        )
    });

    // Structural markers that only the real, fully-wired UI contains.
    for marker in [
        "wireRealSolver",
        "data-cancel-solve",
        "CubeLab",
        "solver-worker.js",
    ] {
        assert!(
            html.contains(marker),
            "Cube Solver build aborted: web/index.html is missing the `{marker}` marker — \
             it looks stale or incomplete. Regenerate it with `python3 tools/gen-index.py`."
        );
    }
    assert!(
        html.len() > 50_000,
        "Cube Solver build aborted: web/index.html is only {} bytes — almost certainly stale. \
         Regenerate it with `python3 tools/gen-index.py`.",
        html.len()
    );

    let worker = web.join("solver-worker.js");
    let worker_src = std::fs::read_to_string(&worker).unwrap_or_else(|e| {
        panic!("Cube Solver build aborted: cannot read web/solver-worker.js ({e}).")
    });
    assert!(
        worker_src.contains("postMessage") && worker_src.contains("CubeLab"),
        "Cube Solver build aborted: web/solver-worker.js looks stale/incomplete."
    );

    // Re-run this check (and re-embed) whenever the frontend changes.
    println!("cargo:rerun-if-changed=../web/index.html");
    println!("cargo:rerun-if-changed=../web/solver-worker.js");
}

fn main() {
    assert_frontend_is_fresh();
    tauri_build::build()
}
