<script lang="ts">
  // Manual calibration, nothing else: the joystick (X/Y) with its speed slider, the height slider (Z) with up/down buttons, and
  // four corner buttons. Move the pen to a corner of the paper, press that corner's button: a dot is made there and the position
  // is remembered. Then Save.
  import { app, refresh, errText } from '../lib/app.svelte';
  import { call } from '../lib/rpc';
  import { DEFAULT_JOY, type JoyOpts } from '../lib/joystick';
  import Joystick from './Joystick.svelte';
  import ZSlider from './ZSlider.svelte';

  let { onclose }: { onclose: () => void } = $props();
  const st = $derived(app.st!);
  const ready = $derived(st.controller.ready && !(st.job?.live || st.phase === 'preparing'));

  let pos = $state<[number, number, number] | null>(null);
  let inflight = false, zInflight = false;
  $effect(() => { const p = st.controller.pos; if (p && !inflight && !zInflight) pos = [p[0], p[1], p[2]]; });
  let note = $state('');
  const zMin = 0; // no limit of ours; the controller's floor follows what you ask for

  let speed = $state(15);
  const joy = $derived<JoyOpts>({ ...DEFAULT_JOY, speedMmS: speed });
  async function jog(dx: number, dy: number) {
    if (inflight || !ready) return;
    inflight = true;
    try { const r = await call<{ pos: [number, number, number] }>('jog', { dx, dy, feed: Math.max(300, Math.min(3600, speed * 60 * 1.4)), t: Date.now() }); pos = r.pos; note = ''; }
    catch (e) { const m = errText(e); if (m !== 'busy' && m !== 'stale') note = m; }
    finally { inflight = false; }
  }
  async function zStep(dz: number) {
    if (zInflight || !ready) return;
    zInflight = true;
    try { const r = await call<{ pos: [number, number, number] }>('z_nudge', { dz, t: Date.now() }); pos = r.pos; note = ''; }
    catch (e) { const m = errText(e); if (m !== 'busy' && m !== 'stale') note = m; }
    finally { zInflight = false; }
  }
  const estop = () => call('estop').catch((e) => (note = errText(e)));

  // corners
  const CORNERS = [['tl', 'Top left'], ['tr', 'Top right'], ['br', 'Bottom right'], ['bl', 'Bottom left']] as const;
  let marks = $state<Record<string, { x: number; y: number; z: number }>>({});
  let busy = $state(false);
  // The pen is wherever you put it: lower it until it touches the paper, move to the corner, press the corner's button.
  function mark(name: string) {
    if (!pos) return;
    marks[name] = { x: Math.round(pos[0] * 100) / 100, y: Math.round(pos[1] * 100) / 100, z: Math.round(pos[2] * 100) / 100 };
    savedMsg = '';
  }
  let savedMsg = $state('');
  async function save() {
    busy = true; note = '';
    try {
      await call('calib_save', { marks: CORNERS.filter(([n]) => marks[n]).map(([n]) => ({ kind: 'corner', name: n, x: marks[n].x, y: marks[n].y, z: marks[n].z })), name: 'A4 Frame' });
      savedMsg = 'Saved.';
    } catch (e) { note = errText(e); }
    finally { busy = false; refresh(); }
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
        <div class="col">
          <Joystick opts={joy} disabled={!ready || !pos} onstep={(dx, dy) => jog(dx, dy)} />
          <label class="speed">Speed {speed} mm/s <input type="range" min="1" max="80" step="1" bind:value={speed} /></label>
        </div>

        <div class="col">
          <div class="zrow">
            <ZSlider z={pos ? pos[2] : null} min={zMin} max={45} disabled={!ready || !pos} onstep={zStep} />
            <div class="zside">
              <button disabled={!ready || !pos} onclick={() => zStep(0.5)}>+0.5</button>
              <button disabled={!ready || !pos} onclick={() => zStep(0.1)}>+0.1</button>
              <div class="zval mono"><b>{pos ? pos[2].toFixed(2) : '–'}</b><span class="small mute">mm</span></div>
              <button disabled={!ready || !pos} onclick={() => zStep(-0.1)}>−0.1</button>
              <button disabled={!ready || !pos} onclick={() => zStep(-0.5)}>−0.5</button>
            </div>
          </div>
        </div>

        <div class="col corners">
          <div class="pos mono">X {pos ? pos[0].toFixed(1) : '–'} · Y {pos ? pos[1].toFixed(1) : '–'}</div>
          {#each CORNERS as [n, label]}
            <button class:done={!!marks[n]} disabled={!pos} onclick={() => mark(n)}>{label} corner{marks[n] ? `  ✓  ${marks[n].x}, ${marks[n].y}  Z ${marks[n].z}` : ''}</button>
          {/each}
          <button class="primary" disabled={busy || Object.keys(marks).length === 0} onclick={save}>Save</button>
          {#if savedMsg}<p class="small ok">{savedMsg}</p>{/if}
        </div>
      </div>
      {#if note}<p class="small err">{note}</p>{/if}
    </section>
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; background: var(--bg); z-index: 22; overflow: auto; }
  .page { max-width: 960px; margin: 0 auto; padding: 14px 20px 40px; display: grid; gap: 12px; }
  header { display: flex; justify-content: space-between; align-items: center; } .hbtn { display: flex; gap: 8px; }
  .estop { background: var(--danger); border-color: var(--danger); color: #fff; font-weight: 700; }
  .card { background: var(--surface); border: 1px solid var(--line); border-radius: var(--radius); padding: 18px; }
  .cols { display: flex; gap: 36px; flex-wrap: wrap; align-items: flex-start; justify-content: center; }
  .col { display: grid; gap: 12px; justify-items: center; align-content: start; }
  .speed { width: 260px; margin: 0; }
  .zrow { display: flex; gap: 12px; align-items: center; }
  .zside { display: grid; gap: 10px; justify-items: center; } .zside button { width: 66px; padding: 10px 0; }
  .zval { display: grid; text-align: center; } .zval b { font-size: 22px; }
  .corners { min-width: 250px; } .corners button { width: 100%; text-align: left; padding: 11px 14px; } .corners button.done { border-color: var(--ok); }
  .pos { font-size: 15px; }
</style>
