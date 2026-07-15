#!/usr/bin/env python3
# Regenerates web/index.html: wraps the design component (design-source.txt) with
# the real Rust/WASM solver wiring. Run from anywhere: `python3 tools/gen-index.py`.
import json, re, os, sys

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
_texture_status = "if (solving) { this.updateLanes(this.texProg); this.ui.status.textContent = 'Solving · reduction sweep'; }\n    else this.ui.status.textContent = 'Scrambling';"
assert _texture_status in js
js = js.replace(_texture_status, "if (solving) { this.updateLanes(this.texProg); if (this.ui.status.textContent !== 'Playing visual demo') this.ui.status.textContent = 'Playing visual demo'; }\n    else if (this.ui.status.textContent !== 'Scrambling') this.ui.status.textContent = 'Scrambling';")
_scramble_status = "if (cur) { this.ui.status.textContent = 'Scrambling';"
assert _scramble_status in js
js = js.replace(_scramble_status, "if (cur) { if (this.ui.status.textContent !== 'Scrambling') this.ui.status.textContent = 'Scrambling';")
_replay_status = "if (cur) { this.ui.status.textContent = 'Solving';"
assert _replay_status in js
js = js.replace(_replay_status, "if (cur) { const solveStatus = this._lastSolveTelemetry?.verified ? 'Replaying verified solution' : 'Solving'; if (this.ui.status.textContent !== solveStatus) this.ui.status.textContent = solveStatus;")

body = re.sub(r'ref="\{\{\s*rootRef\s*\}\}"', 'data-dcref="rootRef"', body)
body = re.sub(r'onClick="\{\{\s*(\w+)\s*\}\}"', r'data-onclick="\1"', body)
body = re.sub(r'onChange="\{\{\s*(\w+)\s*\}\}"', r'data-onchange="\1"', body)
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
    const KEYS = { deterministic:'det', beam:'beam', evolution:'evo', kociemba:'det', reduction:'det' };
    const origScramble = inst.scramble.bind(inst);
    const origSolve = inst.solve.bind(inst);
    const origReset = inst.resetSolved ? inst.resetSolved.bind(inst) : null;
    const origSetN = inst.setN ? inst.setN.bind(inst) : null;
    const origReplay = inst.replay ? inst.replay.bind(inst) : null;
    const origSetView = inst.setView ? inst.setView.bind(inst) : null;
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
      this.scrambleMoveCount = 0;
      this._lastSolveTelemetry = null;
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

    function secureBelow(limit){
      if (limit <= 1) return 0;
      if (globalThis.crypto && globalThis.crypto.getRandomValues){
        var buf = new Uint32Array(1);
        var ceiling = 0x100000000 - (0x100000000 % limit);
        do { globalThis.crypto.getRandomValues(buf); } while (buf[0] >= ceiling);
        return buf[0] % limit;
      }
      return Math.floor(Math.random() * limit); // legacy webview fallback
    }

    inst.scramble = function(){
      if (this.busy) return;
      if (this.mode !== 'cubie') return origScramble();   // huge (texture) cubes: design path
      this.resetSolved(true);
      this._lastSolveTelemetry = null;
      const N = this.n;
      // 2×2/3×3 use outer turns. Supported 4×4+ cubes use standard contiguous
      // wide turns from either face, mixing inner layers while staying inside the
      // replay corpus exercised by the deterministic reduction solver. Bigger
      // visualization-only cubes may use arbitrary single slices.
      const solvable = (N <= SOLVE_MAX_N) && !!this.lab;
      // Never scramble a solvable cube deeper than the exact solver's reach, or
      // Solve would search to SOLVE_MAX_DEPTH and find no solution.
      const depth = solvable ? Math.min(this.scrambleDepth, N >= 3 ? SOLVE_REAL_DEPTH : SOLVE_MAX_DEPTH) : this.scrambleDepth;
      this.lastScramble = [];
      let prev = -1;
      for (let i=0;i<depth;i++){
        // Adjacent turns never share an axis, avoiding trivial cancellations and
        // making every generated challenge a stronger, independently random state.
        let axis = prev < 0 ? secureBelow(3) : secureBelow(2);
        if (prev >= 0 && axis >= prev) axis++;
        prev = axis;
        const dir = secureBelow(2) ? 1 : -1;
        if (solvable && N >= 4){
          const width = 1 + secureBelow(Math.max(1, Math.floor(N/2)));
          const start = secureBelow(2) ? N-width : 0;
          for (let layer=start; layer<start+width; layer++) this.lastScramble.push({axis, layer, dir});
        } else {
          const layer = (N <= 3) ? (secureBelow(2) ? N-1 : 0) : secureBelow(N);
          this.lastScramble.push({axis, layer, dir});
        }
      }
      this.scrambleMoveCount = depth;
      // Apply the whole scramble instantly — fast at any depth, no stuck queue.
      for (const m of this.lastScramble) this.applyInstant(m);
      // Mirror into the solver's cube only when it can actually solve it (N<=SOLVE_MAX_N).
      if (solvable){ this.lab.reset(); for (const m of this.lastScramble) this.lab.apply_design_move(m.axis, m.layer, m.dir); }
      this.queue = []; this.activeMove = null;
      this.movesDone = depth; this.totalMoves = depth; this.solveProgress = 0;
      this.phase = 'idle'; this.busy = false; this.scrambled = true;
      if (this.setSolvedPct) this.setSolvedPct(0);
      if (this.resetLanes) this.resetLanes();
      if (this.ui && this.ui.status) this.ui.status.textContent = 'Scrambled — ready to solve';
      if (this.ui && this.ui.count) this.ui.count.style.display = 'none';
      if (this.ui && this.ui.move) this.ui.move.style.display = 'none';
      if (this.syncControls) this.syncControls();
    };

    // A user can click Swarm during the brief async WASM boot window, when the
    // authored scramble method is still installed. Reconstruct that exact target
    // state in CubeLab before any post-wire solve reads sticker colours.
    function syncLabFromVisibleScramble(self){
      if (!self.lab) return;
      self.lab.set_size(self.n);
      self.lab.reset();
      if (self.scrambled) (self.lastScramble||[]).forEach(function(m){ self.lab.apply_design_move(m.axis, m.layer, m.dir); });
    }
    syncLabFromVisibleScramble(inst);

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

    function nativeCore(){
      return globalThis.__TAURI__ && globalThis.__TAURI__.core && globalThis.__TAURI__.core.invoke
        ? globalThis.__TAURI__.core : null;
    }
    function hasNativeReduction(self){ return !!nativeCore() && (self.n||0) >= 4 && (self.n||0) <= 11; }
    function solveRequestToken(){
      if (globalThis.crypto && typeof globalThis.crypto.randomUUID === 'function') return globalThis.crypto.randomUUID();
      var parts=[]; for(var i=0;i<4;i++) parts.push(secureBelow(0x100000000).toString(16).padStart(8,'0'));
      return Date.now().toString(16)+'-'+parts.join('');
    }
    function elapsedLabel(ms){
      var seconds = Math.max(0, Math.floor(ms / 1000));
      return seconds < 60 ? (seconds + 's') : (Math.floor(seconds/60) + 'm ' + (seconds%60) + 's');
    }
    function refreshSolveActivity(self){
      if (!self._solvePending) return;
      var elapsed = performance.now() - (self._solveStartedAt || performance.now());
      self._solveElapsedMs = elapsed;
      var engine = self._solveDispatch === 'native' ? 'Native reduction' :
                   self._solveDispatch === 'worker' ? 'WASM solver' : 'Solver';
      var statusKey = engine+':'+self.n;
      if (self.ui && self.ui.status && self._solveStatusKey !== statusKey){
        self._solveStatusKey = statusKey;
        self.ui.status.textContent = engine + ' active on ' + self.n + '×' + self.n + ' · Cancel available';
      }
      var badge = self.root.querySelector('[data-solve-elapsed]');
      if (!badge && self.ui && self.ui.status){
        badge=document.createElement('span'); badge.setAttribute('data-solve-elapsed',''); badge.setAttribute('aria-hidden','true');
        badge.style.cssText='margin-left:7px;font:600 10.5px/1.2 ui-monospace,SFMono-Regular,monospace;color:#1573E6;background:#EAF3FF;border-radius:999px;padding:3px 7px;';
        self.ui.status.insertAdjacentElement('afterend', badge);
      }
      if (badge){ badge.style.display='inline-block'; badge.textContent=elapsedLabel(elapsed); }
      var travel = self.reducedMotion ? 0 : ((Math.floor(elapsed/500)%4) * 80);
      var lane = self.root.querySelector('[data-lane="det"]');
      if (lane){
        lane.removeAttribute('aria-valuenow'); lane.setAttribute('aria-valuetext','Solver active');
        var stat=lane.querySelector('[data-stat]'), fill=lane.querySelector('[data-fill]'), pct=lane.querySelector('[data-pct2]');
        if (stat) stat.textContent = 'working from sticker state · ' + elapsedLabel(elapsed);
        if (pct){ pct.textContent = 'LIVE'; pct.style.color = '#1573E6'; }
        if (fill){ fill.style.width='28%'; fill.style.opacity='0.65'; fill.style.transform='translateX('+travel+'%)'; fill.style.transition='transform .5s linear'; }
      }
      var progress=self.root.querySelector('[data-progress]');
      if (progress){
        if (progress.parentElement) progress.parentElement.style.overflow='hidden';
        progress.removeAttribute('aria-valuenow'); progress.setAttribute('aria-valuetext','Solver active');
        progress.style.width='28%'; progress.style.transform='translateX('+travel+'%)'; progress.style.transition='transform .5s linear';
      }
    }
    function startSolveActivity(self){
      self._solveStartedAt = performance.now();
      clearInterval(self._solveHeartbeat);
      refreshSolveActivity(self);
      self._solveHeartbeat = setInterval(function(){ refreshSolveActivity(self); }, 500);
    }
    function stopSolveActivity(self){
      clearInterval(self._solveHeartbeat);
      self._solveHeartbeat = null;
      self._solveElapsedMs = self._solveStartedAt ? performance.now() - self._solveStartedAt : 0;
      self._solveStatusKey = null;
      var badge=self.root.querySelector('[data-solve-elapsed]'); if (badge) badge.style.display='none';
      var lane=self.root.querySelector('[data-lane="det"]');
      if (lane){ lane.removeAttribute('aria-valuetext'); var fill=lane.querySelector('[data-fill]'); if(fill){ fill.style.opacity='1'; fill.style.transform='none'; } }
      var progress=self.root.querySelector('[data-progress]');
      if (progress){ progress.removeAttribute('aria-valuetext'); progress.style.transform='none'; progress.style.transition='width .12s linear'; progress.style.width='0%'; progress.setAttribute('aria-valuenow','0'); }
    }
    function postNativeSolve(self){
      var core = nativeCore();
      if (!core || !self._solvePending || self._solveDispatch) return false;
      self._solveDispatch = 'native';
      showCancel(self, true);
      startSolveActivity(self);
      var jobId = self._solveJobId, requestToken = self._solveRequestToken;
      var colors = Array.from(self.lab.face_colors(self.n));
      core.invoke('solve_stickers', { requestToken:requestToken, n:self.n, colors:colors }).then(function(result){
        onSolveResult(self, { type:'result', jobId:jobId, ok:true, result:result });
      }).catch(function(error){
        onSolveResult(self, { type:'result', jobId:jobId, ok:false, error:String(error) });
      });
      return true;
    }

    inst.solve = function(){
      if (this.mode !== 'cubie'){ this._visualSolve=true; return origSolve(); }   // texture cubes: honest visual path
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
      // Real solve for every supported size. _solvePending (not busy) blocks
      // re-entry without letting the animation loop finish an empty queue.
      this._visualSolve = false;
      this._solvePending = true;
      this._solveDispatch = null;
      this._solveJobId = (this._solveJobId || 0) + 1;
      this._solveRequestToken = solveRequestToken();
      var solveJobId = this._solveJobId;
      // Reduction grows quickly with N. Keep a watchdog, but size it from measured
      // release behavior so a valid 9×9–11×11 solve is not killed at eight seconds.
      var _sw = this;
      var watchdogMs = this.n <= 3 ? 8000 : this.n <= 5 ? 30000 : this.n <= 8 ? 120000 : 300000;
      clearTimeout(this._solveWatchdog);
      this._solveWatchdog = setTimeout(function(){
        if (!_sw._solvePending || _sw._solveJobId !== solveJobId) return;
        abortPendingSolve(_sw);
        _sw._lastSolveTelemetry = { n:_sw.n, elapsedMs:_sw._solveElapsedMs||watchdogMs, verified:false, error:'Solve timed out' };
        if (_sw.ui && _sw.ui.status) _sw.ui.status.textContent = 'Solve timed out — try a smaller cube or shallower scramble';
        if (_sw.syncControls) _sw.syncControls();
      }, watchdogMs);
      if (this.ui && this.ui.status) this.ui.status.textContent = 'Starting solver…';
      if (this.startLanes) this.startLanes();
      if (this.syncControls) this.syncControls();
      if (hasNativeReduction(this)){
        postNativeSolve(this);
      } else {
        var worker = ensureWorker(this);
        if (this._workerReady && worker){
          postWorkerSolve(this);
        } else if (!worker) {
          handleWorkerFailure(this);
        } else {
          showCancel(this, true);
          if (this.ui && this.ui.status) this.ui.status.textContent = 'Loading solver worker…';
          clearTimeout(this._workerLoadTimer);
          this._workerLoadTimer = setTimeout(function(){
            if (!_sw._solvePending || _sw._workerReady) return;
            if (_sw.n <= 3) runSmallFallback(_sw);
            else handleWorkerFailure(_sw);
          }, 1500);
        }
      }
    };

    // ---- Web Worker solve plumbing (responsive + cancellable) ----
    function postWorkerSolve(self){
      if (!self._solvePending || self._solveDispatch || !self._workerReady || !self._worker) return;
      self._solveDispatch = 'worker';
      clearTimeout(self._workerLoadTimer);
      showCancel(self, true);
      startSolveActivity(self);
      // Send only the visible sticker state. The worker never receives the
      // scramble sequence, so its verified path cannot be a hidden inverse.
      var colors = self.lab.face_colors(self.n);
      self._worker.postMessage({ type:'solve', jobId:self._solveJobId, n:self.n, depth:self.scrambleDepth||6, time:6000, colors:colors });
    }

    function runSmallFallback(self){
      if (!self._solvePending || self._solveDispatch) return;
      self._solveDispatch = 'fallback';
      clearTimeout(self._workerLoadTimer);
      startSolveActivity(self);
      var jobId = self._solveJobId;
      self._fallbackTimer = setTimeout(function(){
        self._fallbackTimer = null;
        if (!self._solvePending || self._solveJobId !== jobId) return;
        var r; try { r = self.lab.solve(Math.min(self.scrambleDepth||6, 9), 1500); } catch(e){ r = '{"found":false}'; }
        onSolveResult(self, { type:'result', jobId:jobId, ok:true, result:r });
      }, 0);
    }

    function handleWorkerFailure(self){
      clearTimeout(self._workerLoadTimer);
      self._workerReady = false;
      self._workerBroken = true;
      if (self._worker){ try { self._worker.terminate(); } catch(e){} }
      self._worker = null;
      if (!self._solvePending) return;
      self._solveDispatch = null;
      if (self.n <= 3){ runSmallFallback(self); return; }
      clearTimeout(self._solveWatchdog);
      stopSolveActivity(self);
      self._lastSolveTelemetry = { n:self.n, elapsedMs:self._solveElapsedMs||0, verified:false, error:'Solver worker unavailable' };
      self._solvePending = false;
      self._solveRequestToken = null;
      self._solveDispatch = null;
      showCancel(self, false);
      if (self.ui && self.ui.status) self.ui.status.textContent = 'Solver worker unavailable — reduction was not run on the UI thread';
      if (self.syncControls) self.syncControls();
    }

    function ensureWorker(self){
      if (self._workerBroken) return null;
      if (self._worker) return self._worker;
      try {
        var w = new Worker('./solver-worker.js', { type:'module' });
        w.onmessage = function(e){
          if (self._worker !== w) return;
          var d = e.data || {};
          if (d.type === 'ready'){
            self._workerReady = true;
            postWorkerSolve(self);
            return;
          }
          if (d.type === 'error'){ handleWorkerFailure(self); return; }
          if (d.type === 'result'){ onSolveResult(self, d); }
        };
        w.onerror = function(){ if (self._worker === w) handleWorkerFailure(self); };
        self._worker = w;
      } catch(e){
        self._worker = null;
        handleWorkerFailure(self);
      }
      return self._worker;
    }
    // Stop an in-flight worker solve or a not-yet-started fallback. Returns true
    // when an operation was pending; a synchronous fallback already executing
    // cannot receive events until it returns, but duplicate dispatch is prevented.
    function abortPendingSolve(self){
      if (!self._solvePending) return false;
      var dispatch = self._solveDispatch, requestToken = self._solveRequestToken;
      clearTimeout(self._solveWatchdog);
      clearTimeout(self._workerLoadTimer);
      clearTimeout(self._fallbackTimer);
      stopSolveActivity(self);
      if (dispatch === 'native' && nativeCore()){
        nativeCore().invoke('cancel_solve', { requestToken:requestToken }).catch(function(){});
      }
      self._solvePending = false;
      self._solveRequestToken = null;
      self._solveDispatch = null;
      self._solveJobId = (self._solveJobId || 0) + 1;
      var terminatedWorker = false;
      if (dispatch !== 'native' && self._worker){
        try { self._worker.terminate(); } catch(e){}
        self._worker = null; self._workerReady = false; terminatedWorker = true;
      }
      showCancel(self, false);
      if (terminatedWorker) ensureWorker(self);
      return true;
    }
    function onSolveResult(self, data){
      if (!self._solvePending || !data || data.jobId !== self._solveJobId) return;
      clearTimeout(self._solveWatchdog);
      clearTimeout(self._workerLoadTimer);
      clearTimeout(self._fallbackTimer);
      stopSolveActivity(self);
      var elapsedMs = self._solveElapsedMs || 0;
      self._solvePending = false;
      self._solveRequestToken = null;
      self._solveDispatch = null;
      showCancel(self, false);
      var res = null;
      if (data && data.ok){ try { res = JSON.parse(data.result); } catch(e){} }
      if (!res || !res.found || !res.moves || !res.moves.length){
        self._lastSolveTelemetry = { n:self.n, elapsedMs:elapsedMs, verified:false, error:(data && data.error) || 'No verified solution within budget' };
        if (self.ui && self.ui.status) self.ui.status.textContent = data && data.error
          ? ('Solver stopped: ' + data.error)
          : 'No verified solution within budget — lower the scramble depth';
        lanesUpdate(self, (res && res.lanes) || [], null, 0);
        if (self.syncControls) self.syncControls();
        return;
      }
      self._lastSolveTelemetry = { n:self.n, elapsedMs:elapsedMs, verified:true, moveCount:res.moveCount, winner:res.winner };
      self.realWinner = res.winner; self.realLanes = res.lanes || [];
      self.lastSolution = res.moves;
      // res.moveCount is the face-turn (HTM) count; res.moves is the half-turn-
      // expanded animation list. Keep the HTM count + notation for the lane/proof/
      // solution panel so all three agree (the panel must show U2, not "U U").
      self.realMoveCount = res.moveCount;
      self.realNotation = res.notation || null;
      updateProof(self, self.scrambleMoveCount || (self.lastScramble||[]).length, res.moveCount);
      self.queue = res.moves.slice();   // fill the queue BEFORE claiming busy
      self.movesDone = 0; self.totalMoves = self.queue.length;
      self.phase = 'solving'; self.busy = true;
      if (self.ui && self.ui.status) self.ui.status.textContent = 'Verified in ' + elapsedLabel(elapsedMs) + ' — replaying solution';
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
      if (hadSearch) self._lastSolveTelemetry = { n:self.n, elapsedMs:self._solveElapsedMs||0, verified:false, error:'Solve cancelled' };
      if (self.resetLanes) self.resetLanes();
      if (self.ui && self.ui.status) self.ui.status.textContent = 'Solve cancelled — scramble or solve again';
      if (self.syncControls) self.syncControls();
    }
    // Solve is disabled for the whole solve (search + replay). Cancel is only
    // exposed while there is an operation it can actually stop.
    function updateSolveButtons(self){
      var solving = !!self._solvePending || (self.busy && self.phase === 'solving');
      var solveBtn = self.root.querySelector('[data-onclick="onSolve"]');
      // Solve is greyed ONLY while a solve is in progress. Otherwise it stays
      // clickable even on a solved/fresh cube (it scrambles first), so it never
      // silently does nothing. This overrides the design's 'grey when unscrambled'.
      if (solveBtn){
        solveBtn.disabled = solving;
        solveBtn.setAttribute('aria-disabled', String(solving));
        solveBtn.style.opacity = solving ? '0.45' : '1';
        solveBtn.style.pointerEvents = solving ? 'none' : 'auto';
        solveBtn.style.cursor = solving ? 'default' : 'pointer';
      }
      var cb = self.root.querySelector('[data-cancel-solve]');
      if (cb){
        cb.disabled = !solving;
        cb.setAttribute('aria-disabled', String(!solving));
        cb.style.display = solving ? 'block' : 'none';
        cb.style.opacity = solving ? '1' : '0';
        cb.style.pointerEvents = solving ? 'auto' : 'none';
        cb.style.cursor = solving ? 'pointer' : 'default';
      }
    }
    // Kept for existing call sites — state is derived from _solvePending/busy.
    function showCancel(self){ updateSolveButtons(self); }
    // Re-sync the Solve/Cancel buttons on every UI state change the design makes.
    if (inst.syncControls){
      var origSyncC = inst.syncControls.bind(inst);
      inst.syncControls = function(){ origSyncC(); updateSolveButtons(this); };
    }
    var origUnmount = inst.componentWillUnmount ? inst.componentWillUnmount.bind(inst) : null;
    inst.componentWillUnmount = function(){
      clearTimeout(this._solveWatchdog);
      clearTimeout(this._workerLoadTimer);
      clearTimeout(this._fallbackTimer);
      clearTimeout(this._swarmAutoStartTimer);
      clearInterval(this._solveHeartbeat);
      if (this._solvePending && this._solveDispatch === 'native' && nativeCore()){
        nativeCore().invoke('cancel_solve', { requestToken:this._solveRequestToken }).catch(function(){});
      }
      if (this._worker){ try { this._worker.terminate(); } catch(e){} this._worker=null; }
      if (origUnmount) origUnmount();
    };
    ensureWorker(inst);   // warm up the solver worker so it's ready before the first solve
    updateSolveButtons(inst);   // initial state: Solve enabled, Cancel hidden

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
        self.swarmLab = new Swarm(cnt, self.n||3, Math.max(3, self.scrambleDepth||6), BigInt(secureBelow(0x100000000))+1n);
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
      var note = this.root.querySelector('[data-swarm-note]');
      // Remember the grid's real display value so we can restore it (don't blank it).
      if (grid && this._gridDisp === undefined) this._gridDisp = grid.style.display || 'grid';
      // Truthful dual mode: evolutionary population cards for 2×2/3×3; live
      // reduction telemetry for 4×4–11×11. Never imply the GA solves big cubes.
      if ((this.n||3) > SWARM_MAX){
        this.swarmLab = null;
        if (grid) grid.style.display = 'none';
        var n = this.n||3, telem = this._lastSolveTelemetry;
        var replaying = !!(this.busy && this.phase === 'solving' && telem && telem.n === n && telem.verified);
        if (n > SOLVE_MAX_N){
          if (note) note.innerHTML = 'This size is <b>visualization-only</b> in the current platform. No search or reduction progress is fabricated.';
          if (liveEl && liveEl.textContent !== 'Visualization mode — choose a supported size for live solving') liveEl.textContent = 'Visualization mode — choose a supported size for live solving';
        } else {
          if (note) note.innerHTML = 'For 4×4–11×11 this view switches from evolutionary cards to <b>truthful deterministic-reduction telemetry</b>. It shows elapsed activity and replay proof without inventing node counts or percentages.';
          if (liveEl && liveEl.textContent !== 'Reduction telemetry — evolutionary search remains available on 2×2 and 3×3') liveEl.textContent = 'Reduction telemetry — evolutionary search remains available on 2×2 and 3×3';
        }
        if (msg){
          msg.style.display = 'block';
          var stateKey = n > SOLVE_MAX_N ? ('visual:'+n+':'+SOLVE_MAX_N) :
            this._solvePending ? ('active:'+n+':'+this._solveDispatch) :
            replaying ? ('replay:'+n+':'+telem.moveCount) :
            telem && telem.n === n && telem.verified ? ('verified:'+n+':'+telem.moveCount+':'+Math.round(telem.elapsedMs)) :
            telem && telem.n === n ? ('failed:'+n+':'+String(telem.error)) : ('ready:'+n);
          if (msg._telemetryState !== stateKey){
            msg._telemetryState = stateKey;
            var pipeline = '<div style="display:flex;flex-wrap:wrap;gap:6px;margin:14px 0 10px;">' +
              ['sticker state','centers','edge wings','reduced 3×3','independent replay'].map(function(s){ return '<span style="padding:5px 8px;border:1px solid #D9E2D5;border-radius:999px;background:#fff;font-size:11px;color:#5E6C59;">'+s+'</span>'; }).join('<span style="color:#A9B3A5;padding:5px 0;">→</span>') + '</div>';
            if (n > SOLVE_MAX_N){
              msg.innerHTML = '<b>'+n+'×'+n+' is visualization-only.</b><div style="margin-top:7px;">This build runs verified solves through '+SOLVE_MAX_N+'×'+SOLVE_MAX_N+'. Use 2×2/3×3 to watch the evolutionary population, or a supported reduction size to watch solver telemetry.</div>';
            } else if (this._solvePending){
              msg.innerHTML = '<div style="display:flex;align-items:center;gap:9px;color:#1573E6;"><span style="width:10px;height:10px;border-radius:50%;background:#1573E6;box-shadow:0 0 0 5px rgba(21,115,230,.12);"></span><b>'+
                (this._solveDispatch === 'native' ? 'Native reduction is active' : 'WASM solver is active') +
                '</b></div><div style="font-size:22px;font-weight:650;color:#262622;margin-top:12px;">'+n+'×'+n+' · <span data-reduction-elapsed aria-hidden="true">0s</span></div>' + pipeline +
                '<div style="font-size:12px;color:#6F756A;">Working from '+(6*n*n).toLocaleString()+' visible sticker colours. Progress percentages are intentionally omitted because the reduction stages have data-dependent costs.</div>'+
                '<button type="button" data-reduction-cancel style="margin-top:16px;padding:9px 13px;border:1px solid #E6C9C9;border-radius:10px;background:#FCF1F1;color:#A23B3B;font-weight:600;cursor:pointer;">Cancel reduction</button>';
            } else if (replaying){
              msg.innerHTML = '<div style="color:#1B8A45;font-weight:650;">✓ Verified solution is replaying</div><div style="font-size:22px;font-weight:650;color:#262622;margin-top:10px;">'+telem.moveCount+' moves · '+elapsedLabel(telem.elapsedMs)+'</div>' + pipeline +
                '<div data-replay-moves style="font-size:12px;color:#6F756A;">0 / '+this.totalMoves+' animation steps</div>'+
                '<button type="button" data-reduction-cancel style="margin-top:16px;padding:9px 13px;border:1px solid #E6C9C9;border-radius:10px;background:#FCF1F1;color:#A23B3B;font-weight:600;cursor:pointer;">Cancel playback</button>';
            } else if (telem && telem.n === n && telem.verified){
              msg.innerHTML = '<div style="color:#1B8A45;font-weight:650;">✓ Replay-verified reduction solve</div><div style="font-size:22px;font-weight:650;color:#262622;margin-top:10px;">'+telem.moveCount+' moves · '+elapsedLabel(telem.elapsedMs)+'</div>' + pipeline +
                '<div style="font-size:12px;color:#6F756A;">The returned legal moves were independently replayed to solved before animation began.</div>';
            } else if (telem && telem.n === n && !telem.verified){
              msg.innerHTML = '<div style="color:#A35A24;font-weight:650;">Reduction stopped without a verified path</div><div style="margin-top:8px;font-size:12px;color:#6F756A;">See the Studio status for details, then retry or choose a smaller cube.</div>' + pipeline;
            } else {
              msg.innerHTML = '<b>'+n+'×'+n+' uses deterministic reduction, not evolutionary search.</b><div style="margin-top:7px;">Start it here to watch live elapsed activity, the real reduction pipeline, and replay-verified results.</div>' + pipeline +
                '<button type="button" data-reduction-start style="margin-top:8px;padding:10px 14px;border:0;border-radius:10px;background:#161514;color:#fff;font-weight:600;cursor:pointer;">Start reduction</button>';
            }
          }
          var timer=msg.querySelector('[data-reduction-elapsed]');
          if (timer) timer.textContent=elapsedLabel(this._solveElapsedMs||0);
          var replay=msg.querySelector('[data-replay-moves]');
          if (replay) replay.textContent=(this.movesDone||0).toLocaleString()+' / '+(this.totalMoves||0).toLocaleString()+' animation steps';
          var start=msg.querySelector('[data-reduction-start]');
          if (start && !start._wired){ start._wired=true; var self=this; start.addEventListener('click', function(){ if (!self._solvePending && !self.busy) self.solve(); }); }
          var cancel=msg.querySelector('[data-reduction-cancel]');
          if (cancel && !cancel._wired){ cancel._wired=true; var self2=this; cancel.addEventListener('click', function(){ cancelSolve(self2); }); }
        }
        return;
      }
      if (grid) grid.style.display = this._gridDisp;
      if (msg) msg.style.display = 'none';
      if (note) note.innerHTML = 'Each cell is one <b>trial of the evolutionary solver</b>, and every trial starts as an exact copy of <b>your Studio cube</b>. They mutate and recombine until a trial reaches solved (it turns green, then restarts from the cube). Change the Studio scramble and the whole wall instantly re-syncs to it. The trials see only the cube state, never the scramble.';
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
        // Spin every frame (cheap); only re-read the WASM population metadata and
        // 54 sampled stickers when evolution advanced (or on first paint).
        var repaint = steps > 0 || !this._swPainted;
        var buf = repaint ? this.swarmLab.render() : null;
        var N = Math.min(this.cards.length, this.swarmLab.member_count());
        for (var i=0;i<N;i++){
          var c = this.cards[i];
          if (!this.reducedMotion){
            c.rot += dt * c.spin;
            if (c.inner) c.inner.style.transform = 'rotateX(-20deg) rotateY('+c.rot+'deg)';
          }
          if (!repaint) continue;
          var b = i*62, pct = buf[b];
          var mismatch = buf[b+1] | (buf[b+2] << 8);
          var genes = buf[b+3] | (buf[b+4] << 8);
          var stuck = buf[b+5];
          var op = ['seed','mutation','crossover','restart'][buf[b+7]] || 'search';
          // per-cell colour cache; reset if the design rebuilt this card's cells
          if (!c.last || c._lastCells !== c.cells){ c.last = new Int8Array(54).fill(-1); c._lastCells = c.cells; }
          for (var k=0;k<54;k++){
            var cf=(k/9)|0, cell=k%9, wf=FACE_FOR_CARD[cf];
            var ci = buf[b + 8 + wf*9 + cell];
            // only touch the DOM when a sticker's colour actually changes
            if (c.cells[k] && c.last[k] !== ci){ c.cells[k].style.background = SWPAL[ci] || SWPAL[0]; c.last[k] = ci; }
          }
          c.cring.style.strokeDashoffset = c.circ * (1 - pct/100);
          if (pct >= 100){
            c.cring.style.stroke = '#00A24B'; c.check.style.display='flex';
            c.cnt.textContent='solved · '+op; c.cnt.style.color='#1BA64B';
            c.card.style.borderColor='#CDEAD4'; c.card.style.boxShadow='0 4px 16px -6px rgba(27,166,75,.4)';
          } else {
            c.cring.style.stroke = this.accent; c.check.style.display='none';
            c.cnt.textContent = mismatch + ' off · ' + genes + ' genes · ' + (stuck ? 'plateau '+stuck : op); c.cnt.style.color='#8C887F';
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
      var stickerCount = 6 * self.n * self.n;
      body.innerHTML = 'The scramble move history was withheld from the worker. It received only the cube state ('+stickerCount+' sticker colours) and searched for its own verified path: '+cmp;
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
        msg.setAttribute('role','status'); msg.setAttribute('aria-live','polite');
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
    // Native Tauri reduction is the measured 4×4–11×11 desktop path. The
    // standalone browser build stays at the WASM-runtime-verified 5×5 ceiling.
    var SOLVE_MAX_N = nativeCore() ? 11 : 5, SOLVE_MAX_DEPTH = 9, SOLVE_REAL_DEPTH = 30;

    // Entering Swarm should visibly do work. Small cubes already launch genuine
    // evolutionary trials; supported large cubes auto-start truthful reduction.
    // Polling while an early, pre-WASM visual scramble finishes closes the mount
    // race where the user opens Swarm before wireRealSolver() is installed.
    function scheduleSwarmReduction(self){
      clearTimeout(self._swarmAutoStartTimer);
      self._swarmAutoStartTimer=null;
      if (self.view !== 'swarm' || (self.n||3) <= SWARM_MAX || (self.n||3) > SOLVE_MAX_N || self._solvePending || (self.busy && self.phase === 'solving')) return;
      var attempt=function(){
        self._swarmAutoStartTimer=null;
        if (self.view !== 'swarm' || (self.n||3) <= SWARM_MAX || (self.n||3) > SOLVE_MAX_N || self._solvePending || (self.busy && self.phase === 'solving')) return;
        if (self.busy){
          if (self.phase === 'scrambling') self._swarmAutoStartTimer=setTimeout(attempt, 120);
          return;
        }
        if (!self.scrambled) self.scramble();
        if (self.busy){ self._swarmAutoStartTimer=setTimeout(attempt, 120); return; }
        if (self.scrambled && !self._solvePending) self.solve();
      };
      self._swarmAutoStartTimer=setTimeout(attempt, 120);
    }
    if (origSetView) inst.setView = function(v){
      origSetView(v);
      scheduleSwarmReduction(this);
    };

    // Make the "Solver Race" panel honest about which engine actually runs:
    //   • 2×2 — three real engines race (meet-in-the-middle, beam, island genetic);
    //   • 3×3 — the two-phase (Kociemba) solver, shown as one near-optimal lane with
    //           the other two hidden (they don't run for the 3×3);
    //   • 4×4–11×11 — one reduction lane (centers → edges → 3×3 + parity);
    //   • N>11 — visual playback, honestly labeled as unsearched.
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
      var reduction = (n >= 4 && n <= SOLVE_MAX_N);
      var singleEngine = twoPhase || reduction;
      // Hide non-running lanes whenever a dedicated complete solver owns the size.
      ['beam','evo'].forEach(function(k){
        var el = self.root.querySelector('[data-lane="'+k+'"]'); if (el) el.style.display = singleEngine ? 'none' : '';
      });
      var detEl = self.root.querySelector('[data-lane="det"]');
      var detLab = laneLabelEl(detEl);
      var detDesc = detEl && detEl.querySelector('[data-lane-desc]');
      if (detLab) detLab.textContent = twoPhase
        ? 'Two-phase (Kociemba)'
        : reduction ? 'N×N reduction' : (self._origLane.det.label || 'Meet-in-the-middle');
      if (detDesc) detDesc.textContent = twoPhase
        ? 'Orients corners and edges into the UD-slice subgroup, then solves the permutation within it — a near-optimal solution for any 3×3 scramble.'
        : reduction
          ? 'Solves centers, pairs wing orbits, resolves parity, then finishes the reduced 3×3; every returned path is replay-verified.'
          : 'Searches forward from the scrambled state and backward from solved until the paths meet — exact & shortest.';
      var head = self.root.querySelector('[data-race-head]');
      var sub = self.root.querySelector('[data-race-sub]');
      if (head) head.textContent = twoPhase ? 'BEST-PATH SOLVER' : reduction ? 'REDUCTION SOLVER' : 'SOLVER RACE';
      if (sub) sub.textContent = twoPhase
        ? 'two-phase solver — near-optimal, never the scramble inverse'
        : reduction
          ? 'centers → wing pairing → reduced 3×3 + parity'
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
      } else if (n === 2 && d > SOLVE_MAX_DEPTH){
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
        cb.style.cssText = 'display:none;width:100%;height:42px;margin-top:10px;border-radius:13px;border:1px solid #E6C9C9;background:#FCF1F1;color:#A23B3B;font-size:13px;font-weight:600;cursor:pointer;';
        cb.addEventListener('click', function(){ cancelSolve(self); });
        solveBtn.parentNode.insertBefore(cb, solveBtn.nextSibling);
      }
      ['4','5','7','20','50','100','500','1000'].forEach(function(v){
        var b = self.root.querySelector('[data-n="'+v+'"]');
        if(!b) return;
        if(Number(v) > SOLVE_MAX_N){
          b.style.opacity='0.5'; b.title='Visual only — solved for real on 2×2 through '+SOLVE_MAX_N+'×'+SOLVE_MAX_N;
        } else {
          b.style.opacity='1'; b.title='Runs the real replay-verified solver';
        }
      });
      var sd2 = self.root.querySelector('[data-scramble]');
      if (sd2) sd2.addEventListener('input', function(){ setTimeout(function(){ refreshSolvability(self); }, 0); });
      refreshSolvability(self);
    })(inst);

    // componentDidMount starts before WASM finishes loading. Rebuild the wall
    // once the real Swarm constructor is available so an early WebGL failure or
    // mount race can never leave the Swarm tab blank/inert.
    try { if (inst.initSwarm) inst.initSwarm(); } catch(e){ console.warn('swarm remount failed', e); }
    function ensureAnimationLoop(self){
      if (self._raf || !window.THREE || !self.loop) return;
      if (!self.clock) self.clock = new window.THREE.Clock();
      self.loop();
    }
    ensureAnimationLoop(inst);
    scheduleSwarmReduction(inst);
  }

  function markLive(){}
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
          if (T){ this.scene = this.scene || new T.Scene(); this.camera = this.camera || new T.PerspectiveCamera(50,1,0.1,100); this.cubeGroup = this.cubeGroup || new T.Group(); this.clock = this.clock || new T.Clock(); }
          const stage = this.root && this.root.querySelector('[data-stage]');
          if (stage){ const m=document.createElement('div'); m.style.cssText='position:absolute;inset:0;display:flex;align-items:center;justify-content:center;text-align:center;color:#8C887F;font-size:13px;padding:24px;'; m.innerHTML='3D view unavailable (no WebGL on this system).<br>Scramble &amp; Solve still work.'; stage.appendChild(m); }
        }
      };
    }
    // componentDidMount calls buildCube immediately after initThree. If renderer
    // creation failed, the authored materials do not exist yet; skip only the 3D
    // build so mount, WASM loading, controls, and Swarm can still complete.
    function recoverNoWebGLBuild(self, n){
      self.N = n; self.mode = 'cubie';
      self.cubies = []; self.activeMove = null;
      if (self.updateBadges) self.updateBadges();
    }
    function useTextureBuildWithoutWebGL(self, n){ return n > (self.CUBIE_MAX || 11); }
    if (typeof inst.buildCube === 'function'){
      const origMountBuildCube = inst.buildCube.bind(inst);
      inst.buildCube = function(n){
        // The texture path is renderer-independent and initializes texFaces,
        // which scrambleTexture/advanceTexture require even when rendering is stubbed.
        if (!this._noWebGL || useTextureBuildWithoutWebGL(this, n)) return origMountBuildCube(n);
        recoverNoWebGLBuild(this, n);
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
      console.warn('WASM failed to load — solver unavailable; visualization only.', e);
      const status = inst.ui && inst.ui.status;
      if (status) status.textContent = 'Solver failed to load — visualization only';
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
if '--check' in sys.argv:
    current = open(out, encoding='utf-8').read() if os.path.exists(out) else None
    if current != html:
        print(f"stale generated frontend: run python3 tools/gen-index.py", file=sys.stderr)
        raise SystemExit(1)
    print("frontend is current:", out)
else:
    open(out, 'w', encoding='utf-8').write(html)
    print("wrote", out, len(html), "bytes")
