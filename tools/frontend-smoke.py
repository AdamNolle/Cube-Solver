#!/usr/bin/env python3
"""Dependency-free structural/runtime-contract smoke checks for the generated UI."""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
import tempfile
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
INDEX = ROOT / "web" / "index.html"
WORKER = ROOT / "web" / "solver-worker.js"
LICENSE = ROOT / "LICENSE"
WASM_LICENSE = ROOT / "crates" / "cube_wasm" / "LICENSE"
TAURI_CONFIG = ROOT / "src-tauri" / "tauri.conf.json"


class AuditParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.ids: set[str] = set()
        self.roles: list[tuple[str, dict[str, str]]] = []
        self.external_resources: list[str] = []
        self.tag_counts: dict[str, int] = {}

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        values = {key: value or "" for key, value in attrs}
        self.tag_counts[tag] = self.tag_counts.get(tag, 0) + 1
        if values.get("id"):
            self.ids.add(values["id"])
        if values.get("role"):
            self.roles.append((values["role"], values))
        for key in ("src", "href"):
            ref = values.get(key, "")
            if ref.startswith(("http://", "https://", "//")):
                self.external_resources.append(ref)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def node_check(source: str, suffix: str) -> None:
    node = shutil.which("node")
    require(node is not None, "node is required for frontend syntax smoke checks")
    with tempfile.NamedTemporaryFile("w", suffix=suffix, encoding="utf-8", delete=False) as file:
        path = Path(file.name)
        file.write(source)
    try:
        subprocess.run([node, "--check", str(path)], check=True)
    finally:
        path.unlink(missing_ok=True)


def check_lattice_rotation(module: str) -> None:
    """Execute the authored index rotator across odd and even cube lattices."""
    match = re.search(
        r"rotatedIdx\(idx, m\) \{(?P<body>.*?)\n  \}\n\n  positionForIdx",
        module,
        re.DOTALL,
    )
    require(match is not None, "exact cubie lattice rotator missing")
    script = f"""
const rotate = function(idx, m) {{{match.group('body')}}};
for (let n = 2; n <= 11; n++) {{
  const ctx = {{ N: n }};
  const points = [];
  for (let x=0; x<n; x++) for (let y=0; y<n; y++) for (let z=0; z<n; z++)
    if (x===0 || y===0 || z===0 || x===n-1 || y===n-1 || z===n-1) points.push([x,y,z]);
  const matrixRotate = (point, axis, dir) => {{
    const half = (n - 1) / 2;
    const [x,y,z] = point.map(v => v - half);
    let q;
    if (axis === 0) q = dir > 0 ? [x,-z,y] : [x,z,-y];
    else if (axis === 1) q = dir > 0 ? [z,y,-x] : [-z,y,x];
    else q = dir > 0 ? [-y,x,z] : [y,-x,z];
    return q.map(v => Math.round(v + half));
  }};
  for (let axis=0; axis<3; axis++) for (const dir of [-1, 1]) {{
    const mapped = points.map(p => rotate.call(ctx, p, {{axis, dir}}));
    const keys = new Set(mapped.map(p => p.join(',')));
    if (keys.size !== points.length || mapped.some(p => p.some(v => v < 0 || v >= n)))
      throw new Error(`invalid ${{n}}x${{n}} axis=${{axis}} dir=${{dir}} lattice map`);
    for (const point of points) {{
      const expected = matrixRotate(point, axis, dir);
      const actual = rotate.call(ctx, point, {{axis, dir}});
      if (actual.some((v, i) => v !== expected[i])) throw new Error('index turn disagrees with right-hand quaternion rotation');
      let p = point;
      for (let turn=0; turn<4; turn++) p = rotate.call(ctx, p, {{axis, dir}});
      if (p.some((v, i) => v !== point[i])) throw new Error('quarter turn is not order four');
      p = rotate.call(ctx, rotate.call(ctx, point, {{axis, dir}}), {{axis, dir:-dir}});
      if (p.some((v, i) => v !== point[i])) throw new Error('inverse turn did not restore point');
    }}
  }}
}}
"""
    subprocess.run([shutil.which("node") or "node", "-e", script], check=True)


def check_swarm_scheduler(module: str) -> None:
    """Exercise the generated early-mount/idle Swarm scheduling state machine."""
    match = re.search(
        r"function scheduleSwarmReduction\(self\)\{(?P<body>.*?)\n    \}\n    if \(origSetView\)",
        module,
        re.DOTALL,
    )
    require(match is not None, "Swarm reduction scheduler missing")
    script = f"""
const SWARM_MAX=3, SOLVE_MAX_N=11;
let timers=[];
globalThis.setTimeout=(fn) => {{ timers.push(fn); return timers.length; }};
globalThis.clearTimeout=() => {{}};
const schedule=function(self) {{{match.group('body')}}};
function runOne() {{ const fn=timers.shift(); if (!fn) throw new Error('expected timer'); fn(); }}
let calls={{solve:0,scramble:0}};
let state={{view:'swarm',n:6,busy:true,phase:'scrambling',scrambled:true,_solvePending:false,solve(){{calls.solve++;}},scramble(){{calls.scramble++;this.scrambled=true;}}}};
schedule(state); runOne();
if (calls.solve !== 0 || timers.length !== 1) throw new Error('busy pre-WASM scramble was not deferred');
state.busy=false; runOne();
if (calls.solve !== 1) throw new Error('idle reduction did not auto-start');
timers=[]; calls={{solve:0,scramble:0}};
state={{view:'swarm',n:4,busy:false,scrambled:false,_solvePending:false,solve(){{calls.solve++;}},scramble(){{calls.scramble++;this.scrambled=true;}}}};
schedule(state); runOne();
if (calls.scramble !== 1 || calls.solve !== 1) throw new Error('unscrambled Swarm did not scramble then solve');
timers=[]; calls={{solve:0,scramble:0}};
state={{view:'studio',n:6,busy:false,scrambled:true,_solvePending:false,solve(){{calls.solve++;}},scramble(){{calls.scramble++;}}}};
schedule(state);
if (timers.length || calls.solve) throw new Error('Studio view scheduled a Swarm solve');
state.view='swarm'; state._solvePending=true; schedule(state);
if (timers.length || calls.solve) throw new Error('pending solve was duplicated');
state._solvePending=false; state.busy=true; state.phase='solving'; schedule(state);
if (timers.length || calls.solve) throw new Error('verified replay scheduled a duplicate solve');
"""
    subprocess.run([shutil.which("node") or "node", "-e", script], check=True)


def check_boot_recovery(module: str) -> None:
    """Execute pre-WASM state sync and no-WebGL/RAF recovery helpers."""
    sync_match = re.search(
        r"function syncLabFromVisibleScramble\(self\)\{(?P<body>.*?)\n    \}\n    syncLabFromVisibleScramble\(inst\)",
        module,
        re.DOTALL,
    )
    build_match = re.search(
        r"function recoverNoWebGLBuild\(self, n\)\{(?P<body>.*?)\n    \}\n    function useTextureBuildWithoutWebGL",
        module,
        re.DOTALL,
    )
    texture_match = re.search(
        r"function useTextureBuildWithoutWebGL\(self, n\)\{(?P<body>.*?)\}",
        module,
        re.DOTALL,
    )
    loop_match = re.search(
        r"function ensureAnimationLoop\(self\)\{(?P<body>.*?)\n    \}\n    ensureAnimationLoop\(inst\)",
        module,
        re.DOTALL,
    )
    require(sync_match is not None, "pre-WASM scramble synchronizer missing")
    require(build_match is not None, "no-WebGL build recovery missing")
    require(texture_match is not None, "no-WebGL texture initialization route missing")
    require(loop_match is not None, "single-RAF recovery helper missing")
    require(
        module.index("syncLabFromVisibleScramble(inst)") < module.index("scheduleSwarmReduction(inst)"),
        "pre-WASM scramble must synchronize before Swarm auto-start",
    )
    script = f"""
const sync=function(self) {{{sync_match.group('body')}}};
let applied=[];
const lab={{set_size(n){{this.n=n;}},reset(){{this.resets=(this.resets||0)+1;applied=[];}},apply_design_move(a,l,d){{applied.push([a,l,d]);}}}};
let state={{lab,n:6,scrambled:true,lastScramble:[{{axis:0,layer:5,dir:1}},{{axis:2,layer:0,dir:-1}}]}};
sync(state);
if (lab.n!==6 || lab.resets!==1 || JSON.stringify(applied)!=='[[0,5,1],[2,0,-1]]') throw new Error('pre-WASM scramble was not mirrored exactly');
state.scrambled=false; sync(state);
if (lab.resets!==2 || applied.length) throw new Error('solved visible state did not reset the lab');
const recover=function(self,n) {{{build_match.group('body')}}};
let badges=0, view={{CUBIE_MAX:11,cubies:[1],activeMove:{{}},updateBadges(){{badges++;}}}};
recover(view,6);
if (view.N!==6 || view.mode!=='cubie' || view.cubies.length || view.activeMove!==null || badges!==1) throw new Error('cubie no-WebGL recovery failed');
const useTexture=function(self,n) {{{texture_match.group('body')}}};
if (useTexture(view,6) || !useTexture(view,20)) throw new Error('no-WebGL texture path was not preserved for large cubes');
let loops=0;
globalThis.window={{THREE:{{Clock:class Clock{{}}}}}};
const ensure=function(self) {{{loop_match.group('body')}}};
let runtime={{_raf:null,clock:null,loop(){{loops++;this._raf=7;}}}};
ensure(runtime); ensure(runtime);
if (loops!==1 || !runtime.clock) throw new Error('RAF recovery duplicated or omitted the loop');
"""
    subprocess.run([shutil.which("node") or "node", "-e", script], check=True)


def main() -> int:
    html = INDEX.read_text(encoding="utf-8")
    worker = WORKER.read_text(encoding="utf-8")
    parser = AuditParser()
    parser.feed(html)

    require(LICENSE.read_bytes() == WASM_LICENSE.read_bytes(), "WASM package license must match root MIT license")
    security = json.loads(TAURI_CONFIG.read_text(encoding="utf-8"))["app"]["security"]
    require("'unsafe-inline'" in security["csp"].split("style-src", 1)[1].split(";", 1)[0], "inline-heavy authored UI requires style-src unsafe-inline")
    require("style-src" in security.get("dangerousDisableAssetCspModification", []), "Tauri nonce injection must stay disabled for style-src or packaged inline styles are blocked")

    require(parser.tag_counts.get("main") == 1, "generated page must have one <main>")
    require(parser.tag_counts.get("header") == 1, "generated page must have one <header>")
    require(not parser.external_resources, f"desktop UI must be offline-only: {parser.external_resources}")
    require({"studio-tab", "swarm-tab", "studio-panel", "swarm-panel"} <= parser.ids, "tab IDs/panels missing")

    tabs = [attrs for role, attrs in parser.roles if role == "tab"]
    require(len(tabs) == 2, "Studio and Swarm must expose exactly two ARIA tabs")
    for tab in tabs:
        require(tab.get("aria-controls") in parser.ids, "tab aria-controls must target a real panel")
    require(any(role == "status" and attrs.get("aria-live") == "polite" for role, attrs in parser.roles), "live status missing")
    require(any(role == "progressbar" for role, _ in parser.roles), "solve progressbar semantics missing")

    module_match = re.search(r'<script type="module">(?P<source>.*?)</script>', html, re.DOTALL)
    require(module_match is not None, "generated module script missing")
    module = module_match.group("source")
    for marker in (
        "colors:colors",
        "reduction:'det'",
        "_solveDispatch",
        "jobId:self._solveJobId",
        "self._worker !== w",
        "core.invoke('solve_stickers'",
        "invoke('cancel_solve'",
        "requestToken:requestToken",
        "Number(v) > SOLVE_MAX_N",
        "data-reduction-elapsed aria-hidden",
        "data-reduction-start",
        "data-reduction-cancel",
        "data-replay-moves",
        "Cancel playback",
        "display:none;width:100%;height:42px",
        "Native reduction is active",
        "Progress percentages are intentionally omitted",
        "nativeCore() ? 11 : 5",
        "Math.floor(N/2)",
        "rotatedIdx(idx, m)",
        "positionForIdx(idx)",
        "this.setSolvedPct(0)",
        "if (inst.initSwarm) inst.initSwarm()",
        "syncLabFromVisibleScramble(inst)",
        "recoverNoWebGLBuild",
        "useTextureBuildWithoutWebGL",
        "origMountBuildCube",
        "ensureAnimationLoop(inst)",
        "scheduleSwarmReduction(inst)",
        "_swarmAutoStartTimer",
        "e.ctrlKey || e.metaKey || e.altKey",
        "role=\"tab\"",
    ):
        # role="tab" lives in HTML rather than JavaScript.
        haystack = html if marker.startswith("role=") else module
        require(marker in haystack, f"frontend contract marker missing: {marker}")

    require("Math.round(np.x / this.step)" not in module, "even-N cubies must never snap to the integer-origin lattice")
    check_lattice_rotation(module)
    check_swarm_scheduler(module)
    check_boot_recovery(module)

    require("d.moves" not in worker, "solver worker must not receive scramble moves")
    require("lab.load_face_colors(d.colors)" in worker, "solver worker sticker-only boundary missing")
    require("jobId: d.jobId" in worker, "solver worker stale-job correlation missing")
    node_check(module, ".mjs")
    node_check(worker, ".mjs")

    print("frontend smoke passed: structure, accessibility, exact cubie lattice, Swarm startup, JS syntax, worker/native privacy")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as error:
        print(f"frontend smoke failed: {error}", file=sys.stderr)
        raise SystemExit(1)
