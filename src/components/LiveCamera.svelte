<script lang="ts">
  // Optional live view. Nothing depends on it: progress and safety come from the plotter, not from this picture.
  import { app, errText } from '../lib/app.svelte';
  import { call } from '../lib/rpc';

  let open = $state(false);
  try { open = localStorage.getItem('ctop-cam-open') === '1'; } catch { /* storage unavailable */ }
  let side = $state(false);
  let img = $state('');
  let err = $state('');
  let msg = $state('');
  const st = $derived(app.st);

  function toggle() {
    open = !open;
    try { localStorage.setItem('ctop-cam-open', open ? '1' : '0'); } catch { /* ignore */ }
  }

  $effect(() => {
    if (!open) return;
    let alive = true;
    const isSide = side;
    (async () => {
      while (alive) {
        if (!document.hidden) {
          try {
            const f = await call<{ jpeg: string }>('frame', isSide ? { side: true } : undefined);
            if (alive) { img = 'data:image/jpeg;base64,' + f.jpeg; err = ''; }
          } catch (e) { if (alive) { img = ''; err = errText(e); } }
        }
        await new Promise((r) => setTimeout(r, err ? 2000 : 300));
      }
    })();
    return () => { alive = false; };
  });

  async function start() {
    msg = '';
    try { msg = await call<string>('start_camera'); } catch (e) { msg = errText(e); }
  }
</script>

<section class="cam">
  <button class="head ghost" onclick={toggle} aria-expanded={open}>
    <span>Live camera <span class="mute small">(optional)</span></span>
    <span class="chev" class:up={open}>⌄</span>
  </button>
  {#if open}
    <div class="body">
      {#if img}
        <img src={img} alt="Live view of the plotter" />
      {:else}
        <div class="none">
          <p class="mute small">{err ? 'The camera is not available. Drawing does not need it.' : 'Waiting for a picture…'}</p>
          {#if !st?.camera.reachable}<button class="small" onclick={start}>Start camera</button>{/if}
          {#if msg}<p class="small mute">{msg}</p>{/if}
        </div>
      {/if}
      <div class="row small">
        <label class="check"><input type="radio" checked={!side} onchange={() => (side = false)} /> Top view</label>
        <label class="check"><input type="radio" checked={side} onchange={() => (side = true)} /> Side view</label>
      </div>
    </div>
  {/if}
</section>

<style>
  .cam { border: 1px solid var(--line); border-radius: var(--radius); background: var(--surface); overflow: hidden; }
  .head { width: 100%; display: flex; justify-content: space-between; align-items: center; border: 0; border-radius: 0; padding: 9px 12px; color: var(--ink); }
  .chev { transition: transform .15s; display: inline-block; } .chev.up { transform: rotate(180deg); }
  .body { padding: 0 12px 10px; }
  img { width: 100%; display: block; border-radius: 6px; background: #000; margin-bottom: 6px; }
  .none { padding: 14px 4px; text-align: center; }
  .row { display: flex; gap: 14px; } .row label { margin: 0; }
</style>
