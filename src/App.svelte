<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from './lib/api';
  import { act, machine, ui, toasts, startState, type View } from './lib/state.svelte';
  import { confirmDialog } from './lib/dialogs.svelte';
  import ConfirmHost from './components/ConfirmHost.svelte';
  import ConnectionMenu from './components/ConnectionMenu.svelte';
  import Workspace from './views/Workspace.svelte';
  import Manual from './views/Manual.svelte';
  import Calibration from './views/Calibration.svelte';
  import Jobs from './views/Jobs.svelte';
  import Settings from './views/Settings.svelte';

  const tabs: { id: View; label: string }[] = [
    { id: 'workspace', label: 'Workspace' }, { id: 'manual', label: 'Manual Control' },
    { id: 'calibration', label: 'Calibration' }, { id: 'jobs', label: 'Jobs' }, { id: 'settings', label: 'Settings' },
  ];
  let menu = $state(false);
  onMount(startState);

  const s = $derived(machine.s);
  const modeCls = $derived(s.mode === 'hardware' ? 'hw' : s.mode === 'simulation' ? 'sim' : 'off');
  const linkText = $derived(
    s.link === 'ready' ? (s.armed ? 'ARMED' : 'READY · DISARMED') : s.link === 'connecting' ? 'CONNECTING' :
    s.link === 'halted' ? 'HALTED' : s.link === 'lost' ? 'LINK LOST' : 'NOT CONNECTED');
  const canStop = $derived(s.link === 'ready' || s.link === 'connecting');

  async function estop() {
    // No confirmation: an emergency control must not need a second click.
    await act(() => api.emergencyStop());
  }
  async function key(e: KeyboardEvent) {
    if (e.key === 'Escape' && e.shiftKey) { e.preventDefault(); estop(); }
  }
  void confirmDialog;
</script>

<svelte:window onkeydown={key} />

<div class="shell">
  <header class="top">
    <div class="brand">
      <svg width="26" height="26" viewBox="0 0 32 32" aria-hidden="true"><rect x="1" y="1" width="30" height="30" fill="none" stroke="#ece9df" stroke-width="1.5"/><path d="M6 26 L22 10 l4 4 L10 30z" fill="#ece9df"/><path d="M22 10l3-3 4 4-3 3z" fill="#85827a"/></svg>
      <div><div class="name">CtoP</div><div class="sub">COMPUTER TO PENCIL</div></div>
    </div>
    <nav>
      {#each tabs as t, i}
        <button class:on={ui.view === t.id} onclick={() => (ui.view = t.id)}><span class="idx mono">{String(i + 1).padStart(2, '0')}</span>{t.label}</button>
      {/each}
    </nav>
    <div class="conn">
      <button class="pillbtn {modeCls}" onclick={() => (menu = !menu)} title="Connection">
        <span class="led {s.link}"></span>
        <span class="mode">{s.mode === 'simulation' ? 'SIMULATION' : s.mode === 'hardware' ? 'HARDWARE' : 'OFFLINE'}</span>
        <span class="state">{linkText}</span>
      </button>
      {#if menu}<ConnectionMenu onclose={() => (menu = false)} />{/if}
    </div>
    <button class="estop" onclick={estop} disabled={!canStop} title="Sends M112 immediately (Shift+Esc). Not a substitute for cutting power.">
      <span>EMERGENCY HALT</span><small>M112 · Shift+Esc</small>
    </button>
  </header>

  {#if s.attention}
    <div class="banner" class:bad={s.link === 'halted' || s.link === 'lost'}>
      <b>{s.link === 'halted' ? 'HALTED' : s.link === 'lost' ? 'CONNECTION LOST' : 'ATTENTION'}</b> {s.attention}
    </div>
  {/if}

  <main>
    {#if ui.view === 'workspace'}<Workspace />
    {:else if ui.view === 'manual'}<Manual />
    {:else if ui.view === 'calibration'}<Calibration />
    {:else if ui.view === 'jobs'}<Jobs />
    {:else}<Settings />{/if}
  </main>

  <footer>
    <span class="mono">{s.activity}</span>
    <span class="grow"></span>
    <span class="mute">Software safeguards do not replace supervision or a hardware power cut-off.</span>
    <span class="mono mute">tx {s.txCount} · rx {s.rxCount}</span>
  </footer>
</div>

<div class="toasts">
  {#each toasts.items as t (t.id)}<div class="toast {t.kind}">{t.text}</div>{/each}
</div>
<ConfirmHost />

<style>
  .shell { display: flex; flex-direction: column; height: 100%; }
  .top { flex: 0 0 46px; }
  footer { flex: 0 0 26px; }
  .top { background: var(--charcoal); color: var(--on-dark); display: flex; align-items: stretch; gap: 0; position: relative; }
  .brand { display: flex; align-items: center; gap: 10px; padding: 0 18px 0 14px; border-right: 1px solid var(--charcoal-3); }
  .name { font-weight: 700; font-size: 16px; letter-spacing: .04em; line-height: 1; }
  .sub { font-size: 8.5px; letter-spacing: .16em; color: var(--on-dark-mute); margin-top: 3px; }
  nav { display: flex; }
  nav button { background: none; border: 0; color: var(--on-dark-mute); padding: 0 16px; cursor: pointer; border-right: 1px solid var(--charcoal-3); font-weight: 500; display: flex; align-items: center; gap: 8px; }
  nav button .idx { font-size: 10px; color: #6e6c63; }
  nav button:hover { color: var(--on-dark); background: var(--charcoal-2); }
  nav button.on { color: var(--charcoal); background: var(--paper); }
  nav button.on .idx { color: var(--mute-c); }
  .conn { margin-left: auto; position: relative; display: flex; align-items: stretch; }
  .pillbtn { border: 0; border-left: 1px solid var(--charcoal-3); border-right: 1px solid var(--charcoal-3); background: var(--charcoal-2); color: var(--on-dark); display: flex; align-items: center; gap: 10px; padding: 0 16px; cursor: pointer; font-weight: 600; letter-spacing: .05em; font-size: 11.5px; }
  .pillbtn:hover { background: var(--charcoal-3); }
  .pillbtn .mode { padding: 2px 7px; border: 1px solid currentColor; }
  .pillbtn.sim .mode { color: #e6b04a; } .pillbtn.hw .mode { background: #e8e2d2; color: var(--charcoal); border-color: #e8e2d2; } .pillbtn.off .mode { color: var(--on-dark-mute); }
  .state { color: var(--on-dark-mute); }
  .led { width: 9px; height: 9px; background: #6e6c63; display: inline-block; }
  .led.ready { background: #5fb08f; } .led.connecting { background: #e6b04a; } .led.halted, .led.lost { background: #e0573b; }
  .estop { background: var(--danger); color: #fff; border: 0; padding: 0 18px; display: flex; flex-direction: column; justify-content: center; cursor: pointer; font-weight: 700; letter-spacing: .06em; font-size: 12px; text-align: left; }
  .estop small { font-weight: 500; opacity: .8; font-size: 9.5px; letter-spacing: .08em; }
  .estop:hover:not(:disabled) { background: #8f3220; }
  .estop:disabled { background: var(--charcoal-3); color: var(--on-dark-mute); cursor: not-allowed; }
  .banner { background: var(--warn-bg); border-bottom: 1px solid var(--warn); padding: 6px 16px; font-size: 12px; }
  .banner.bad { background: var(--danger-bg); border-color: var(--danger); }
  main { flex: 1 1 0; min-height: 0; min-width: 0; overflow: hidden; }
  footer { border-top: 1px solid var(--line-strong); background: var(--paper-2); display: flex; align-items: center; gap: 14px; padding: 0 14px; font-size: 11px; }
  .toasts { position: fixed; right: 16px; bottom: 40px; display: flex; flex-direction: column; gap: 8px; z-index: 200; max-width: 460px; }
  .toast { background: var(--charcoal); color: var(--on-dark); padding: 9px 14px; border-left: 4px solid var(--on-dark-mute); box-shadow: 0 6px 20px rgba(0,0,0,.3); white-space: pre-wrap; }
  .toast.error { border-color: #e0573b; } .toast.ok { border-color: #5fb08f; }
</style>
