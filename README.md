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

`cargo tauri build` can produce native bundles per OS — `.app`/`.dmg` (macOS),
`.msi`/`.exe` (Windows), and `.deb`/`.AppImage` (Linux). The three-OS CI matrix
builds `.app`/`.dmg`, `.msi`/NSIS `.exe`, and `.deb`; it intentionally skips the
flakier Linux AppImage bundler. Requires a stable Rust toolchain plus the
[Tauri prerequisites](https://tauri.app/start/prerequisites/)
(on Linux, the WebKitGTK dev packages listed in `desktop.yml`).

## How Cube Solver works

Cube Solver is the polished web UI **driven by the real Rust solver**, packaged as a
native desktop app. Sizes 2×2–11×11 use compiled Rust/WASM solvers; larger sizes are
explicitly labeled visualization-only rather than presented as searched solutions:

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
2. **Solve** — the cube's complete **sticker state** (never the scramble moves) is
   handed to the solver, which returns a replay-verified solution; the cube then
   animates it.
   - It runs in a **Web Worker** (off the main thread), so the UI stays responsive
     and a long/hard solve can be **cancelled**. Reduction also has cooperative
     internal deadlines; worker termination remains the hard-stop backstop.
   - Automatic scrambles use the platform cryptographic RNG where available and
     avoid adjacent same-axis turns. The worker reconstructs the cube from only the
     `6·N²` visible color indices, so it cannot replay a hidden inverse sequence.
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

**What actually solves:** **2×2 and 3×3 are production solver paths** — the 3×3 by
the two-phase solver (any legal scramble), the 2×2 by the engine race. **4×4–11×11
use the real reduction implementation** (centers → wing pairing → reduced 3×3 +
parity), and every returned path is replayed before success is reported; this range
remains experimental while slow reliability gates are moved into release CI. **12×12
and above are currently visualization-only** in the app and are labeled as such.
Research toward resource-bounded arbitrary N is tracked in
[`docs/ARBITRARY_N_RESEARCH.md`](docs/ARBITRARY_N_RESEARCH.md).

### Swarm

A wall of independent **evolutionary trials**, each a candidate solution to *your
Studio cube* — it re-syncs the moment you re-scramble. Trials start as exact copies
of the cube and mutate/recombine (a (1+λ) elitist search with stagnation restarts)
until they reach solved (the card turns green, then restarts). The **"# off"** on a
card is how many stickers are still out of place.

### Robustness notes

- `web/solver-worker.js` loads WASM and solves off-thread. A bounded main-thread
  fallback exists only for 2×2/3×3; reduction never runs on the UI thread.
- three.js (r128) is vendored in `web/vendor/`, so the app works offline.
- The Tauri shell builds the WASM via `beforeBuildCommand`, so a fresh
  `cargo tauri build` is self-contained.
- A WebGL failure (e.g. headless Linux WebKitGTK) degrades gracefully — the solver
  UI stays usable with a "3D unavailable" notice instead of a blank window.

## Workspace layout

| Crate | Responsibility |
|-------|----------------|
| `cube_core` | Cube model (`StickerCube`), moves (incl. wide/inner-slice), scramble generation. O(N) in-place layer rotation so huge cubes stay fast. |
| `cube_solver` | The real 3×3 two-phase solver (`kociemba`); exact, beam, and island-evolution workers; and the feature-gated experimental N×N reduction engine. |
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

The cube model touches only the affected bands: a strict inner-slice turn is O(N)
and an outer-face turn is O(N²). A repeatable release benchmark is provided at
`crates/cube_core/examples/turn_scaling.rs`. The research-backed path to solving
resource-bounded arbitrary N is deterministic **reduction**; evolutionary/RL
methods alone do not provide completeness across dimensions. The reduction pipeline
now includes dynamic visible-form parity normalization and orbit-local correction;
full legal-move replay evidence extends through research-only N=44. The shipped UI
remains capped at N=11 because frontier reliability and finite-resource costs—not a
hidden history shortcut—still limit honest product claims. See
[`docs/ARBITRARY_N_RESEARCH.md`](docs/ARBITRARY_N_RESEARCH.md) for evidence, failed
experiments, required gates, and primary references.

## Development

```sh
cargo test --workspace                                   # all tests
cargo clippy --workspace --all-targets -- -D warnings    # lint gate
cargo fmt --all -- --check                               # format gate
python3 tools/gen-index.py                                # regenerate web/index.html
python3 tools/frontend-smoke.py                           # HTML/ARIA/JS/worker contract smoke
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

MIT — see [`LICENSE`](LICENSE).
