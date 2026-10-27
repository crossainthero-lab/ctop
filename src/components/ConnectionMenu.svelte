<script lang="ts">
  import { api, type PortInfo, type PortCheck } from '../lib/api';
  import { act, machine, ui, toast } from '../lib/state.svelte';
  import { confirmDialog } from '../lib/dialogs.svelte';

  let { onclose }: { onclose: () => void } = $props();
  let ports = $state<PortInfo[]>([]);
  let chosen = $state('');
  let check = $state<PortCheck | null>(null);
  const s = $derived(machine.s);
  const connected = $derived(s.link !== 'disconnected');
  const unlocked = $derived(ui.settings.hardwareUnlocked === true);

  async function refresh() {
    ports = await api.listPorts().catch(() => []);
    if (!chosen) chosen = ports[0]?.path ?? '';
  }
  $effect(() => { refresh(); });

  async function doCheck() {
    if (!chosen) return;
    check = await api.checkPort(chosen);
  }
  async function connectSim() {
    if (await act(() => api.connectSimulation())) onclose();
  }
  async function connectHw() {
    if (!unlocked) { toast('Hardware mode is locked — unlock it in Settings first.', 'error'); return; }
    await doCheck();
    if (check && !check.available) { toast(check.message, 'error'); return; }
    const ok = await confirmDialog({
      title: 'Connect to the real printer?',
      body: `Device: ${chosen}\n\nOpening the serial port normally RESETS the printer's controller. Nothing will move on connect; position will be unknown until you home or verify it.`,
      checks: [
        'The other session that was calibrating has disconnected and released the printer.',
        'I have physically checked the printer: bed clear, pencil clear of the paper, power switch within reach.',
        'I understand CtoP will not move anything until I arm it and confirm each action.',
      ],
      confirmLabel: 'Connect to printer',
    });
    if (!ok) return;
    if (await act(() => api.connectHardware(chosen))) onclose();
  }
  async function disconnect() {
    if (await act(() => api.disconnect())) onclose();
  }
</script>

<div class="menu" role="dialog">
  <div class="row between"><h3>Connection</h3><button class="btn ghost sm" onclick={onclose}>✕</button></div>
  {#if connected}
    <div class="kv">
      <span>Mode</span><span>{s.mode}</span>
      <span>Link</span><span>{s.link}</span>
      <span>Device</span><span class="mono">{s.device}</span>
    </div>
    <button class="btn" onclick={disconnect}>Disconnect</button>
  {:else}
    <div class="opt">
      <div><b>Simulation</b><div class="mute">A virtual Marlin controller. Safe default; the whole app works.</div></div>
      <button class="btn primary" onclick={connectSim}>Connect simulator</button>
    </div>
    <div class="opt">
      <div>
        <b>Real printer</b>
        <div class="mute">115200 baud, exclusive access. {unlocked ? 'Unlocked.' : 'LOCKED in Settings.'}</div>
      </div>
      <div class="col">
        <select bind:value={chosen} onchange={() => (check = null)}>
          {#each ports as p}<option value={p.path}>{p.label}{p.byId ? ' (by-id)' : ''}</option>{/each}
          {#if !ports.length}<option value="">No serial devices found</option>{/if}
        </select>
        <div class="row">
          <button class="btn sm" onclick={doCheck} disabled={!chosen}>Check if in use</button>
          <button class="btn" onclick={connectHw} disabled={!chosen || !unlocked}>Connect…</button>
        </div>
        {#if check}<div class="notice" class:ok={check.available} class:bad={!check.available}>
          {check.message}
          {#each check.holders as h}<div class="mono">{h.command} · pid {h.pid} · {h.how}</div>{/each}
        </div>{/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .menu { position: absolute; top: 44px; left: 0; width: 380px; background: var(--paper); color: var(--ink); border: 1px solid var(--ink); padding: 12px; z-index: 50; display: flex; flex-direction: column; gap: 12px; box-shadow: 0 12px 30px rgba(0,0,0,.3); }
  .between { justify-content: space-between; }
  .opt { display: flex; flex-direction: column; gap: 8px; padding: 10px; border: 1px solid var(--line); background: var(--panel); }
</style>
