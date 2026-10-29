<script lang="ts">
  // One screen: the joystick (X/Y), the height stick (Z) and everything else beside them. Nothing here limits movement
  // except the controller's own floor and travel box.
  import { untrack } from 'svelte';
  import { app, refresh, errText } from '../lib/app.svelte';
  import { call } from '../lib/rpc';
  import { DEFAULT_JOY, fineStep, type Dir, type JoyOpts } from '../lib/joystick';
  import { initialSteps, marksOf, nextEdgeSteps, type Step } from '../lib/calibsteps';
  import Joystick from './Joystick.svelte';
  import ZSlider from './ZSlider.svelte';
  import LiveCamera from './LiveCamera.svelte';

  let { onclose }: { onclose: () => void } = $props();
  const st = $derived(app.st!);
  const frame = $derived(st.frame.active);
  const live = $derived(!!st.job?.live || st.phase === 'preparing' || st.job?.state === 'paused');
  const ready = $derived(st.controller.ready && !live);

  let pos = $state<[number, number, number] | null>(null);
  let inflight = false, zInflight = false;
  $effect(() => { const p = st.controller.pos; if (p && !inflight && !zInflight) pos = [p[0], p[1], p[2]]; });
  let note = $state('');
  let busy = $state(false);

  // ── X / Y ──
  let speed = $state(12);
  let flipX = $state(false), flipY = $state(false);
  try { flipX = localStorage.getItem('ctop-flipx') === '1'; flipY = localStorage.getItem('ctop-flipy') === '1'; } catch { /* storage unavailable */ }
  const joy = $derived<JoyOpts>({ ...DEFAULT_JOY, speedMmS: speed, flipX, flipY });
  async function jog(dx: number, dy: number, feed: number) {
    if (inflight || !ready) return;
    inflight = true;
    try { const r = await call<{ pos: [number, number, number] }>('jog', { dx, dy, feed, t: Date.now() }); pos = r.pos; note = ''; }
    catch (e) { const m = errText(e); if (m !== 'busy' && m !== 'stale') note = m; }
    finally { inflight = false; }
  }
  const stick = (dx: number, dy: number) => jog(dx, dy, Math.max(300, Math.min(2400, speed * 60 * 1.4)));
  let fine = $state(1);
  const nudge = (d: Dir) => { const s = fineStep(d, fine, { flipX, flipY }); jog(s.dx, s.dy, 600); };
  function setFlip(k: 'x' | 'y', v: boolean) { if (k === 'x') flipX = v; else flipY = v; try { localStorage.setItem('ctop-flip' + k, v ? '1' : '0'); } catch { /* ignore */ } }

  // ── Z: up = bigger number, down = smaller number ──
  let zTarget = $state<number | null>(null);
  async function zStep(dz: number) {
    if (zInflight || !ready) return;
    zInflight = true;
    try { const r = await call<{ pos: [number, number, number] }>('z_nudge', { dz, t: Date.now() }); pos = r.pos; zTarget = r.pos[2]; note = ''; }
    catch (e) { const m = errText(e); if (m !== 'busy' && m !== 'stale') note = m; }
    finally { zInflight = false; }
  }

  async function act<T>(cmd: string, args?: Record<string, unknown>, ok = ''): Promise<T | undefined> {
    busy = true; note = '';
    try { const r = await call<T>(cmd, args); if (ok) note = ok; return r; }
    catch (e) { note = errText(e); }
    finally { busy = false; refresh(); }
  }
  const penUpNow = async () => { const r = await act<{ pos: [number, number, number] }>('pen_raise'); if (r?.pos) pos = r.pos; };
  const estop = () => call('estop').catch((e) => (note = errText(e)));

  // ── pen height ──
  const saved0 = untrack(() => ({ c: frame?.surfaceZ ?? Number(st.settings.surfaceZ ?? 15.3), d: frame?.penDepth ?? Number(st.settings.depth ?? 0.6), u: frame?.penUpZ ?? Number(st.settings.travelZ ?? 30) }));
  let contactL = $state(saved0.c), depthL = $state(saved0.d), penUpL = $state(saved0.u);
  const penDownL = $derived(Math.round((contactL - depthL) * 100) / 100);
  const touches = () => { if (pos) { contactL = Math.round(pos[2] * 100) / 100; note = `Contact height ${contactL.toFixed(2)} mm (saved with Save pen height).`; } };
  const setDepth = (d: number) => { depthL = Math.round(Math.min(1, Math.max(0.1, d)) * 10) / 10; };
  const setPenUp = (u: number) => { penUpL = Math.max(1, Math.round(u)); };
  let lastDot = $state<{ x: number; y: number } | null>(null);
  let dots = $state<{ inked: boolean }[]>([]);
  async function testDot() {
    const r = await act<{ pos: [number, number, number]; downZ: number }>('dot', { contact: contactL, depth: depthL });
    if (r?.pos) { pos = r.pos; lastDot = { x: r.pos[0], y: r.pos[1] }; note = `Dot at Z ${r.downZ.toFixed(2)} mm.`; }
  }
  const inked = (v: boolean) => { dots.push({ inked: v }); lastDot = null; };
  const varies = $derived(dots.some((d) => d.inked) && dots.some((d) => !d.inked));
  let savedPen = $state('');
  async function savePen() {
    const r = await act('pen_save', { contact: contactL, depth: depthL, penUp: penUpL });
    if (r) savedPen = `Saved: contact ${contactL.toFixed(2)}, pen down ${penDownL.toFixed(2)}, pen up ${penUpL} mm.`;
  }

  // ── marking the sheet ──
  let steps = $state<Step[]>(initialSteps(null, null));
  let cur = $state(0);
  let result = $state<{ origin: [number, number]; skewDeg: number; rmsMm: number; warnings: string[] } | null>(null);
  let solveErr = $state('');
  let savedSheet = $state('');
  const advance = () => { const i = steps.findIndex((s) => s.status === 'todo'); cur = i < 0 ? steps.length : i; };
  const step = $derived(steps[cur]);
  async function mark() {
    if (!step || !pos) return;
    const here = pos;
    const r = await act<{ pos: [number, number, number] }>('dot');
    if (!r) return;
    pos = r.pos;
    step.mark = { kind: step.kind, name: step.name, x: Math.round(here[0] * 100) / 100, y: Math.round(here[1] * 100) / 100 };
    step.status = 'done';
    await penUpNow();
    await afterChange();
  }
  async function skip() {
    if (!step) return;
    step.status = 'skipped';
    if (step.kind === 'corner' && !steps.some((s) => s.kind === 'edge')) steps.push(...nextEdgeSteps(steps).slice(0, 4));
    await afterChange();
  }
  async function afterChange() {
    advance(); solveErr = ''; result = null; savedSheet = '';
    const marks = marksOf(steps);
    if (!marks.length) return;
    try { result = await call('calib_solve', { marks }); }
    catch (e) {
      solveErr = errText(e);
      if (cur >= steps.length) { const more = nextEdgeSteps(steps); if (more.length && /fix the sheet|Not enough|pin down/.test(solveErr)) { steps.push(more[0]); advance(); } }
    }
  }
  function redo(i: number) { steps[i].status = 'todo'; steps[i].mark = undefined; cur = i; result = null; savedSheet = ''; }
  async function saveSheet() {
    const r = await act('calib_save', { marks: marksOf(steps), name: 'A4 Frame' });
    if (r) savedSheet = 'Sheet position saved. The preview uses it now.';
  }
</script>

<div class="scrim" role="presentation">
  <div class="page" role="dialog" aria-modal="true" aria-label="Calibration">
    <header>
      <h2>Calibration</h2>
      <div class="hbtn"><button class="estop" onclick={estop}>EMERGENCY STOP</button><button onclick={onclose}>Done</button></div>
    </header>
    {#if !st.controller.ready}<p class="err">The plotter is not ready: {st.controller.blockers.join('; ') || st.controller.error}</p>{/if}

    <section class="card">
      <div class="cols">
        <!-- X / Y -->
        <div class="col">
          <Joystick opts={joy} disabled={!ready || !pos} onstep={stick} />
          <label>Speed {speed} mm/s <input type="range" min="1" max="60" step="1" bind:value={speed} /></label>
          <div class="fgrid">
            <span></span><button disabled={!ready} onclick={() => nudge('forward')} aria-label="Away from you">▲</button><span></span>
            <button disabled={!ready} onclick={() => nudge('left')} aria-label="Left">◀</button><span class="mono small mute">{fine} mm</span><button disabled={!ready} onclick={() => nudge('right')} aria-label="Right">▶</button>
            <span></span><button disabled={!ready} onclick={() => nudge('back')} aria-label="Toward you">▼</button><span></span>
          </div>
          <div class="seg">{#each [0.1, 1, 5] as sz}<button class:on={fine === sz} onclick={() => (fine = sz)}>{sz} mm</button>{/each}</div>
        </div>

        <!-- Z -->
        <div class="col zc">
          <div class="zrow">
            <ZSlider z={pos ? pos[2] : null} contact={contactL} disabled={!ready || !pos} onstep={zStep} />
            <div class="zbtns">
              <button disabled={!ready || !pos} onclick={() => zStep(0.5)}>+0.5</button>
              <button disabled={!ready || !pos} onclick={() => zStep(0.1)}>+0.1</button>
              <div class="zval mono"><b>{pos ? pos[2].toFixed(2) : '–'}</b><span class="small mute">mm{zTarget !== null ? ` → ${zTarget.toFixed(2)}` : ''}</span></div>
              <button disabled={!ready || !pos} onclick={() => zStep(-0.1)}>−0.1</button>
              <button disabled={!ready || !pos} onclick={() => zStep(-0.5)}>−0.5</button>
            </div>
          </div>
          <button class="primary" disabled={!ready || busy} onclick={penUpNow}>Raise pen</button>
        </div>

        <!-- everything else -->
        <div class="col info">
          <div class="pos mono">X <b>{pos ? pos[0].toFixed(1) : '–'}</b> · Y <b>{pos ? pos[1].toFixed(1) : '–'}</b> · Z <b>{pos ? pos[2].toFixed(2) : '–'}</b></div>

          <div class="blk">
            <div class="lbl">Pen height</div>
            <button class="primary" disabled={!ready || !pos} onclick={touches}>Pen touches here</button>
            <div class="r2">
              <span class="small mute">Pressure</span><span class="stepper"><button onclick={() => setDepth(depthL - 0.1)}>−</button><b class="mono">{depthL.toFixed(1)}</b><button onclick={() => setDepth(depthL + 0.1)}>+</button></span>
              <span class="small mute">Pen up</span><span class="stepper"><button onclick={() => setPenUp(penUpL - 5)}>−</button><b class="mono">{penUpL}</b><button onclick={() => setPenUp(penUpL + 5)}>+</button></span>
            </div>
            <div class="mono small">contact {contactL.toFixed(2)} · pen down <b>{penDownL.toFixed(2)}</b> · up {penUpL}</div>
            <div class="r2b"><button disabled={!ready || busy} onclick={testDot}>Test dot</button><button class="primary" disabled={busy || !frame} onclick={savePen}>Save pen height</button></div>
            {#if lastDot}<div class="r2b"><span class="small">Ink?</span><span class="yn"><button onclick={() => inked(true)}>Yes</button><button onclick={() => inked(false)}>No</button></span></div>{/if}
            {#if varies}<p class="small warn">Inked in some places, not others: the sheet is not flat, one height will not work everywhere.</p>{/if}
            {#if savedPen}<p class="small ok">{savedPen}</p>{/if}
          </div>

          <div class="blk">
            <div class="lbl">Sheet corners</div>
            {#if step}
              <p class="small"><b>{step.title}</b>: {step.where}.</p>
              <div class="r2b"><button class="primary" disabled={!ready || busy || !pos} onclick={mark}>Mark</button><button disabled={busy} onclick={skip}>Can't reach it</button></div>
            {:else}<p class="small">All points marked.</p>{/if}
            <ol class="list">
              {#each steps as s, i}
                <li class={s.status} class:cur={i === cur}><span class="n">{s.status === 'done' ? '✓' : s.status === 'skipped' ? '–' : i + 1}</span><span class="t">{s.title}</span>{#if s.status !== 'todo'}<button class="ghost small" onclick={() => redo(i)}>Redo</button>{/if}</li>
              {/each}
            </ol>
            {#if solveErr}<p class="small {cur >= steps.length ? 'err' : 'mute'}">{solveErr}</p>{/if}
            {#if result}
              <p class="small">Sheet corner X {result.origin[0].toFixed(1)}, Y {result.origin[1].toFixed(1)} · tilt {result.skewDeg.toFixed(2)}° · error {result.rmsMm.toFixed(1)} mm</p>
              {#each result.warnings as w}<p class="small warn">{w}</p>{/each}
              <button class="primary" disabled={busy || !!savedSheet} onclick={saveSheet}>Save sheet position</button>
              {#if savedSheet}<p class="small ok">{savedSheet}</p>{/if}
            {/if}
          </div>
        </div>
      </div>
      {#if note}<p class="small {/Dot at|Saved|Contact height/.test(note) ? 'ok' : 'err'}">{note}</p>{/if}
      <details><summary class="small mute">Directions wrong?</summary>
        <label class="check"><input type="checkbox" checked={flipX} onchange={(e) => setFlip('x', e.currentTarget.checked)} /> Reverse left/right</label>
        <label class="check"><input type="checkbox" checked={flipY} onchange={(e) => setFlip('y', e.currentTarget.checked)} /> Reverse forward/back</label>
      </details>
    </section>
    <LiveCamera />
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; background: var(--bg); z-index: 22; overflow: auto; }
  .page { max-width: 1100px; margin: 0 auto; padding: 14px 20px 50px; display: grid; gap: 12px; }
  header { display: flex; justify-content: space-between; align-items: center; } .hbtn { display: flex; gap: 8px; }
  .estop { background: var(--danger); border-color: var(--danger); color: #fff; font-weight: 700; }
  .card { background: var(--surface); border: 1px solid var(--line); border-radius: var(--radius); padding: 16px; }
  .cols { display: flex; gap: 28px; flex-wrap: wrap; align-items: flex-start; }
  .col { display: grid; gap: 10px; justify-items: center; align-content: start; }
  .col > label { width: 100%; margin: 0; }
  .zc { min-width: 150px; } .zrow { display: flex; gap: 10px; align-items: center; }
  .zbtns { display: grid; gap: 8px; justify-items: center; } .zbtns button { width: 64px; padding: 10px 0; }
  .zval { display: grid; text-align: center; padding: 6px 0; } .zval b { font-size: 20px; }
  .info { flex: 1; min-width: 300px; justify-items: stretch; }
  .pos { font-size: 15px; }
  .blk { border-top: 1px solid var(--line); padding-top: 10px; display: grid; gap: 8px; } .lbl { font-weight: 600; font-size: 13px; }
  .r2 { display: grid; grid-template-columns: auto 1fr auto 1fr; gap: 8px 10px; align-items: center; }
  .r2b { display: flex; gap: 8px; align-items: center; } .r2b > button { flex: 1; } .yn { display: flex; gap: 8px; }
  .stepper { display: flex; gap: 8px; align-items: center; } .stepper button { width: 34px; padding: 5px 0; }
  .fgrid { display: grid; grid-template-columns: repeat(3, 48px); gap: 6px; align-items: center; justify-items: center; } .fgrid button { width: 48px; padding: 8px 0; }
  .seg { display: flex; border: 1px solid var(--line); border-radius: 8px; overflow: hidden; } .seg button { border: 0; border-radius: 0; background: transparent; padding: 6px 12px; } .seg button.on { background: var(--ink); color: var(--bg); }
  .list { list-style: none; margin: 0; padding: 0; display: grid; gap: 2px; }
  .list li { display: flex; gap: 8px; align-items: center; padding: 2px 4px; border-radius: 6px; font-size: 13px; } .list li.cur { background: var(--soft); }
  .list .n { width: 20px; height: 20px; border-radius: 50%; border: 1.5px solid var(--mute); display: grid; place-items: center; font-size: 11px; flex: none; } .list .done .n { background: var(--ok); border-color: var(--ok); color: #fff; }
  .list .t { flex: 1; } .list .skipped { opacity: .6; }
</style>
