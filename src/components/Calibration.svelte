<script lang="ts">
  // Manual calibration: move the pen with the on-screen joystick, mark the sheet's corners (or points along its edges where
  // the machine cannot reach a corner), save. The machine only ever moves when you drag or press a button here.
  import { untrack } from 'svelte';
  import { app, refresh, errText } from '../lib/app.svelte';
  import { call } from '../lib/rpc';
  import { DEFAULT_JOY, fineStep, type Dir, type JoyOpts } from '../lib/joystick';
  import { initialSteps, marksOf, nextEdgeSteps, type Step } from '../lib/calibsteps';
  import { reachOnPaper, A4, type Reach } from '../lib/frame';
  import Joystick from './Joystick.svelte';
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
  $effect(() => { const p = st.controller.pos; if (p && !inflight) pos = [p[0], p[1], p[2]]; });
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

  // ── pen height ──
  const depth = $derived(Number(st.settings.depth ?? 0.6));
  const contact = $derived(frame?.surfaceZ ?? Number(st.settings.surfaceZ ?? 15.3));
  const penDown = $derived(Math.max(13.5, contact - depth));
  async function setDepth(d: number) { const v = Math.round(Math.min(1, Math.max(0.2, d)) * 10) / 10; await act('set_setting', { key: 'depth', value: v }); }
  async function zn(dz: number) { const r = await act<{ pos: [number, number, number] }>('z_nudge', { dz }); if (r?.pos) pos = r.pos; }
  const contactHere = () => act('contact_here', undefined, 'Contact height saved from the current pen height.');
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
        <h3>1 · Move the pen</h3>
        <div class="pos mono">X <b>{pos ? pos[0].toFixed(1) : '–'}</b> &nbsp; Y <b>{pos ? pos[1].toFixed(1) : '–'}</b> &nbsp; Z <b>{pos ? pos[2].toFixed(1) : '–'}</b> mm
          <span class="state {aimed ? 'ok' : penUp ? 'mute' : 'warn'}">{aimed ? 'aim height' : penUp ? 'pen up' : 'pen low'}</span></div>
        <Joystick opts={joy} disabled={!ready || !pos || pos[2] < zup - 0.2} onstep={stick} />
        {#if clampedNote}<p class="small warn">At the machine's travel limit in that direction.</p>{/if}
        {#if pos && pos[2] < zup - 0.2}<p class="small warn">The pen is too low to move sideways. Press Pen up.</p>{/if}
        <label>Speed: {speed} mm/s <input type="range" min="2" max="40" step="1" bind:value={speed} /></label>
        <div class="fine">
          <div class="fgrid">
            <span></span><button disabled={!ready} onclick={() => nudge('forward')} aria-label="Away from you">▲</button><span></span>
            <button disabled={!ready} onclick={() => nudge('left')} aria-label="Left">◀</button><span class="mono small mute">{fine} mm</span><button disabled={!ready} onclick={() => nudge('right')} aria-label="Right">▶</button>
            <span></span><button disabled={!ready} onclick={() => nudge('back')} aria-label="Toward you">▼</button><span></span>
          </div>
          <div class="seg">{#each [0.1, 1, 5] as s}<button class:on={fine === s} onclick={() => (fine = s)}>{s} mm</button>{/each}</div>
        </div>
        <div class="row3">
          <button disabled={!ready || busy} onclick={penUpNow}>Pen up</button>
          <button class:primary={!aimed && !!pos} disabled={!ready || busy} onclick={aim} title="Lowers the pen to a few mm above the paper so you can line it up without parallax">Aim height</button>
          <button disabled={!ready || busy || !aimed} onclick={dotNow}>Test dot</button>
        </div>
        <details><summary class="small mute">Directions wrong?</summary>
          <p class="small mute">Seen from the front of the printer (LCD side): pushing right moves the pen right, pushing up moves it away from you.</p>
          <label class="check"><input type="checkbox" checked={flipX} onchange={(e) => setFlip('x', e.currentTarget.checked)} /> Reverse left/right</label>
          <label class="check"><input type="checkbox" checked={flipY} onchange={(e) => setFlip('y', e.currentTarget.checked)} /> Reverse forward/back</label>
        </details>
        {#if note}<p class="small {/Dot made|saved/.test(note) ? 'ok' : 'err'}">{note}</p>{/if}
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

    <section class="card ph">
      <h3>3 · Pen height <span class="mute small">(separate from the sheet position)</span></h3>
      <div class="phgrid">
        <div>
          <p class="mono">Contact <b>{contact.toFixed(2)}</b> mm · pressure <b>{depth.toFixed(1)}</b> mm · pen down at <b>{penDown.toFixed(2)}</b> mm · pen up at <b>{String(st.settings.travelZ ?? 30)}</b> mm</p>
          <p class="small mute">Lowest height this page will ever command: 13.5 mm.</p>
          <div class="row3">
            <button disabled={!ready || busy || !aimed} onclick={() => zn(-0.5)}>Lower 0.5</button>
            <button disabled={!ready || busy || !aimed} onclick={() => zn(-0.1)}>Lower 0.1</button>
            <button disabled={!ready || busy || !pos} onclick={() => zn(0.1)}>Raise 0.1</button>
          </div>
          <div class="row3">
            <button disabled={!ready || busy || !pos} onclick={() => zn(0.5)}>Raise 0.5</button>
            <button disabled={!ready || busy || !aimed} onclick={dotNow}>Make a test dot</button>
            <button disabled={!ready || busy || !frame} onclick={contactHere} title="Use when the tip is just touching the paper">Pen touches here</button>
          </div>
          <div class="row"><button onclick={() => setDepth(depth - 0.1)} disabled={busy}>Pressure −0.1</button><button onclick={() => setDepth(depth + 0.1)} disabled={busy}>Pressure +0.1</button></div>
        </div>
        <p class="small mute">The pen is rigid, so it only marks where the paper is within a fraction of a millimetre of the contact height. If it marks in some places but not others, the sheet is not flat: tape or flatten it, or use a spring-loaded pen holder. Raising the pressure for the whole page only makes it score the paper where it is highest.</p>
      </div>
    </section>

    <LiveCamera />
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; background: var(--bg); z-index: 22; overflow: auto; }
  .page { max-width: 1100px; margin: 0 auto; padding: 18px 20px 60px; display: grid; gap: 14px; }
  header { display: flex; justify-content: space-between; gap: 12px; align-items: flex-start; }
  .hbtn { display: flex; gap: 8px; }
  .estop { background: var(--danger); border-color: var(--danger); color: #fff; font-weight: 700; letter-spacing: .03em; }
  .grid { display: grid; grid-template-columns: 340px minmax(0, 1fr); gap: 14px; align-items: start; }
  .card { background: var(--surface); border: 1px solid var(--line); border-radius: var(--radius); padding: 14px 16px; }
  .ctl { display: grid; gap: 10px; justify-items: center; }
  .ctl > * { width: 100%; } .ctl :global(.wrap) { width: auto; }
  h3 { margin-bottom: 6px; } h3.big { font-size: 18px; margin: 2px 0 6px; }
  .pos { text-align: center; font-size: 15px; } .state { font-size: 12px; margin-left: 8px; }
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
  .ph .phgrid { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 16px; align-items: start; } .ph .row3, .ph .row { margin-top: 8px; }
  @media (max-width: 900px) { .grid, .ph .phgrid { grid-template-columns: 1fr; } }
</style>
