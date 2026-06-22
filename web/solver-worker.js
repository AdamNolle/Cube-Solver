// Off-main-thread solver: keeps the UI responsive and lets a long/hard solve be
// cancelled (the main thread just terminates this worker). Posts a "ready" ping
// once the wasm is loaded so the page knows the worker is usable; otherwise the
// page falls back to a bounded main-thread solve.
import init, { CubeLab } from './pkg/cube_wasm.js';

init()
  .then(() => self.postMessage({ type: 'ready' }))
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
