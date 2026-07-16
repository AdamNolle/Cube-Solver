# Cube Solver web interface

The polished Studio/Swarm UI, generated from `tools/design-source.txt` and wired to the real Rust solver by `tools/gen-index.py`.

## Supported sizes

- **Standalone browser:** 2×2–5×5
- **Tauri desktop:** 2×2–11×11

Unsupported sizes are not exposed. The ceiling is deliberate: every offered size must have a measured solver path, independent legal-move replay, responsive cancellation, and acceptable resource use.

## Run in a browser

Build the generated WASM package:

```sh
wasm-pack build crates/cube_wasm --release --target web \
  --out-dir "$PWD/web/pkg" --out-name cube_wasm
```

Serve the directory over HTTP:

```sh
python3 -m http.server -d web 8000
```

Open <http://localhost:8000>. ES modules and WebAssembly do not work through a direct `file://` URL.

## Solver routing

- 2×2/3×3 and standalone-browser 4×4/5×5 run in `solver-worker.js`.
- Tauri desktop 4×4–11×11 uses native `solve_stickers` / `cancel_solve` commands.
- Studio solve requests contain the complete sticker-color state—not scramble history.
- Returned moves are accepted only after independent replay reaches solved.

Custom scrambles can be entered with `U D L R F B` notation, prime/half-turn suffixes, and supported wide turns such as `Rw` or `3Rw2`. The interactive controls use the same legal move representation.

## Generated source

Do not edit `web/index.html` directly.

1. Edit `tools/design-source.txt` for authored UI changes.
2. Edit `tools/gen-index.py` for runtime/solver wiring.
3. Regenerate and verify:

```sh
python3 tools/gen-index.py
python3 tools/frontend-smoke.py
```

three.js is vendored under `web/vendor/`; the packaged interface has no runtime CDN dependency.
