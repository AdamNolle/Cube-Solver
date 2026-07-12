#!/usr/bin/env python3
# Regenerates web/index.html: wraps the design component (design-source.txt) with
# the real Rust/WASM solver wiring. Run from anywhere: `python3 tools/gen-index.py`.
import json, re, os

_HERE = os.path.dirname(os.path.abspath(__file__))
_ROOT = os.path.dirname(_HERE)
design = open(os.path.join(_HERE, 'design-source.txt'), encoding='utf-8').read()
helmet = design[design.index('<helmet>')+8: design.index('</helmet>')]
xdc = design[design.index('<x-dc>')+6: design.index('</x-dc>')]
body = xdc[xdc.index('</helmet>')+9:]
helmet = helmet.replace('https://cdnjs.cloudflare.com/ajax/libs/three.js/r128/three.min.js', './vendor/three.min.js')
sopen = design.index('>', design.index('data-dc-script'))+1
js = design[sopen: design.rindex('</script>')]
js = js.replace('} else if (this.autoSpin && !this.activeMove) {', '} else if (this.autoSpin && !this.activeMove && !this.scrambled) {')

body = re.sub(r'ref="\{\{\s*rootRef\s*\}\}"', 'data-dcref="rootRef"', body)
body = re.sub(r'onClick="\{\{\s*(\w+)\s*\}\}"', r'data-onclick="\1"', body)
body = re.sub(r'onChange="\{\{\s*(\w+)\s*\}\}"', r'data-onchange="\1"', body)
# Correct a misleading design caption: sampling is a RENDERING technique, not how
# the (real) solver works — the solver only handles 2×2/3×3.
body = body.replace(
    'Renders any size. Past 11³ the faces are sampled — exactly how the real solver handles giant cubes.',
    'Renders any size. Past 11³ the cube switches to sampled texture mode for speed. 2×2 through 11×11 are solved for real (3×3 two-phase, 4×4–11×11 reduction); larger sizes are visual.')
assert '{{' not in body

SHIM = r'''
  class DCLogic {
    constructor(props){ this.props = props || {}; this.state = {}; }
    q(sel){ return (this.root || document).querySelector(sel); }
    qa(sel){ return Array.from((this.root || document).querySelectorAll(sel)); }
    setState(u){ Object.assign(this.state, typeof u === 'function' ? u(this.state) : u); }
    forceUpdate(){}
  }
'''

WIRE = r'''
  // ===== wire the real Rust solver (WASM) into the design =====
  function wireRealSolver(inst){
    // Engine id -> lane slot. The 3×3 is solved by the two-phase (Kociemba) solver,
    // which reuses the first ('det') lane slot — relabeled "Two-phase" for N=3 by
    // applyEngineLabels(), with the other two lanes hidden (they only run for 2×2).
    const KEYS = { deterministic:'det', beam:'beam', evolution:'evo', kociemba:'det' };
    const origScramble = inst.scramble.bind(inst);
    const origSolve = inst.solve.bind(inst);
    const origReset = inst.resetSolved ? inst.resetSolved.bind(inst) : null;
    const origSetN = inst.setN ? inst.setN.bind(inst) : null;
    const origReplay = inst.replay ? inst.replay.bind(inst) : null;
    const origBuildCube = inst.buildCube ? inst.buildCube.bind(inst) : null;
    const origFinalizeMove = inst.finalizeMove ? inst.finalizeMove.bind(inst) : null;

    inst.scrambleDepth = 6;
    const sd = inst.root.querySelector('[data-scramble]'); if (sd) sd.value = 6;
    const sdv = inst.root.querySelector('[data-scramble-val]'); if (sdv) sdv.textContent = '6';

    function lanesUpdate(self, lanes, winnerKey, moves){
      ['det','beam','evo'].forEach(k=>{
        const el = self.root.querySelector('[data-lane="'+k+'"]'); if(!el) return;
        const st = el.querySelector('[data-star]'); if(st) st.style.display='none';
      });
      (lanes||[]).forEach(L=>{
        const k = KEYS[L.id]; if(!k) return;
        const el = self.root.querySelector('[data-lane="'+k+'"]'); if(!el) return;
        const fill=el.querySelector('[data-fill]'), pct=el.querySelector('[data-pct2]'),
              stat=el.querySelector('[data-stat]'), star=el.querySelector('[data-star]');
        const isWin = (k===winnerKey);
        const p = isWin?100:Math.max(0,Math.min(100,(L.pct|0)));
        if(fill) fill.style.width = p+'%';
        if(pct){ pct.textContent = p+'%'; pct.style.color = (L.solved||isWin)?'#1BA64B':''; }
        if(stat) stat.textContent = isWin ? ('verified · '+moves+' moves') : (L.solved?'solved':(L.label||'searching…'));
        if(star) star.style.display = isWin ? 'inline' : 'none';
      });
    }

    if (origReset) inst.resetSolved = function(anim){
      abortPendingSolve(this);   // a pending solve must not land on a reset/rescrambled cube
      origReset(anim);
      if (this.lab){ this.lab.set_size(this.n); this.lab.reset(); }
    };
    if (origSetN) inst.setN = function(n){
      abortPendingSolve(this);   // changing N invalidates an in-flight solve's moves
      origSetN(n);
      this.lastScramble = [];   // a new N invalidates the old scramble's layers
      if (this.lab){ this.lab.set_size(this.n); this.lab.reset(); }
      if (this._refreshSolvability) this._refreshSolvability(this);
    };
    if (origReplay) inst.replay = function(){
      abortPendingSolve(this);   // don't let an in-flight solve collide with a replay
      return origReplay();
    };

    // ---- GPU memory: free three.js geometries/textures instead of leaking them ----
    // buildCube() rebuilt the cube and per-move slabs were dropped from the scene
    // without .dispose(), leaking ~6 MB per texture-cube rebuild — a size-slider
    // drag could leak 100 MB+ and crash the webview. Shared materials
    // (bodyMat / faceMats) are reused across builds, so they are never freed.
    function disposeCube(self){
      if (!window.THREE || !self.cubeGroup) return;
      var shared = new Set();
      if (self.bodyMat) shared.add(self.bodyMat);
      (self.faceMats || []).forEach(function(m){ shared.add(m); });
      var geos = new Set();
      (function scrub(o){
        if (!o) return;
        if (o.geometry) geos.add(o.geometry);
        var m = o.material;
        if (m) (Array.isArray(m) ? m : [m]).forEach(function(mm){
          if (mm && !shared.has(mm)){
            if (mm.map && mm.map.dispose) mm.map.dispose();
            if (mm.dispose) mm.dispose();
          }
        });
        if (o.children) o.children.slice().forEach(scrub);
      })(self.cubeGroup);
      geos.forEach(function(g){ if (g && g.dispose) g.dispose(); });
    }
    if (origBuildCube) inst.buildCube = function(n){
      try { disposeCube(this); } catch(e){}
      return origBuildCube(n);
    };
    if (origFinalizeMove) inst.finalizeMove = function(){
      var am = this.activeMove, slab = am && am.slab;
      origFinalizeMove();
      if (slab){ try {
        if (slab.geometry && slab.geometry.dispose) slab.geometry.dispose();
        var sm = slab.material;
        if (sm) (Array.isArray(sm) ? sm : [sm]).forEach(function(mm){ if (mm && mm.dispose) mm.dispose(); });
      } catch(e){} }
    };

    inst.scramble = function(){
      if (this.busy) return;
      if (this.mode !== 'cubie') return origScramble();   // huge (texture) cubes: design path
      this.resetSolved(true);
      const N = this.n;
      // 2×2/3×3 can be inverted by the real (outer-move) solver, so scramble with
      // outer faces only (the two-phase solver inverts outer turns). 4×4+ up to
      // SOLVE_MAX_N go through the reduction solver, which handles ANY scramble, so
      // mix every layer for a proper full scramble; bigger cubes stay visual-only.
      const solvable = (N <= SOLVE_MAX_N) && !!this.lab;
      // Never scramble a solvable cube deeper than the exact solver's reach, or
      // Solve would search to SOLVE_MAX_DEPTH and find no solution.
      const depth = solvable ? Math.min(this.scrambleDepth, N >= 3 ? SOLVE_REAL_DEPTH : SOLVE_MAX_DEPTH) : this.scrambleDepth;
      this.lastScramble = [];
      let prev = -1;
      for (let i=0;i<depth;i++){
        let axis;
        do { axis = (Math.random()*3)|0; } while (axis===prev && Math.random()<0.55);
        prev = axis;
        const layer = (N <= 3) ? (Math.random()<0.5 ? 0 : N-1) : (Math.random()*N)|0;
        const dir = Math.random()<0.5 ? 1 : -1;
        this.lastScramble.push({axis, layer, dir});
      }
      // Apply the whole scramble instantly — fast at any depth, no stuck queue.
      for (const m of this.lastScramble) this.applyInstant(m);
      // Mirror into the solver's cube only when it can actually solve it (N<=SOLVE_MAX_N).
      if (solvable){ this.lab.reset(); for (const m of this.lastScramble) this.lab.apply_design_move(m.axis, m.layer, m.dir); }
      this.queue = []; this.activeMove = null;
      this.movesDone = depth; this.totalMoves = depth;
      this.phase = 'idle'; this.busy = false; this.scrambled = true;
      if (this.resetLanes) this.resetLanes();
      if (this.ui && this.ui.status) this.ui.status.textContent = 'Scrambled — ready to solve';
      if (this.ui && this.ui.count) this.ui.count.style.display = 'none';
      if (this.ui && this.ui.move) this.ui.move.style.display = 'none';
      if (this.syncControls) this.syncControls();
    };

    // Mark all race lanes as a visual playback (no real search happened).
    function setLanesVisual(self){
      ['det','beam','evo'].forEach(function(k){
        var el = self.root.querySelector('[data-lane="'+k+'"]'); if(!el) return;
        var fill=el.querySelector('[data-fill]'), pct=el.querySelector('[data-pct2]'),
            stat=el.querySelector('[data-stat]'), star=el.querySelector('[data-star]');
        if(fill) fill.style.width='0%';
        if(pct){ pct.textContent=''; pct.style.color=''; }
        if(stat) stat.textContent='visual playback — not searched';
        if(star) star.style.display='none';
      });
    }

    inst.finishLanes = function(){
      if (this._visualSolve){ setLanesVisual(this); return; }
      const winnerKey = KEYS[this.realWinner] || 'det';
      // Use the face-turn (HTM) count, not the half-turn-expanded animation list
      // length, so the lane agrees with the proof box and the README.
      var moves = (this.realMoveCount != null) ? this.realMoveCount : (this.lastSolution||[]).length;
      lanesUpdate(this, this.realLanes || [], winnerKey, moves);
    };

    // The solution panel must show the standard face-turn notation (e.g. "U2",
    // "R'"), not the half-turn-expanded animation list ("U U", "R'"). The solver
    // returns that as res.notation; render the panel and clipboard from it so the
    // count and moves match the lane, the proof box and the README.
    var origShowSolution = inst.showSolution ? inst.showSolution.bind(inst) : null;
    if (origShowSolution) inst.showSolution = function(){
      if (this.mode !== 'texture' && this.realNotation && this.realNotation.length && this.ui && this.ui.solCount && this.ui.solChips){
        this.ui.solCount.textContent = this.realNotation.length;
        this.ui.solChips.innerHTML = '';
        this.realNotation.forEach((function(txt){
          var s = document.createElement('span');
          s.textContent = txt;
          s.style.cssText = "font-family:'JetBrains Mono',monospace;font-size:11.5px;padding:3px 7px;background:#F4F3EF;border-radius:6px;color:#3a3933;";
          this.ui.solChips.appendChild(s);
        }).bind(this));
        return;
      }
      return origShowSolution();
    };
    var origCopyPath = inst.copyPath ? inst.copyPath.bind(inst) : null;
    if (origCopyPath) inst.copyPath = function(){
      if (this.realNotation && this.realNotation.length){
        var txt = this.realNotation.join(' ');
        try { if (navigator.clipboard) navigator.clipboard.writeText(txt); } catch (e) {}
        var btn = this.q && this.q('[data-copy]');
        if (btn){ var o = btn.innerHTML; btn.innerHTML = '<span style="font-size:12px;color:#1BA64B;">Copied ✓</span>'; setTimeout(function(){ btn.innerHTML = o; }, 1400); }
        return;
      }
      return origCopyPath();
    };

    inst.solve = function(){
      if (this.mode !== 'cubie') return origSolve();   // texture (huge) cubes: design path
      if (this.busy || this._solvePending) return;
      // Solve must always do something. On a fresh / already-solved cube
      // (scrambled=false) it used to silently no-op; scramble first, then solve.
      if (!this.scrambled){ if (this.scramble) this.scramble(); if (!this.scrambled) return; }
      // Clear last solve's real-solver data so a visual (N>3) solve can't show a
      // stale 3×3 notation/count; the real branch repopulates it via onSolveResult.
      this.realNotation = null; this.realMoveCount = null;
      // Visual-only cubes (N > SOLVE_MAX_N): animate the inverse with HONEST lanes — no
      // real search. Queue is filled synchronously so the loop never finishes early.
      if (this.n > SOLVE_MAX_N || !this.lab){
        var inv = this.lastScramble.slice().reverse().map(function(m){ return {axis:m.axis, layer:m.layer, dir:-m.dir}; });
        this.lastSolution = inv;
        this.queue = inv.slice();
        this.movesDone = 0; this.totalMoves = inv.length;
        this._visualSolve = true;
        this.phase = 'solving'; this.busy = true;
        if (this.resetLanes) this.resetLanes();
        setLanesVisual(this);
        if (this.ui && this.ui.status) this.ui.status.textContent = 'Playing solution…';
        if (this.ui && this.ui.count) this.ui.count.style.display = 'inline-block';
        if (this.syncControls) this.syncControls();
        return;
      }
      // Real solve (2×2/3×3). _solvePending (not busy) blocks re-entry without
      // letting the rAF loop finish early on the empty queue.
      this._visualSolve = false;
      this._solvePending = true;
      // Watchdog: if a solve stalls (a slow or hung worker on a hard deep
      // scramble), recover the UI instead of sticking on 'Solving…' forever.
      var _sw = this;
      clearTimeout(this._solveWatchdog);
      this._solveWatchdog = setTimeout(function(){
        if (!_sw._solvePending) return;
        abortPendingSolve(_sw);
        if (_sw.ui && _sw.ui.status) _sw.ui.status.textContent = 'No verified solution within budget — try a lower scramble depth';
        if (_sw.syncControls) _sw.syncControls();
      }, 8000);
      if (this.ui && this.ui.status) this.ui.status.textContent = 'Solving…';
      if (this.startLanes) this.startLanes();
      if (this.syncControls) this.syncControls();
      ensureWorker(this);
      if (this._workerReady && this._worker){
        // Off the main thread → UI stays responsive and the solve is cancellable.
        showCancel(this, true);
        this._worker.postMessage({ type:'solve', n: this.n, depth: this.scrambleDepth||6, time: 6000, moves: this.lastScramble });
      } else {
        // Fallback: bounded main-thread solve (~1.5s worst case, never the old 5s).
        var self = this;
        setTimeout(function(){
          var r; try { r = self.lab.solve(Math.min(self.scrambleDepth||6, 9), 1500); } catch(e){ r = '{"found":false}'; }
          onSolveResult(self, { type:'result', ok:true, result:r });
        }, 30);
      }
    };

    // ---- Web Worker solve plumbing (responsive + cancellable) ----
    function ensureWorker(self){
      if (self._workerBroken) return null;   // a worker failed here before → use the main thread
      if (self._worker) return self._worker;
      try {
        var w = new Worker('./solver-worker.js', { type: 'module' });
        w.onmessage = function(e){
          var d = e.data || {};
          if (d.type === 'ready'){ self._workerReady = true; return; }
          if (d.type === 'error'){ self._workerReady = false; self._workerBroken = true; return; }
          if (d.type === 'result'){ onSolveResult(self, d); return; }
        };
        w.onerror = function(){
          // Worker failed to load/run (common in some webviews). Give up on workers
          // entirely — do NOT recreate (that loops forever) — and fall back to the
          // bounded main-thread solve. Recover any in-flight solve immediately.
          self._workerReady = false; self._workerBroken = true;
          if (self._worker){ try { self._worker.terminate(); } catch(e){} }
          self._worker = null;
          if (self._solvePending){
            var r; try { r = self.lab.solve(Math.min(self.scrambleDepth||6, 9), 1500); } catch(e){ r = '{"found":false}'; }
            onSolveResult(self, { type:'result', ok:true, result:r });
          }
        };
        self._worker = w;
      } catch(e){ self._workerBroken = true; self._worker = null; self._workerReady = false; }
      return self._worker;
    }
    // Stop an in-flight worker solve without touching status (guard before
    // scramble/reset/N/replay). Returns true if a solve was aborted.
    function abortPendingSolve(self){
      if (!self._solvePending) return false;
      clearTimeout(self._solveWatchdog);
      self._solvePending = false;
      if (self._worker){ try { self._worker.terminate(); } catch(e){} self._worker = null; self._workerReady = false; }
      showCancel(self, false);
      ensureWorker(self);
      return true;
    }
    function onSolveResult(self, data){
      if (!self._solvePending) return;   // cancelled or stale
      clearTimeout(self._solveWatchdog);
      self._solvePending = false;
      showCancel(self, false);
      var res = null;
      if (data && data.ok){ try { res = JSON.parse(data.result); } catch(e){} }
      if (!res || !res.found || !res.moves || !res.moves.length){
        if (self.ui && self.ui.status) self.ui.status.textContent = 'No verified solution within budget — lower the scramble depth';
        lanesUpdate(self, (res && res.lanes) || [], null, 0);
        if (self.syncControls) self.syncControls();
        return;
      }
      self.realWinner = res.winner; self.realLanes = res.lanes || [];
      self.lastSolution = res.moves;
      // res.moveCount is the face-turn (HTM) count; res.moves is the half-turn-
      // expanded animation list. Keep the HTM count + notation for the lane/proof/
      // solution panel so all three agree (the panel must show U2, not "U U").
      self.realMoveCount = res.moveCount;
      self.realNotation = res.notation || null;
      updateProof(self, (self.lastScramble||[]).length, res.moveCount);
      self.queue = res.moves.slice();   // fill the queue BEFORE claiming busy
      self.movesDone = 0; self.totalMoves = self.queue.length;
      self.phase = 'solving'; self.busy = true;
      if (self.ui && self.ui.count) self.ui.count.style.display = 'inline-block';
      if (self.syncControls) self.syncControls();
    }
    // Stop an in-flight solution *replay* (the visible, multi-second animation) and
    // put the cube back to its scrambled state so a cancel cleanly returns to
    // "ready to solve". This is what the user is actually watching when they hit
    // Cancel — the search itself finishes in ~100ms.
    function stopReplay(self){
      var animating = (self.busy && self.phase === 'solving') || (self.queue && self.queue.length) || self._visualSolve;
      if (!animating) return false;
      if (self.activeMove && self.finalizeMove){ try { self.finalizeMove(); } catch(e){} }
      self.activeMove = null;
      self.queue = []; self.movesDone = 0; self.totalMoves = 0; self._visualSolve = false;
      self.phase = 'idle'; self.busy = false;
      // Rebuild and re-apply the scramble so the visible cube matches the solver's
      // state again (a half-played solution would otherwise desync the next solve).
      if (self.buildCube){
        try {
          self.buildCube(self.n);
          if (self.applyInstant) (self.lastScramble || []).forEach(function(m){ self.applyInstant(m); });
        } catch(e){}
        self.scrambled = (self.lastScramble || []).length > 0;
      }
      if (self.ui && self.ui.move) self.ui.move.style.display = 'none';
      if (self.ui && self.ui.count) self.ui.count.style.display = 'none';
      return true;
    }
    function cancelSolve(self){
      var hadSearch = abortPendingSolve(self);   // stop an off-thread search, if any
      var hadReplay = stopReplay(self);          // stop the visible replay, if any
      if (!hadSearch && !hadReplay) return;      // nothing to cancel
      if (self.resetLanes) self.resetLanes();
      if (self.ui && self.ui.status) self.ui.status.textContent = 'Solve cancelled — scramble or solve again';
      if (self.syncControls) self.syncControls();
    }
    // Solve is greyed/disabled for the whole solve (search + replay). When NOT
    // solving we leave the button to the design's syncControls (it greys Solve
    // when the cube is already solved / unscrambled). Cancel always stays on
    // screen and active (a no-op when there's nothing to cancel).
    function updateSolveButtons(self){
      var solving = !!self._solvePending || (self.busy && self.phase === 'solving');
      var solveBtn = self.root.querySelector('[data-onclick="onSolve"]');
      // Solve is greyed ONLY while a solve is in progress. Otherwise it stays
      // clickable even on a solved/fresh cube (it scrambles first), so it never
      // silently does nothing. This overrides the design's 'grey when unscrambled'.
      if (solveBtn){
        solveBtn.style.opacity = solving ? '0.45' : '1';
        solveBtn.style.pointerEvents = solving ? 'none' : 'auto';
        solveBtn.style.cursor = solving ? 'default' : 'pointer';
      }
      var cb = self.root.querySelector('[data-cancel-solve]');
      if (cb){ cb.disabled = false; cb.style.opacity = '1'; cb.style.pointerEvents = 'auto'; cb.style.cursor = 'pointer'; }
    }
    // Kept for existing call sites — state is derived from _solvePending/busy.
    function showCancel(self){ updateSolveButtons(self); }
    // Re-sync the Solve/Cancel buttons on every UI state change the design makes.
    if (inst.syncControls){
      var origSyncC = inst.syncControls.bind(inst);
      inst.syncControls = function(){ origSyncC(); updateSolveButtons(this); };
    }
    ensureWorker(inst);   // warm up the solver worker so it's ready before the first solve
    updateSolveButtons(inst);   // initial state: Solve enabled, Cancel greyed

    // During a visual-only solve there's no real search — don't animate the
    // fabricated node/generation telemetry the design shows per frame.
    var origUpdateLanes = inst.updateLanes ? inst.updateLanes.bind(inst) : null;
    if (origUpdateLanes) inst.updateLanes = function(f){
      if (this._visualSolve){ setLanesVisual(this); return; }
      // The 3×3 two-phase solve finishes before the replay even starts, so don't
      // fabricate per-frame beam-search node/generation telemetry on its lane —
      // that would contradict the "two-phase" label. Show it as found; finishLanes
      // then stamps the verified move count.
      if ((this.n||3) === 3){
        var el = this.root.querySelector('[data-lane="det"]');
        if (el){
          var stat = el.querySelector('[data-stat]'), fill = el.querySelector('[data-fill]'), pct = el.querySelector('[data-pct2]');
          if (stat) stat.textContent = 'solution found — replaying';
          if (fill) fill.style.width = '100%';
          if (pct) pct.textContent = '100%';
        }
        return;
      }
      return origUpdateLanes(f);
    };

    // ---- real evolutionary swarm: each card is a live learning trial ----
    var SWPAL = [inst.PAL.w, inst.PAL.y, inst.PAL.g, inst.PAL.b, inst.PAL.o, inst.PAL.r];
    var FACE_FOR_CARD = [0,2,5,3,4,1]; // card faces [U,F,R,B,L,D] -> wasm [Up,Down,Front,Back,Left,Right]
    var origInitSwarm = inst.initSwarm ? inst.initSwarm.bind(inst) : null;
    var origUpdateSwarm = inst.updateSwarm ? inst.updateSwarm.bind(inst) : null;

    function swarmKey(self){ return (self.n||3)+':'+JSON.stringify(self.lastScramble||[]); }
    var SWARM_MAX = 3;   // search/evolution only genuinely solves 2×2 / 3×3
    function makeSwarm(self){
      try {
        if ((self.n||3) > SWARM_MAX){ self.swarmLab = null; self._swarmKey = swarmKey(self); return; }
        var cnt = (self.cards||[]).length||16;
        var ls = self.lastScramble || [];
        self.swarmLab = new Swarm(cnt, self.n||3, Math.max(3, self.scrambleDepth||6), BigInt((Math.random()*1e9)|0)+1n);
        // Always seed the swarm from the EXACT Studio cube. Empty scramble (a
        // solved Studio) → solved base, so the wall mirrors the on-screen cube.
        self.swarmLab.set_scramble(
          Uint8Array.from(ls.map(function(m){return m.axis;})),
          Uint32Array.from(ls.map(function(m){return m.layer;})),
          Int32Array.from(ls.map(function(m){return m.dir;})));
        self.swarmShared = ls.length > 0;
        self._swarmKey = swarmKey(self);
        self._swAccum=0; self._swPainted=false;
      } catch(e){ console.warn('swarm init failed', e); self.swarmLab=null; }
    }

    if (origInitSwarm) inst.initSwarm = function(){
      origInitSwarm();
      this.cumulativeSolves = 0;
      makeSwarm(this);
    };

    if (origUpdateSwarm) inst.updateSwarm = function(dt){
      if (!this.cards || !this.cards.length) return;
      var grid = this.root.querySelector('[data-swarm]');
      var msg = this.root.querySelector('[data-swarm-msg]');
      var liveEl = this.root.querySelector('[data-swarm-live]');
      // Remember the grid's real display value so we can restore it (don't blank it).
      if (grid && this._gridDisp === undefined) this._gridDisp = grid.style.display || 'grid';
      // Bigger cubes aren't solvable by search/evolution — explain instead of stalling.
      if ((this.n||3) > SWARM_MAX){
        this.swarmLab = null;
        if (grid) grid.style.display = 'none';
        if (liveEl) liveEl.textContent = '';
        if (msg){
          msg.style.display = 'block';
          msg.innerHTML = 'The evolutionary swarm races to solve the cube the engines can actually crack — the <b>3×3</b> (and 2×2). A <b>' + this.n + '×' + this.n + '</b> cube is beyond any search or evolutionary solver. Set Studio to <b>3×3</b> to watch the whole wall learn to solve your cube.';
        }
        return;
      }
      if (grid) grid.style.display = this._gridDisp;
      if (msg) msg.style.display = 'none';
      // Always track the Studio cube: rebuild whenever its scramble (or N) changes.
      if (!this.swarmLab) makeSwarm(this);
      else if (this._swarmKey !== swarmKey(this)) makeSwarm(this);
      if (liveEl){
        liveEl.textContent = this.swarmShared
          ? ('Tracking your Studio cube — ' + this.n + '×' + this.n + ', ' + (this.lastScramble||[]).length + '-move scramble')
          : 'Studio cube is solved — scramble it to watch the swarm learn to solve it';
      }
      if (!this.swarmLab) return origUpdateSwarm(dt);
      try {
        var g = this.swarmSpeed || 1;
        this._swAccum = (this._swAccum||0) + dt * g * 100;
        var steps=0; while (this._swAccum >= 1 && steps < 24){ this.swarmLab.step(); this._swAccum -= 1; steps++; }
        // Spin every frame (cheap); only re-read the WASM buffer and repaint the
        // 54 stickers when the population actually advanced (or on first paint).
        var repaint = steps > 0 || !this._swPainted;
        var buf = repaint ? this.swarmLab.render() : null;
        var N = Math.min(this.cards.length, this.swarmLab.member_count());
        for (var i=0;i<N;i++){
          var c = this.cards[i];
          c.rot += dt * c.spin; if (c.inner) c.inner.style.transform = 'rotateX(-20deg) rotateY('+c.rot+'deg)';
          if (!repaint) continue;
          var b = i*55, pct = buf[b];
          // per-cell colour cache; reset if the design rebuilt this card's cells
          if (!c.last || c._lastCells !== c.cells){ c.last = new Int8Array(54).fill(-1); c._lastCells = c.cells; }
          for (var k=0;k<54;k++){
            var cf=(k/9)|0, cell=k%9, wf=FACE_FOR_CARD[cf];
            var ci = buf[b + 1 + wf*9 + cell];
            // only touch the DOM when a sticker's colour actually changes
            if (c.cells[k] && c.last[k] !== ci){ c.cells[k].style.background = SWPAL[ci] || SWPAL[0]; c.last[k] = ci; }
          }
          c.cring.style.strokeDashoffset = c.circ * (1 - pct/100);
          if (pct >= 100){
            c.cring.style.stroke = '#00A24B'; c.check.style.display='flex';
            c.cnt.textContent='solved'; c.cnt.style.color='#1BA64B';
            c.card.style.borderColor='#CDEAD4'; c.card.style.boxShadow='0 4px 16px -6px rgba(27,166,75,.4)';
          } else {
            c.cring.style.stroke = this.accent; c.check.style.display='none';
            c.cnt.textContent = Math.round(54*(1-pct/100)) + ' off'; c.cnt.style.color='#C7C3BA';
            c.card.style.borderColor='#ECEAE4'; c.card.style.boxShadow='0 1px 2px rgba(20,20,18,.03)';
          }
        }
        if (repaint){
          this._swPainted = true;
          var t=this.q('[data-swarm-total]'); if(t) t.textContent = this.swarmLab.converged().toLocaleString();
          var r=this.q('[data-swarm-run]'); if(r) r.textContent = this.swarmLab.solving_now();
          var a=this.q('[data-swarm-avg]'); if(a) a.textContent = Math.round(this.swarmLab.avg_progress()*100)+'%';
        }
      } catch(e){ console.warn('swarm update failed', e); this.swarmLab = null; }
    };

    function updateProof(self, scrambleLen, solutionLen){
      var p = self.root.querySelector('[data-proof]'); var body = self.root.querySelector('[data-proof-body]');
      if (!p || !body) return;
      var cmp = (solutionLen < scrambleLen)
        ? ('<b>'+solutionLen+' moves</b> — shorter than the '+scrambleLen+'-move scramble, so it cannot be replaying it.')
        : ('<b>'+solutionLen+' moves</b> from the state alone — a fresh, verified path, not the scramble inverse.');
      body.innerHTML = 'The '+scrambleLen+'-move scramble was applied, then discarded. The solver was given only the cube state (54 sticker colours) and searched for its own path: '+cmp;
      p.style.display='block';
    }

    function wireExplanations(self){
      var chips = self.root.querySelector('[data-sol-chips]');
      if (chips && !self.root.querySelector('[data-proof]')){
        var p = document.createElement('div'); p.setAttribute('data-proof','');
        p.style.cssText = 'display:none;margin:2px 0 12px;padding:11px 13px;background:#F4F8F3;border:1px solid #E0EBDB;border-radius:12px;';
        p.innerHTML = '<div style="display:flex;align-items:center;gap:7px;font-size:10px;font-weight:600;letter-spacing:.07em;color:#4E8A4A;margin-bottom:6px;"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="11" width="18" height="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg>SCRAMBLE HIDDEN FROM SOLVER</div><div data-proof-body style="font-size:12px;color:#5b5f54;line-height:1.5;"></div>';
        chips.parentNode.insertBefore(p, chips);
      }
      self.root.querySelectorAll('span').forEach(function(s){
        if (s.textContent && s.textContent.trim()==='3 strategies, in parallel'){
          s.textContent = '3 strategies on the cube state — none see the scramble';
          s.setAttribute('data-race-sub','');
        }
        if (s.textContent && s.textContent.trim()==='SOLVER RACE'){
          s.setAttribute('data-race-head','');
        }
      });
      var DESC = { det:'Searches forward from the scrambled state and backward from solved until the paths meet — exact & shortest.',
                   beam:'Keeps the handful of partial solutions closest to solved at each step.',
                   evo:'Evolves a population of move sequences — fittest survive, recombine, and mutate.' };
      ['det','beam','evo'].forEach(function(k){
        var el = self.root.querySelector('[data-lane="'+k+'"]'); if(!el) return;
        el.setAttribute('title', DESC[k]);
        if (!el.querySelector('[data-lane-desc]') && el.firstElementChild){
          var d = document.createElement('div'); d.setAttribute('data-lane-desc','');
          d.style.cssText='font-size:10px;color:#B7B3AA;margin:-2px 0 7px;line-height:1.35;';
          d.textContent = DESC[k];
          el.firstElementChild.insertAdjacentElement('afterend', d);
        }
      });
      var grid = self.root.querySelector('[data-swarm]');
      if (grid && !self.root.querySelector('[data-swarm-note]')){
        var note = document.createElement('div'); note.setAttribute('data-swarm-note','');
        note.style.cssText='font-size:12px;color:#7d7a72;line-height:1.55;margin:0 2px 14px;max-width:780px;';
        note.innerHTML = 'Each cell is one <b>trial of the evolutionary solver</b>, and every trial starts as an exact copy of <b>your Studio cube</b>. They mutate and recombine until a trial reaches solved (it turns green, then restarts from the cube). Change the Studio scramble and the whole wall instantly re-syncs to it. The trials see only the cube state, never the scramble.';
        var live = document.createElement('div'); live.setAttribute('data-swarm-live','');
        live.style.cssText='font-size:11.5px;font-weight:500;color:#1573E6;margin:0 2px 12px;';
        live.textContent = 'Tracking your Studio cube';
        var msg = document.createElement('div'); msg.setAttribute('data-swarm-msg','');
        msg.style.cssText='display:none;font-size:14px;color:#8C887F;line-height:1.6;background:#F7F6F2;border:1px solid #ECEAE4;border-radius:16px;padding:28px 26px;margin:6px 2px;max-width:720px;';
        grid.parentNode.insertBefore(note, grid);
        grid.parentNode.insertBefore(live, grid);
        grid.parentNode.insertBefore(msg, grid);
      }
    }
    (function(self){
      // Scramble depth: enough to fully mix any cube, without the absurd range
      // that made Solve a guessing game. Cube size can still go big (visual).
      var sd = self.root.querySelector('[data-scramble]'); if (sd){ sd.max = 40; }
      var ns = self.root.querySelector('[data-nslider]'); if (ns){ ns.max = 1000; }
      // The Calm/Lively swarm-speed toggle and the swarm count (16/36/…) selector
      // don't add anything — hide them; the swarm just shows the whole wall.
      self.root.querySelectorAll('[data-sspeed]').forEach(function(b){ var grp=b.parentElement; if(grp) grp.style.display='none'; });
      self.root.querySelectorAll('[data-count]').forEach(function(b){ var grp=b.parentElement; if(grp) grp.style.display='none'; });
    })(inst);
    wireExplanations(inst);

    // ---------- Solvability guidance: always say what Solve will do ----------
    // SOLVE_MAX_DEPTH: reach of the legacy search (2x2). SOLVE_REAL_DEPTH: the 3x3
    // now uses the two-phase (Kociemba) solver, which cracks ANY scramble.
    var SOLVE_MAX_N = 11, SOLVE_MAX_DEPTH = 9, SOLVE_REAL_DEPTH = 30;

    // Make the "Solver Race" panel honest about which engine actually runs:
    //   • 2×2 — three real engines race (meet-in-the-middle, beam, island genetic);
    //   • 3×3 — the two-phase (Kociemba) solver, shown as one near-optimal lane with
    //           the other two hidden (they don't run for the 3×3);
    //   • N>3 — no real search (visual playback), labels restored.
    function laneLabelEl(el){ return (el && el.firstElementChild) ? el.firstElementChild.children[1] : null; }
    function applyEngineLabels(self){
      var n = self.n || 3;
      if (!self._origLane){
        self._origLane = {};
        ['det','beam','evo'].forEach(function(k){
          var el = self.root.querySelector('[data-lane="'+k+'"]');
          var lab = laneLabelEl(el);
          var st = el && el.querySelector('[data-stat]');
          self._origLane[k] = { label: lab ? lab.textContent : '', stat: st ? st.textContent : '' };
        });
      }
      var twoPhase = (n === 3);
      // Hide the two non-running lanes on the 3×3; show all three otherwise.
      ['beam','evo'].forEach(function(k){
        var el = self.root.querySelector('[data-lane="'+k+'"]'); if (el) el.style.display = twoPhase ? 'none' : '';
      });
      var detEl = self.root.querySelector('[data-lane="det"]');
      var detLab = laneLabelEl(detEl);
      var detDesc = detEl && detEl.querySelector('[data-lane-desc]');
      if (detLab) detLab.textContent = twoPhase ? 'Two-phase (Kociemba)' : (self._origLane.det.label || 'Meet-in-the-middle');
      if (detDesc) detDesc.textContent = twoPhase
        ? 'Orients corners and edges into the UD-slice subgroup, then solves the permutation within it — a near-optimal solution for any 3×3 scramble.'
        : 'Searches forward from the scrambled state and backward from solved until the paths meet — exact & shortest.';
      var head = self.root.querySelector('[data-race-head]');
      var sub = self.root.querySelector('[data-race-sub]');
      if (head) head.textContent = twoPhase ? 'BEST-PATH SOLVER' : 'SOLVER RACE';
      if (sub) sub.textContent = twoPhase
        ? 'two-phase solver — near-optimal, never the scramble inverse'
        : '3 strategies on the cube state — none see the scramble';
      // Clear any stale per-lane result (e.g. when switching N) so a previous
      // solve's "verified · N moves ★" doesn't linger. Never touch lanes mid-solve.
      if (!self.busy){
        ['det','beam','evo'].forEach(function(k){
          var el = self.root.querySelector('[data-lane="'+k+'"]'); if (!el) return;
          var st = el.querySelector('[data-stat]'), fl = el.querySelector('[data-fill]'),
              pc = el.querySelector('[data-pct2]'), sr = el.querySelector('[data-star]');
          if (st) st.textContent = self._origLane[k].stat;
          if (fl) fl.style.width = '0%';
          if (pc) pc.textContent = '0%';
          if (sr) sr.style.display = 'none';
        });
      }
    }
    inst._applyEngineLabels = applyEngineLabels;

    function refreshSolvability(self){
      applyEngineLabels(self);
      var ban = self.root.querySelector('[data-solvability]'); if(!ban) return;
      var n = self.n||3, d = self.scrambleDepth||6;
      // Cap the scramble-depth slider to the exact solver's reach for solvable
      // cubes — past SOLVE_MAX_DEPTH the solve just returns "no solution". Visual
      // cubes keep the full deep-scramble range.
      var sd = self.root.querySelector('[data-scramble]');
      if (sd){
        var cap = (n > SOLVE_MAX_N) ? 40 : (n >= 3 ? SOLVE_REAL_DEPTH : SOLVE_MAX_DEPTH);
        if (+sd.max !== cap){
          sd.max = cap;
          if (+sd.value > cap){ sd.value = cap; sd.dispatchEvent(new Event('input', { bubbles: true })); d = self.scrambleDepth || cap; }
        }
      }
      if (n > SOLVE_MAX_N){
        ban.style.cssText = ban._base + 'background:#FBF4E8;color:#9A6A1E;border-color:#F0E0C2;';
        ban.innerHTML = '<b>'+n+'×'+n+' is visual.</b> 2×2 through '+SOLVE_MAX_N+'×'+SOLVE_MAX_N+' are solved for real; bigger cubes scramble fully and Solve plays back a visual demo.';
      } else if (d > SOLVE_MAX_DEPTH){
        ban.style.cssText = ban._base + 'background:#FBF4E8;color:#9A6A1E;border-color:#F0E0C2;';
        ban.innerHTML = '<b>Deep scramble (depth '+d+').</b> The exact solver may run out of budget. Keep depth ≤ '+SOLVE_MAX_DEPTH+' for a guaranteed solve.';
      } else {
        ban.style.cssText = ban._base + 'background:#F1F8F0;color:#3F7A3A;border-color:#D9EAD5;';
        ban.innerHTML = '<b>Ready to solve.</b> Press <b>Scramble</b>, then <b>Solve</b> — the engines below find a verified solution and play it back.';
      }
    }
    inst._refreshSolvability = refreshSolvability;
    (function(self){
      var solveBtn = self.root.querySelector('[data-onclick="onSolve"]');
      if (solveBtn && solveBtn.parentNode && !self.root.querySelector('[data-solvability]')){
        var ban = document.createElement('div'); ban.setAttribute('data-solvability','');
        ban._base = 'font-size:12px;line-height:1.5;border:1px solid;border-radius:12px;padding:10px 12px;margin:0 0 12px;';
        solveBtn.parentNode.insertBefore(ban, solveBtn);
      }
      // Cancel button (shown only while a solve is running in the worker).
      if (solveBtn && solveBtn.parentNode && !self.root.querySelector('[data-cancel-solve]')){
        var cb = document.createElement('button'); cb.setAttribute('data-cancel-solve',''); cb.type='button';
        cb.textContent = '✕  Cancel solve';
        cb.style.cssText = 'display:block;width:100%;height:42px;margin-top:10px;border-radius:13px;border:1px solid #E6C9C9;background:#FCF1F1;color:#A23B3B;font-size:13px;font-weight:600;cursor:pointer;';
        cb.addEventListener('click', function(){ cancelSolve(self); });
        solveBtn.parentNode.insertBefore(cb, solveBtn.nextSibling);
      }
      ['4','5','7','20','50','100','500','1000'].forEach(function(v){
        var b = self.root.querySelector('[data-n="'+v+'"]');
        if(b){ b.style.opacity='0.5'; b.title='Visual only — solved for real on 2×2 through '+SOLVE_MAX_N+'×'+SOLVE_MAX_N; }
      });
      var sd2 = self.root.querySelector('[data-scramble]');
      if (sd2) sd2.addEventListener('input', function(){ setTimeout(function(){ refreshSolvability(self); }, 0); });
      refreshSolvability(self);
    })(inst);
  }

  function markLive(){
    // Remove the corner status pill entirely (the design's "Simulated demo" badge).
    document.querySelectorAll('span').forEach(s=>{
      if (s.textContent && s.textContent.trim() === 'Simulated demo'){
        s.style.display = 'none';
      }
    });
  }
'''

BOOT = r'''
  function bindAndMount(){
    const props = { autoSpin: true, accent: '#161514', palette: 'Classic' };
    const inst = new Component(props);
    const vals = inst.renderVals ? inst.renderVals() : {};
    document.querySelectorAll('[data-dcref]').forEach(el => {
      const fn = vals[el.getAttribute('data-dcref')];
      if (typeof fn === 'function') fn(el);
    });
    const evmap = { 'data-onclick':'click', 'data-onchange':'input' };
    Object.keys(evmap).forEach(attr => {
      document.querySelectorAll('['+attr+']').forEach(el => {
        const fn = vals[el.getAttribute(attr)];
        if (typeof fn === 'function') el.addEventListener(evmap[attr], fn);
      });
    });
    // Some webviews (esp. WebKitGTK on Linux VMs/headless) can't create a WebGL
    // context. Without this guard the thrown error aborts mount and the WHOLE app
    // is dead. Catch it, stub the renderer, and keep the solver UI usable.
    if (typeof inst.initThree === 'function'){
      const origInitThree = inst.initThree.bind(inst);
      inst.initThree = function(){
        try { origInitThree(); }
        catch(e){
          console.warn('WebGL unavailable — 3D view disabled; the solver still works.', e);
          const T = window.THREE; this._noWebGL = true;
          this.renderer = this.renderer || { render:function(){}, setSize:function(){}, setPixelRatio:function(){}, domElement: document.createElement('canvas') };
          if (T){ this.scene = this.scene || new T.Scene(); this.camera = this.camera || new T.PerspectiveCamera(50,1,0.1,100); this.cubeGroup = this.cubeGroup || new T.Group(); }
          const stage = this.root && this.root.querySelector('[data-stage]');
          if (stage){ const m=document.createElement('div'); m.style.cssText='position:absolute;inset:0;display:flex;align-items:center;justify-content:center;text-align:center;color:#8C887F;font-size:13px;padding:24px;'; m.innerHTML='3D view unavailable (no WebGL on this system).<br>Scramble &amp; Solve still work.'; stage.appendChild(m); }
        }
      };
    }
    if (typeof inst.componentDidMount === 'function') inst.componentDidMount();
    return inst;
  }
  async function boot(){
    const inst = bindAndMount();
    window.__cubeLab = inst;
    try {
      await init();
      inst.lab = new CubeLab(inst.n);
      wireRealSolver(inst);
      markLive();
    } catch (e){
      console.warn('WASM failed to load — running the simulated demo.', e);
    }
  }
  boot();
'''

html = ('<!DOCTYPE html>\n<html lang="en">\n<head>\n'
        '<meta charset="utf-8">\n<meta name="viewport" content="width=device-width, initial-scale=1">\n'
        '<title>Cube Solver — N×N Solver Studio</title>\n'
        + helmet + '\n</head>\n<body>\n' + body + '\n'
        '<script type="module">\n'
        "import init, { CubeLab, Swarm } from './pkg/cube_wasm.js';\n"
        + SHIM + '\n' + js + '\n' + WIRE + '\n' + BOOT + '\n</script>\n</body>\n</html>\n')

out = os.path.join(_ROOT, 'web', 'index.html')
open(out,'w').write(html)
print("wrote", out, len(html), "bytes")
