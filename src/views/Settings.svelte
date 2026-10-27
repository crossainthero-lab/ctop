<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type PortInfo, type PortCheck, type ProfilesFile } from '../lib/api';
  import { act, machine, ui, toast } from '../lib/state.svelte';
  import { confirmDialog } from '../lib/dialogs.svelte';

  let ports = $state<PortInfo[]>([]);
  let checks = $state<Record<string, PortCheck>>({});
  let profiles = $state<ProfilesFile | null>(null);
  const unlocked = $derived(ui.settings.hardwareUnlocked === true);
  const connected = $derived(machine.s.link !== 'disconnected');

  async function load() {
    ports = await api.listPorts().catch(() => []);
    profiles = await api.profiles().catch(() => null);
  }
  onMount(load);

  async function setUnlock(v: boolean) {
    if (v) {
      const ok = await confirmDialog({
        title: 'Unlock hardware mode?',
        body: 'This allows CtoP to open the serial port and send motion commands to the real printer (only after you also connect, arm and verify the position).',
        checks: [
          'The other Claude session that was calibrating the printer has released the serial port.',
          'The printer has been physically checked: pencil mounted, paper placed, bed clear, power switch within reach.',
          'I accept that software safeguards do not replace supervision.',
        ],
        confirmLabel: 'Unlock hardware mode', danger: true,
      });
      if (!ok) return;
    }
    if (await act(() => api.setSetting('hardwareUnlocked', v))) ui.settings = { ...ui.settings, hardwareUnlocked: v };
  }
  async function check(p: string) { checks[p] = await api.checkPort(p); }
  async function selectProfile(id: string) { if (await act(() => api.selectProfile(id))) { await load(); } }
  async function savePath(id: string, path: string) { if (await act(() => api.updateProfile(id, undefined, path))) toast('Saved', 'ok', 1500); }
</script>

<div class="settings">
  <section class="col">
    <div class="panel">
      <header><h3>Hardware access</h3>{#if unlocked}<span class="pill bad">Unlocked</span>{:else}<span class="pill ok">Locked</span>{/if}</header>
      <div class="body col">
        <p class="m0">Hardware mode is <b>locked by default</b>. While locked, CtoP cannot open the serial device at all and runs only against the simulator.</p>
        <div class="row">
          <button class="btn" class:danger={!unlocked} onclick={() => setUnlock(!unlocked)} disabled={connected && unlocked}>{unlocked ? 'Lock hardware mode' : 'Unlock hardware mode…'}</button>
          {#if connected && unlocked}<span class="mute">Disconnect first.</span>{/if}
        </div>
      </div>
    </div>

    <div class="panel">
      <header><h3>Device profiles</h3></header>
      <div class="body col">
        {#if profiles}
          {#each profiles.profiles as p}
            <div class="prof" class:act={profiles.active === p.id}>
              <div class="row">
                <b>{p.name}</b><span class="pill">{p.kind}</span>
                <span class="grow"></span>
                {#if profiles.active === p.id}<span class="pill ok">active</span>{:else}<button class="btn sm" disabled={connected} onclick={() => selectProfile(p.id)}>Use</button>{/if}
              </div>
              {#if p.kind === 'hardware'}
                <div class="field"><span class="label">Serial device</span>
                  <input type="text" class="mono" value={p.serialPath ?? ''} onchange={(e) => savePath(p.id, (e.target as HTMLInputElement).value)} /></div>
                <div class="mute small">Expected firmware: <span class="mono">{p.expectedFirmware}</span> · 115200 baud</div>
              {:else}<div class="mute small">Simulated machine with its own self-consistent calibration. It says nothing about real hardware.</div>{/if}
            </div>
          {/each}
        {/if}
      </div>
    </div>
  </section>

  <section class="col">
    <div class="panel">
      <header><h3>Serial devices</h3><button class="btn sm" onclick={load}>Rescan</button></header>
      <div class="body col">
        {#each ports as p}
          <div class="prof">
            <div class="row"><span class="mono grow">{p.label}</span>{#if p.byId}<span class="pill">stable id</span>{/if}</div>
            <div class="mono mute small">{p.path} → {p.realPath}</div>
            <div class="row"><button class="btn sm" onclick={() => check(p.path)}>Check if in use (does not open it)</button></div>
            {#if checks[p.path]}
              <div class="notice small" class:ok={checks[p.path].available} class:bad={!checks[p.path].available}>{checks[p.path].message}
                {#each checks[p.path].holders as h}<div class="mono">{h.command} · pid {h.pid} · {h.how}</div>{/each}</div>
            {/if}
          </div>
        {:else}<div class="mute">No serial devices present.</div>{/each}
        <p class="mute small m0">Checks read /proc and lock tables only; they never open the port, because opening an FTDI/Arduino port usually resets the controller. Processes owned by other users may be invisible to this check.</p>
      </div>
    </div>

    <div class="panel">
      <header><h3>Keyboard</h3></header>
      <div class="body kv small mono">
        <span>Shift+Esc</span><span>Emergency halt (M112)</span>
        <span>Manual: ← → ↑ ↓</span><span>Jog X / Y</span>
        <span>Manual: PgUp PgDn</span><span>Jog Z (raise / lower)</span>
        <span>Manual: 1 2 3 4</span><span>Select step 0.1 / 1 / 5 / 10 mm</span>
        <span>Workspace: V T P H</span><span>Select / Text / Pen / Pan tools</span>
        <span>Ctrl+Z / Ctrl+Shift+Z</span><span>Undo / redo</span>
        <span>Ctrl+S / Ctrl+O</span><span>Save / open project</span>
        <span>Ctrl+D · Del</span><span>Duplicate · delete</span>
        <span>0 · Ctrl+wheel</span><span>Fit to page · zoom</span>
      </div>
    </div>

    <div class="panel">
      <header><h3>About &amp; safety</h3></header>
      <div class="body small col">
        <div><b>CtoP — Computer to Pencil</b> 0.1.0</div>
        <p class="m0">CtoP validates every move against a calibrated safe envelope, caps feedrates, never homes or moves automatically, and stops sending on any link fault. These are software safeguards only: they are not collision-proof, and cannot replace supervision or a hardware power cut-off. If anything collides, cut printer power.</p>
        <p class="m0">Never sent by design: heater, fan, extruder, relative-mode or probing commands.</p>
      </div>
    </div>
  </section>
</div>

<style>
  .settings { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; padding: 12px; height: 100%; overflow-y: auto; align-items: start; }
  .m0 { margin: 0; } .small { font-size: 11.5px; }
  .prof { border: 1px solid var(--line); background: #fcfbf8; padding: 9px 10px; display: flex; flex-direction: column; gap: 6px; }
  .prof.act { border-color: var(--ink); }
</style>
