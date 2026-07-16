# Cube Solver v0.1.0

> **Unsigned draft — not yet a public release.**
>
> These packages are built and structurally validated by GitHub-hosted Linux, macOS, and Windows runners. Apple notarization and Windows Authenticode publisher signing are not yet configured. Gatekeeper and SmartScreen warnings are therefore expected. Keep this release in draft until signed replacement assets pass the same workflow.

## Highlights

- Real, replay-verified solving on **2×2–11×11** in the native desktop app.
- Two-phase 3×3 solving and deterministic large-cube reduction.
- A truthful dual-mode **Swarm**:
  - live evolutionary trials on 2×2/3×3,
  - real deterministic-reduction telemetry on 4×4–11×11.
- Custom scrambles through standard move notation and interactive face/wide-turn controls.
- Solver requests contain sticker colors rather than scramble history.
- Cooperative native cancellation and stale-job protection.
- Offline assets, accessibility semantics, responsive layout, and packaged-style CSP guards.

## Draft artifacts

| Platform | Packages |
|---|---|
| Linux x86_64 | `.deb`, `.rpm`, `.AppImage` |
| macOS universal | `.app.zip`, `.dmg` |
| Windows x86_64 | `.msi`, NSIS setup `.exe` |

`SHA256SUMS` covers every installer. `release-manifest.json` binds filenames and hashes to the exact commit and workflow run. GitHub build-provenance attestations are generated for the packages.

## Automated release gates

- generated frontend and accessibility/runtime-contract smoke,
- final generated-WASM execution and independent replay,
- native N=11 sticker-state solve and replay on every operating system,
- package metadata and architecture checks,
- Linux package inspection, AppImage extraction, and Xvfb startup smoke,
- universal macOS architecture, bundle metadata, DMG verification, and startup smoke,
- Windows MSI extraction, NSIS inspection, signature-state check, and startup smoke,
- exact seven-file asset allowlist and verified SHA-256 manifest.

## Before publishing

- [ ] Configure Developer ID signing and Apple notarization; verify/staple the app and DMG.
- [ ] Configure Windows Authenticode signing; require `Valid` signatures on MSI and NSIS packages.
- [ ] Add fail-closed signing/notarization steps to the workflow, then replace this draft’s unsigned assets.
- [ ] Install and manually exercise Studio, custom scramble, Solve, Cancel, Replay, and Swarm on physical Linux, macOS, and Windows machines.
- [ ] Re-check `SHA256SUMS`, provenance, release notes, and download names.
- [ ] Publish only after all checks above are complete.
