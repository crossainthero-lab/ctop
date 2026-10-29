<script lang="ts">
  import { app } from '../lib/app.svelte';
  import { call } from '../lib/rpc';

  let { onclose }: { onclose: () => void } = $props();
  let text = $state('Collecting…');
  let copied = $state('');
  async function load() {
    try { text = await call<string>('diagnostics', { notes: app.clientErrors.join('\n') }); } catch (e) { text = 'Could not collect diagnostics: ' + (e instanceof Error ? e.message : e) + '\n\n' + app.clientErrors.join('\n'); }
  }
  load();
  let moveMsg = $state('');
  async function testMove() {
    moveMsg = 'Moving the bed a few millimetres and back…';
    try { const r = await call<{ ok?: boolean; axis?: string; seenMm?: number }>('verify_motion'); moveMsg = r.ok ? `Movement confirmed by the camera (${r.axis} axis, ${r.seenMm?.toFixed(1)} mm).` : 'Movement check skipped.'; }
    catch (e) { moveMsg = e instanceof Error ? e.message : String(e); }
    load();
  }
  async function copy() {
    try { await navigator.clipboard.writeText(text); copied = 'Copied.'; }
    catch { const t = document.createElement('textarea'); t.value = text; document.body.appendChild(t); t.select(); copied = document.execCommand('copy') ? 'Copied.' : 'Select the text and copy it manually.'; t.remove(); }
  }
</script>

<div class="panel">
  <div class="row head"><b>Technical view</b><span class="bar"><button onclick={load}>Refresh</button><button onclick={testMove} title="Pen up: moves the bed 8 mm and back and checks that the camera sees the paper move">Test movement</button><button class="primary" onclick={copy}>Copy Diagnostics</button><button onclick={onclose}>Close</button></span></div>
  {#if moveMsg}<p>{moveMsg}</p>{/if}
  {#if copied}<p class="mute">{copied}</p>{/if}
  <pre>{text}</pre>
</div>

<style>
  .panel { position: fixed; inset: 0; background: var(--bg); overflow: auto; padding: 16px; z-index: 30; }
  .head { align-items: center; justify-content: space-between; } .head > * { flex: 0; } .bar { display: flex; gap: 8px; }
</style>
