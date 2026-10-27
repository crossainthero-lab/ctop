<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type SimSnapshot, type Fault } from '../lib/api';
  import { act, logs, machine, ui } from '../lib/state.svelte';
  import { confirmDialog } from '../lib/dialogs.svelte';
  import BedMap from '../components/BedMap.svelte';

  const s = $derived(machine.s);
  const steps = [0.1, 1, 5, 10];
  let step = $state(1);
  let custom = $state(2.5);
  let useCustom = $state(false);
  let feedXY = $state(3000);
  let feedZ = $state(600);
  let ack = $state(false);
  let sim = $state<SimSnapshot | null>(null);
  let trail = $state<[number, number, number, number][]>([]);
  let trailN = 0;
  let logBox: HTMLDivElement | undefined = $state();

  const dist = $derived(useCustom ? custom : step);
  const ready = $derived(s.link === 'ready');
  const job = $derived(s.job && ['running', 'paused', 'cancelling'].includes(s.job.state));
  const allKnown = $derived(s.axesKnown.every(Boolean));
  const canMove = $derived(ready && !!s.armed && allKnown && !job);
  const why = $derived(
    !ready ? 'Not connected' : job ? 'A job is running — manual motion is locked' : !s.armed ? 'Arm the machine to enable motion'
    : !allKnown ? 'Position not verified — home axes or read & verify the position' : '');

  async function jog(axis: number, sign: number) {
    await act(() => api.jog(axis, sign * dist, axis === 2 ? feedZ : feedXY));
  }
  async function arm(mode: 'envelope' | 'firmwareOnly') {
    if (mode === 'firmwareOnly') {
      const ok = await confirmDialog({
        title: 'Arm with firmware limits only?',
        body: 'No calibrated safe envelope protects the machine in this mode. Only the firmware\'s own software limits apply, and downward Z steps are limited to 1 mm. Use only for supervised calibration moves.',
        checks: ['I am watching the machine and can cut power.', 'I accept that the pencil/head can hit the bed or clamps in this mode.'],
        confirmLabel: 'Arm (uncalibrated)', danger: true,
      });
      if (!ok) return;
    }
    await act(() => api.arm(mode, mode === 'firmwareOnly'));
  }
  async function home(x: boolean, y: boolean, z: boolean) {
    const ok = await confirmDialog({
      title: `Home ${[x && 'X', y && 'Y', z && 'Z'].filter(Boolean).join(' ')}?`,
      body: z
        ? 'Z homing drives the head TOWARD THE BED until the Z endstop/probe triggers. With a pencil clamped below the nozzle this can crush the pencil into the paper or bed. Raise or remove the pencil first.\n\nCtoP never homes automatically.'
        : 'The carriage will move to the X/Y endstops. Z is not touched, but make sure the pencil is lifted clear of the paper, clips and bed edges first.',
      checks: z ? ['The pencil is raised/removed so the head can lower to the bed safely.', 'I am ready to cut power.'] : ['The pencil is clear of the paper and bed hardware.'],
      confirmLabel: 'Home', danger: z,
    });
    if (ok) await act(() => api.home(x, y, z));
  }
  async function verify() {
    const p = s.position;
    if (!p) return;
    const ok = await confirmDialog({
      title: 'Verify position',
      body: `The firmware reports X ${p[0].toFixed(2)}  Y ${p[1].toFixed(2)}  Z ${p[2].toFixed(2)}.\n\nConfirm ONLY if you have physically checked that this matches reality — e.g. the printer was homed in this power session and has not been reset. After a reset or reconnect the firmware reports 0,0,0 whatever the carriage position is.`,
      checks: ['The printer has been homed since it was last powered/reset, and these coordinates are true.'],
      confirmLabel: 'Position is correct',
    });
    if (ok) await act(() => api.verifyPosition());
  }

  function key(e: KeyboardEvent) {
    if (ui.view !== 'manual') return;
    const t = e.target as HTMLElement;
    if (t && ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName)) return;
    const map: Record<string, [number, number]> = { ArrowRight: [0, 1], ArrowLeft: [0, -1], ArrowUp: [1, 1], ArrowDown: [1, -1], PageUp: [2, 1], PageDown: [2, -1] };
    const m = map[e.key];
    if (m && canMove) { e.preventDefault(); jog(m[0], m[1]); }
    const n = ['1', '2', '3', '4'].indexOf(e.key);
    if (n >= 0) { useCustom = false; step = steps[n]; }
  }

  async function poll() {
    if (s.mode !== 'simulation') { sim = null; return; }
    try {
      sim = await api.simState();
      const [n, segs] = await api.simTrail(trailN);
      if (n < trailN) { trail = []; }
      trail.push(...segs); trailN = n;
      if (trail.length > 20000) trail.splice(0, trail.length - 20000);
    } catch { sim = null; }
  }
  onMount(() => { const t = setInterval(poll, 150); return () => clearInterval(t); });
  $effect(() => { logs.lines.length; if (logBox) logBox.scrollTop = logBox.scrollHeight; });

  const fault = (f: Fault) => act(() => api.simFault(f));
  async function simSpeed(v: number) { await act(() => api.simSpeed(v)); }
  async function clearSim() { await act(() => api.simClear()); trail = []; trailN = 0; }
  const fmt = (v: number | undefined) => (v === undefined ? '—' : v.toFixed(2).padStart(7, ' '));
</script>

<svelte:window onkeydown={key} />

<div class="manual">
  <section class="left col">
    <div class="panel">
      <header><h3>Position</h3><span class="mono mute">{s.positionSource}</span></header>
      <div class="body">
        <div class="axes">
          {#each ['X', 'Y', 'Z'] as a, i}
            <div class="axis" class:unk={!s.axesKnown[i]}>
              <span class="al">{a}</span>
              <span class="av mono">{s.position ? s.position[i].toFixed(2) : '— — —'}</span>
              <span class="ak">{s.axesKnown[i] ? 'known' : 'unverified'}</span>
            </div>
          {/each}
        </div>
        <div class="row" style="margin-top:10px">
          <button class="btn sm" disabled={!ready || !!job} onclick={() => act(() => api.readPosition())}>Read position</button>
          <button class="btn sm" disabled={!ready || !s.position || !!job} onclick={verify}>Verify position…</button>
          <span class="grow"></span>
          <span class="mute mono">F {s.feed || '—'}</span>
        </div>
      </div>
    </div>

    <div class="panel">
      <header>
        <h3>Jog</h3>
        {#if why}<span class="pill warn">{why}</span>{:else}<span class="pill ok">Motion enabled</span>{/if}
      </header>
      <div class="body col">
        <div class="row">
          <span class="label">Step</span>
          <div class="seg">
            {#each steps as st, i}<button class:on={!useCustom && step === st} onclick={() => { useCustom = false; step = st; }}>{st} mm <kbd>{i + 1}</kbd></button>{/each}
            <button class:on={useCustom} onclick={() => (useCustom = true)}>Custom</button>
          </div>
          {#if useCustom}<input type="number" step="0.1" min="0.01" max="50" bind:value={custom} style="width:70px" /> mm{/if}
        </div>
        <div class="pads">
          <div class="xy">
            <span></span><button class="btn pad" disabled={!canMove} onclick={() => jog(1, 1)}>Y+<kbd>↑</kbd></button><span></span>
            <button class="btn pad" disabled={!canMove} onclick={() => jog(0, -1)}>X−<kbd>←</kbd></button>
            <span class="ctr mono">{dist}</span>
            <button class="btn pad" disabled={!canMove} onclick={() => jog(0, 1)}>X+<kbd>→</kbd></button>
            <span></span><button class="btn pad" disabled={!canMove} onclick={() => jog(1, -1)}>Y−<kbd>↓</kbd></button><span></span>
          </div>
          <div class="z">
            <button class="btn pad" disabled={!canMove} onclick={() => jog(2, 1)}>Z+ <small>raise</small><kbd>PgUp</kbd></button>
            <button class="btn pad" disabled={!canMove} onclick={() => jog(2, -1)}>Z− <small>lower</small><kbd>PgDn</kbd></button>
          </div>
        </div>
        <div class="field"><span class="label">XY feed</span><span class="row"><input type="number" bind:value={feedXY} min="60" max="6000" step="100" style="width:90px" /> mm/min</span></div>
        <div class="field"><span class="label">Z feed</span><span class="row"><input type="number" bind:value={feedZ} min="30" max="1200" step="30" style="width:90px" /> mm/min</span></div>
        <p class="mute small">Every jog is checked against the safe envelope before it is sent. Z− below the calibrated pen-down height is refused.</p>
      </div>
    </div>

    <div class="panel">
      <header><h3>Arming &amp; homing</h3>{#if s.armed}<span class="pill ok">Armed · {s.armed === 'envelope' ? 'calibrated envelope' : 'firmware limits only'}</span>{:else}<span class="pill">Disarmed</span>{/if}</header>
      <div class="body col">
        <div class="row wrap">
          {#if s.armed}
            <button class="btn" disabled={!ready || !!job} onclick={() => act(() => api.disarm())}>Disarm</button>
          {:else}
            <button class="btn primary" disabled={!ready} onclick={() => arm('envelope')}>Arm (calibrated envelope)</button>
            <button class="btn" disabled={!ready} onclick={() => arm('firmwareOnly')}>Arm — firmware limits only…</button>
          {/if}
        </div>
        <div class="row wrap">
          <button class="btn sm" disabled={!ready || !s.armed || !!job} onclick={() => home(true, true, false)}>Home XY…</button>
          <button class="btn sm" disabled={!ready || !s.armed || !!job} onclick={() => home(false, false, true)}>Home Z…</button>
          <button class="btn sm" disabled={!ready || !s.armed || !!job} onclick={() => home(true, true, true)}>Home all…</button>
        </div>
        <p class="mute small">CtoP never homes or moves on connect. Arming is required for every kind of motion.</p>
      </div>
    </div>

    <div class="panel">
      <header><h3>Stopping</h3></header>
      <div class="body col">
        <div class="row wrap">
          <button class="btn" disabled={!ready} onclick={() => act(() => api.quickStop())}>Quick stop (M410)</button>
          <button class="btn danger" disabled={s.link !== 'ready'} onclick={() => act(() => api.emergencyStop())}>Emergency halt (M112)</button>
        </div>
        <p class="small mute">
          <b>Quick stop</b> flushes the firmware's motion buffer, but up to 4 commands already in its command queue may still run{s.firmware?.emergencyParser === false ? ' — and this firmware reports NO emergency parser, so M410 waits behind queued commands' : s.firmware?.emergencyParser ? ' (this firmware has an emergency parser)' : ''}.
          <b>Emergency halt</b> halts the controller; it must then be reset. If anything is colliding, <b>cut printer power</b> — software cannot guarantee an instant stop.
        </p>
      </div>
    </div>
  </section>

  <section class="mid col">
    <div class="panel status">
      <header><h3>Machine</h3></header>
      <div class="body kv">
        <span>Mode</span><span>{s.mode}</span>
        <span>Link</span><span>{s.link}</span>
        <span>Device</span><span class="mono">{s.device || '—'}</span>
        <span>Firmware</span><span class="mono">{s.firmware?.name ?? '—'}</span>
        <span>Machine type</span><span class="mono">{s.firmware?.machineType || '—'}</span>
        <span>Identity</span><span>{s.firmware ? (s.firmware.isMarlin ? 'Marlin confirmed' : 'NOT Marlin') : '—'}</span>
        <span>FW limits</span><span class="mono">{s.fwLimits ? `X${s.fwLimits.xMax} Y${s.fwLimits.yMax} Z${s.fwLimits.zMax}` : '—'}</span>
        <span>Homed axes</span><span class="mono">{s.axesKnown.map((k, i) => (k ? 'XYZ'[i] : '·')).join(' ')}</span>
        <span>Activity</span><span>{s.activity}</span>
        <span>Outstanding cmds</span><span class="mono">{s.pending}</span>
        <span>Last error</span><span class:err={s.lastError}>{s.lastError ?? 'none'}</span>
      </div>
    </div>
    <div class="panel map">
      <header><h3>Bed map</h3><span class="mute small">○ nozzle  ● pencil tip  ┈ safe envelope</span></header>
      <div class="mapbox"><BedMap {sim} {trail} /></div>
    </div>
  </section>

  <section class="right col">
    <div class="panel console">
      <header><h3>Command log</h3><button class="btn sm ghost" onclick={() => (logs.lines = [])}>Clear</button></header>
      <div class="log mono" bind:this={logBox}>
        {#each logs.lines.slice(-400) as l}<div class={l.dir}><i>{(l.tMs / 1000).toFixed(1).padStart(7)}</i> {l.dir === 'tx' ? '→' : l.dir === 'rx' ? '←' : '•'} {l.text}</div>{/each}
      </div>
    </div>
    {#if s.mode === 'simulation'}
      <div class="panel">
        <header><h3>Simulator</h3>{#if sim?.collisions.length}<span class="pill bad">{sim.collisions.length} collision{sim.collisions.length > 1 ? 's' : ''}</span>{/if}</header>
        <div class="body col">
          {#if sim}
            <div class="kv mono small">
              <span>Physical</span><span>{sim.phys.map((v) => v.toFixed(2)).join(' / ')}</span>
              <span>Pencil tip XY</span><span>{sim.tip.map((v) => v.toFixed(2)).join(' / ')}</span>
              <span>Pen</span><span>{sim.penDown ? 'DOWN (touching)' : 'up'}</span>
              <span>Planner</span><span>{sim.plannerDepth}/16</span>
            </div>
            {#each sim.collisions.slice(-3) as c}<div class="notice bad small">{c.message}</div>{/each}
            <div class="row"><span class="label">Speed</span>
              <div class="seg">{#each [1, 4, 20, 100] as v}<button class:on={sim.speed === v} onclick={() => simSpeed(v)}>{v}×</button>{/each}</div>
              <button class="btn sm" onclick={clearSim}>Clear marks</button></div>
            <span class="label">Fault injection</span>
            <div class="row wrap">
              <button class="btn sm" onclick={() => fault({ kind: 'disconnect' })}>Unplug</button>
              <button class="btn sm" onclick={() => fault({ kind: 'reset' })}>MCU reset</button>
              <button class="btn sm" onclick={() => fault({ kind: 'halt' })}>Firmware halt</button>
              <button class="btn sm" onclick={() => fault({ kind: 'errorNext' })}>Error reply</button>
              <button class="btn sm" onclick={() => fault({ kind: 'stall', seconds: 45 })}>Go silent 45 s</button>
              <button class="btn sm" onclick={() => fault({ kind: 'skipSteps', axis: 0, mm: 3 })}>Lose 3 mm X</button>
            </div>
          {/if}
        </div>
      </div>
    {/if}
  </section>
</div>

<style>
  .manual { display: grid; grid-template-columns: 420px minmax(300px, 1fr) 400px; gap: 12px; padding: 12px; height: 100%; overflow: hidden; }
  section { min-height: 0; overflow-y: auto; }
  .axes { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; }
  .axis { border: 1px solid var(--ink); background: #fff; padding: 6px 10px; display: grid; grid-template-columns: auto 1fr; align-items: baseline; }
  .axis.unk { border-style: dashed; border-color: var(--line-strong); background: var(--paper-2); }
  .al { font-weight: 700; font-size: 12px; color: var(--mute-c); } .av { font-size: 21px; text-align: right; }
  .ak { grid-column: 1 / -1; text-align: right; font-size: 9.5px; letter-spacing: .08em; text-transform: uppercase; color: var(--mute-c); }
  .pads { display: flex; gap: 22px; align-items: center; }
  .xy { display: grid; grid-template-columns: repeat(3, 62px); grid-auto-rows: 52px; gap: 4px; }
  .z { display: flex; flex-direction: column; gap: 4px; }
  .pad { height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; font-weight: 700; font-size: 14px; gap: 1px; }
  .z .pad { width: 92px; height: 50px; }
  .pad small { font-weight: 400; font-size: 10px; color: var(--mute-c); } .pad kbd { font-size: 9px; }
  .ctr { display: grid; place-items: center; color: var(--mute-c); font-size: 12px; }
  .small { font-size: 11.5px; margin: 0; } .wrap { flex-wrap: wrap; }
  .err { color: var(--danger); }
  .mapbox { padding: 8px; aspect-ratio: 1; } .map { flex: 1; min-height: 280px; display: flex; flex-direction: column; } .mapbox { flex: 1; min-height: 0; }
  .console { flex: 1; display: flex; flex-direction: column; min-height: 260px; }
  .log { flex: 1; overflow-y: auto; background: var(--charcoal); color: var(--on-dark); padding: 8px; font-size: 11px; line-height: 1.5; min-height: 0; }
  .log i { color: #6e6c63; font-style: normal; } .log .tx { color: #9ec8f0; } .log .rx { color: #cfcabb; } .log .error { color: #f08a72; } .log .warn { color: #e6b04a; } .log .info { color: #8fcfae; }
  .status .kv { font-size: 12.5px; }
</style>
