#!/usr/bin/env python3
"""Dependency-free structural/runtime-contract smoke checks for the generated UI."""

from __future__ import annotations

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


def main() -> int:
    html = INDEX.read_text(encoding="utf-8")
    worker = WORKER.read_text(encoding="utf-8")
    parser = AuditParser()
    parser.feed(html)

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
        "e.ctrlKey || e.metaKey || e.altKey",
        "role=\"tab\"",
    ):
        # role="tab" lives in HTML rather than JavaScript.
        haystack = html if marker.startswith("role=") else module
        require(marker in haystack, f"frontend contract marker missing: {marker}")

    require("d.moves" not in worker, "solver worker must not receive scramble moves")
    require("lab.load_face_colors(d.colors)" in worker, "solver worker sticker-only boundary missing")
    require("jobId: d.jobId" in worker, "solver worker stale-job correlation missing")
    node_check(module, ".mjs")
    node_check(worker, ".mjs")

    print("frontend smoke passed: structure, accessibility, offline assets, JS syntax, worker privacy")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as error:
        print(f"frontend smoke failed: {error}", file=sys.stderr)
        raise SystemExit(1)
