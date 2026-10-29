<script lang="ts">
  // One-time guided calibration of the fixed A4 frame. Every physical movement is started by an explicit click here.
  // The camera, if present, may only PROPOSE a paper position; the real pen's test marks decide.
  import { app, refresh, errText } from '../lib/app.svelte';
  import { call } from '../lib/rpc';
  import type { FrameProfile } from '../lib/frame';
  import { A4 } from '../lib/frame';

  let { onclose }: { onclose: () => void } = $props();
  const st = $derived(app.st!);
  const live = $derived(!!st.job?.live || st.phase === 'preparing');

  function fresh(): FrameProfile {
    const a = st.frame.active;
    if (a) return structuredClone($state.snapshot(a)) as FrameProfile;
    const o = st.frame.prior ?? [30, 40];
    return { name: 'My A4 frame', created: 0, updated: 0, pageW: A4.w, pageH: A4.h, originPen: [o[0], o[1]], rotation: 0, skewDeg: 0, surfaceZ: 0, marginMm: 8, source: 'manual', verified: false, verifiedAt: null, pageToPen: [[1, 0, 0], [0, 1, 0]] };
  }
  let draft = $state<FrameProfile>(fresh());
  let profiles = $state<FrameProfile[]>([]);
  let step = $state(1);
  let msg = $state<Record<number, { text: string; bad: boolean }>>({});
  let busy = $state(false);
  const say = (n: number, text: string, bad = false) => (msg[n] = { text, bad });

  async function loadProfiles() {
    try { profiles = (await call<{ profiles: FrameProfile[] }>('frame_list')).profiles; } catch { /* ignore */ }
  }
  loadProfiles();

  async function run<T>(n: number, f: () => Promise<T>, ok = ''): Promise<T | undefined> {
    busy = true; say(n, '');
    try { const r = await f(); if (ok) say(n, ok); return r; }
    catch (e) { say(n, errText(e), true); }
    finally { busy = false; refresh(); }
  }
  const touch = () => { draft.verified = false; draft.verifiedAt = null; };

  // ── 1. plotter ──
  const ctl = $derived(st.controller);
  let homeOk = $state(false);
  async function home() { await run(1, async () => { const r = await call<string>('home_xy', { confirmed: true }); homeOk = false; return r; }, 'X/Y homed. Z was not touched.'); }

  // ── 2. paper position ──
  type Check = { cornersPen: [number, number][]; reach: number[] | null; reachReport: { usableHeightMm: number; fraction: number } | null; warnings: string[] };
  let check = $state<Check | null>(null);
  let checkErr = $state('');
  let t: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const p = { ...$state.snapshot(draft) }; void p.originPen[0]; void p.originPen[1]; void p.rotation; void p.skewDeg; void p.marginMm;
    clearTimeout(t);
    t = setTimeout(async () => {
      try { check = await call<Check>('frame_check', { profile: p }); checkErr = ''; } catch (e) { check = null; checkErr = errText(e); }
    }, 200);
    return () => clearTimeout(t);
  });
  async function propose() {
    await run(2, async () => {
      const r = await call<{ profile: FrameProfile; notes: string[]; note: string }>('frame_propose');
      draft.originPen = r.profile.originPen; draft.rotation = r.profile.rotation; draft.skewDeg = r.profile.skewDeg; draft.source = 'camera-assisted'; touch();
      say(2, `Camera suggestion applied (about ±2 mm). ${r.notes.join(' ')} The test pattern in step 4 checks it against the real pen.`);
    });
  }

  // ── 3. pen contact ──
  let probing = $state(false);
  let probeAck = $state(false);
  let contactSet = $state(false);
  const penZ = $derived(ctl.pos?.[2] ?? null);
  async function probeBegin() { await run(3, async () => { await call('probe_begin', { profile: $state.snapshot(draft), confirmed: probeAck }); probing = true; }, 'The pen is above the test point. Lower it a little at a time.'); }
  async function probeStep(dz: number) { await run(3, async () => { await call('probe_step', { dz }); }); }
  async function probeDone(record: boolean) {
    const r = await run(3, () => call<{ z: number | null }>('probe_finish', { record }));
    if (!r) return;
    probing = false;
    if (record && r.z != null) { draft.surfaceZ = Math.round(r.z * 100) / 100; contactSet = true; touch(); say(3, `Contact height recorded: ${draft.surfaceZ.toFixed(2)} mm. The pen has been lifted.`); }
    else say(3, 'Cancelled. The pen has been lifted.');
  }

  // ── 4. test pattern + verification ──
  let testAck = $state(false);
  let testDrawn = $state(false);
  let inset = $state(14);
  let meas = $state({ blLeft: '', blBottom: '', brRight: '', brBottom: '' });
  const wasTest = $derived(st.job?.kind === 'test');
  $effect(() => { if (wasTest && st.job?.state === 'completed') testDrawn = true; });
  async function drawTest() {
    await run(4, async () => { const r = await call<{ inset: number }>('frame_test', { profile: $state.snapshot(draft) }); inset = r.inset; testDrawn = false; }, 'Drawing the test pattern…');
  }
  async function applyMeasure() {
    const n = (s: string) => parseFloat(s.replace(',', '.'));
    const v = [meas.blLeft, meas.blBottom, meas.brRight, meas.brBottom].map(n);
    if (v.some((x) => !isFinite(x))) { say(4, 'Enter all four distances in millimetres.', true); return; }
    await run(4, async () => {
      const r = await call<{ profile: FrameProfile; warnings: string[] }>('frame_adjust', { profile: $state.snapshot(draft), inset, d1: [v[0] - inset, v[1] - inset], d2: [inset - v[2], v[3] - inset] });
      draft.originPen = r.profile.originPen; draft.skewDeg = r.profile.skewDeg; touch(); testDrawn = false;
      say(4, `Position corrected. ${r.warnings.join(' ')} Draw the test pattern again (on a fresh sheet if you can) to confirm.`);
    });
  }
  function nudgeContact(d: number) { draft.surfaceZ = Math.round((draft.surfaceZ + d) * 100) / 100; touch(); testDrawn = false; }
  function confirmRight() { draft.verified = true; draft.verifiedAt = Math.floor(Date.now() / 1000); say(4, 'Verified. Save it in step 5.'); }

  // ── 5. save ──
  async function save() {
    await run(5, async () => { await call('frame_save', { profile: $state.snapshot(draft), activate: true }); await loadProfiles(); }, 'Saved and in use. CtoP remembers this after a restart and does not need the camera.');
  }
  function useProfile(name: string) {
    const p = profiles.find((x) => x.name === name);
    if (p) { draft = structuredClone($state.snapshot(p)) as FrameProfile; contactSet = p.surfaceZ > 0; testDrawn = false; }
  }
  async function activate(name: string) { await run(5, () => call('frame_activate', { name }), `"${name}" is now in use.`); }
  async function remove(name: string) { if (confirm(`Delete the calibration "${name}"?`)) await run(5, async () => { await call('frame_delete', { name }); await loadProfiles(); }); }

  // diagram of the bed: reach box + sheet, machine +Y up
  const dia = $derived.by(() => {
    const pts: [number, number][] = [...(check?.cornersPen ?? [])];
    const r = check?.reach ?? ctl.box;
    if (r) pts.push([r[0], r[2]], [r[1], r[3]]);
    if (!pts.length) return null;
    const xs = pts.map((p) => p[0]), ys = pts.map((p) => p[1]);
    const x0 = Math.min(...xs) - 10, x1 = Math.max(...xs) + 10, y0 = Math.min(...ys) - 10, y1 = Math.max(...ys) + 10;
    return { x0, w: x1 - x0, y0, h: y1 - y0, r };
  });
  const bottomEdge = $derived.by(() => {
    const c = check?.cornersPen;
    if (!c) return 'bottom edge';
    const mid = [(c[0][0] + c[1][0]) / 2, (c[0][1] + c[1][1]) / 2], cen = [(c[0][0] + c[2][0]) / 2, (c[0][1] + c[2][1]) / 2];
    const dx = mid[0] - cen[0], dy = mid[1] - cen[1];
    return Math.abs(dy) > Math.abs(dx) ? (dy < 0 ? 'edge nearest you (front)' : 'edge farthest from you (back)') : dx < 0 ? 'left-hand edge' : 'right-hand edge';
  });
  const canTest = $derived(contactSet && !!check && !checkErr && ctl.ready && !live);
</script>

<div class="scrim" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="panel" role="dialog" aria-modal="true" aria-label="Frame calibration">
    <header>
      <div><h2>Set up your A4 frame</h2><p class="mute small">Done once. The result is saved, and drawing never needs the camera again.</p></div>
      <button class="ghost" onclick={onclose} aria-label="Close">✕</button>
    </header>

    {#if profiles.length}
      <div class="profiles">
        <span class="small mute">Saved calibrations</span>
        {#each profiles as p}
          <div class="prof">
            <button class="ghost" onclick={() => useProfile(p.name)} title="Open this calibration to review or change it">{p.name}</button>
            <span class="small {p.verified ? 'ok' : 'warn'}">{p.verified ? 'verified' : 'not verified'}</span>
            {#if st.frame.activeName === p.name}<span class="tag">in use</span>{:else}<button class="ghost small" onclick={() => activate(p.name)}>Use</button>{/if}
            <button class="ghost small" onclick={() => remove(p.name)}>Delete</button>
          </div>
        {/each}
      </div>
    {/if}

    <!-- 1 -->
    <section class:cur={step === 1}>
      <button class="stephead" onclick={() => (step = 1)}><span class="n" class:done={ctl.ready}>1</span> Plotter and homing</button>
      {#if step === 1}
        <p class={ctl.ready ? 'ok' : 'err'}>{ctl.ready ? `Connected and homed. Pen at X ${ctl.pos?.[0].toFixed(1)}  Y ${ctl.pos?.[1].toFixed(1)}  Z ${ctl.pos?.[2].toFixed(1)}.` : ctl.reachable ? ctl.blockers.join('; ') : 'The plotter is not connected: ' + (ctl.error ?? '')}</p>
        <p class="mute small">The saved frame is measured in the plotter's homed coordinates, so the plotter must be homed. It normally already is. Re-home only after a power cycle or if the head was moved by hand.</p>
        <details><summary class="small">Re-home X/Y</summary>
          <p class="small">The pen rises to Z 30 mm, then the head homes to its X/Y end stops. Make sure nothing, including the frame, can catch the pen on the way.</p>
          <label class="check"><input type="checkbox" bind:checked={homeOk} /> The pen is raised and the head is clear.</label>
          <button disabled={!homeOk || busy || live || !ctl.reachable} onclick={home}>Home X/Y now</button>
        </details>
        {#if msg[1]?.text}<p class={msg[1].bad ? 'err' : 'ok'}>{msg[1].text}</p>{/if}
        <button class="primary" disabled={!ctl.ready} onclick={() => (step = 2)}>Next</button>
      {/if}
    </section>

    <!-- 2 -->
    <section class:cur={step === 2}>
      <button class="stephead" onclick={() => (step = 2)}><span class="n" class:done={!!check && !checkErr}>2</span> Where the paper sits</button>
      {#if step === 2}
        <p class="mute small">Put an A4 sheet in the frame, pushed fully into it. Then either let the camera suggest the position, or enter it.</p>
        <div class="row"><button disabled={busy || !st.camera.reachable} onclick={propose} title={st.camera.reachable ? '' : 'No camera connected: enter the values instead'}>Suggest position with camera</button></div>
        {#if !st.camera.reachable}<p class="small mute">No camera is connected. That is fine, enter the values below.</p>{/if}
        <div class="row">
          <label>Sheet corner X (mm)<input type="number" step="0.1" bind:value={draft.originPen[0]} oninput={touch} /></label>
          <label>Sheet corner Y (mm)<input type="number" step="0.1" bind:value={draft.originPen[1]} oninput={touch} /></label>
        </div>
        <p class="small mute">The corner is the bottom-left corner of the page when it is held upright, in the plotter's pen coordinates. {#if st.frame.prior && !st.frame.active}Pre-filled from your earlier measurement; the test pattern checks it.{/if}</p>
        <label>Which way is the top of the page?
          <select bind:value={draft.rotation} onchange={touch}>
            <option value={0}>Away from me, toward the back of the plotter</option>
            <option value={180}>Toward me, the front of the plotter</option>
            <option value={90}>Toward the left</option>
            <option value={270}>Toward the right</option>
          </select>
        </label>
        <div class="row">
          <label>Safe margin (mm)<input type="number" min="3" max="40" step="1" bind:value={draft.marginMm} oninput={touch} /></label>
          <label>Fine tilt (°)<input type="number" min="-5" max="5" step="0.05" bind:value={draft.skewDeg} oninput={touch} /></label>
        </div>
        {#if dia}
          <svg class="dia" viewBox="{dia.x0} {-(dia.y0 + dia.h)} {dia.w} {dia.h}" role="img" aria-label="Plan view of the plotter's reach and the paper">
            {#if dia.r}<rect x={dia.r[0]} y={-dia.r[3]} width={dia.r[1] - dia.r[0]} height={dia.r[3] - dia.r[2]} class="reach" />{/if}
            {#if check}<polygon class="paper" points={check.cornersPen.map((c) => `${c[0]},${-c[1]}`).join(' ')} />
              <circle cx={check.cornersPen[0][0]} cy={-check.cornersPen[0][1]} r="4" class="origin" />{/if}
          </svg>
          <p class="small mute">Blue area: what the plotter can reach. Grey sheet: your paper. Dot: bottom-left of the page. Back of the plotter is at the top.</p>
        {/if}
        {#if checkErr}<p class="err">{checkErr}</p>{/if}
        {#if check}
          {#each check.warnings as w}<p class="warn small">{w}</p>{/each}
          {#if check.reachReport}<p class="small mute">Usable height on the sheet: about {check.reachReport.usableHeightMm.toFixed(0)} of {(draft.pageH - 2 * draft.marginMm).toFixed(0)} mm.</p>{/if}
        {/if}
        {#if msg[2]?.text}<p class={msg[2].bad ? 'err' : 'ok'}>{msg[2].text}</p>{/if}
        <button class="primary" disabled={!check || !!checkErr} onclick={() => (step = 3)}>Next</button>
      {/if}
    </section>

    <!-- 3 -->
    <section class:cur={step === 3}>
      <button class="stephead" onclick={() => (step = 3)}><span class="n" class:done={contactSet}>3</span> Pen contact height</button>
      {#if step === 3}
        <p class="mute small">The plotter has to know exactly how high the pen tip is when it just touches the paper. It is never guessed: you find it here, one small step at a time.</p>
        {#if !probing}
          <label class="check"><input type="checkbox" bind:checked={probeAck} /> The sheet is seated and taped flat, the pen is fitted, and I can see the pen.</label>
          <button disabled={!probeAck || busy || live || !ctl.ready || !check} onclick={probeBegin}>Move the pen over the middle of the sheet</button>
          <p class="small mute">The pen rises first and moves at travel height. It stops above the middle of the page.</p>
        {:else}
          <p class="big-z">Pen height <b class="mono">{penZ?.toFixed(2) ?? '–'}</b> mm</p>
          <p class="small mute">Lower in small steps until the pen tip just touches the paper (slide a thin strip of paper under it, or watch for the sheet to dimple). Stop at first touch; never press into the paper. The plotter will not go below its own safety floor.</p>
          <div class="steps">
            {#each [1, 0.5, 0.2, 0.1] as d}<button disabled={busy} onclick={() => probeStep(-d)}>Lower {d}</button>{/each}
          </div>
          <div class="steps">
            {#each [0.1, 0.5, 1] as d}<button disabled={busy} onclick={() => probeStep(d)}>Raise {d}</button>{/each}
          </div>
          <div class="row">
            <button class="primary" disabled={busy} onclick={() => probeDone(true)}>It is touching now: record</button>
            <button disabled={busy} onclick={() => probeDone(false)}>Cancel, lift the pen</button>
          </div>
        {/if}
        <label>Contact height (mm){#if draft.surfaceZ > 0 && !contactSet} (from saved calibration){/if}
          <input type="number" step="0.01" bind:value={draft.surfaceZ} oninput={() => { touch(); contactSet = draft.surfaceZ > 0; }} />
        </label>
        {#if msg[3]?.text}<p class={msg[3].bad ? 'err' : 'ok'}>{msg[3].text}</p>{/if}
        <button class="primary" disabled={!contactSet || probing} onclick={() => (step = 4)}>Next</button>
      {/if}
    </section>

    <!-- 4 -->
    <section class:cur={step === 4}>
      <button class="stephead" onclick={() => (step = 4)}><span class="n" class:done={draft.verified}>4</span> Check with a test pattern</button>
      {#if step === 4}
        <p class="mute small">The plotter draws four small crosses near the corners of the safe area ({inset} mm in from the edges) and a box in the middle. It uses the same code path as real drawings.</p>
        <label class="check"><input type="checkbox" bind:checked={testAck} /> A sheet is in the frame and I am ready for the pen to move and draw.</label>
        <button class="primary" disabled={!testAck || !canTest} onclick={drawTest}>Draw the test pattern</button>
        {#if wasTest && st.job}
          <div class="mini"><div class="bar"><div style="width:{st.job.percent}%"></div></div><span class="small mute">{st.job.operation} · {st.job.percent.toFixed(0)}%</span></div>
        {/if}
        {#if st.phase === 'error' && st.error}<p class="err">{st.error.message} {st.error.detail}</p><button class="small" onclick={() => call('dismiss')}>Clear</button>{/if}
        {#if testDrawn}
          <h3>Do the crosses sit where they should?</h3>
          <p class="small mute">With a ruler, measure the lower two crosses from the sheet edges. The page's bottom is the {bottomEdge}. They should be {inset} mm from each edge.</p>
          <div class="row">
            <label>Left cross: from left edge<input inputmode="decimal" bind:value={meas.blLeft} placeholder={String(inset)} /></label>
            <label>Left cross: from bottom edge<input inputmode="decimal" bind:value={meas.blBottom} placeholder={String(inset)} /></label>
          </div>
          <div class="row">
            <label>Right cross: from right edge<input inputmode="decimal" bind:value={meas.brRight} placeholder={String(inset)} /></label>
            <label>Right cross: from bottom edge<input inputmode="decimal" bind:value={meas.brBottom} placeholder={String(inset)} /></label>
          </div>
          <div class="row">
            <button disabled={busy || live} onclick={applyMeasure}>Correct using these measurements</button>
            <button class="primary" disabled={busy || live} onclick={confirmRight}>They are in the right place</button>
          </div>
          <p class="small mute">Marks too faint or the pen skipping? <button class="link" onclick={() => nudgeContact(-0.1)}>press 0.1 mm harder</button>. Marks heavy or the paper scored? <button class="link" onclick={() => nudgeContact(0.1)}>press 0.1 mm lighter</button>. Then draw the pattern again. Contact now {draft.surfaceZ.toFixed(2)} mm.</p>
        {/if}
        {#if msg[4]?.text}<p class={msg[4].bad ? 'err' : 'ok'}>{msg[4].text}</p>{/if}
        <p class="small {draft.verified ? 'ok' : 'warn'}">{draft.verified ? 'Verified with a physical test.' : 'Not verified yet: drawing stays locked until you confirm the test pattern.'}</p>
      {/if}
    </section>

    <!-- 5 -->
    <section class:cur={step === 5}>
      <button class="stephead" onclick={() => (step = 5)}><span class="n" class:done={st.frame.activeName === draft.name && st.frame.ready}>5</span> Save</button>
      {#if step === 5}
        <label>Name this calibration<input bind:value={draft.name} maxlength="60" /></label>
        <p class="small mute">Recalibrate only if the frame, the pen mounting or the machine changes.</p>
        <button class="primary" disabled={busy || !draft.name.trim()} onclick={save}>Save and use</button>
        {#if msg[5]?.text}<p class={msg[5].bad ? 'err' : 'ok'}>{msg[5].text}</p>{/if}
      {/if}
    </section>
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; background: rgba(0,0,0,.4); display: flex; justify-content: flex-end; z-index: 20; }
  .panel { width: min(560px, 100%); height: 100%; overflow: auto; background: var(--bg); padding: 18px 20px 40px; box-shadow: var(--shadow); }
  header { display: flex; justify-content: space-between; gap: 12px; margin-bottom: 12px; }
  section { border: 1px solid var(--line); background: var(--surface); border-radius: var(--radius); padding: 4px 14px 12px; margin-bottom: 10px; }
  section.cur { border-color: var(--accent); }
  .stephead { display: flex; gap: 10px; align-items: center; width: 100%; border: 0; background: none; font-weight: 600; padding: 10px 0; text-align: left; }
  .n { width: 22px; height: 22px; border-radius: 50%; border: 1.5px solid var(--mute); display: grid; place-items: center; font-size: 12px; flex: none; }
  .n.done { background: var(--ok); border-color: var(--ok); color: #fff; }
  .profiles { margin-bottom: 12px; display: grid; gap: 4px; }
  .prof { display: flex; gap: 8px; align-items: center; } .prof > :first-child { flex: 1; text-align: left; }
  .tag { background: var(--accent-soft); color: var(--accent); border-radius: 99px; padding: 1px 8px; font-size: 12px; }
  .dia { width: 100%; max-height: 260px; background: var(--soft); border-radius: 8px; margin: 6px 0; }
  .dia .reach { fill: var(--accent-soft); stroke: var(--accent); stroke-width: 1; vector-effect: non-scaling-stroke; }
  .dia .paper { fill: rgba(120,120,120,.35); stroke: var(--ink); stroke-width: 1; vector-effect: non-scaling-stroke; }
  .dia .origin { fill: var(--danger); }
  .big-z { font-size: 20px; margin: 6px 0; }
  .steps { display: flex; gap: 6px; margin-bottom: 6px; } .steps > * { flex: 1; padding: 9px 4px; }
  .mini { display: flex; gap: 10px; align-items: center; margin: 8px 0; } .bar { flex: 1; height: 6px; background: var(--soft); border-radius: 4px; overflow: hidden; } .bar > div { height: 100%; background: var(--accent); }
  button.link { border: 0; background: none; padding: 0; color: var(--accent); text-decoration: underline; }
  details { margin: 8px 0; }
  summary { cursor: pointer; }
</style>
