<script lang="ts">
  import { app, refresh } from '../lib/app.svelte';
  import { call } from '../lib/rpc';

  let { onclose, oncalibrate, ontech }: { onclose: () => void; oncalibrate: () => void; ontech: () => void } = $props();
  const st = $derived(app.st!);
  let msg = $state('');
  let busy = $state(false);
  let sock = $state(String(app.st?.settings.controllerSocket ?? app.st?.controller.sock ?? ''));
  let camUrl = $state(String(app.st?.settings.cameraUrl ?? app.st?.camera.url ?? ''));
  let feed = $state(Number(app.st?.settings.feed ?? 1000));
  let depth = $state(Number(app.st?.settings.depth ?? 0.5));
  let travel = $state(Number(app.st?.settings.travelZ ?? 24));

  async function run(f: () => Promise<unknown>, ok = '') {
    busy = true; msg = '';
    try { const r = await f(); msg = typeof r === 'string' && r ? r : ok; } catch (e) { msg = e instanceof Error ? e.message : String(e); }
    busy = false; await refresh();
  }
  const set = (key: string, value: unknown) => call('set_setting', { key, value });

  // ── camera calibration ──
  let step = $state<'idle' | 'confirm' | 'drawing' | 'click'>('idle');
  let marks = $state<[number, number][]>([]);
  let clicks = $state<[number, number][]>([]);
  let snap = $state('');
  let cvs: HTMLCanvasElement | undefined = $state();
  let snapImg: HTMLImageElement | undefined;

  async function drawMarks() {
    step = 'drawing';
    await run(async () => { marks = (await call<[number, number][]>('cal_marks_draw')); });
  }
  $effect(() => { if (step === 'drawing' && st.phase === 'finished') step = 'idle'; });

  async function takeSnapshot() {
    await run(async () => {
      const f = await call<{ jpeg: string }>('frame');
      snap = 'data:image/jpeg;base64,' + f.jpeg; clicks = []; step = 'click';
    });
  }
  $effect(() => {
    if (step !== 'click' || !snap || !cvs) return;
    const im = new Image();
    im.onload = () => { snapImg = im; cvs!.width = im.width; cvs!.height = im.height; paint(); };
    im.src = snap;
  });
  function paint() {
    if (!cvs || !snapImg) return;
    const c = cvs.getContext('2d')!;
    c.drawImage(snapImg, 0, 0);
    c.strokeStyle = '#0f0'; c.lineWidth = 2;
    clicks.forEach((p, i) => { const x = p[0] * cvs!.width, y = p[1] * cvs!.height; c.beginPath(); c.arc(x, y, 10, 0, 7); c.stroke(); c.fillStyle = '#0f0'; c.font = '16px sans-serif'; c.fillText(String(i + 1), x + 12, y - 8); });
  }
  /** snap a click to the darkest blob nearby (the pencil cross) */
  function refine(px: number, py: number): [number, number] {
    const c = cvs!.getContext('2d', { willReadFrequently: true })!;
    const r = 14, x0 = Math.max(0, Math.round(px - r)), y0 = Math.max(0, Math.round(py - r));
    const w = Math.min(cvs!.width - x0, 2 * r), h = Math.min(cvs!.height - y0, 2 * r);
    const d = c.getImageData(x0, y0, w, h).data;
    const lum: number[] = []; for (let i = 0; i < d.length; i += 4) lum.push(0.3 * d[i] + 0.59 * d[i + 1] + 0.11 * d[i + 2]);
    const med = [...lum].sort((a, b) => a - b)[lum.length >> 1];
    let sx = 0, sy = 0, sw = 0;
    lum.forEach((v, i) => { const dk = med - v; if (dk > 18) { sx += (x0 + (i % w)) * dk; sy += (y0 + ((i / w) | 0)) * dk; sw += dk; } });
    return sw > 0 ? [sx / sw, sy / sw] : [px, py];
  }
  function onClick(e: MouseEvent) {
    if (clicks.length >= marks.length) return;
    const b = cvs!.getBoundingClientRect();
    const [x, y] = refine((e.clientX - b.left) * cvs!.width / b.width, (e.clientY - b.top) * cvs!.height / b.height);
    clicks = [...clicks, [x / cvs!.width, y / cvs!.height]]; paint();
  }
  async function saveCal() {
    await run(async () => { await call('cal_solve_clicks', { clicks }); step = 'idle'; return 'Camera calibrated.'; });
  }
</script>

<div class="panel">
  <div class="row head"><b>Setup</b><button onclick={onclose}>Close</button></div>

  <h2>Plotter</h2>
  <p class={st.controller.ready ? 'ok' : 'err'}>{st.controller.ready ? 'Connected and ready.' : st.controller.reachable ? st.controller.blockers.join('; ') : 'Not connected: ' + (st.controller.error ?? '')}</p>
  <label>Controller socket<input bind:value={sock} /></label>
  <button disabled={busy} onclick={() => run(() => set('controllerSocket', sock), 'Saved.')}>Save</button>

  <h2>A4 frame calibration</h2>
  <p class={st.frame.ready ? 'ok' : 'warn'}>{st.frame.active ? `"${st.frame.active.name}": ${st.frame.active.verified ? 'verified' : 'not verified'}` : 'Not calibrated yet.'}</p>
  <button onclick={oncalibrate}>{st.frame.active ? 'Review or redo the calibration' : 'Set up the frame'}</button>

  <h2>Drawing</h2>
  <div class="row">
    <label>Speed (mm/min)<input type="number" min="100" max="1500" step="50" bind:value={feed} /></label>
    <label>Pen pressure (mm below contact)<input type="number" min="0.2" max="1" step="0.1" bind:value={depth} /></label>
    <label>Travel height (Z mm)<input type="number" min="18" max="60" step="1" bind:value={travel} /></label>
  </div>
  <button disabled={busy} onclick={() => run(async () => { await set('feed', Number(feed)); await set('depth', Number(depth)); await set('travelZ', Number(travel)); }, 'Saved.')}>Save drawing settings</button>
  <p class="mute small">Pen pressure is how far below the measured contact height the pen goes. Keep it small: the pen is rigid.</p>

  <h2>Camera (optional)</h2>
  <p class="mute small">Drawing never needs it. It only gives you a live view and can suggest the paper position during calibration.</p>
  <p class={st.camera.reachable ? '' : 'mute'}>{st.camera.reachable ? 'Camera service running.' : 'No camera service: ' + (st.camera.error ?? 'not running')}</p>
  <label>Camera service address<input bind:value={camUrl} /></label>
  <div class="row">
    <button disabled={busy} onclick={() => run(() => set('cameraUrl', camUrl), 'Saved.')}>Save</button>
    <button disabled={busy || st.camera.reachable} onclick={() => run(() => call('start_camera'))}>Start camera</button>
  </div>

  <h3>Camera calibration (only for the position suggestion)</h3>
  {#if st.cal}
    <p>Calibrated (error {st.cal.rmsMm.toFixed(2)} mm). {st.cal.poseOk ? '' : 'The plotter is not in its camera position.'}</p>
  {:else}
    <p class="mute small">Not calibrated. Optional: only needed if you want the camera to suggest the paper position.</p>
  {/if}
  {#if step === 'idle'}
    <p class="mute">Put a plain sheet of paper on the bed over the middle of the working area and keep it still. The plotter draws 6 small crosses; you then click each cross in the camera picture.</p>
    <div class="row">
      <button disabled={busy || !st.controller.ready} onclick={() => run(() => call('goto_camera'), 'Moved to the camera position.')}>Move to camera position</button>
      <button disabled={busy || !st.controller.ready} onclick={() => (step = 'confirm')}>{st.cal ? 'Recalibrate…' : 'Calibrate…'}</button>
    </div>
  {:else if step === 'confirm'}
    <p><b>The plotter will now draw 6 small crosses on the paper.</b></p>
    <div class="row"><button class="primary" disabled={busy} onclick={drawMarks}>Draw crosses</button><button onclick={() => (step = 'idle')}>Cancel</button></div>
  {:else if step === 'drawing'}
    <p>{st.phase === 'finished' ? 'Done.' : 'Drawing…'}</p>
    <p class="mute">When it has finished, move the plotter back to the camera position, then take the picture.</p>
    <div class="row">
      <button disabled={busy || st.phase === 'writing' || st.phase === 'preparing'} onclick={() => run(() => call('goto_camera'))}>Move to camera position</button>
      <button disabled={busy || !st.cal?.poseOk && false} onclick={takeSnapshot}>Take picture</button>
    </div>
  {:else}
    <p>Click each of the {marks.length} crosses (any order). Clicked: {clicks.length}/{marks.length}</p>
    <canvas bind:this={cvs} onclick={onClick} class="snap"></canvas>
    <div class="row">
      <button onclick={() => { clicks = clicks.slice(0, -1); paint(); }} disabled={!clicks.length}>Undo</button>
      <button class="primary" disabled={busy || clicks.length !== marks.length} onclick={saveCal}>Save calibration</button>
      <button onclick={() => (step = 'idle')}>Cancel</button>
    </div>
  {/if}

  <p class="mute small" style="margin-top:14px"><button class="link" onclick={ontech}>Technical view and diagnostics</button></p>

  {#if msg}<p class={msg.startsWith('Camera calibrated') || msg === 'Saved.' || msg.startsWith('Moved') ? '' : 'err'}>{msg}</p>{/if}
</div>

<style>
  .panel { position: fixed; top: 0; right: 0; bottom: 0; width: min(520px, 100%); background: var(--bg); overflow: auto; padding: 16px 20px 40px; z-index: 20; box-shadow: var(--shadow); }
  button.link { border: 0; background: none; padding: 0; color: var(--accent); text-decoration: underline; }
  .head { align-items: center; justify-content: space-between; } .head > * { flex: 0; }
  .snap { width: 100%; cursor: crosshair; border: 1px solid var(--line); }
  h2 { margin: 18px 0 6px; }
  p { margin: 4px 0 8px; }
</style>
