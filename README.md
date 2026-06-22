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

## Workspace layout

| Crate | Responsibility |
|-------|----------------|
| `cube_core` | Cube model (`StickerCube`), moves (incl. wide/inner-slice), scramble generation. O(N) in-place layer rotation so huge cubes stay fast. |
| `cube_solver` | Solver workers (`DeterministicSolver`, `BeamSearchWorker`, `EvolutionaryWorker`) behind a `SolverWorker` trait, run concurrently. Plus a WIP `reduction` module for arbitrary-N solving. |
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
```

CI runs format + clippy + tests + release build on Linux, macOS, and Windows.

## License

See `Cargo.toml` workspace metadata.
