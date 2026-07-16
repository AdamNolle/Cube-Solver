# Cube Solver v0.1.0

> **Unsigned public release.**
>
> These packages are built and structurally validated by GitHub-hosted Linux, macOS, and Windows runners. Apple notarization and Windows Authenticode publisher signing are not configured. Gatekeeper and SmartScreen warnings are therefore expected. This release was published unsigned by explicit owner decision; checksums and provenance attest build integrity, not publisher identity.

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

| Platform | Package |
|---|---|
| Linux x86_64 | `.flatpak` bundle (GNOME 50 runtime) |
| macOS universal | `.dmg` |
| Windows x86_64 | NSIS setup `.exe` |

`SHA256SUMS` covers every installer. `release-manifest.json` binds filenames and hashes to the exact commit and workflow run. GitHub build-provenance attestations are generated for the packages.

## Automated release gates

- generated frontend and accessibility/runtime-contract smoke,
- final generated-WASM execution and independent replay,
- native N=11 sticker-state solve and replay on every operating system,
- package metadata and architecture checks,
- Linux Flatpak metadata, sandbox permissions, x86_64 payload, library closure, install, and Xvfb startup smoke,
- universal macOS architecture, bundle metadata, DMG verification, unsigned-state check, and startup smoke,
- Windows NSIS inspection, x86_64 payload/version, signature-state check, and startup smoke,
- exact three-package asset allowlist and verified SHA-256 manifest.

## Remaining platform-trust work

- [ ] Configure Developer ID signing and Apple notarization for a future release; verify and staple the app and DMG.
- [ ] Configure Windows Authenticode signing for a future release; require a `Valid` signature on the NSIS setup executable.
- [ ] Install and manually exercise Studio, custom scramble, Solve, Cancel, Replay, and Swarm on physical Linux, macOS, and Windows machines.

Because `v0.1.0` is now public, signed replacements must use a new patch version rather than silently replacing these downloads.
