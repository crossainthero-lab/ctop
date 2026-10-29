<script lang="ts">
  // Manual calibration: move the pen with the on-screen joystick, mark the sheet's corners (or points along its edges where
  // the machine cannot reach a corner), save. The machine only ever moves when you drag or press a button here.
  import { untrack } from 'svelte';
  import { app, refresh, errText } from '../lib/app.svelte';
  import { call } from '../lib/rpc';
  import { DEFAULT_JOY, DEFAULT_Z, fineStep, type Dir, type JoyOpts } from '../lib/joystick';
  import { initialSteps, marksOf, nextEdgeSteps, type Step } from '../lib/calibsteps';
  import { reachOnPaper, A4, type Reach } from '../lib/frame';
  import Joystick from './Joystick.svelte';
  import ZSlider from './ZSlider.svelte';
  import LiveCamera from './LiveCamera.svelte';

  let { onclose }: { onclose: () => void } = $props();
  const st = $derived(app.st!);
  const frame = $derived(st.frame.active);
  const reach = $derived((st.controller.box as Reach | null) ?? null);
  const live = $derived(!!st.job?.live || st.phase === 'preparing' || st.job?.state === 'paused');
  const ready = $derived(st.controller.ready && !live);
  const zup = $derived(18);

  // ── position ──
  let pos = $state<[number, number, number] | null>(null);
  $effect(() => { const p = st.controller.pos; if (p && !inflight && !zInflight) pos = [p[0], p[1], p[2]]; });
  const aimed = $derived(!!pos && pos[2] <= zup + 1.2 && pos[2] >= zup - 0.2);
  const penUp = $derived(!!pos && pos[2] >= 24);

  // ── joystick / jog ──
  let speed = $state(12);
  let flipX = $state(false), flipY = $state(false);
  try { flipX = localStorage.getItem('ctop-flipx') === '1'; flipY = localStorage.getItem('ctop-flipy') === '1'; } catch { /* storage unavailable */ }
  const joy = $derived<JoyOpts>({ ...DEFAULT_JOY, speedMmS: speed, flipX, flipY });
  let inflight = false;
  let note = $state('');
  let clampedNote = $state(false);
  async function jog(dx: number, dy: number, feed: number) {
    if (inflight || !ready) return;
    inflight = true;
    try {
      const r = await call<{ pos: [number, number, number]; clamped: boolean }>('jog', { dx, dy, feed, t: Date.now() });
      pos = r.pos; clampedNote = r.clamped; note = '';
    } catch (e) {
      const m = errText(e);
      if (m !== 'busy' && m !== 'stale') note = m;
    } finally { inflight = false; }
  }
  const stick = (dx: number, dy: number) => jog(dx, dy, Math.max(300, Math.min(2400, speed * 60 * 1.4)));
  let fine = $state(1);
  const nudge = (d: Dir) => { const s = fineStep(d, fine, { flipX, flipY }); jog(s.dx, s.dy, 600); };
  function setFlip(k: 'x' | 'y', v: boolean) {
    if (k === 'x') flipX = v; else flipY = v;
    try { localStorage.setItem('ctop-flip' + k, v ? '1' : '0'); } catch { /* ignore */ }
  }

  let busy = $state(false);
  async function act<T>(cmd: string, args?: Record<string, unknown>, ok = ''): Promise<T | undefined> {
    busy = true; note = '';
    try { const r = await call<T>(cmd, args); if (ok) note = ok; return r; }
    catch (e) { note = errText(e); }
    finally { busy = false; refresh(); }
  }
  const penUpNow = async () => { const r = await act<{ pos: [number, number, number] }>('pen_raise'); if (r?.pos) pos = r.pos; };
  const aim = async () => { const r = await act<{ pos: [number, number, number] }>('pen_hover'); if (r?.pos) pos = r.pos; };
  const dotNow = async () => { const r = await act<{ pos: [number, number, number]; downZ: number }>('dot', undefined, 'Dot made.'); if (r?.pos) pos = r.pos; };
  const estop = () => call('estop').catch((e) => (note = errText(e)));

  // ── the steps ──
  let steps = $state<Step[]>(untrack(() => initialSteps(frame, reach)));
  let cur = $state(0);
  let result = $state<{ origin: [number, number]; skewDeg: number; rmsMm: number; residuals: [string, number][]; warnings: string[] } | null>(null);
  let solveErr = $state('');
  let saved = $state('');
  // corners the current guess puts out of reach are skipped up front, with the reason shown
  $effect(() => {
    if (cur === 0 && steps.every((s) => s.status === 'todo')) {
      const un = steps.filter((s) => s.likelyUnreachable);
      if (un.length) { un.forEach((s) => (s.status = 'skipped')); steps.push(...nextEdgeSteps(steps).slice(0, 4)); advance(); }
    }
  });
  function advance() { const i = steps.findIndex((s) => s.status === 'todo'); cur = i < 0 ? steps.length : i; }
  const step = $derived(steps[cur]);
  const doneCount = $derived(steps.filter((s) => s.status === 'done').length);

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
  /** after each change: move on and try to solve; if the marks do not fix the sheet yet, add reference points */
  async function afterChange() {
    advance(); solveErr = ''; result = null;
    const marks = marksOf(steps);
    if (!marks.length) return;
    try { result = await call('calib_solve', { marks }); }
    catch (e) {
      solveErr = errText(e);
      if (cur >= steps.length) {
        const more = nextEdgeSteps(steps);
        if (more.length && /fix the sheet|Not enough|pin down/.test(solveErr)) { steps.push(more[0]); advance(); }
      }
    }
  }
  function back() {
    const i = [...steps.keys()].reverse().find((k) => steps[k].status !== 'todo');
    if (i === undefined) return;
    steps[i].status = 'todo'; steps[i].mark = undefined; cur = i; result = null; solveErr = ''; saved = '';
  }
  function redo(i: number) { steps[i].status = 'todo'; steps[i].mark = undefined; cur = i; result = null; saved = ''; }
  async function save() {
    const marks = marksOf(steps);
    const r = await act('calib_save', { marks, name: 'A4 Frame' });
    if (r) saved = 'Saved as "A4 Frame" and in use. The preview now shows the real position of your sheet.';
  }

  // ── A4 diagram (seen from the front of the printer) ──
  const D = 0.62;
  const unreach = $derived(frame && reach ? reachOnPaper(frame, reach).map((p) => `${p[0] * D},${p[1] * D}`).join(' ') : '');
  const hi = $derived.by(() => {
    if (!step) return null;
    const W = A4.w * D, H = A4.h * D;
    const c: Record<string, [number, number]> = { tl: [0, 0], tr: [W, 0], br: [W, H], bl: [0, H] };
    if (step.kind === 'corner') return { dot: c[step.name] };
    const e: Record<string, [number, number, number, number]> = { right: [W, 0, W, H], left: [0, 0, 0, H], front: [0, H, W, H], back: [0, 0, W, 0] };
    return { line: e[step.name] };
  });

  // ── pen height (Z) ──
  // Physical up = machine Z increasing. The pen is lowered toward the paper by DECREASING Z; the page always speaks in
  // physical terms (up / down) and shows the real millimetre value the controller reports.
  const saved0 = untrack(() => ({ c: frame?.surfaceZ ?? Number(st.settings.surfaceZ ?? 15.3), d: frame?.penDepth ?? Number(st.settings.depth ?? 0.6), u: frame?.penUpZ ?? Number(st.settings.travelZ ?? 30) }));
  let contactL = $state(saved0.c);      // contact height being tried (saved only by "Save pen height")
  let depthL = $state(saved0.d);        // writing pressure: mm below contact
  let penUpL = $state(saved0.u);        // pen-up travel height
  let zTarget = $state<number | null>(null);
  let zInflight = false;
  const Z_LOW = DEFAULT_Z.zMin;
  const penDownL = $derived(Math.round((contactL - depthL) * 100) / 100);
  const downOk = $derived(penDownL >= Z_LOW);
  const dirty = $derived(!!frame && (Math.abs(contactL - frame.surfaceZ) > 1e-6 || Math.abs(depthL - (frame.penDepth ?? Number(st.settings.depth ?? 0.6))) > 1e-6 || Math.abs(penUpL - (frame.penUpZ ?? Number(st.settings.travelZ ?? 30))) > 1e-6));
  async function zStep(dz: number) {
    if (zInflight || !ready) return;
    zInflight = true;
    try {
      const r = await call<{ pos: [number, number, number] }>('z_nudge', { dz, t: Date.now() });
      pos = r.pos; zTarget = r.pos[2]; note = '';
    } catch (e) { const m = errText(e); if (m !== 'busy' && m !== 'stale') note = m; }
    finally { zInflight = false; }
  }
  const zFine = (dz: number) => zStep(dz);
  function touches() {
    if (!pos) return;
    if (pos[2] < Z_LOW || pos[2] > 30) { note = 'That height cannot be a contact height.'; return; }
    contactL = Math.round(pos[2] * 100) / 100; note = `Contact height set to ${contactL.toFixed(2)} mm (saved when you press Save pen height).`;
  }
  const setDepth = (d: number) => { depthL = Math.round(Math.min(1, Math.max(0.2, d)) * 10) / 10; };
  const setPenUp = (u: number) => { penUpL = Math.min(60, Math.max(20, Math.round(u))); };
  async function testDot() {
    const r = await act<{ pos: [number, number, number]; downZ: number }>('dot', { contact: contactL, depth: depthL });
    if (r?.pos) { pos = r.pos; lastDot = { x: pos[0], y: pos[1], z: r.downZ }; note = `Dot made at Z ${r.downZ.toFixed(2)} mm.`; }
  }
  let lastDot = $state<{ x: number; y: number; z: number } | null>(null);
  let dots = $state<{ x: number; y: number; inked: boolean }[]>([]);
  function inked(v: boolean) { if (!lastDot) return; dots.push({ x: lastDot.x, y: lastDot.y, inked: v }); lastDot = null; }
  const varies = $derived(dots.some((d) => d.inked) && dots.some((d) => !d.inked));
  let savedPen = $state('');
  async function savePen() {
    savedPen = '';
    const r = await act('pen_save', { contact: contactL, depth: depthL, penUp: penUpL });
    if (r) savedPen = `Saved: contact ${contactL.toFixed(2)} mm, pen down ${penDownL.toFixed(2)} mm, pen up ${penUpL} mm. Text, Image and Lined paper use these.`;
  }
</script>

<div class="scrim" role="presentation">
  <div class="page" role="dialog" aria-modal="true" aria-label="Calibration">
    <header>
      <div><h2>Calibration</h2><p class="mute small">Move the pen with the joystick, mark the sheet's corners, save. Nothing moves unless you drag or press a button.</p></div>
      <div class="hbtn"><button class="estop" onclick={estop} title="Stops everything at once">EMERGENCY STOP</button><button onclick={onclose}>Done</button></div>
    </header>

    {#if !st.controller.ready}<p class="err">The plotter is not ready: {st.controller.blockers.join('; ') || st.controller.error}</p>{/if}

    <div class="grid">
      <!-- left: controls -->
      <section class="card ctl">
        <h3>1 · Move the pen <span class="mute small">(X, Y and height)</span></h3>
        <div class="pos mono">X <b>{pos ? pos[0].toFixed(1) : '–'}</b> &nbsp; Y <b>{pos ? pos[1].toFixed(1) : '–'}</b> &nbsp; Z <b>{pos ? pos[2].toFixed(2) : '–'}</b> mm
          <span class="state {aimed ? 'ok' : penUp ? 'mute' : 'warn'}">{aimed ? 'aim height' : penUp ? 'pen up' : 'pen low'}</span></div>
        <div class="sticks">
          <Joystick opts={joy} disabled={!ready || !pos || pos[2] < zup - 0.2} onstep={stick} />
          <div class="zcol">
            <div class="zbox">
              <ZSlider z={pos ? pos[2] : null} contact={contactL} disabled={!ready || !pos} onstep={zStep} />
              <div class="zread mono">
                <div><span class="mute small">now</span><br /><b>{pos ? pos[2].toFixed(2) : '–'}</b></div>
                <div><span class="mute small">target</span><br /><b>{zTarget !== null ? zTarget.toFixed(2) : '–'}</b></div>
                <div class="small mute">touches at<br />{contactL.toFixed(2)}</div>
                <div class="small mute">lowest<br />{Z_LOW.toFixed(1)}</div>
              </div>
            </div>
            <button class="primary" disabled={!ready || busy} onclick={penUpNow}>Raise pen</button>
          </div>
        </div>
        {#if clampedNote}<p class="small warn">At the machine's travel limit in that direction.</p>{/if}
        {#if pos && pos[2] < zup - 0.2}<p class="small warn">The pen is low: raise it before moving sideways.</p>{/if}
        <label>Speed: {speed} mm/s <input type="range" min="2" max="40" step="1" bind:value={speed} /></label>
        <div class="finerow">
          <div class="fgrid">
            <span></span><button disabled={!ready} onclick={() => nudge('forward')} aria-label="Away from you">▲</button><span></span>
            <button disabled={!ready} onclick={() => nudge('left')} aria-label="Left">◀</button><span class="mono small mute">{fine} mm</span><button disabled={!ready} onclick={() => nudge('right')} aria-label="Right">▶</button>
            <span></span><button disabled={!ready} onclick={() => nudge('back')} aria-label="Toward you">▼</button><span></span>
          </div>
          <div class="zfine">
            <div class="small mute">Pen height</div>
            <div class="zbtn"><button disabled={!ready || !pos} onclick={() => zFine(-0.5)}>−0.5</button><button disabled={!ready || !pos} onclick={() => zFine(-0.1)}>−0.1</button></div>
            <div class="zbtn"><button disabled={!ready || !pos} onclick={() => zFine(0.1)}>+0.1</button><button disabled={!ready || !pos} onclick={() => zFine(0.5)}>+0.5</button></div>
            <div class="small mute">− lowers · + raises</div>
          </div>
        </div>
        <div class="seg">{#each [0.1, 1, 5] as sz}<button class:on={fine === sz} onclick={() => (fine = sz)}>{sz} mm sideways</button>{/each}</div>
        <div class="row3">
          <button class:primary={!aimed && !!pos && !penUp} disabled={!ready || busy} onclick={aim} title="A few mm above the paper, to line the tip up without parallax">Aim height</button>
          <button disabled={!ready || busy || !aimed} onclick={dotNow}>Dot</button>
          <button disabled={!ready || busy} onclick={penUpNow}>Pen up</button>
        </div>
        <details><summary class="small mute">Directions wrong?</summary>
          <p class="small mute">Seen from the front of the printer (LCD side): pushing right moves the pen right, pushing up moves it away from you.</p>
          <label class="check"><input type="checkbox" checked={flipX} onchange={(e) => setFlip('x', e.currentTarget.checked)} /> Reverse left/right</label>
          <label class="check"><input type="checkbox" checked={flipY} onchange={(e) => setFlip('y', e.currentTarget.checked)} /> Reverse forward/back</label>
        </details>
        {#if note}<p class="small {/Dot made|Saved|set to/.test(note) ? 'ok' : 'err'}">{note}</p>{/if}

        <div class="pen">
          <h3>2 · Pen height</h3>
          <p class="small mute">Position the pen over the paper, lower it with the slider until the tip just touches (watch the side camera), then press <b>Pen touches here</b>.</p>
          <button class="primary big" disabled={!ready || !pos || pos[2] > 24} onclick={touches}>Pen touches here ({pos ? pos[2].toFixed(2) : '–'} mm)</button>
          <div class="penrow">
            <div>
              <div class="small mute">Writing pressure (mm below contact)</div>
              <div class="stepper"><button onclick={() => setDepth(depthL - 0.1)}>−</button><b class="mono">{depthL.toFixed(1)}</b><button onclick={() => setDepth(depthL + 0.1)}>+</button></div>
            </div>
            <div>
              <div class="small mute">Pen-up height</div>
              <div class="stepper"><button onclick={() => setPenUp(penUpL - 5)}>−</button><b class="mono">{penUpL}</b><button onclick={() => setPenUp(penUpL + 5)}>+</button></div>
            </div>
          </div>
          <p class="mono">Contact <b>{contactL.toFixed(2)}</b> · pen down <b class:err={!downOk}>{penDownL.toFixed(2)}</b> · pen up <b>{penUpL}</b> mm</p>
          {#if !downOk}<p class="small err">Pen down would be below the verified limit ({Z_LOW} mm). Use less pressure.</p>{/if}
          <div class="row">
            <button disabled={!ready || busy || !aimed} onclick={testDot} title="Lowers to the pen-down height above, makes a dot, comes back up">Test dot</button>
            <button class="primary" disabled={busy || !frame || !downOk || (!dirty && !!frame)} onclick={savePen}>Save pen height</button>
          </div>
          {#if lastDot}<div class="row"><span class="small">Did the dot leave ink?</span><button onclick={() => inked(true)}>Yes</button><button onclick={() => inked(false)}>No</button></div>{/if}
          {#if varies}<p class="small warn">The pen inked in some places but not others, so one height will not work everywhere: the sheet is not flat. Flatten or tape it, or use a spring-loaded pen holder. Raising the pressure for the whole page would only score the paper where it is highest.</p>{/if}
          {#if !frame}<p class="small warn">Mark the sheet first (right): the pen heights are saved with that calibration.</p>{/if}
          {#if savedPen}<p class="small ok">{savedPen}</p>{/if}
        </div>
      </section>

      <!-- right: steps -->
      <section class="card stp">
        <h3>2 · Mark the sheet</h3>
        <div class="diagwrap">
          <svg viewBox="-34 -22 {A4.w * D + 68} {A4.h * D + 50}" class="diag" role="img" aria-label="A4 sheet seen from the front of the printer">
            <rect x="0" y="0" width={A4.w * D} height={A4.h * D} class="sheet" />
            {#if unreach}<clipPath id="sh"><rect x="0" y="0" width={A4.w * D} height={A4.h * D} /></clipPath>
              <g clip-path="url(#sh)"><rect x="-200" y="-200" width="800" height="800" class="hatch" /><polygon points={unreach} class="reach" /></g>{/if}
            {#each steps as s}{#if s.status === 'done' && s.kind === 'corner'}<circle r="4" class="doneDot"
              cx={{ tl: 0, tr: A4.w * D, br: A4.w * D, bl: 0 }[s.name]} cy={{ tl: 0, tr: 0, br: A4.h * D, bl: A4.h * D }[s.name]} />{/if}{/each}
            {#if hi?.dot}<circle cx={hi.dot[0]} cy={hi.dot[1]} r="9" class="target" />{/if}
            {#if hi?.line}<line x1={hi.line[0]} y1={hi.line[1]} x2={hi.line[2]} y2={hi.line[3]} class="tline" />{/if}
            <text x={A4.w * D / 2} y="-8" text-anchor="middle" class="dl">BACK</text>
            <text x={A4.w * D / 2} y={A4.h * D + 16} text-anchor="middle" class="dl">FRONT · LCD screen</text>
            <text x="-6" y={A4.h * D / 2} text-anchor="end" class="dl">LEFT</text>
            <text x={A4.w * D + 6} y={A4.h * D / 2} class="dl">RIGHT</text>
          </svg>
          {#if frame && reach}<p class="small mute">Hatched: beyond the machine's travel (from the last saved position).</p>{/if}
        </div>

        {#if step}
          <div class="instr">
            <div class="small mute">Step {Math.min(cur + 1, steps.length)} of {steps.length}</div>
            <h3 class="big">{step.title}</h3>
            <p>Move the pen over {step.where}. Press <b>Aim height</b>, line the pen tip up with it, then press <b>Mark</b>.</p>
            {#if step.kind === 'edge'}<p class="small mute">The corners of this sheet are beyond the machine's travel, so these edge points fix its position instead.</p>{/if}
            <div class="row">
              <button class="primary big" disabled={!ready || busy || !aimed} onclick={mark}>{step.kind === 'corner' ? 'Mark corner' : 'Mark point'}</button>
              <button disabled={busy} onclick={skip} title="The pen cannot get there: use reference points along the edges instead">Can't reach it</button>
            </div>
            {#if !aimed}<p class="small warn">Press Aim height first so the mark is accurate.</p>{/if}
          </div>
        {:else}
          <div class="instr"><h3 class="big">All points marked</h3></div>
        {/if}

        <ol class="list">
          {#each steps as s, i}
            <li class={s.status} class:cur={i === cur}>
              <span class="n">{s.status === 'done' ? '✓' : s.status === 'skipped' ? '–' : i + 1}</span>
              <span class="t">{s.title}{#if s.status === 'skipped'} <span class="mute small">(out of reach)</span>{/if}{#if s.mark} <span class="mono small mute">X {s.mark.x} Y {s.mark.y}</span>{/if}</span>
              {#if s.status !== 'todo'}<button class="ghost small" onclick={() => redo(i)}>Retry</button>{/if}
            </li>
          {/each}
        </ol>
        <div class="row"><button disabled={doneCount === 0 && !steps.some((s) => s.status === 'skipped')} onclick={back}>Previous</button></div>

        {#if solveErr}<p class="small {cur >= steps.length ? 'err' : 'mute'}">{solveErr}</p>{/if}
        {#if result}
          <div class="res">
            <b>Sheet position found</b>
            <p class="small">Corner of the sheet at X {result.origin[0].toFixed(1)}, Y {result.origin[1].toFixed(1)} · tilt {result.skewDeg.toFixed(2)}° · fit error {result.rmsMm.toFixed(1)} mm</p>
            {#each result.warnings as w}<p class="small warn">{w}</p>{/each}
            <button class="primary" disabled={busy || !!saved} onclick={save}>Save calibration</button>
            {#if saved}<p class="small ok">{saved}</p>{/if}
          </div>
        {/if}
      </section>
    </div>

    <LiveCamera />
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; background: var(--bg); z-index: 22; overflow: auto; }
  .page { max-width: 1100px; margin: 0 auto; padding: 18px 20px 60px; display: grid; gap: 14px; }
  header { display: flex; justify-content: space-between; gap: 12px; align-items: flex-start; }
  .hbtn { display: flex; gap: 8px; }
  .estop { background: var(--danger); border-color: var(--danger); color: #fff; font-weight: 700; letter-spacing: .03em; }
  .grid { display: grid; grid-template-columns: 430px minmax(0, 1fr); gap: 14px; align-items: start; }
  .card { background: var(--surface); border: 1px solid var(--line); border-radius: var(--radius); padding: 14px 16px; }
  .ctl { display: grid; gap: 10px; justify-items: center; }
  .ctl > * { width: 100%; } .ctl :global(.wrap) { width: auto; }
  h3 { margin-bottom: 6px; } h3.big { font-size: 18px; margin: 2px 0 6px; }
  .pos { text-align: center; font-size: 15px; } .state { font-size: 12px; margin-left: 8px; }
  .sticks { display: flex; gap: 14px; justify-content: center; align-items: flex-start; }
  .zcol { display: grid; gap: 8px; justify-items: center; } .zbox { display: flex; gap: 8px; } .zread { display: grid; align-content: space-between; text-align: center; font-size: 13px; padding: 4px 0; width: 70px; }
  .finerow { display: flex; gap: 18px; align-items: center; justify-content: center; } .zfine { display: grid; gap: 6px; justify-items: center; } .zbtn { display: flex; gap: 6px; } .zbtn button { padding: 8px 10px; min-width: 52px; }
  .pen { border-top: 1px solid var(--line); padding-top: 12px; display: grid; gap: 8px; margin-top: 4px; }
  .penrow { display: flex; gap: 18px; justify-content: space-between; } .stepper { display: flex; gap: 10px; align-items: center; } .stepper button { width: 38px; padding: 6px 0; }
  .fine { display: grid; gap: 8px; justify-items: center; }
  .fgrid { display: grid; grid-template-columns: repeat(3, 52px); gap: 6px; align-items: center; justify-items: center; } .fgrid button { width: 52px; padding: 8px 0; }
  .seg { display: flex; border: 1px solid var(--line); border-radius: 8px; overflow: hidden; } .seg button { border: 0; border-radius: 0; background: transparent; padding: 6px 14px; } .seg button.on { background: var(--ink); color: var(--bg); }
  .row3 { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; }
  .stp { display: grid; gap: 10px; }
  .diagwrap { display: grid; justify-items: center; } .diag { width: min(260px, 100%); }
  .sheet { fill: #fffefb; stroke: var(--ink); stroke-width: 1.2; }
  .hatch { fill: rgba(180, 35, 24, .18); } .reach { fill: #fffefb; }
  .target { fill: none; stroke: var(--accent); stroke-width: 3; } .tline { stroke: var(--accent); stroke-width: 4; } .doneDot { fill: var(--ok); }
  .dl { fill: var(--mute); font-size: 9px; letter-spacing: .06em; }
  .instr { border: 1px solid var(--accent); background: var(--accent-soft); border-radius: var(--radius); padding: 12px 14px; }
  .instr .row { margin-top: 8px; }
  .list { list-style: none; margin: 0; padding: 0; display: grid; gap: 4px; }
  .list li { display: flex; gap: 10px; align-items: center; padding: 4px 6px; border-radius: 6px; } .list li.cur { background: var(--soft); }
  .list .n { width: 22px; height: 22px; border-radius: 50%; border: 1.5px solid var(--mute); display: grid; place-items: center; font-size: 12px; flex: none; } .list .done .n { background: var(--ok); border-color: var(--ok); color: #fff; }
  .list .t { flex: 1; } .list .skipped { opacity: .65; }
  .res { border: 1px solid var(--ok); border-radius: var(--radius); padding: 12px 14px; display: grid; gap: 6px; }
  @media (max-width: 900px) { .grid { grid-template-columns: 1fr; } }
</style>
