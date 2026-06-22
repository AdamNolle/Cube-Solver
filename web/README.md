# Cube Solver — web UI (real solver, via WASM)

The **Cube solver UI redesign** wired to the real Rust solver. The cube model
(`cube_core`) and the solver engines (`cube_solver`) are compiled to WebAssembly
(`crates/cube_wasm`) and drive the page — scrambling and **solving are genuinely
computed by the Rust meet-in-the-middle / beam / island-GA engines**, not faked.

## Run it

ES modules + WebAssembly must be served over HTTP (they won't load from a
`file://` path):

```sh
python3 -m http.server -d web 8000      # then open http://localhost:8000
```

## How the real solver is wired

- `crates/cube_wasm` is a `wasm-bindgen` bridge exposing a `CubeLab` object:
  `apply_design_move(axis, layer, dir)`, `solve(maxDepth, timeMs)` (returns the
  winning, replay-verified solution as `{axis, layer, dir}` quarter-turns plus the
  per-lane race results), `face_colors`, `is_solved`, etc.
- The web UI's cube and `cube_core` share the **same geometry convention**
  (Up=+Y, Down=−Y, Front=+Z, Back=−Z, Left=−X, Right=+X; a `dir=+1` turn is a
  right-hand quarter turn). So a scramble is mirrored move-for-move into the
  solver's cube, the solver runs, and the returned moves animate on screen and
  truly solve it.
- The solver builds for wasm by disabling `cube_solver`'s default `parallel`
  feature (no rayon / OS threads — the three engines run sequentially in the
  browser) and using `web-time` for a wasm-safe clock.

The scramble uses **outer-face turns** so the meet-in-the-middle solver (which
searches the outer move set) can always invert it; the default depth (6) is one
the exact solver cracks quickly. Push it higher and the cube genuinely gets
harder — if no verified solution is found within the budget, the UI says so.
Cubes past 11³ render in the design's sampled "texture" mode and keep the
original simulated animation (the search solvers don't scale to giant cubes).

## Rebuilding the WASM

`web/pkg/` is the generated WASM bundle (the `.wasm` + JS bindings). Rebuild it
after changing any Rust in `cube_wasm`/`cube_solver`/`cube_core`:

```sh
wasm-pack build crates/cube_wasm --release --target web \
  --out-dir "$PWD/web/pkg" --out-name cube_wasm
```

## How the page was built

The original claude.ai design component is transformed into this standalone page:
its markup, styles, and component logic are kept verbatim, the React `dc-runtime`
is replaced with a tiny vanilla shim, and a small wiring layer overrides
`scramble`/`solve` to drive the WASM bridge. three.js (r128) is vendored locally
in `web/vendor/`; the fonts load from Google Fonts.
