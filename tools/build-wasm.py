#!/usr/bin/env python3
"""Build the browser WASM package into a fresh, policy-checked directory."""

from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "web" / "pkg"


def main() -> int:
    shutil.rmtree(OUT, ignore_errors=True)
    OUT.mkdir(parents=True)
    (OUT / ".gitignore").write_text("*\n!.gitignore\n", encoding="utf-8")

    wasm_pack = shutil.which("wasm-pack")
    if wasm_pack is None:
        raise SystemExit("wasm-pack is required to build web/pkg")
    subprocess.run(
        [
            wasm_pack,
            "build",
            "crates/cube_wasm",
            "--release",
            "--target",
            "web",
            "--out-dir",
            "../../web/pkg",
            "--out-name",
            "cube_wasm",
        ],
        cwd=ROOT,
        check=True,
    )

    package = json.loads((OUT / "package.json").read_text(encoding="utf-8"))
    if "license" in package:
        raise SystemExit("generated WASM package unexpectedly declares a project license")
    stale = sorted(path.name for path in OUT.glob("LICENSE*"))
    if stale:
        raise SystemExit(f"generated WASM package contains stale license files: {stale}")
    print(f"built clean WASM package: {OUT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
