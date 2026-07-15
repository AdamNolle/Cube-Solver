# Arbitrary-N Solver Research Contract

## Goal

Build a replay-verified solver for every `N >= 2` that fits available memory and time. “Arbitrary N” does **not** mean mathematically unbounded input on finite hardware. Product claims must include measured resource limits and cancellation behavior.

Evolutionary search is a real supplemental technique for candidate exploration and the Swarm visualization. It is not the completeness mechanism for large cubes. General solving uses deterministic reduction: centers, wing orbits, reduced 3×3, and parity correction.

## Current evidence (2026-07-12)

- 2×2: bounded exact/beam/evolutionary race.
- 3×3: checked, replay-verified two-phase solver.
- Desktop 4×4–11×11: release-mode wide-turn replay corpus passes (20 cases each through 6×6, 10 each through 8×8, and 3 each through 11×11) and is wired into release CI. Tauri runs this path on a cancellable native Rust thread; the standalone browser caps reduction at its runtime-smoked 5×5 range.
- 12×12: full controlled replay gate passes; the application still does not route sizes above 11 because interactive deadlines and frontier reliability remain intentionally conservative.
- 20×20: full corpus 3/3 plus sparse, alternating, and all-nine-orbit adversarial parity/replay gates pass with the polynomial visible-form normalizer.
- 24×24: full corpus 3/3; 28×28, 32×32, 36×36, and 40×40 each pass 2/2; 44×44 passes 2/2 after the current additional-seed gate.
- 66×66 and 132×132: canonical sparse, alternating, all-defect, and noncanonical isolated orbit transports replay without fixed-width masks; these are not full random end-to-end solves.

The larger research gates remain intentionally ignored by routine `cargo test` and must be run explicitly in release mode. Advertised release coverage remains bounded to 11×11.

## Removed scaling blocker

A strict inner-slice quarter turn now reserves only `4N` destination writes instead of `4N + 2N²`; cap-face storage is reserved only when the selected range includes an outer layer. The existing differential tests still compare every axis and contiguous range against the reference permutation.

Repeatable benchmark:

```sh
cargo run --release -p cube_core --example turn_scaling -- 64 256 1024 2000
```

Measured on the development machine:

| N | Inner slice (µs/quarter) | Outer slice (µs/quarter) |
|---:|---:|---:|
| 64 | 1.105 | 16.652 |
| 256 | 4.960 | 242.667 |
| 1024 | 20.219 | 4423.857 |
| 2000 | 38.141 | 16089.396 |

This demonstrates the expected approximately linear inner-slice work and quadratic outer-face work for the sticker representation. It is not yet an end-to-end reduction benchmark.

## Required parity redesign

The existing `2^K` subset search (`K = floor((N-2)/2)` wing orbits) must not be extended by merely deleting its `.min(8)` cap. That would replace a correctness ceiling with exponential runtime and eventually integer-shift failure.

The target design is:

1. Normalize every mirrored 24-wing orbit into a sticker-visible canonical form using complete, center-safe transporters.
2. Distinguish exact all-home form `E_d`, one fixed staged defect form `D_d`, and an explicit `CoverageFailure`; a bounded search stall is never itself called parity.
3. Correct `D_d` with a move template parameterized by slice depth, conservatively one orbit at a time.
4. Recompute the visible signature after every correction and preserve already-established center/edge invariants.
5. Replay the complete simplified move list on the original sticker state before reporting success.
6. Add explicit tests at orbit 9 and beyond word width (for example N=20, N=66, and N=132) before expanding UI routing.

A physical labeled 24-wing permutation sign cannot be recovered from sticker colors alone: each logical edge contributes two visually identical wings to an orbit, and exchanging those twins changes labeled sign without changing the sticker buffer. Labeled probes remain useful for verifying move-template support in tests, but runtime classification must use canonical visible forms.

Noncontiguous multi-slice batching is an optional optimization only after its commutativity and exact piece signature are machine-verified.

### Parity experiments

- RCube’s depth-indexed `FixParity` sequence was translated into the current move convention. Its exact machine-checked signature now passes at every paired depth on N=4, 5, 6, 7, 8, and 20: centers and corner stickers are unchanged, the odd-cube midge and every non-target wing orbit are unchanged, the target orbit changes, and inverse replay restores solved state. Applying the template to a solved cube intentionally creates the canonical local defect; expecting all dedges to remain paired was the earlier incorrect test assumption.
- The edge solver’s first stalled slot is now documented as a bounded coverage diagnostic, not a parity certificate. The existing greedy batched correction remains a legacy optimization only.
- The visible-form normalizer tries exact `E_d` and `D_d` targets per orbit and uses the verified inverse template only after `D_d` is reached. It keeps `CoverageFailure` separate. Its center-safe meta-commutator seeds and final general-cycle cap are now stratified fairly by physical wing orbit; this removed the global shortest/cap bias that starved later depths.
- The formerly failing sparse N=20 `{1,9}` replay now passes in 119.87 s. Alternating `{1,3,5,7,9}` and all-nine-orbit replay gates pass in 165.90 s combined. In each case the visible normalizer classified exactly the expected E/D forms and replay solved the original sticker state.
- Canonical sparse, alternating, and all-defect vectors at N=66 and N=132 pass correction and replay without a fixed-width mask. Exact-form classification compares only 24 ordered sticker pairs per depth and is O(N) across all orbits; the shared defect signature is machine-verified through N=132.
- The deterministic edge repertoire is now generated per physical orbit rather than from an all-depth setup×base cartesian product, reducing repertoire generation from O(N²) to O(N). Orbit libraries are built and cached lazily, so normalizing one large orbit does not materialize full-width permutations for every depth.
- Isolated noncanonical high-depth transport now passes and replay-verifies at both N=66 and N=132. A warm release run covering two disturbed orbits at each size took 23.91 s combined with 194,052,096 bytes maximum resident set size (`/usr/bin/time -l` on macOS; peak-memory-footprint field 59,900,528 bytes).
- Full bounded N=12 center→edge→3×3 reduction now solves and replay-verifies a 144-wide-turn scramble in 21.30 s warm (21.46 s wall), with 280,395,776 bytes maximum RSS and zero swaps under `/usr/bin/time -l` on macOS.
- Center-library permutation dedup now stores only moved dst→src pairs instead of identity-padded full permutations. On the same warm N=12 gate this reduced maximum RSS from 685,654,016 to 280,395,776 bytes and test time from 25.11 to 21.30 s.
- The formerly timing-out full N=20 80-wide-turn center+edge probe now solves and replay-verifies in 85.45 s warm (85.65 s wall), with 618,463,232 bytes maximum RSS and zero swaps. The fix is an exact last-two-center solver: it partitions the final target/reservoir faces into ≤16-cell geometric orbits and BFSes their visible binary color masks using only finalized-face-safe, orbit-preserving center cycles.
- The exact last-two-center path rejects multi-orbit generators that transfer target/reservoir colors across faces, revalidates every previously solved orbit after each path, checks cancellation during generator/BFS scans, and refuses center counts that cannot be represented by its compact u32 library indices.
- Two additional independent N=20 80-wide-turn seeds also solve and replay-verify, completing in 89.16 s combined when sharing per-size caches. Together with the measured seed, the full N=20 corpus is now 3/3.
- The first full N=24 probe (72 randomized wide turns) remains bounded even with a research-only 600-second budget. It returns without caller mutation, uses 569,360,384 bytes maximum RSS, and records zero swaps. Profiling shows the center library completes and repeated center placement/recovery across 484 center cells per face dominates before a successful edge/parity finish.
- Center direct placement now scores safe candidates by multi-cell gain instead of always taking the first shortest cycle. This improves the N=12 gate from 21.30 s to 17.68 s, but is insufficient to close N=24.
- Isolated N=24 center profiling distinguishes coverage from raw timeout: the center stage returns before its 300-second control but stalls on the third face with 286/484 correct. Trying every wrong cell for a safe direct placement improved the prior 234/484, ~216-second stall to 286/484 in ~105 seconds, proving the lexical first-wrong policy was wasteful but not the whole gap.
- Two attempted generalizations were rejected after measurement: per-orbit meta-seed/final-cap stratification reduced useful library coverage, and a generic 32/40-cell target-color BFS hit its 500,000-state cap or a disconnected generator component. Both were reverted; the exact binary BFS remains only for the proven last-two-center case.
- A structural correction explains the apparent 32-cell disconnect: sorting two face-local depths merges the two chiral 24-piece physical orbits for a generic oblique center. Center orbits are now canonicalized from centered 3-D cubie coordinates under the 24 determinant-+1 cube rotations. N=24 has exactly 121 physical center orbits of 24 cells; even-N and odd-N cardinality formulas are unit-tested. Last-two generation/BFS now keeps mirror orbits separate.
- A release structural gate freezes Up/Down and checks every N=24 physical orbit. Existing safe center actions connect all 16 remaining-face positions and provide a pure 3-cycle seed for all 121/121 orbits. Pure 3-cycles alone are disconnected on deeper obliques, so the constructive path is to use general safe actions as conjugators and synthesize exact support-three transports, rather than expand color-mask search.
- External source review supports the direction but not a drop-in proof. [RCube](https://github.com/ShellPuppy/RCube/blob/c0e6df125db141eaf0044bf5a39cb54c942cfab2/RCube/Cube.cpp#L304-L423) uses an N-parameterized row/column center commutator and batches commuting slices, but its production fast path directly mutates facelets while the legal move word is commented out. Any borrowed scheduler/commutator must therefore be translated through this project’s move API and permutation/replay checked.
- The constructive transport is now implemented. For each stalled physical orbit with at least two reservoir faces, it deduplicates safe source→destination actions, adds legal inverses, explores ordered triples over at most 16 active cells, and reconstructs a conjugator. `h⁻¹ τ h` turns a verified exact seed 3-cycle into `source → target → safe buffer`; runtime checks require the target-orbit correct count to increase by exactly one, and completed orbits are revalidated cumulatively.
- The formerly failing isolated N=24 center gate now solves all six 484-cell centers and replay-verifies in 126.11 seconds. The same 72-wide-turn N=24 state then passes a strict full center→edge→3×3 solve and independent replay in 194.13 seconds test time (211.93 seconds wall), with 696,795,136-byte maximum RSS and zero swaps under `/usr/bin/time -l` on macOS. The N=20 measured regression remains green and improves to 71.82 seconds.
- Two independent additional N=24 seeds also solve and strictly replay with shared libraries in 245.07 seconds combined (262.68 seconds wall), with 723,959,808-byte maximum RSS and zero swaps. The full N=24 corpus is now 3/3.
- The first N=28 full 84-wide-turn probe also solved and replay-verified before its 900-second control in 407.27 seconds test time (425.58 seconds wall), with 595,886,080-byte maximum RSS and zero swaps. Its test is promoted from a bounded outcome probe to a strict replay gate.
- The renamed strict N=28 gate was externally rerun and passes in 401.43 seconds test time (419.13 seconds wall), with 546,258,944-byte maximum RSS and zero swaps. A second independent N=28 seed also strictly solves/replays in 344.56 seconds test time (362.98 seconds wall), with 571,621,376-byte maximum RSS and zero swaps. Full N=28 evidence is now 2/2.
- A focused enabled N=4 regression forces the constructive helper on a cross-face pure-cycle disturbance, applies its returned legal word, and checks both the target center and exact snapshot replay. This complements the ignored large research gates with routine direction/composition coverage.
- The first N=32 full 96-wide-turn probe reaches its 1,200-second cooperative deadline, returns `CancelledOrTimedOut` without mutating the caller, uses 594,051,072-byte maximum RSS, and records zero swaps. This is a throughput limit, not memory pressure or an unsafely interrupted result.
- A 600-second center-only replay probe localizes the N=32 cliff before edges: the 271,193-cycle library builds and Up solves in 690 placements, but the stage times out during the second face. All partial moves replay exactly to the interrupted center snapshot; maximum RSS is 640,958,464 bytes with zero swaps.
- Profiling corrected the initial diagnosis: at N=24 the library build takes ~103 seconds while ordinary Up/Down placement is subsecond; a legacy bridge/DFS ran for ~30 seconds before the exact transporter. At N=32 the library build takes ~317 seconds and left too little of the old 600-second center budget for the same pre-fallback work.
- Three scheduling changes remove that cliff without changing the move group: safe cycles are indexed once by physical orbit instead of rescanning the entire library per orbit; ordered-triple BFS expands only until the currently required `source → target → buffer` is reached; and the exact transporter runs before legacy bounded bridge/DFS searches. N=24 Front drops from ~32 seconds to 0.287 seconds and the complete center gate to 104.08 seconds.
- The formerly timing-out N=32 centers now all solve/replay in 320.27 seconds: the 271,193-cycle build takes 316.89 seconds and all six placement phases complete in about three seconds. Maximum RSS is 723,238,912 bytes with zero swaps.
- The same N=32 96-wide-turn state then solves through centers, edges, parity recovery, and 3×3 finish and independently replays in 401.74 seconds test time (420.29 seconds wall), with 1,076,789,248-byte maximum RSS and zero swaps. The bounded probe is promoted to a strict replay gate; its renamed command still needs an external rerun.
- The renamed strict N=32 gate was externally rerun and passes in 419.49 seconds test time (439.00 seconds wall), with 702,185,472-byte maximum RSS and zero swaps.
- Build review identified repeated dense permutation work without changing candidate coverage: every base word was evaluated once for dedup and again for the 200-word shortlist, while accepted meta-commutators were evaluated once for filtering and again for insertion. The pipeline now carries known dense permutations into sparse dedup and stores borrowed base references until the deterministic shortlist is truncated; generated words, ordering, filters, and caps remain unchanged.
- This low-risk change reduces the N=24 cold library build from ~97.05 to 89.44 seconds and N=32 from 316.89 to 285.65 seconds. The strict N=32 center gate completes in 289.58 seconds with 585,777,152-byte maximum RSS and zero swaps. The full strict replay gate remains green at 386.41 seconds with 1,039,286,272-byte maximum RSS and zero swaps.
- Reusing two dense composition buffers was measured and rejected: despite reducing allocations, the manual fill/swap loop regressed the N=24 cold build from 89.44 to 125.72 seconds. The original optimized `collect` composition was restored; allocator-count intuition is not a substitute for end-to-end timing.
- The first N=36 full 108-wide-turn probe solves through reduction and independently replays before its 1,500-second control in 583.00 seconds test time (601.82 seconds wall), with 1,243,136,000-byte maximum RSS and zero swaps. Its accepting bounded probe is promoted to a strict replay gate.
- The renamed strict N=36 command was externally rerun and passes in 593.46 seconds test time (611.60 seconds wall), with 673,660,928-byte maximum RSS and zero swaps.
- A larger low-risk build optimization derives each face-turn conjugate permutation directly as `P_setup ∘ P_core ∘ P_setup⁻¹` from the already-computed dense core permutation. It preserves all generated move words and filters while avoiding a full recomposition of every long conjugated word. A focused N=4/5/8 test proves the derived destination→source permutation equals full move-word composition.
- This reduces the N=24 cold library build from 89.44 to 41.00 seconds and N=32 from 285.65 to 108.42 seconds. Strict N=32 centers finish in 111.89 seconds, and strict full N=32 replay improves from 386.41 to 202.68 seconds; zero-swap resource gates and all focused correctness tests remain green.
- Post-optimization strict N=36 replay passes in 289.16 seconds test time (308.04 seconds wall), with 707,330,048-byte maximum RSS and zero swaps—roughly half the pre-optimization 593.46 seconds.
- A second independent N=32 seed strictly solves/replays in 298.06 seconds test time (322.88 seconds wall), with 604,487,680-byte maximum RSS and zero swaps. Full N=32 evidence is now 2/2.
- The first N=40 full 120-wide-turn probe solves and independently replays before its 1,800-second control in 442.14 seconds test time (461.37 seconds wall), with 837,632,000-byte maximum RSS and zero swaps. Its accepting bounded probe is promoted to a strict replay gate.
- The renamed strict N=40 command was externally rerun and passes in 497.93 seconds test time (517.02 seconds wall), with 854,917,120-byte maximum RSS and zero swaps.
- Read-only memory review found no correctness blocker and cautioned that process maximum RSS mixes transient build high-water pages, center/edge caches, solution words, and solver indexes. Environment-gated deep-size accounting now separates retained center-cycle classes and lower-bound owned bytes without changing production layout.
- At N=12, 72,822 retained cycles split into 39,562 pure-three, 26,260 two-face-confined, and 7,000 capped-general effects; headers/moves/duplicate support/hash-pair lower bounds total ~86.7 MB. At N=24, 196,964 cycles split into 132,581 pure, 57,383 confined, and 7,000 general; corresponding lower-bound bytes total ~256.5 MB. Pure cycles dominate count, while `support` duplicates hash-map destinations and costs 48.3 MB by itself at N=24.
- The representation-only experiment succeeds. Each retained cycle now owns one deterministic boxed `(destination, source)` array that serves support iteration, exact lookup, safe filtering, constructive actions, and diagnostics; the duplicate support vector and per-cycle hash allocation are removed without changing cycle counts or move words.
- N=12 lower-bound retained bytes fall from ~86.7 to 56.3 MB. N=24 falls from ~256.5 to 164.8 MB; center-gate maximum RSS drops from 512,638,976 to 390,381,568 bytes. N=32 retains 271,193 cycles (190,209 pure / 73,984 confined / 7,000 general) in ~256.5 MB lower-bound headers/moves/effects; center-gate maximum RSS falls from 740,671,488 to 608,124,928 bytes.
- Strict full N=32 replay remains green in 208.09 seconds test time, while maximum RSS falls from 1,047,740,416 to 701,759,488 bytes (about 33%). Cold build timing regresses modestly from 108.42 to 116.73 seconds, an accepted trade for the measured memory reduction; exact permutation, constructive, and small/full replay gates pass.
- Strict N=40 under compact effects remains green in 463.74 seconds test time (464.39 seconds wall), while maximum RSS drops from 854,917,120 to 698,925,056 bytes with zero swaps.
- A second independent N=36 seed strictly solves/replays in 407.80 seconds test time (426.68 seconds wall), with 653,705,216-byte maximum RSS and zero swaps. Full N=36 evidence is now 2/2.
- The first N=44 full 132-wide-turn probe solves and independently replays before its 2,100-second control in 649.23 seconds test time (670.66 seconds wall), with 819,724,288-byte maximum RSS and zero swaps. Its accepting bounded probe is promoted to a strict replay gate.
- The renamed strict N=44 command was externally rerun and passes in 656.08 seconds test time (674.98 seconds wall), with 889,159,680-byte maximum RSS and zero swaps.
- A second independent N=40 seed strictly solves/replays in 389.17 seconds test time (407.87 seconds wall), with 768,950,272-byte maximum RSS and zero swaps. Full N=40 evidence is now 2/2.
- The complete enabled `cube_solver` library suite passes after compaction: 39 passed / 48 explicitly ignored research gates in 41.84 seconds; focused format, Clippy, and diff checks also pass.
- A second independent N=44 seed strictly solves/replays in 529.75 seconds test time (548.20 seconds wall), with 1,522,728,960-byte maximum RSS and zero swaps. Full N=44 evidence is now 2/2; the much higher seed-dependent peak reinforces finite-resource caveats.
- A fresh full-project gate passes after compact effects: generated frontend check, accessibility/privacy smoke, workspace all-feature Clippy, workspace tests, wasm32 build, release workspace build, Tauri check, and worker syntax. Two fresh reviewers found no solver blocker/high/medium issue; their frontend findings (stale worker correlation, empty MIT file, and stale documentation) were fixed.
- The exact final project gate passes again after those frontend fixes. Provenance is still required before deleting generation classes, because shortest-word dedup erases alternate origins; no class is deleted in this release. Frontier expansion stops at N=44 because the second seed's 1.52 GB peak makes an N=48 probe poor release-value per resource cost. The advertised N=11 product ceiling remains unchanged.
- Desktop runtime follow-up found two integration gaps that native unit tests could not expose: production center timing used `std::time::Instant`, which traps on `wasm32-unknown-unknown`, and a six-turn N=11 state made from unrelated bare inner slices reached the 290-second fallback deadline. Center timing now uses `web_time`; a generated-WASM Node smoke executes sticker-only 4×4 reduction plus independent replay on every desktop CI OS.
- Desktop 4×4–11×11 now routes sticker colors to native Tauri `solve_stickers`/`cancel_solve` commands. Automatic large-cube challenges use standard contiguous wide turns from either face, matching the advertised release corpus rather than arbitrary unrelated bare slices. The native sticker-only N=11 wide-turn gate solves and replays in 10.93 seconds test time, and CI runs it on macOS, Windows, and Linux.

## Safety gates before lifting the app’s N=11 solve ceiling

- [Completed] Cooperative cancellation/deadline checks inside reduction library construction, placement/search loops, parity recovery, and WASM size-aware deadlines; worker termination remains the hard-stop backstop.
- Non-ignored release-mode replay tests for every advertised size.
- Sparse, dense, alternating, first/last, and all-odd orbit parity tests above eight orbits.
- End-to-end time and peak-memory data for representative odd/even sizes.
- Honest failure states; never fall back to hidden scramble inversion while presenting a real solve.
- Worker-only large reduction so a failed worker cannot freeze the UI thread.

## Primary references

- Bonzio, Loi, and Peruzzi, *On the n×n×n Rubik’s Cube*: https://arxiv.org/abs/1708.05598
- Demaine et al., *Algorithms for Solving Rubik’s Cubes*: https://erikdemaine.org/papers/Rubik_ESA2011/paper.pdf
- Randelshofer, *OrbitCube*: https://randelshofer.ch/rubik/virtual_cubes/vcube7/picture_cubes/pdf/OrbitCube_24072010.pdf
- RCube large-N implementation: https://github.com/ShellPuppy/RCube
- Walton NxNxN solver: https://github.com/dwalton76/rubiks-cube-NxNxN-solver
