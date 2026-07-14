// Off-main-thread solver: keeps the UI responsive and lets a long/hard solve be
// cancelled (the main thread just terminates this worker). Posts a "ready" ping
// once WASM loads. Only 2×2/3×3 may use a bounded main-thread fallback; reduction
// remains worker-only so it can never freeze the UI thread.
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
    if (!d.colors || !lab.load_face_colors(d.colors)) {
      throw new Error('invalid or incomplete sticker state');
    }
    self.postMessage({ type: 'result', jobId: d.jobId, ok: true, result: lab.solve(Math.min(d.depth, 9), d.time) });
  } catch (err) {
    self.postMessage({ type: 'result', jobId: d.jobId, ok: false, error: String(err && err.message ? err.message : err) });
  }
};
