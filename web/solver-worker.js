// Off-main-thread solver: keeps the UI responsive and lets a long/hard solve be
// cancelled (the main thread just terminates this worker). Posts a "ready" ping
// once the wasm is loaded so the page knows the worker is usable; otherwise the
// page falls back to a bounded main-thread solve.
import init, { CubeLab, warm_solver } from './pkg/cube_wasm.js';

init()
  .then(() => {
    // Announce readiness, then pre-build the 3×3 two-phase tables off the main
    // thread so the first real Solve isn't slowed by the one-time table build.
    self.postMessage({ type: 'ready' });
    try {
      warm_solver();
    } catch (e) {
      /* warming is best-effort; solve() will build lazily if needed */
    }
  })
  .catch((e) => self.postMessage({ type: 'error', error: String(e) }));

self.onmessage = (e) => {
  const d = e.data || {};
  if (d.type !== 'solve') return;
  try {
    const lab = new CubeLab(d.n);
    for (const m of d.moves || []) lab.apply_design_move(m.axis, m.layer, m.dir);
    self.postMessage({ type: 'result', ok: true, result: lab.solve(Math.min(d.depth, 9), d.time) });
  } catch (err) {
    self.postMessage({ type: 'result', ok: false, error: String(err && err.message ? err.message : err) });
  }
};
