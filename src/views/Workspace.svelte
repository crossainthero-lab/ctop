<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { open, save } from '@tauri-apps/plugin-dialog';
  import { api, fmtDuration, DEFAULT_GCODE, DEFAULT_PLAN, type Order } from '../lib/api';
  import { act, cal, machine, toast, ui } from '../lib/state.svelte';
  import { confirmDialog } from '../lib/dialogs.svelte';
  import { project, sel, hist, undo, redo, addText, addImageFromDataUrl, addSvg, removeSelected, duplicateSelected, nudge, serialize, load, newProject, checkpoint } from '../lib/doc.svelte';
  import { pipe, schedule } from '../lib/pipeline.svelte';
  import { resourceTick } from '../lib/render.svelte';
  import { importSvg } from '../lib/svg/import';
  import { fileToDataUrl } from '../lib/imageio';
  import Canvas, { type Tool } from '../components/Canvas.svelte';
  import Inspector from '../components/Inspector.svelte';

  let tool = $state<Tool>('select');
  let canvas: { fit: () => void; zoomBy: (f: number) => void } | undefined = $state();
  let textarea = $state<HTMLTextAreaElement | undefined>();
  let showPath = $state(false);
  let scrub = $state(1);
  let playing = $state(false);
  let over = $state(false);
  const s = $derived(machine.s);

  // recompute the machine plan whenever the document changes
  $effect(() => {
    JSON.stringify([project.objects, project.paper, project.settings]);
    void resourceTick.n;
    schedule();
  });

  // ── import ──
  async function importFile(name: string, mime: string, getBlob: () => Promise<Blob>, getText: () => Promise<string>) {
    try {
      if (mime === 'image/svg+xml' || /\.svg$/i.test(name)) {
        const text = await getText();
        const r = importSvg(text);
        if (!r.strokes.length) { toast(`${name}: no drawable shapes found. ${r.warnings.join(' ')}`, 'error'); return; }
        addSvg(text, name.replace(/\.svg$/i, ''), { w: r.widthMm, h: r.heightMm });
        if (r.warnings.length) toast(`${name}: ${r.warnings.join(' · ')}`, 'info', 8000);
      } else if (mime.startsWith('image/') || /\.(png|jpe?g|webp|gif|bmp)$/i.test(name)) {
        await addImageFromDataUrl(await fileToDataUrl(await getBlob()), name.replace(/\.[^.]+$/, '') || 'Image');
      } else if (/\.ctop$|\.json$/i.test(name)) {
        await load(await getText(), null); toast(`Opened ${name}`, 'ok');
      } else if (mime.startsWith('text/') || /\.txt$/i.test(name)) {
        addText(await getText());
      } else toast(`${name}: unsupported file type`, 'error');
    } catch (e) { toast(`${name}: ${e}`, 'error'); }
  }
  const importFromFile = (f: File) => importFile(f.name, f.type, async () => f, () => f.text());

  async function pickImport(kind: 'image' | 'svg') {
    const filters = kind === 'image' ? [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'] }] : [{ name: 'SVG', extensions: ['svg'] }];
    const p = await open({ multiple: true, filters });
    if (!p) return;
    for (const path of Array.isArray(p) ? p : [p]) {
      const name = path.split('/').pop()!;
      if (kind === 'svg') await importFile(name, 'image/svg+xml', async () => new Blob(), () => api.readTextFile(path));
      else {
        const mime = /\.png$/i.test(name) ? 'image/png' : /\.webp$/i.test(name) ? 'image/webp' : /\.gif$/i.test(name) ? 'image/gif' : 'image/jpeg';
        await importFile(name, mime, async () => new Blob([await api.readBinaryFile(path)], { type: mime }), async () => '');
      }
    }
  }

  function onPaste(e: ClipboardEvent) {
    if (ui.view !== 'workspace') return;
    const t = e.target as HTMLElement;
    if (t && ['INPUT', 'TEXTAREA'].includes(t.tagName)) return;
    const items = Array.from(e.clipboardData?.items ?? []);
    const img = items.find((i) => i.type.startsWith('image/'));
    if (img) { e.preventDefault(); const f = img.getAsFile(); if (f) importFile(`Pasted image`, f.type, async () => f, async () => ''); return; }
    const txt = e.clipboardData?.getData('text/plain');
    if (txt?.trim()) {
      e.preventDefault();
      if (txt.trimStart().startsWith('<svg')) importFile('Pasted.svg', 'image/svg+xml', async () => new Blob(), async () => txt);
      else addText(txt.trim());
    }
  }
  async function onDrop(e: DragEvent) {
    e.preventDefault(); over = false;
    for (const f of Array.from(e.dataTransfer?.files ?? [])) await importFromFile(f);
  }

  // ── project files ──
  async function saveProject(as = false) {
    let path = project.path;
    if (!path || as) {
      const p = await save({ defaultPath: `${project.name}.ctop`, filters: [{ name: 'CtoP project', extensions: ['ctop'] }] });
      if (!p) return;
      path = p;
    }
    project.name = path.split('/').pop()!.replace(/\.ctop$/, '');
    if (await act(() => api.writeTextFile(path!, serialize()))) { project.path = path; project.dirty = false; toast('Saved', 'ok', 1500); }
  }
  async function openProject() {
    const p = await open({ multiple: false, filters: [{ name: 'CtoP project', extensions: ['ctop'] }] });
    if (!p || typeof p !== 'string') return;
    try { await load(await api.readTextFile(p), p); project.name = p.split('/').pop()!.replace(/\.ctop$/, ''); } catch (e) { toast(String(e), 'error'); }
  }
  async function resetProject() {
    if (project.objects.length && !(await confirmDialog({ title: 'Start a new project?', body: 'Unsaved changes will be lost.', confirmLabel: 'New project' }))) return;
    newProject();
  }
  onMount(() => {
    const t = setInterval(() => { if (project.dirty && project.objects.length) api.setSetting('autosave', serialize()).catch(() => {}); }, 30000);
    return () => clearInterval(t);
  });

  // ── jobs ──
  const blockers = $derived.by(() => {
    const b: string[] = [];
    if (!pipe.strokes.length) b.push('Nothing to draw yet');
    if (pipe.outside && !project.settings.clipToMargins) b.push(`${pipe.outside} object(s) extend beyond the margins`);
    return b;
  });
  const drawWhy = $derived.by(() => {
    const w: string[] = [];
    if (!pipe.strokes.length) w.push('nothing to draw');
    if (cal.status && !cal.status.readyForDrawing) w.push('calibration incomplete');
    if (s.link !== 'ready') w.push('not connected');
    else if (s.armed !== 'envelope') w.push('not armed (Manual Control)');
    else if (!s.axesKnown.every(Boolean)) w.push('position not verified');
    if (s.job && ['running', 'paused', 'cancelling'].includes(s.job.state)) w.push('job already running');
    return w;
  });

  async function makeJob() {
    return api.createJob({ name: project.name, strokes: pipe.strokes, plan: project.settings.plan, settings: project.settings.gcode, paper: [project.paper.w, project.paper.h] });
  }
  async function queue() {
    try { const j = await makeJob(); toast(`Queued “${j.name}” — ${j.stats.strokes} strokes, ≈${fmtDuration(j.stats.estimatedSeconds)}`, 'ok'); return j; } catch (e) { toast(String(e), 'error'); return null; }
  }
  async function drawNow() {
    const j = await queue();
    if (!j) return;
    const ok = await confirmDialog({
      title: `Draw “${j.name}” now?`,
      body: `The machine will move and the pencil will touch the paper.\nDraw ${j.stats.drawLengthMm.toFixed(0)} mm · ${j.stats.strokes} strokes · est. ${fmtDuration(j.stats.estimatedSeconds)} (estimate only).`,
      checks: ['Paper is loaded, flat, and matches the calibrated sheet size and position.', 'The area is clear and I can cut printer power immediately.'],
      confirmLabel: 'Start drawing',
    });
    if (ok && (await act(() => api.startJob(j.id, true)))) ui.view = 'jobs';
  }
  async function exportGcode() {
    const j = await queue();
    if (!j) return;
    const p = await save({ defaultPath: `${project.name}.gcode`, filters: [{ name: 'G-code', extensions: ['gcode'] }] });
    if (p && (await act(() => api.exportJob(j.id, p)))) toast(`Exported ${p}`, 'ok');
  }

  // scrub animation
  let raf = 0;
  function play() {
    if (playing) { playing = false; cancelAnimationFrame(raf); return; }
    playing = true; if (scrub >= 0.999) scrub = 0;
    const total = Math.max(4, Math.min(40, (pipe.plan?.stats.estimatedSeconds ?? 20) / 30));
    let last = performance.now();
    const step = (t: number) => {
      scrub = Math.min(1, scrub + (t - last) / 1000 / total); last = t;
      if (scrub < 1 && playing) raf = requestAnimationFrame(step); else playing = false;
    };
    raf = requestAnimationFrame(step);
  }

  // ── keys ──
  function key(e: KeyboardEvent) {
    if (ui.view !== 'workspace') return;
    const t = e.target as HTMLElement;
    const typing = t && ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName);
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === 'z') { e.preventDefault(); e.shiftKey ? redo() : undo(); return; }
    if (mod && e.key.toLowerCase() === 'y') { e.preventDefault(); redo(); return; }
    if (mod && e.key.toLowerCase() === 's') { e.preventDefault(); saveProject(e.shiftKey); return; }
    if (mod && e.key.toLowerCase() === 'o') { e.preventDefault(); openProject(); return; }
    if (typing) return;
    if (mod && e.key.toLowerCase() === 'd') { e.preventDefault(); duplicateSelected(); return; }
    if (mod && e.key.toLowerCase() === 'a') { e.preventDefault(); sel.ids = project.objects.map((o) => o.id); return; }
    if (e.key === 'Delete' || e.key === 'Backspace') { removeSelected(); return; }
    const d = e.shiftKey ? 10 : e.altKey ? 0.1 : 1;
    if (e.key === 'ArrowLeft') { e.preventDefault(); nudge(-d, 0); } else if (e.key === 'ArrowRight') { e.preventDefault(); nudge(d, 0); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); nudge(0, -d); } else if (e.key === 'ArrowDown') { e.preventDefault(); nudge(0, d); }
    else if (e.key === 'Escape') sel.ids = [];
    else if (!mod) {
      const k = e.key.toLowerCase();
      if (k === 'v') tool = 'select'; else if (k === 't') tool = 'text'; else if (k === 'p') tool = 'pen'; else if (k === 'h') tool = 'hand';
      else if (k === '0') canvas?.fit(); else if (k === '=' || k === '+') canvas?.zoomBy(1.25); else if (k === '-') canvas?.zoomBy(0.8);
    }
  }
  async function textCreated() { await tick(); textarea?.focus(); textarea?.select(); }
  const setPlan = (p: Partial<typeof project.settings.plan>) => { checkpoint('plan'); project.settings.plan = { ...project.settings.plan, ...p }; };
  const setG = (p: Partial<typeof project.settings.gcode>) => { checkpoint('g'); project.settings.gcode = { ...project.settings.gcode, ...p }; };
  void DEFAULT_GCODE; void DEFAULT_PLAN;
</script>

<svelte:window onkeydown={key} onpaste={onPaste} />

<div class="ws">
  <aside class="tools">
    {#each [['select', 'V', 'Select', '⌖'], ['text', 'T', 'Text', 'T'], ['pen', 'P', 'Pen', '✎'], ['hand', 'H', 'Pan', '✥']] as [id, k, n, ic]}
      <button class="tool" class:on={tool === id} title="{n} ({k})" onclick={() => (tool = id as Tool)}><span class="ic">{ic}</span><small>{n}</small></button>
    {/each}
    <hr />
    <button class="tool" title="Import image" onclick={() => pickImport('image')}><span class="ic">▣</span><small>Image</small></button>
    <button class="tool" title="Import SVG" onclick={() => pickImport('svg')}><span class="ic">◇</span><small>SVG</small></button>
    <hr />
    <button class="tool" title="Undo (Ctrl+Z)" disabled={!hist.past.length} onclick={undo}><span class="ic">↶</span><small>Undo</small></button>
    <button class="tool" title="Redo" disabled={!hist.future.length} onclick={redo}><span class="ic">↷</span><small>Redo</small></button>
    <span class="grow"></span>
    <button class="tool" title="Open project (Ctrl+O)" onclick={openProject}><span class="ic">⤓</span><small>Open</small></button>
    <button class="tool" title="Save project (Ctrl+S)" onclick={() => saveProject()}><span class="ic">⤒</span><small>Save</small></button>
    <button class="tool" title="New project" onclick={resetProject}><span class="ic">＋</span><small>New</small></button>
  </aside>

  <section class="stage" role="presentation" ondragover={(e) => { e.preventDefault(); over = true; }} ondragleave={() => (over = false)} ondrop={onDrop}>
    <div class="pbar">
      <b>{project.name}{project.dirty ? ' •' : ''}</b>
      <span class="mute mono">{project.paper.name} {project.paper.orientation} · {project.paper.w}×{project.paper.h} mm</span>
      <span class="grow"></span>
      <span class="mute small">Drop images / SVG / text · paste from clipboard · {tool === 'pen' ? 'drag to sketch' : tool === 'text' ? 'click to place text' : 'click to select'}</span>
    </div>
    <div class="cv"><Canvas bind:this={canvas} bind:tool {showPath} {scrub} ontextcreated={textCreated} />
      {#if over}<div class="dropmark">Drop to import</div>{/if}</div>
  </section>

  <aside class="side">
    <div class="scroll">
      <Inspector bind:textarea />
      <div class="panel">
        <header><h3>Pencil path</h3>{#if pipe.busy}<span class="mute small">planning…</span>{/if}</header>
        <div class="body col">
          <div class="field"><span class="label">Drawing order</span>
            <select value={project.settings.plan.order} onchange={(e) => setPlan({ order: (e.target as HTMLSelectElement).value as Order })}>
              <option value="optimized">Optimised (least pen-up travel)</option><option value="asDrawn">As drawn (document order)</option><option value="topToBottom">Top to bottom</option></select></div>
          <div class="field"><span class="label">Join gap</span><span class="row"><input type="number" style="width:72px" min="0" max="2" step="0.05" value={project.settings.plan.joinGapMm} oninput={(e) => setPlan({ joinGapMm: +(e.target as HTMLInputElement).value })} /> mm</span></div>
          <div class="field"><span class="label">Line width (preview)</span><span class="row"><input type="number" style="width:72px" min="0.1" max="3" step="0.1" bind:value={project.settings.strokeWidthMm} /> mm</span></div>
          <div class="field"><span class="label">Draw speed</span><span class="row"><input type="number" style="width:72px" min="60" max="6000" step="100" value={project.settings.gcode.drawFeed} oninput={(e) => setG({ drawFeed: +(e.target as HTMLInputElement).value })} /> mm/min</span></div>
          <div class="field"><span class="label">Travel speed</span><span class="row"><input type="number" style="width:72px" min="60" max="9000" step="100" value={project.settings.gcode.travelFeed} oninput={(e) => setG({ travelFeed: +(e.target as HTMLInputElement).value })} /> mm/min</span></div>
          <div class="field"><span class="label">Z speed</span><span class="row"><input type="number" style="width:72px" min="30" max="1200" step="30" value={project.settings.gcode.zFeed} oninput={(e) => setG({ zFeed: +(e.target as HTMLInputElement).value })} /> mm/min</span></div>
          <label class="row"><input type="checkbox" bind:checked={project.settings.clipToMargins} /> Clip artwork to margins</label>

          <hr />
          {#if pipe.error}<div class="notice bad">{pipe.error}</div>{/if}
          {#if pipe.plan}
            <div class="kv mono">
              <span>Strokes</span><span>{pipe.plan.stats.strokes}</span>
              <span>Pen-down</span><span>{pipe.plan.stats.drawLengthMm.toFixed(0)} mm</span>
              <span>Pen-up travel</span><span>{pipe.plan.stats.travelLengthMm.toFixed(0)} mm</span>
              <span>Pen lifts</span><span>{pipe.plan.stats.penLifts}</span>
              <span>Est. time</span><span>≈ {fmtDuration(pipe.plan.stats.estimatedSeconds)}</span>
            </div>
            <div class="mute small">Estimate from path length, speeds and a simple acceleration model.{pipe.plan.zAssumed ? ' Z travel assumed (calibration incomplete).' : ''}</div>
          {:else if !pipe.error}<div class="mute small">Add something to see the machine path.</div>{/if}
          {#if pipe.outside}<div class="notice small">{pipe.outside} stroke(s) cross the margins{project.settings.clipToMargins ? ' and will be clipped' : ' — they would be refused'}.</div>{/if}
          {#each pipe.notes.slice(0, 3) as n}<div class="notice small">{n}</div>{/each}

          <label class="row"><input type="checkbox" bind:checked={showPath} /> <b>Inspect machine path</b> <span class="mute small">(solid = pen down, dashed = pen up)</span></label>
          {#if showPath}
            <div class="row"><button class="btn sm" onclick={play}>{playing ? '❚❚' : '▶'}</button><input type="range" min="0" max="1" step="0.002" bind:value={scrub} oninput={() => { playing = false; }} /></div>
          {/if}

          <hr />
          <div class="row wrap">
            <button class="btn primary" disabled={drawWhy.length > 0 || !!blockers.length} title={drawWhy.join(' · ')} onclick={drawNow}>Draw…</button>
            <button class="btn" disabled={!pipe.strokes.length} onclick={queue}>Add to queue</button>
            <button class="btn" disabled={!pipe.strokes.length} onclick={exportGcode}>Export G-code…</button>
          </div>
          {#if drawWhy.length}<div class="mute small">Draw disabled: {drawWhy.join(' · ')}</div>{/if}
        </div>
      </div>
    </div>
  </aside>
</div>

<style>
  .ws { display: grid; grid-template-columns: 62px 1fr 368px; height: 100%; min-height: 0; }
  .tools { background: var(--paper-2); border-right: 1px solid var(--line-strong); display: flex; flex-direction: column; padding: 6px; gap: 4px; align-items: stretch; }
  .tools hr { margin: 4px 0; }
  .tool { border: 1px solid transparent; background: transparent; padding: 6px 2px; cursor: pointer; display: flex; flex-direction: column; align-items: center; gap: 1px; }
  .tool:hover:not(:disabled) { border-color: var(--line-strong); background: #fff; } .tool:disabled { opacity: .35; cursor: not-allowed; }
  .tool.on { background: var(--ink); color: var(--paper); }
  .tool .ic { font-size: 18px; line-height: 1; } .tool small { font-size: 9.5px; letter-spacing: .04em; }
  .stage { display: grid; grid-template-rows: 32px 1fr; min-width: 0; min-height: 0; }
  .pbar { display: flex; gap: 14px; align-items: center; padding: 0 14px; border-bottom: 1px solid var(--line); background: var(--panel); }
  .cv { position: relative; min-height: 0; }
  .dropmark { position: absolute; inset: 14px; border: 2px dashed var(--ink); background: rgba(243,241,234,.7); display: grid; place-items: center; font-weight: 600; pointer-events: none; }
  .side { border-left: 1px solid var(--line-strong); background: var(--paper); min-height: 0; }
  .scroll { height: 100%; overflow-y: auto; padding: 10px; display: flex; flex-direction: column; gap: 10px; }
  .small { font-size: 11.5px; } .wrap { flex-wrap: wrap; }
</style>
