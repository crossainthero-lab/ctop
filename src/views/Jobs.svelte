<script lang="ts">
  import { save } from '@tauri-apps/plugin-dialog';
  import { api, fmtDuration, type JobDetail, type JobSummary } from '../lib/api';
  import { act, jobs, machine, refreshJobs, toast } from '../lib/state.svelte';
  import { confirmDialog } from '../lib/dialogs.svelte';
  import StrokePreview from '../components/StrokePreview.svelte';

  let selId = $state<string | null>(null);
  let detail = $state<JobDetail | null>(null);
  const s = $derived(machine.s);
  const active = $derived(jobs.list.filter((j) => j.status === 'active'));
  const queued = $derived(jobs.list.filter((j) => j.status === 'queued'));
  const done = $derived(jobs.list.filter((j) => j.status === 'completed').reverse());
  const failed = $derived(jobs.list.filter((j) => ['failed', 'cancelled', 'stopped', 'interrupted'].includes(j.status)).reverse());
  const cur = $derived(jobs.list.find((j) => j.id === selId) ?? null);
  const live = $derived(s.job && cur && s.job.id === cur.id ? s.job : cur?.progress ?? null);

  $effect(() => {
    const id = selId;
    if (!id) { detail = null; return; }
    api.jobDetail(id).then((d) => { if (selId === id) detail = d; }).catch(() => (detail = null));
  });
  $effect(() => { if (!selId && jobs.list.length) selId = (active[0] ?? queued[0] ?? jobs.list[jobs.list.length - 1]).id; });

  const canStart = $derived(s.link === 'ready' && s.armed === 'envelope' && s.axesKnown.every(Boolean) && !s.job?.state?.match(/running|paused|cancelling/));
  const startWhy = $derived(
    s.link !== 'ready' ? 'Connect first' : s.armed !== 'envelope' ? 'Arm with the calibrated envelope (Manual Control)' :
    !s.axesKnown.every(Boolean) ? 'Verify the position (Manual Control)' : active.length ? 'Another job is running' : '');

  async function start(j: JobSummary) {
    const ok = await confirmDialog({
      title: `Start drawing “${j.name}”?`,
      body: `The machine will move and the pencil will touch the paper.\nDraw ${j.stats.drawLengthMm.toFixed(0)} mm · est. ${fmtDuration(j.stats.estimatedSeconds)} (estimate only).`,
      checks: ['Paper is loaded and held flat; the sheet matches the calibrated size and position.', 'The area is clear and I am ready to cut power if anything collides.'],
      confirmLabel: 'Start drawing',
    });
    if (ok) await act(() => api.startJob(j.id, true));
  }
  async function exportG(j: JobSummary) {
    const p = await save({ defaultPath: `${j.name.replace(/[^\w-]+/g, '_')}.gcode`, filters: [{ name: 'G-code', extensions: ['gcode', 'nc'] }] });
    if (p && (await act(() => api.exportJob(j.id, p)))) toast(`Exported ${p}`, 'ok');
  }
  async function del(j: JobSummary) {
    if (await act(() => api.deleteJob(j.id))) { if (selId === j.id) selId = null; refreshJobs(); }
  }
  const stat = (j: JobSummary) => (j.status === 'active' ? (live?.state ?? 'active') : j.status);
  const pct = (a: number, b: number) => (b ? Math.min(100, (a / b) * 100) : 0);
  const when = (t: number) => new Date(t).toLocaleString();
</script>

{#snippet row(j: JobSummary)}
  <tr class:sel={selId === j.id} onclick={() => (selId = j.id)} style="cursor:pointer">
    <td><b>{j.name}</b><div class="mute mono tiny">{when(j.createdAt)}</div></td>
    <td class="mono">{j.stats.strokes}</td>
    <td class="mono">{j.stats.drawLengthMm.toFixed(0)} mm</td>
    <td class="mono">≈{fmtDuration(j.stats.estimatedSeconds)}</td>
    <td><span class="pill" class:ok={j.status === 'completed'} class:bad={['failed', 'interrupted', 'stopped'].includes(j.status)} class:warn={j.status === 'active' || j.status === 'cancelled'}>{stat(j)}</span></td>
  </tr>
{/snippet}

<div class="jobs">
  <section class="list col">
    {#each [['Active', active], ['Pending', queued], ['Completed', done], ['Failed / stopped', failed]] as [title, items]}
      <div class="panel">
        <header><h3>{title}</h3><span class="mono mute">{(items as JobSummary[]).length}</span></header>
        {#if (items as JobSummary[]).length}
          <table><thead><tr><th>Job</th><th>Strokes</th><th>Draw</th><th>Est.</th><th>Status</th></tr></thead>
            <tbody>{#each items as j}{@render row(j as JobSummary)}{/each}</tbody></table>
        {:else}<div class="body mute">None</div>{/if}
      </div>
    {/each}
    <p class="mute small">Queued jobs start only when you press Start — CtoP never auto-starts the next job, because the paper has to be changed by hand.</p>
  </section>

  <section class="detail col">
    {#if cur}
      <div class="panel">
        <header>
          <h2>{cur.name}</h2>
          <div class="row">
            {#if cur.status === 'queued'}
              <button class="btn primary" disabled={!canStart} title={startWhy} onclick={() => start(cur)}>Start drawing…</button>
            {/if}
            <button class="btn" onclick={() => exportG(cur)}>Export G-code…</button>
            <button class="btn" disabled={cur.status === 'active'} onclick={() => del(cur)}>Delete</button>
          </div>
        </header>
        {#if cur.status === 'queued' && startWhy}<div class="notice">{startWhy}</div>{/if}
        <div class="body grid2">
          <div class="pv">{#if detail}<StrokePreview strokes={detail.strokes} paper={cur.paper} done={live?.confirmedStrokes ?? 0} />{/if}</div>
          <div class="col">
            <div class="kv">
              <span>Status</span><span><b>{stat(cur)}</b></span>
              <span>Strokes</span><span class="mono">{cur.stats.strokes}</span>
              <span>Pen-down distance</span><span class="mono">{cur.stats.drawLengthMm.toFixed(1)} mm</span>
              <span>Pen-up travel</span><span class="mono">{cur.stats.travelLengthMm.toFixed(1)} mm</span>
              <span>Pen lifts</span><span class="mono">{cur.stats.penLifts}</span>
              <span>G-code lines</span><span class="mono">{cur.stats.lines}</span>
              <span>Time estimate</span><span class="mono">≈ {fmtDuration(cur.stats.estimatedSeconds)} <span class="mute">(estimate from path &amp; speeds)</span></span>
            </div>

            {#if live}
              <hr />
              <div class="bars">
                <div><div class="row between"><span class="label">Accepted by firmware (queued)</span><span class="mono">{live.accepted}/{live.totalLines}</span></div>
                  <div class="bar"><i class="soft" style="width:{pct(live.accepted, live.totalLines)}%"></i></div></div>
                <div><div class="row between"><span class="label">Confirmed complete (M400)</span><span class="mono">{live.confirmedStrokes}/{live.totalStrokes} strokes</span></div>
                  <div class="bar"><i style="width:{pct(live.confirmedStrokes, live.totalStrokes)}%"></i></div></div>
              </div>
              <div class="kv"><span>Elapsed</span><span class="mono">{fmtDuration(live.elapsedS)}</span><span>Message</span><span>{live.message}</span></div>
              {#each live.warnings as w}<div class="notice">{w}</div>{/each}
              {#if live.error}<div class="notice bad"><b>Error:</b> {live.error}</div>{/if}
              {#if ['running', 'paused', 'cancelling'].includes(live.state)}
                <div class="row wrap">
                  {#if live.state === 'running'}<button class="btn" onclick={() => act(() => api.pauseJob())}>Pause submission</button>{/if}
                  {#if live.state === 'paused'}<button class="btn" onclick={() => act(() => api.resumeJob())}>Resume</button>{/if}
                  <button class="btn" disabled={live.state === 'cancelling'} onclick={() => act(() => api.cancelJob())}>Cancel &amp; lift pencil</button>
                  <button class="btn" onclick={() => act(() => api.quickStop())}>Quick stop (M410)</button>
                  <button class="btn danger" onclick={() => act(() => api.emergencyStop())}>Emergency halt</button>
                </div>
                <p class="small mute"><b>Pause</b> only stops CtoP sending new commands; moves already buffered in the firmware keep running. <b>Cancel</b> discards unsent commands, waits for the buffer to drain, then lifts the pencil. A job is "complete" only after the final M400 returns.</p>
              {/if}
            {/if}
          </div>
        </div>
      </div>
    {:else}
      <div class="panel"><div class="body mute">No job selected. Prepare something in the Workspace and press “Add to queue”.</div></div>
    {/if}
  </section>
</div>

<style>
  .jobs { display: grid; grid-template-columns: minmax(420px, 1fr) minmax(480px, 1.25fr); gap: 12px; padding: 12px; height: 100%; overflow: hidden; }
  section { overflow-y: auto; min-height: 0; }
  .grid2 { display: grid; grid-template-columns: minmax(200px, 300px) 1fr; gap: 16px; }
  .pv { aspect-ratio: 210/297; border: 1px solid var(--line-strong); background: var(--paper-2); max-height: 440px; }
  .tiny { font-size: 10px; } .small { font-size: 11.5px; margin: 0; } .wrap { flex-wrap: wrap; }
  .between { justify-content: space-between; }
  .bars { display: flex; flex-direction: column; gap: 8px; }
  .bar { height: 8px; background: var(--paper-3); position: relative; margin-top: 3px; }
  .bar i { display: block; height: 100%; background: var(--ink); } .bar i.soft { background: var(--line-strong); position: absolute; inset: 0 auto 0 0; }
  .bar i:not(.soft) { position: absolute; inset: 0 auto 0 0; }
</style>
