# Cube Solver — web UI (WASM + native desktop reduction)

The **Cube Solver UI** wired to the real Rust solver. 2×2/3×3 run in a WebAssembly
worker. In the Tauri desktop app, 4×4–11×11 use a cancellable native Rust command;
the standalone browser build exposes only its runtime-smoked WASM reduction range
through 5×5. Larger cubes are explicitly visualization-only.

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

- `crates/cube_wasm` is both a `wasm-bindgen` bridge exposing `CubeLab` and the
  shared sticker-only entry point used by Tauri's native `solve_stickers` command.
  Both load a complete color buffer and return replay-verified `{axis, layer, dir}`
  quarter-turns plus lane metadata.
- The web UI's cube and `cube_core` share the **same geometry convention**
  (Up=+Y, Down=−Y, Front=+Z, Back=−Z, Left=−X, Right=+X; a `dir=+1` turn is a
  right-hand quarter turn). The solver reconstructs the cube from visible stickers,
  then the returned moves animate on screen and truly solve it.
- The solver builds for wasm by disabling `cube_solver`'s default `parallel`
  feature (no rayon / OS threads — the three engines run sequentially in the
  browser) and using `web-time` for a wasm-safe clock.

Automatic scrambles use `crypto.getRandomValues` when available, avoid adjacent
same-axis turns, and use outer turns on 2×2/3×3 or standard contiguous wide turns
on the desktop reduction range. Every solver boundary receives only visible sticker
colors—not move history—so it cannot simply invert the generated sequence. Cubes
past the active platform's measured limit remain an honestly labeled visualization.

## Rebuilding the WASM

`web/pkg/` is the generated WASM bundle (the `.wasm` + JS bindings). Rebuild it
after changing any Rust in `cube_wasm`/`cube_solver`/`cube_core`:

```sh
wasm-pack build crates/cube_wasm --release --target web \
  --out-dir "$PWD/web/pkg" --out-name cube_wasm
node tools/wasm-runtime-smoke.mjs
```

## How the page was built

The original claude.ai design component is preserved in `tools/design-source.txt`
and transformed into this standalone page. The generator adapts its markup and
component logic, replaces the React `dc-runtime` with a tiny vanilla shim, and adds
a wiring layer that drives the WASM bridge. three.js (r128) is vendored locally in
`web/vendor/`; typography uses local system font stacks so the desktop UI remains
offline-only.
