<script lang="ts">
  import { onMount } from 'svelte';
  import { open, save } from '@tauri-apps/plugin-dialog';
  import { api, type Calibration as Cal, type ProfilesFile } from '../lib/api';
  import { act, cal, machine, refreshCalibration, toast } from '../lib/state.svelte';
  import { confirmDialog } from '../lib/dialogs.svelte';
  import NumField from '../components/NumField.svelte';
  import BedMap from '../components/BedMap.svelte';

  let profiles = $state<ProfilesFile | null>(null);
  let draft = $state<Cal | null>(null);
  let dirty = $state(false);
  let authorised = $state(false);
  let tx = $state(0), ty = $state(0);
  const s = $derived(machine.s);
  const active = $derived(profiles?.profiles.find((p) => p.id === profiles?.active) ?? null);
  const r = $derived(cal.status?.resolved ?? null);
  const live = $derived(s.link === 'ready' && s.position && s.axesKnown.every(Boolean));

  async function load() {
    profiles = await api.profiles();
    const p = profiles.profiles.find((x) => x.id === profiles!.active)!;
    draft = JSON.parse(JSON.stringify(p.calibration));
    dirty = false;
    await refreshCalibration();
  }
  onMount(load);
  const touch = () => (dirty = true);

  async function saveCal() {
    if (!draft) return;
    const st = await api.setCalibration(draft).catch((e) => { toast(String(e), 'error'); return null; });
    if (st) { cal.status = st; dirty = false; toast('Calibration saved', 'ok', 1800); }
  }
  async function exportCal() {
    if (!draft) return;
    const p = await save({ defaultPath: 'ctop-calibration.json', filters: [{ name: 'CtoP calibration', extensions: ['json'] }] });
    if (p) await act(() => api.writeTextFile(p, JSON.stringify(draft, null, 2)));
  }
  async function importCal() {
    const p = await open({ multiple: false, filters: [{ name: 'CtoP calibration', extensions: ['json'] }] });
    if (!p || typeof p !== 'string') return;
    try {
      const d = JSON.parse(await api.readTextFile(p));
      draft = { ...draft!, ...d, margins: { ...draft!.margins, ...d.margins }, envelope: { ...draft!.envelope, ...d.envelope }, verified: { offsets: false, z: false, paper: false, envelope: false, orientation: false } };
      dirty = true;
      toast('Imported. Verification flags were reset — confirm each item against the real machine.', 'info', 7000);
    } catch (e) { toast(`Import failed: ${e}`, 'error'); }
  }
  function useZ(k: 'zDown' | 'zUp' | 'zSafe') {
    if (!live || !draft) return;
    draft[k] = Number(s.position![2].toFixed(3)); touch();
  }
  function useOrigin() {
    if (!live || !draft || draft.pencilOffsetX === null || draft.pencilOffsetY === null) { toast('Enter the pencil offset first, and have a verified position.', 'error'); return; }
    draft.paperOriginX = Number((s.position![0] + draft.pencilOffsetX).toFixed(3));
    draft.paperOriginY = Number((s.position![1] + draft.pencilOffsetY).toFixed(3));
    touch();
  }
  async function resetVerified() { if (draft) { draft.verified = { offsets: false, z: false, paper: false, envelope: false, orientation: false }; touch(); } }

  // ── test moves (all explicit, one click each) ──
  const moveOK = $derived(!!r && s.link === 'ready' && !!s.armed && s.axesKnown.every(Boolean) && !s.job && (s.mode === 'simulation' || authorised));
  const moveWhy = $derived(!r ? 'Save a complete calibration first' : s.link !== 'ready' ? 'Not connected' : !s.armed ? 'Arm the machine in Manual Control' : !s.axesKnown.every(Boolean) ? 'Verify the position first' : s.mode === 'hardware' && !authorised ? 'Tick the authorisation box to allow real test moves' : '');
  const FZ = 600, FXY = 2400;
  async function toCorner(px: number, py: number) {
    if (!r) return;
    const ok = s.mode === 'simulation' || (await confirmDialog({ title: 'Move to paper position?', body: `The head will rise to safe Z, travel to paper (${px}, ${py}) mm, then lower to pen-UP height. Pencil stays above the paper.`, checks: ['Area is clear; I am watching the machine.'], confirmLabel: 'Move' }));
    if (!ok) return;
    const tipx = r.origin[0] + (r.swapXy ? (r.flipY ? -py : py) : (r.flipX ? -px : px));
    const tipy = r.origin[1] + (r.swapXy ? (r.flipX ? -px : px) : (r.flipY ? -py : py));
    const nx = tipx - r.offset[0], ny = tipy - r.offset[1];
    if (await act(() => api.goto(null, null, r.zSafe, FZ))) if (await act(() => api.goto(nx, ny, null, FXY))) await act(() => api.goto(null, null, r.zUp, FZ));
  }
  async function lowerDown() {
    if (!r) return;
    const ok = await confirmDialog({ title: 'Lower pencil to pen-down height?', body: `Z will go to ${r.zDown}. The pencil will touch the paper here. Verify it marks lightly without bending.`, checks: ['I am watching the pencil tip.'], confirmLabel: 'Lower', danger: true });
    if (ok) await act(() => api.goto(null, null, r.zDown, 300));
  }
  async function raise(z: number) { await act(() => api.goto(null, null, z, FZ)); }

  const mapped = $derived.by(() => {
    if (!r) return null;
    const pt = (px: number, py: number) => {
      const u = r.swapXy ? py : px, v = r.swapXy ? px : py;
      const tip = [r.origin[0] + (r.flipX ? -u : u), r.origin[1] + (r.flipY ? -v : v)];
      return { tip, nozzle: [tip[0] - r.offset[0], tip[1] - r.offset[1]] };
    };
    return pt(tx, ty);
  });
  const verifyItems: [keyof Cal['verified'], string, string][] = [
    ['offsets', 'Pencil offset', 'Measured the tip-to-nozzle offset on the real machine'],
    ['z', 'Z heights', 'Pen-down touches the paper lightly, pen-up clears it, safe Z clears everything'],
    ['paper', 'Paper origin & size', 'Origin and sheet size match the sheet taped on the bed'],
    ['envelope', 'Safe envelope', 'The carriage can reach every point in the envelope without hitting anything'],
    ['orientation', 'Axis orientation', 'Paper +x / +y map to the machine directions set above (tested with a corner move)'],
  ];
</script>

{#if draft}
<div class="cal">
  <section class="col forms">
    <div class="notice info">
      Profile <b>{active?.name}</b> ({active?.kind}).
      {#if active?.kind === 'hardware'}Hardware values are never pre-filled — measure them or import them from the calibration session.{:else}Simulator values describe the virtual machine only.{/if}
    </div>

    <div class="panel">
      <header><h3>1 · Pencil offset (tip − nozzle)</h3></header>
      <div class="body col">
        <div class="field"><span class="label">Offset X</span><NumField bind:value={draft.pencilOffsetX} unit="mm" onchange={touch} /></div>
        <div class="field"><span class="label">Offset Y</span><NumField bind:value={draft.pencilOffsetY} unit="mm" onchange={touch} /></div>
        <p class="hint">Positive X: the pencil tip sits to the +X side of the nozzle tip. The firmware positions the <em>nozzle</em>, so CtoP moves the nozzle to “tip − offset”.</p>
      </div>
    </div>

    <div class="panel">
      <header><h3>2 · Z heights (machine Z)</h3></header>
      <div class="body col">
        <label class="row"><input type="checkbox" bind:checked={draft.zIncreasesUp} onchange={touch} /> Increasing Z raises the head</label>
        {#each [['Pen-down Z', 'zDown', 'pencil just touches paper'], ['Pen-up Z', 'zUp', 'travel across paper'], ['Safe clearance Z', 'zSafe', 'approach, parking, long moves']] as [label, key, hint]}
          <div class="field"><span class="label">{label}</span>
            <span class="row"><NumField bind:value={draft[key as 'zDown']} unit="mm" onchange={touch} /><button class="btn sm" disabled={!live} title="Use the current verified Z" onclick={() => useZ(key as 'zDown')}>use current</button><span class="mute small">{hint}</span></span></div>
        {/each}
        <p class="hint">Pencil pressure depends on how the pencil is mounted (spring, play, tip geometry), not on Z alone — find pen-down by test, and prefer a compliant holder.</p>
      </div>
    </div>

    <div class="panel">
      <header><h3>3 · Paper &amp; axes</h3></header>
      <div class="body col">
        <div class="field"><span class="label">Sheet width</span><NumField bind:value={draft.paperWidth} unit="mm" onchange={touch} /></div>
        <div class="field"><span class="label">Sheet height</span><NumField bind:value={draft.paperHeight} unit="mm" onchange={touch} /></div>
        <div class="field"><span class="label">Origin X (tip)</span><NumField bind:value={draft.paperOriginX} unit="mm" onchange={touch} /></div>
        <div class="field"><span class="label">Origin Y (tip)</span><span class="row"><NumField bind:value={draft.paperOriginY} unit="mm" onchange={touch} /><button class="btn sm" disabled={!live} onclick={useOrigin}>origin = current tip</button></span></div>
        <p class="hint">Origin = machine X/Y of the pencil <em>tip</em> over the sheet's top-left corner as the sheet is read.</p>
        <div class="row wrap">
          <label class="row"><input type="checkbox" bind:checked={draft.flipX} onchange={touch} /> paper +x → machine −X</label>
          <label class="row"><input type="checkbox" bind:checked={draft.flipY} onchange={touch} /> paper +y (down) → machine −Y</label>
          <label class="row"><input type="checkbox" bind:checked={draft.swapXy} onchange={touch} /> swap X/Y</label>
        </div>
        <div class="field"><span class="label">Margins L/R/T/B</span>
          <span class="row"><NumField bind:value={draft.margins.left as number | null} width={52} onchange={touch} /><NumField bind:value={draft.margins.right as number | null} width={52} onchange={touch} /><NumField bind:value={draft.margins.top as number | null} width={52} onchange={touch} /><NumField bind:value={draft.margins.bottom as number | null} width={52} onchange={touch} /> mm</span></div>
      </div>
    </div>

    <div class="panel">
      <header><h3>4 · Safe working envelope (nozzle / carriage)</h3>{#if s.fwLimits}<span class="mono mute small">firmware: X{s.fwLimits.xMax} Y{s.fwLimits.yMax} Z{s.fwLimits.zMax}</span>{/if}</header>
      <div class="body col">
        {#each [['X', 'xMin', 'xMax'], ['Y', 'yMin', 'yMax'], ['Z', 'zMin', 'zMax']] as [a, lo, hi]}
          <div class="field"><span class="label">{a} min / max</span><span class="row"><NumField bind:value={draft.envelope[lo as 'xMin']} unit="" onchange={touch} width={72} /> – <NumField bind:value={draft.envelope[hi as 'xMax']} unit="mm" onchange={touch} width={72} /></span></div>
        {/each}
        <p class="hint">Firmware limits are not a safe area: define where the <em>carriage</em> may actually go with the pencil fitted (clips, bed edges, cables).</p>
      </div>
    </div>

    <div class="panel">
      <header><h3>5 · Operator verification</h3><button class="btn sm ghost" onclick={resetVerified}>clear all</button></header>
      <div class="body col">
        {#each verifyItems as [k, title, desc]}
          <label class="vrow"><input type="checkbox" bind:checked={draft.verified[k]} onchange={touch} /><span><b>{title}</b><br /><span class="mute small">{desc}</span></span></label>
        {/each}
        <textarea rows="2" placeholder="Notes (who measured, when, how)" bind:value={draft.notes} oninput={touch}></textarea>
      </div>
    </div>

    <div class="row sticky">
      <button class="btn primary" onclick={saveCal} disabled={!dirty}>Save calibration</button>
      <button class="btn" onclick={load} disabled={!dirty}>Revert</button>
      <span class="grow"></span>
      <button class="btn" onclick={importCal}>Import…</button>
      <button class="btn" onclick={exportCal}>Export…</button>
    </div>
  </section>

  <section class="col side">
    <div class="panel">
      <header><h3>Status</h3>
        {#if dirty}<span class="pill warn">unsaved edits</span>{:else if cal.status?.readyForDrawing}<span class="pill ok">drawing enabled</span>{:else}<span class="pill bad">drawing disabled</span>{/if}
      </header>
      <div class="body col">
        {#if cal.status?.reasons.length}
          <div class="notice bad"><b>Drawing is disabled until:</b>
            <ul>{#each cal.status.reasons as w}<li>{w}</li>{/each}</ul></div>
        {/if}
        {#each cal.status?.issues.filter((i) => i.severity === 'warning') ?? [] as i}<div class="notice small">{i.field}: {i.message}</div>{/each}
        {#if cal.status?.readyForDrawing}<div class="notice ok">Calibration complete and verified. Software checks still don't replace supervision.</div>{/if}
      </div>
    </div>

    <div class="panel">
      <header><h3>Transform check</h3><span class="mute small">uses the saved calibration</span></header>
      <div class="body col">
        <div class="row">Paper <NumField bind:value={tx as number | null} step={1} width={64} /> , <NumField bind:value={ty as number | null} step={1} width={64} /> mm</div>
        {#if mapped}
          <div class="kv mono"><span>Pencil tip</span><span>X {mapped.tip[0].toFixed(2)}  Y {mapped.tip[1].toFixed(2)}</span><span>Nozzle command</span><span>X {mapped.nozzle[0].toFixed(2)}  Y {mapped.nozzle[1].toFixed(2)}</span></div>
        {:else}<span class="mute">Needs a complete calibration.</span>{/if}
      </div>
    </div>

    <div class="panel mapp"><header><h3>Map</h3></header><div class="mapbox"><BedMap /></div></div>

    <div class="panel">
      <header><h3>Guided test moves</h3>{#if s.mode === 'simulation'}<span class="pill warn">virtual moves</span>{:else if s.mode === 'hardware'}<span class="pill bad">real moves</span>{/if}</header>
      <div class="body col">
        {#if s.mode === 'hardware'}<label class="row"><input type="checkbox" bind:checked={authorised} /> I explicitly authorise real calibration test moves this session</label>{/if}
        {#if moveWhy}<div class="notice small">{moveWhy}</div>{/if}
        <div class="row wrap">
          <button class="btn sm" disabled={!moveOK} onclick={() => r && raise(r.zSafe)}>Raise to safe Z</button>
          <button class="btn sm" disabled={!moveOK} onclick={() => r && raise(r.zUp)}>Pen-up Z</button>
        </div>
        <span class="label">Go to paper position at pen-up height</span>
        <div class="row wrap">
          {#if r}
            {#each [['TL', 'margins-tl'], ['TR', 'margins-tr'], ['BR', 'margins-br'], ['BL', 'margins-bl']] as [name, k]}
              <button class="btn sm" disabled={!moveOK} onclick={() => {
                const x0 = r.margins.left, y0 = r.margins.top, x1 = r.paper[0] - r.margins.right, y1 = r.paper[1] - r.margins.bottom;
                const [px, py] = k === 'margins-tl' ? [x0, y0] : k === 'margins-tr' ? [x1, y0] : k === 'margins-br' ? [x1, y1] : [x0, y1];
                toCorner(px, py);
              }}>{name} corner</button>
            {/each}
          {/if}
          <button class="btn sm danger" disabled={!moveOK} onclick={lowerDown}>Lower to pen-down…</button>
        </div>
        <p class="hint">Each button is one explicit action; nothing runs automatically. Every move is validated against the saved envelope first.</p>
      </div>
    </div>
  </section>
</div>
{/if}

<style>
  .cal { display: grid; grid-template-columns: minmax(480px, 1.1fr) minmax(360px, 1fr); gap: 12px; padding: 12px; height: 100%; overflow: hidden; }
  section { overflow-y: auto; min-height: 0; padding-bottom: 8px; }
  .hint { margin: 0; font-size: 11.5px; color: var(--mute-c); } .small { font-size: 11.5px; } .wrap { flex-wrap: wrap; }
  .vrow { display: flex; gap: 9px; align-items: flex-start; cursor: pointer; padding: 3px 0; }
  .vrow input { margin-top: 3px; }
  .sticky { position: sticky; bottom: 0; background: var(--paper); border-top: 1px solid var(--line-strong); padding: 8px 0; }
  .mapp { display: flex; flex-direction: column; } .mapbox { aspect-ratio: 1; max-height: 340px; padding: 8px; }
  ul { margin: 4px 0 0; padding-left: 18px; }
</style>
