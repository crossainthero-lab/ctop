<script lang="ts">
  // Two independent live views. Nothing depends on them: progress and safety come from the plotter. One failing
  // camera never affects the other, and both reconnect by themselves.
  import { errText } from '../lib/app.svelte';
  import { call } from '../lib/rpc';

  type Info = { id: 'paper' | 'side'; label: string; hub: boolean; live: boolean; state: string; reason: string | null; frameAgeS: number | null; size?: string; fps?: number; measuredFps?: number; wanted?: string };
  const IDS = ['paper', 'side'] as const;
  let open = $state(false);
  let layout = $state<'stack' | 'side'>('stack');
  try { open = localStorage.getItem('ctop-cam-open') === '1'; layout = localStorage.getItem('ctop-cam-layout') === 'side' ? 'side' : 'stack'; } catch { /* storage unavailable */ }
  let img = $state<Record<string, string>>({ paper: '', side: '' });
  let fails = $state<Record<string, number>>({ paper: 0, side: 0 });
  let info = $state<Record<string, Info | undefined>>({});
  let big = $state<'paper' | 'side' | null>(null);
  let msg = $state('');

  function persist() { try { localStorage.setItem('ctop-cam-open', open ? '1' : '0'); localStorage.setItem('ctop-cam-layout', layout); } catch { /* ignore */ } }

  // one loop per camera, started only while the panel is open, torn down when it closes
  $effect(() => {
    if (!open) return;
    let alive = true;
    for (const id of IDS) {
      (async () => {
        while (alive) {
          if (!document.hidden) {
            try {
              const f = await call<{ jpeg: string }>('frame', id === 'side' ? { side: true } : undefined);
              if (!alive) return;
              img[id] = 'data:image/jpeg;base64,' + f.jpeg; fails[id] = 0;
            } catch { if (alive) { fails[id]++; if (fails[id] > 3) img[id] = ''; } }
          }
          await new Promise((r) => setTimeout(r, fails[id] ? Math.min(5000, 600 * fails[id]) : 250));
        }
      })();
    }
    (async () => {
      while (alive) {
        try { const l = await call<Info[]>('cameras'); if (alive) for (const c of l) info[c.id] = c; } catch { /* backend busy */ }
        await new Promise((r) => setTimeout(r, 2000));
      }
    })();
    return () => { alive = false; };
  });

  const status = (id: string) => {
    const i = info[id];
    if (img[id] && fails[id] === 0) return { t: 'Live', c: 'ok' };
    if (!i) return { t: 'Connecting…', c: '' };
    if (!i.hub) return { t: 'Camera service not running', c: 'bad' };
    if (i.state === 'streaming') return { t: 'Reconnecting…', c: 'warn' };
    return { t: `Not streaming (${i.state})`, c: 'bad' };
  };
  async function retry(id: string) {
    msg = '';
    try {
      if (info[id]?.hub) await call('camera_resume', { side: id === 'side' });
      else msg = await call<string>('start_camera');
    } catch (e) { msg = errText(e); }
  }
  const SIZES = [['1920x1080', '1080p (1920×1080)'], ['1280x720', '720p (1280×720)'], ['800x600', '800×600'], ['640x480', '480p (640×480)']];
  let modeErr = $state<Record<string, string>>({});
  const wantSize = (id: string) => { const w = info[id]?.wanted ?? 'auto'; return w === 'auto' ? 'auto' : w.split('@')[0]; };
  const wantFps = (id: string) => { const w = info[id]?.wanted ?? 'auto'; return w === 'auto' ? '30' : w.split('@')[1]; };
  async function setMode(id: string, size: string, fps: string) {
    modeErr[id] = '';
    try { await call('camera_set_mode', { side: id === 'side', mode: size === 'auto' ? 'auto' : `${size}@${fps}` }); }
    catch (e) { modeErr[id] = errText(e); }
  }
  const label = (id: string) => (id === 'paper' ? 'Paper Camera' : 'Side Camera');
</script>

{#snippet feed(id: 'paper' | 'side', enlarge: boolean)}
  <figure class="feed">
    <figcaption><b>{label(id)}</b><span class="st {status(id).c}"><i></i>{status(id).t}</span></figcaption>
    <div class="shot" role="button" tabindex="0" onclick={() => (big = id)} onkeydown={(e) => e.key === 'Enter' && (big = id)} aria-label="Enlarge {label(id)}">
      {#if img[id]}<img src={img[id]} alt="{label(id)} live view" />{:else}<div class="none"><p class="small mute">{info[id]?.reason ?? 'No picture yet.'}</p><button class="small" onclick={(e) => { e.stopPropagation(); retry(id); }}>Retry</button></div>{/if}
    </div>
    <div class="qual small">
      <label>Quality
        <select value={wantSize(id)} onchange={(e) => setMode(id, e.currentTarget.value, wantFps(id))}>
          <option value="auto">Auto (best: 1080p)</option>{#each SIZES as [v, n]}<option value={v}>{n}</option>{/each}
        </select>
      </label>
      <label>Frame rate
        <select value={wantFps(id)} disabled={wantSize(id) === 'auto'} onchange={(e) => setMode(id, wantSize(id), e.currentTarget.value)}>
          <option value="30">30 fps</option><option value="15">15 fps</option><option value="10">10 fps</option>
        </select>
      </label>
    </div>
    <p class="small mute now">{#if info[id]?.size}Now {info[id]?.size} @ {info[id]?.fps} fps{#if info[id]?.measuredFps} (measured {info[id]?.measuredFps?.toFixed(0)}){/if}{/if}</p>
    {#if modeErr[id]}<p class="small err">{modeErr[id]}</p>{/if}
  </figure>
{/snippet}

<section class="cam">
  <button class="head ghost" onclick={() => { open = !open; persist(); }} aria-expanded={open}>
    <span>Cameras <span class="mute small">(optional, drawing does not need them)</span></span>
    <span class="chev" class:up={open}>⌄</span>
  </button>
  {#if open}
    <div class="body">
      <div class="bar small">
        <label class="check"><input type="radio" checked={layout === 'stack'} onchange={() => { layout = 'stack'; persist(); }} /> Stacked</label>
        <label class="check"><input type="radio" checked={layout === 'side'} onchange={() => { layout = 'side'; persist(); }} /> Side by side</label>
      </div>
      <div class="grid" class:two={layout === 'side'}>
        {@render feed('paper', false)}
        {@render feed('side', false)}
      </div>
      {#if msg}<p class="small mute">{msg}</p>{/if}
    </div>
  {/if}
</section>

{#if big}
  <div class="scrim" role="presentation" onclick={() => (big = null)} onkeydown={(e) => e.key === 'Escape' && (big = null)}>
    <div class="lightbox" role="dialog" aria-modal="true" aria-label={label(big)}>
      <div class="lbhead"><b>{label(big)}</b><span class="st {status(big).c}"><i></i>{status(big).t}</span><span class="sp"></span><button onclick={() => (big = null)}>Close</button></div>
      {#if img[big]}<img src={img[big]} alt="{label(big)} live view" />{:else}<p class="mute">{info[big]?.reason ?? 'No picture.'}</p>{/if}
    </div>
  </div>
{/if}

<style>
  .cam { border: 1px solid var(--line); border-radius: var(--radius); background: var(--surface); overflow: hidden; }
  .head { width: 100%; display: flex; justify-content: space-between; align-items: center; border: 0; border-radius: 0; padding: 9px 12px; color: var(--ink); text-align: left; }
  .chev { transition: transform .15s; display: inline-block; } .chev.up { transform: rotate(180deg); }
  .body { padding: 0 12px 10px; }
  .bar { display: flex; gap: 14px; } .bar label { margin: 0 0 6px; }
  .grid { display: grid; gap: 10px; } .grid.two { grid-template-columns: 1fr 1fr; }
  .feed { margin: 0; }
  figcaption { display: flex; justify-content: space-between; align-items: center; font-size: 12.5px; margin-bottom: 4px; }
  .st { display: inline-flex; gap: 5px; align-items: center; font-size: 11.5px; color: var(--mute); } .st i { width: 7px; height: 7px; border-radius: 50%; background: var(--mute); }
  .st.ok i { background: var(--ok); } .st.bad i { background: var(--danger); } .st.warn i { background: var(--warn); }
  .shot { cursor: zoom-in; aspect-ratio: 4 / 3; background: #000; border-radius: 6px; overflow: hidden; display: grid; place-items: center; }
  .shot img { width: 100%; height: 100%; object-fit: contain; display: block; }
  .qual { display: flex; gap: 8px; margin-top: 6px; } .qual label { flex: 1; margin: 0; } .now { margin: 2px 0 0; }
  .none { padding: 10px; text-align: center; color: #bbb; } .none p { margin-bottom: 6px; }
  .scrim { position: fixed; inset: 0; background: rgba(0,0,0,.75); display: grid; place-items: center; z-index: 40; padding: 16px; }
  .lightbox { background: var(--surface); border-radius: var(--radius); padding: 12px; max-width: min(1100px, 100%); max-height: 100%; display: grid; gap: 8px; }
  .lightbox img { max-width: 100%; max-height: 75vh; object-fit: contain; border-radius: 6px; justify-self: center; }
  .lbhead { display: flex; gap: 12px; align-items: center; } .sp { flex: 1; }
</style>
