# Cube Solver — web UI (real solver, via WASM)

The **Cube Solver UI** wired to the real Rust solver. The cube model and engines
are compiled to WebAssembly and drive the page. 2×2 uses the solver race, 3×3 uses
the two-phase engine, and 4×4–11×11 use replay-verified reduction. Larger cubes are
explicitly visualization-only.

## Run it

ES modules + WebAssembly must be served over HTTP (they won't load from a
`file://` path):

Build the browser bindings first (they are generated and git-ignored), then serve:

```sh
wasm-pack build crates/cube_wasm --release --target web \
  --out-dir "$PWD/web/pkg" --out-name cube_wasm
python3 -m http.server -d web 8000      # then open http://localhost:8000
```

## How the real solver is wired

- `crates/cube_wasm` is a `wasm-bindgen` bridge exposing a `CubeLab` object.
  The worker loads a complete sticker-color buffer and returns a replay-verified
  solution as `{axis, layer, dir}` quarter-turns plus lane metadata.
- The web UI's cube and `cube_core` share the **same geometry convention**
  (Up=+Y, Down=−Y, Front=+Z, Back=−Z, Left=−X, Right=+X; a `dir=+1` turn is a
  right-hand quarter turn). So a scramble is mirrored move-for-move into the
  solver's cube, the solver runs, and the returned moves animate on screen and
  truly solve it.
- The solver builds for wasm by disabling `cube_solver`'s default `parallel`
  feature (no rayon / OS threads — the three engines run sequentially in the
  browser) and using `web-time` for a wasm-safe clock.

Automatic scrambles use `crypto.getRandomValues` when available, avoid adjacent
same-axis turns, and use outer layers on 2×2/3×3 or all layers on the reduction
range. The solver worker receives only the visible sticker colors—not the move
history—so it cannot simply invert the generated sequence. Cubes past 11×11 remain
an honestly labeled sampled visualization while arbitrary-N reduction research
continues.

## Rebuilding the WASM

`web/pkg/` is the generated WASM bundle (the `.wasm` + JS bindings). Rebuild it
after changing any Rust in `cube_wasm`/`cube_solver`/`cube_core`:

```sh
wasm-pack build crates/cube_wasm --release --target web \
  --out-dir "$PWD/web/pkg" --out-name cube_wasm
```

## How the page was built

The original claude.ai design component is preserved in `tools/design-source.txt`
and transformed into this standalone page. The generator adapts its markup and
component logic, replaces the React `dc-runtime` with a tiny vanilla shim, and adds
a wiring layer that drives the WASM bridge. three.js (r128) is vendored locally in
`web/vendor/`; typography uses local system font stacks so the desktop UI remains
offline-only.
