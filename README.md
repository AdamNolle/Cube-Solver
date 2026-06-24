# Rust N×N Cube Solver Lab

An interactive desktop app that generates scrambled N×N Rubik's cubes and races
several solver strategies — a deterministic meet-in-the-middle search, a beam
search, and a parallel **island-model genetic algorithm** — to find and replay a
verified solution. It renders an interactive 3D cube, a 2D net, and a "wall of
cubes" grid of many cubes solving at once.

Built as a Rust workspace with a clean separation between the cube model, the
solvers, persistence, and the GUI.

## Two front-ends, one solver core

1. **Cube Solver** — the polished studio UI as a **cross-platform native desktop
   app** (Tauri, `src-tauri/` + `web/`). The interface is the refined web design;
   the cube model and solvers run as WebAssembly compiled from `cube_core` /
   `cube_solver`. *Recommended.*
2. **Solver Lab** — the original `eframe`/`egui` desktop app
   (`crates/solver_lab_app`).

### Run Cube Solver (native desktop app)

```sh
# WASM solver core → generated into web/pkg (rebuild after changing the Rust)
wasm-pack build crates/cube_wasm --release --target web \
  --out-dir "$PWD/web/pkg" --out-name cube_wasm

cd src-tauri
cargo tauri dev      # run the app in a dev window
cargo tauri build    # native installer for the current OS
```

`cargo tauri build` produces a native bundle per OS — `.app`/`.dmg` (macOS),
`.msi`/`.exe` (Windows), `.deb`/`.AppImage` (Linux) — all from this one codebase
(the 3-OS matrix in `.github/workflows/desktop.yml` builds them in CI). Requires
a stable Rust toolchain plus the [Tauri prerequisites](https://tauri.app/start/prerequisites/)
(on Linux, the WebKitGTK dev packages listed in `desktop.yml`).

## How Cube Solver works

Cube Solver is the polished web UI **driven by the real Rust solver**, packaged as a
native desktop app. Nothing in the browser/webview is simulated — every scramble and
solve is computed by the compiled Rust:

```
cube_core + cube_solver  ──wasm-pack──▶  web/pkg/  (WebAssembly)
                                            │  imported by
                                  web/index.html  (three.js 3D UI)
                                            │  embedded by
                                      src-tauri/   (Tauri v2 native shell)
```

### Studio

1. **Scramble** — applies random face turns to the on-screen cube *instantly*. On
   the 2×2/3×3 the turns are outer faces only (so the solver can invert them);
   bigger cubes mix **every layer** for a proper full scramble.
2. **Solve** — the cube's **sticker state** (not the scramble moves) is handed to the
   solver, which returns the fewest-move, replay-verified solution; the cube then
   animates it.
   - It runs in a **Web Worker** (off the main thread), so the UI stays responsive
     and a long/hard solve can be **cancelled** (the worker is terminated).
   - A **"scramble hidden from the solver"** panel proves it isn't cheating: the
     solver only sees the 54 sticker colours, and its solution is usually *shorter*
     than the scramble, so it can't be replaying the inverse.
3. **Solver race / best-path solver** — the panel adapts to the cube:
   - **3×3** — a real **two-phase (Kociemba) solver** (`cube_solver::kociemba`): it
     orients the pieces into the UD-slice subgroup (phase 1) then solves the
     permutation within it (phase 2), using pruning tables built once at startup.
     It cracks **any** 3×3 scramble in about **20 moves** (typically ≤~26, standard
     face-turn metric) — a near-optimal best path, not the scramble inverse — and
     runs off-thread in the worker.
   - **2×2** — three independent engines genuinely race: **meet-in-the-middle**
     (exact, bidirectional BFS), **beam search**, and an **island genetic algorithm**;
     the shortest verified solution wins.

**What actually solves:** the **2×2 and 3×3 are solved for real** — the 3×3 by the
two-phase solver (any scramble), the 2×2 by the engine race. 4×4 and up render and
scramble fully but are **visual** (Solve plays back the inverse); the in-app banner
tells you which mode you're in. Real solving past the 3×3 needs the **reduction
method** for arbitrary N (centers → edge pairing → reduced 3×3 + parity) — that's the
work-in-progress `cube_solver::reduction` module (feature-gated, not yet wired in).

### Swarm

A wall of independent **evolutionary trials**, each a candidate solution to *your
Studio cube* — it re-syncs the moment you re-scramble. Trials start as exact copies
of the cube and mutate/recombine (a (1+λ) elitist search with stagnation restarts)
until they reach solved (the card turns green, then restarts). The **"# off"** on a
card is how many stickers are still out of place.

### Robustness notes

- `web/solver-worker.js` loads the WASM and solves off-thread; the page falls back
  to a bounded (~1.5 s) main-thread solve if Web Workers aren't available.
- three.js (r128) is vendored in `web/vendor/`, so the app works offline.
- The Tauri shell builds the WASM via `beforeBuildCommand`, so a fresh
  `cargo tauri build` is self-contained.
- A WebGL failure (e.g. headless Linux WebKitGTK) degrades gracefully — the solver
  UI stays usable with a "3D unavailable" notice instead of a blank window.

## Workspace layout

| Crate | Responsibility |
|-------|----------------|
| `cube_core` | Cube model (`StickerCube`), moves (incl. wide/inner-slice), scramble generation. O(N) in-place layer rotation so huge cubes stay fast. |
| `cube_solver` | The real 3×3 two-phase solver (`kociemba`); solver workers (`DeterministicSolver`, `BeamSearchWorker`, `EvolutionaryWorker`) behind a `SolverWorker` trait, run concurrently; and a feature-gated WIP `reduction` module for arbitrary-N solving. |
| `solver_store` | SQLite (bundled) persistence of solve history. |
| `solver_lab_app` | `eframe`/`egui` GUI: 3D viewport, 2D net, the wall-of-cubes grid, controls, history. |

## Build & run

```sh
cargo run --release -p solver_lab_app
```

Requires a stable Rust toolchain. On Linux the GUI needs the usual `eframe`
system libraries (`libgtk-3-dev`, `libxcb-*`, `libxkbcommon-dev`); see
`.github/workflows/ci.yml` for the exact list.

## Using the app

- **N / scramble / wide span / seed** — configure the challenge; **New challenge**
  generates it off the UI thread (no freeze even at large N).
- **Solve** — runs all workers in parallel; the fewest-move *verified* path wins.
- **Replay best** — animates the winning solution turn by turn.
- **View tabs** — `3D cube` (drag to orbit), `2D net`, and `Wall` (a grid of
  independent cubes perpetually solving, with level-of-detail + virtualization).
- **Theme / scale** — light/dark toggle and UI scaling.
- **Shortcuts** — `Space` solve · `N` new · `R` replay · `C`/`V`/`G` switch views.

History is stored in an OS-appropriate data directory (Application Support /
`%APPDATA%` / XDG), falling back to in-memory if unavailable.

## Solvers

- **Two-phase (Kociemba)** (`cube_solver::kociemba`) — the real 3×3 engine. A
  cubie-level model with two-phase coordinates (twist/flip/UD-slice, then the
  permutation), BFS pruning tables, and IDA* search. Solves any 3×3 scramble in
  about 20 face turns (typically ≤~26) and is replay-verified against `cube_core`.
- **DeterministicSolver** — bidirectional (meet-in-the-middle) BFS over a
  scramble-aware move set (including wide turns), returning a replay-verified
  shortest path within budget.
- **BeamSearchWorker** — beam search minimizing sticker mismatch.
- **EvolutionaryWorker** — a parallel **island-model GA**: independent islands
  evolved with tournament selection, cut-and-splice crossover, adaptive mutation,
  ring migration, and stagnation restarts. Deterministic per seed.

### Scaling to massive cubes

The cube model's in-place rotation makes a single inner-slice turn ~O(N) instead
of O(N²), so cubes with thousands of layers can be generated, manipulated,
replayed, and visualized quickly. The research-backed path to *solving* arbitrary
N is the **reduction method** (centers → edge pairing → reduced 3×3 + parity),
which is O(N²) in moves and polynomial time (Demaine et al., ESA 2011 — diameter
Θ(N²/log N)). That solver lives in `cube_solver::reduction` and is **work in
progress** (see its module `STATUS` note): odd-cube fixed-center orientation is
implemented and verified; the full centers/edges/3×3+parity pipeline is the next
milestone. RL/ML approaches (DeepCubeA et al.) are size-locked to small fixed
puzzles and do not generalize to arbitrary N.

## Development

```sh
cargo test --workspace                                   # all tests
cargo clippy --workspace --all-targets -- -D warnings    # lint gate
cargo fmt --all -- --check                               # format gate
python3 tools/gen-index.py                                # regenerate web/index.html
```

CI runs format + clippy + tests + release build on Linux, macOS, and Windows.

### The web UI is generated

`web/index.html` is **generated** by `tools/gen-index.py`, which wraps the design
component in `tools/design-source.txt` with the real Rust/WASM solver wiring
(`wireRealSolver`). Edit the generator, not `index.html` directly, then re-run it.
A `build.rs` guard fails the Tauri build loudly if `web/index.html` is missing or
stale, so a broken frontend can never be silently embedded into a bundle.

> ⚠️ **Don't keep this repo in an iCloud/Dropbox-synced folder.** Sync can delete
> or duplicate (`… 2.html`) source files mid-build, producing apps built from a
> stale frontend. Clone it somewhere local (e.g. `~/code/`).

## License

See `Cargo.toml` workspace metadata.
