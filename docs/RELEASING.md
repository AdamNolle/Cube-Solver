# Release guide

Cube Solver uses a two-stage release policy:

1. **Build an unsigned draft** on GitHub-hosted Linux, macOS, and Windows runners.
2. **Publish only after trusted platform signing, hosted validation, and manual installation checks.**

This distinction matters. A checksum proves that a download was not corrupted after the workflow produced it. It does not prove a trusted publisher identity to Gatekeeper or SmartScreen.

## Version contract

The same semantic version must appear in:

- `Cargo.toml` under `[workspace.package]`,
- `src-tauri/Cargo.toml`,
- `src-tauri/tauri.conf.json`.

The workflow tag is `v` plus that version, for example `v0.1.0`.

## Create the draft

1. Ensure `main` is clean, pushed, and green in both **CI** and **Desktop app** workflows.
2. Open **Actions → Draft release → Run workflow**.
3. Enter the matching tag, such as `v0.1.0`.
4. Leave `replace_existing_draft` disabled for the first run.
5. Wait for all matrix and assembly jobs to pass.
6. Inspect the resulting draft release; do not publish it.

The workflow is manual-only and always creates or updates a **draft**. It refuses to modify a published release. Replacing an existing draft requires an explicit input and is allowed only for a draft.

## Artifact contract

A complete draft contains exactly these seven packages:

```text
Cube-Solver-vX.Y.Z-linux-x86_64.deb
Cube-Solver-vX.Y.Z-linux-x86_64.rpm
Cube-Solver-vX.Y.Z-linux-x86_64.AppImage
Cube-Solver-vX.Y.Z-macos-universal.app.zip
Cube-Solver-vX.Y.Z-macos-universal.dmg
Cube-Solver-vX.Y.Z-windows-x86_64.msi
Cube-Solver-vX.Y.Z-windows-x86_64-setup.exe
```

It also includes:

- `SHA256SUMS`,
- `release-manifest.json`,
- GitHub build-provenance attestations.

The assembly job rejects missing, duplicate, empty, symlinked, or unexpected package files.

## Automated validation

### Every platform

- generated frontend is current,
- frontend runtime contracts pass,
- JavaScript parses,
- native N=11 sticker-state solve independently replays,
- packaged/generated WASM executes a real reduction solve and independently replays,
- package version matches the requested tag,
- the packaged process remains alive during a startup smoke.

### Linux

- `.deb` version and `amd64` architecture,
- `.rpm` version and `x86_64` architecture,
- AppImage extraction and executable `AppRun`,
- AppImage startup under Xvfb.

### macOS

- universal binary contains both `arm64` and `x86_64`,
- bundle identifier and version,
- DMG integrity through `hdiutil verify`,
- application startup.

### Windows

- unsigned draft explicitly reports `NotSigned`,
- MSI administrative extraction contains `cube-solver.exe`,
- NSIS installer can be inspected by 7-Zip,
- application startup.

## Signing requirements

### macOS

Before publication, provide protected release-environment secrets for:

- a Developer ID Application certificate and password,
- Apple team identity,
- notarization credentials (App Store Connect API key or supported Apple ID app-password flow).

The signed release must fail closed unless all of these pass:

```sh
codesign --verify --deep --strict "Cube Solver.app"
spctl --assess --type execute --verbose "Cube Solver.app"
xcrun stapler validate "Cube Solver.app"
```

The DMG must also be signed/notarized according to the selected distribution process.

### Windows

Provide a protected Authenticode certificate (for example a PFX plus password, or a managed signing service). Both the MSI and NSIS installer must report:

```powershell
(Get-AuthenticodeSignature $path).Status -eq 'Valid'
```

Use SHA-256 and a trusted timestamp server. Never silently fall back to unsigned assets in a workflow claiming to produce a signed release.

> `TAURI_SIGNING_PRIVATE_KEY` signs Tauri updater metadata. It is not Apple code signing or Windows Authenticode signing.

## Replace the unsigned draft

After signing credentials are available:

1. Add fail-closed macOS notarization and Windows Authenticode steps to the release workflow, guarded by a protected release environment.
2. Require the workflow to reject missing or invalid signatures rather than falling back to unsigned output.
3. Run the updated draft workflow with `replace_existing_draft=true`.
4. Confirm every replacement package is signed/notarized and the manifest says so.
5. Download every artifact and verify:

   ```sh
   sha256sum --check SHA256SUMS
   ```

6. Install and manually test on physical machines.
7. Publish the draft from GitHub only after final approval.

## Manual acceptance checklist

On each operating system:

- launch without bypassing trust warnings,
- verify the Studio layout and 3D cube,
- apply a notation scramble,
- add and undo an interactive turn,
- solve 3×3,
- solve at least one 4×4+ cube,
- cancel an active solve or replay,
- verify Replay,
- verify evolutionary Swarm on 3×3,
- verify reduction telemetry on a large supported cube,
- confirm unsupported sizes are not offered.

## Recovery rules

- Never replace assets on a published release.
- Never reuse a version for a different source commit.
- If a public artifact is wrong, issue a new patch version.
- A failed package format fails the whole draft; do not silently omit it.
- Keep signing secrets in protected GitHub environments, never in pull-request workflows or repository files.
