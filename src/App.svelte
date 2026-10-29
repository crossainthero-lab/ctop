<script lang="ts">
  import { onMount } from 'svelte';
  import { app, startApp, savePrefsSoon, errText } from './lib/app.svelte';
  import { call } from './lib/rpc';
  import { loadFontList, ensureFont, type FontListEntry } from './lib/text/fontregistry';
  import { decodeImageFile, type ImageSource } from './lib/write/imagejob';
  import { textJob, linedJob, imageJob, pageKey, type JobPlan, type Mode } from './lib/plan/jobs';
  import { formatDuration } from './lib/plan/estimate';
  import { checkStrokes, reachInsets, A4, type Reach } from './lib/frame';
  import PagePreview from './components/PagePreview.svelte';
  import LiveCamera from './components/LiveCamera.svelte';
  import Calibration from './components/Calibration.svelte';
  import Setup from './components/Setup.svelte';
  import Technical from './components/Technical.svelte';

  let fonts = $state<FontListEntry[]>([]);
  let fontTick = $state(0);
  let fontErr = $state('');
  let showCal = $state(false);
  let showSetup = $state(false);
  let showTech = $state(false);
  let actionErr = $state('');
  let pageIdx = $state(0);
  onMount(() => { startApp(); loadFontList(call).then((f) => (fonts = f)); });

  const st = $derived(app.st);
  const p = $derived(app.prefs);
  const mode = $derived(p.mode);
  const frame = $derived(st?.frame.active ?? null);
  const sheet = $derived({ w: frame?.pageW ?? A4.w, h: frame?.pageH ?? A4.h, marginMm: frame?.marginMm ?? 8 });
  const reach = $derived((st?.controller.box as Reach | null | undefined) ?? null);
  const orient = $derived(mode === 'text' ? p.orient : mode === 'lined' ? p.lined.orient : p.img.orient);
  const activeFont = $derived(mode === 'lined' ? p.lined.fontId : p.fontId);

  /** margins can never be smaller than the safe margin, nor let anything leave the plotter's reach (in reading orientation) */
  const floor = $derived.by(() => {
    const i = frame ? reachInsets(frame, reach) : { left: sheet.marginMm, right: sheet.marginMm, top: sheet.marginMm, bottom: sheet.marginMm };
    return orient === 'portrait' ? i : { left: i.top, right: i.bottom, top: i.right, bottom: i.left };
  });
  const clampM = (m: { left: number; right: number; top: number; bottom: number }) => ({
    left: Math.max(floor.left, +m.left || 0), right: Math.max(floor.right, +m.right || 0), top: Math.max(floor.top, +m.top || 0), bottom: Math.max(floor.bottom, +m.bottom || 0),
  });
  const widened = $derived(floor.left > sheet.marginMm + 0.5 || floor.right > sheet.marginMm + 0.5 || floor.top > sheet.marginMm + 0.5 || floor.bottom > sheet.marginMm + 0.5);

  // outline fonts are fetched once; layout itself stays synchronous
  $effect(() => {
    const id = activeFont; fontErr = '';
    ensureFont(id, call).then(() => fontTick++).catch((e) => (fontErr = `Could not load this font: ${errText(e)}`));
  });

  // ── Mode A / C: planned synchronously from the inputs ──
  const sync = $derived.by((): { plan: JobPlan | null; error: string } => {
    void fontTick;
    try {
      if (mode === 'text') return { plan: textJob(p.text, sheet, { fontId: p.fontId, sizeMm: Math.max(2, +p.sizeMm || 2), lineSpacing: Math.max(0.5, +p.lineSpacing || 1), align: p.align, margins: clampM(p.margins), orient: p.orient }), error: '' };
      if (mode === 'lined') {
        const l = p.lined;
        return { plan: linedJob(l.text, sheet, { fontId: l.fontId, sizeMm: Math.max(2, +l.sizeMm || 2), align: l.align, orient: l.orient, spacingMm: Math.max(4, +l.spacingMm || 8), margins: clampM(l.margins), border: l.border, marginLine: l.marginLine, marginLineOffsetMm: +l.marginLineOffsetMm || 20, lineStyle: l.lineStyle, lineCount: l.autoLines ? 0 : Math.max(1, Math.round(+l.lineCount || 1)), writeEvery: Math.max(1, Math.round(+l.writeEvery || 1)) }), error: '' };
      }
    } catch (e) { return { plan: null, error: errText(e) }; }
    return { plan: null, error: '' };
  });

  // ── Mode B: image conversion (heavier, so debounced) ──
  let imgSrc = $state<ImageSource | null>(null);
  let imgName = $state('');
  let imgErr = $state('');
  let imgPlan = $state<JobPlan | null>(null);
  let imgWarn = $state<string[]>([]);
  let imgBusy = $state(false);
  let dragging = $state(false);
  async function pickFile(f: File | undefined | null) {
    if (!f) return;
    imgErr = '';
    try { imgSrc = await decodeImageFile(f); imgName = f.name; pageIdx = 0; } catch (e) { imgErr = errText(e); }
  }
  let convTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const src = imgSrc, i = p.img, sh = { ...sheet }, on = mode === 'image';
    const o = { style: i.style, detail: i.detail, scale: i.scale, rotationDeg: i.rotation, offsetX: i.x, offsetY: i.y, invert: i.invert, clean: i.clean, margins: clampM(i.margins), orient: i.orient };
    clearTimeout(convTimer);
    if (!on || !src) { imgPlan = null; imgBusy = false; return; }
    imgBusy = true;
    convTimer = setTimeout(() => {
      try { const r = imageJob(src, sh, o); imgPlan = r.plan; imgWarn = r.warnings; imgErr = ''; }
      catch (e) { imgPlan = null; imgErr = errText(e); }
      imgBusy = false;
    }, 250);
  });

  const plan = $derived<JobPlan | null>(mode === 'image' ? imgPlan : sync.plan);
  const pages = $derived(plan?.pages ?? []);
  $effect(() => { if (pageIdx > Math.max(0, pages.length - 1)) pageIdx = Math.max(0, pages.length - 1); });
  const page = $derived(pages[pageIdx] ?? null);
  const hasContent = $derived(mode === 'text' ? !!p.text.trim() : mode === 'lined' ? true : !!imgSrc);

  const fit = $derived(page && frame ? checkStrokes(page.strokes, frame, reach) : null);
  /** why this content cannot be drawn (as opposed to why the machine is not ready) */
  const contentProblem = $derived.by(() => {
    if (mode === 'text' && !p.text.trim()) return 'Type or paste the text to write.';
    if (mode === 'image' && !imgSrc) return 'Choose an image to draw.';
    if (mode === 'lined' && !p.lined.text.trim() && p.lined.lineStyle === 'hidden') return 'Type some text to write on the lines.';
    if (sync.error) return sync.error;
    if (mode === 'image' && imgErr) return imgErr;
    if (mode === 'image' && imgBusy) return 'Converting the image…';
    if (!page) return mode === 'image' ? 'No lines were found in this image. Try another style.' : 'Nothing to draw yet.';
    if (plan?.overflow) return 'The text is longer than the pages that can be planned.';
    if (fit && fit.offPaper) return 'Part of this is closer to the paper edge than the safe margin. Reduce the size or increase the margins.';
    if (fit && fit.offReach) return 'Part of this is outside what the plotter can reach (hatched in the preview). Make it smaller or move it.';
    return '';
  });
  const blockers = $derived(st?.drawBlockers ?? []);
  const job = $derived(st?.job ?? null);
  const live = $derived(!!job && (job.live || job.state === 'paused'));
  const problem = $derived(contentProblem || blockers[0] || '');
  const verb = $derived(mode === 'image' ? 'Draw' : mode === 'lined' ? 'Rule and write' : 'Draw');
  const drawLabel = $derived(pages.length > 1 ? `${verb} sheet ${pageIdx + 1} of ${pages.length}` : verb);

  // ── draw / pause / resume / cancel ──
  let myJob: string | null = null;
  let waiting = false;
  const label = $derived(mode === 'text' ? p.text.trim().slice(0, 40) : mode === 'lined' ? 'Lined: ' + p.lined.text.trim().slice(0, 30) : imgName.slice(0, 40));
  async function go() {
    if (!page || problem) return;
    actionErr = '';
    try {
      await call('draw', { strokes: page.strokes, stages: page.stages.map((s) => ({ label: s.label, count: s.count })), key: pageKey(page.strokes, `${mode}${pageIdx}`), name: label || 'CtoP drawing' });
      waiting = true; myJob = null;
    } catch (e) { actionErr = errText(e); }
  }
  const act = (cmd: string) => call(cmd).catch((e) => (actionErr = errText(e)));
  const estop = () => act('estop');
  $effect(() => {
    const j = st?.job;
    if (!j) return;
    if (waiting && j.live && j.kind === 'draw') { myJob = j.id; waiting = false; }
    if (myJob && j.id === myJob && j.state === 'completed') {
      myJob = null;
      if (pageIdx < pages.length - 1) pageIdx++;
    }
  });
  function onKey(e: KeyboardEvent) { if (e.shiftKey && e.key === 'Escape') { estop(); } }
  function edit() { savePrefsSoon(); if (st?.phase === 'finished') call('dismiss').catch(() => {}); }
  function setMode(m: Mode) { p.mode = m; pageIdx = 0; edit(); }

  const eta = $derived(job?.etaS != null ? formatDuration(job.etaS) : '');
  const planTime = $derived(page ? formatDuration(page.stats.seconds) : '');
  const mmss = (s: number) => `${Math.floor(s / 60)}:${String(Math.round(s % 60)).padStart(2, '0')}`;
  const NOTEBOOK = [{ n: 'Wide ruled (8.7 mm)', v: 8.7 }, { n: 'School notebook (8 mm)', v: 8 }, { n: 'College ruled (7.1 mm)', v: 7.1 }, { n: 'Narrow ruled (6.4 mm)', v: 6.4 }];
  const plotterText = $derived(!st ? 'Connecting…' : !st.controller.reachable ? 'Plotter not connected' : live ? 'Plotter drawing' : st.controller.estop ? 'Emergency stop latched' : st.controller.ready ? 'Plotter ready · homed' : 'Plotter not ready');
  const plotterOk = $derived(!!st?.controller.ready || (live && !!st?.controller.reachable));
  const spacingForLimit = $derived(p.lined.spacingMm * 0.8);
</script>

<svelte:window onkeydown={onKey} />

{#if !st}
  <div class="center">
    <div>
      <h1>CtoP</h1>
      <p class="mute">{app.backendError ?? 'Connecting…'}</p>
    </div>
  </div>
{:else if !st.frame}
  <div class="center"><div><h1>CtoP</h1><p>The CtoP service that is running is older than this page.</p><p class="mute">Restart it (stop the running <code>ctop</code> and start it again) and reload.</p></div></div>
{:else}
  <div class="app">
    <header class="top">
      <div class="brand"><svg width="22" height="22" viewBox="0 0 24 24" aria-hidden="true"><path d="M4 20l1.2-4.2L16.5 4.5a1.8 1.8 0 012.5 0l.5.5a1.8 1.8 0 010 2.5L8.2 18.8 4 20z" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"/></svg><b>CtoP</b></div>
      <div class="modes" role="tablist" aria-label="Mode">
        <button role="tab" aria-selected={mode === 'text'} class:on={mode === 'text'} onclick={() => setMode('text')} disabled={live}><b>Text</b><span>Type it, draw it</span></button>
        <button role="tab" aria-selected={mode === 'image'} class:on={mode === 'image'} onclick={() => setMode('image')} disabled={live}><b>Image</b><span>PNG, JPG, SVG</span></button>
        <button role="tab" aria-selected={mode === 'lined'} class:on={mode === 'lined'} onclick={() => setMode('lined')} disabled={live}><b>Lined paper</b><span>Ruled page plus writing</span></button>
      </div>
      <div class="chips">
        <button class="chip ghost" onclick={() => (showSetup = true)} title="Plotter connection and settings"><i class="dot" class:ok={plotterOk} class:bad={!st.controller.reachable || st.controller.estop}></i>{plotterText}</button>
        <button class="chip ghost" onclick={() => (showCal = true)} title="Open calibration"><i class="dot" class:ok={st.frame.ready} class:warnc={!st.frame.ready}></i>{st.frame.active ? (st.frame.ready ? `Frame: ${st.frame.active.name}` : 'Frame: not verified') : 'Frame: not calibrated'}</button>
        <button class="gear ghost" onclick={() => (showSetup = true)} aria-label="Settings">⚙</button>
      </div>
    </header>

    {#if !st.frame.ready}
      <div class="banner">
        <span>{st.frame.active ? 'The frame calibration has not been verified yet.' : 'One-time setup: tell CtoP where the paper sits and how high the pen touches.'} You can try everything here; drawing unlocks once this is done.</span>
        <button class="primary" onclick={() => (showCal = true)}>Open calibration</button>
      </div>
    {/if}

    <main>
      <section class="stage" aria-label="Page preview">
        <PagePreview {page} {orient} {sheet} {frame} {reach} showTravel={p.showTravel} progress={live && job?.kind === 'draw' ? { done: job.done, total: job.total } : null} />
        <div class="stagebar">
          {#if pages.length > 1}
            <div class="pager"><button class="ghost" disabled={pageIdx === 0 || live} onclick={() => pageIdx--} aria-label="Previous sheet">‹</button><span>Sheet {pageIdx + 1} of {pages.length}</span><button class="ghost" disabled={pageIdx >= pages.length - 1 || live} onclick={() => pageIdx++} aria-label="Next sheet">›</button></div>
          {/if}
          <label class="check small"><input type="checkbox" bind:checked={p.showTravel} onchange={edit} /> Show pen-up travel</label>
          {#if page}<span class="small mute">{page.stats.strokes} strokes · {Math.round(page.stats.drawMm)} mm drawn · {Math.round(page.stats.travelMm)} mm pen-up travel</span>{/if}
        </div>
        {#if orient === 'landscape'}<p class="small mute note">Landscape is written along the long edge of the portrait sheet in the frame. Turn the finished sheet a quarter turn to read it.</p>{/if}
        {#if widened}<p class="small mute note">Margins are widened where the plotter cannot reach (top {floor.top.toFixed(0)}, bottom {floor.bottom.toFixed(0)}, left {floor.left.toFixed(0)}, right {floor.right.toFixed(0)} mm).</p>{/if}
        {#each plan?.notes ?? [] as n}<p class="small mute note">{n}</p>{/each}
        {#if mode === 'image'}{#each imgWarn as w}<p class="small warn note">{w}</p>{/each}{/if}
        {#if plan?.missing.length}<p class="small warn note">This font cannot draw: {plan.missing.join(' ')}</p>{/if}
      </section>

      <aside class="side">
        <div class="controls">
          {#if mode === 'text'}
            <label>Your text
              <textarea rows="9" bind:value={p.text} oninput={() => { pageIdx = 0; edit(); }} disabled={live} placeholder="Type or paste here. Paragraphs, long text and new lines are fine."></textarea>
            </label>
          {:else if mode === 'lined'}
            <label>Text to write on the lines
              <textarea rows="7" bind:value={p.lined.text} oninput={() => { pageIdx = 0; edit(); }} disabled={live} placeholder="Type or paste here. It is written line by line on the ruled page."></textarea>
            </label>
          {:else}
            <label class="drop" class:dragging ondragover={(e) => { e.preventDefault(); dragging = true; }} ondragleave={() => (dragging = false)} ondrop={(e) => { e.preventDefault(); dragging = false; pickFile(e.dataTransfer?.files[0]); }}>
              <b>{imgSrc ? imgName : 'Choose or drop an image'}</b>
              <span class="small">PNG, JPEG, WEBP or SVG</span>
              <input type="file" accept="image/png,image/jpeg,image/webp,image/svg+xml,.svg" disabled={live} onchange={(e) => pickFile((e.currentTarget as HTMLInputElement).files?.[0])} />
            </label>
            {#if imgErr}<p class="err small">{imgErr}</p>{/if}
          {/if}

          {#if mode === 'text' || mode === 'lined'}
            {@const t = mode === 'text' ? p : p.lined}
            <div class="row">
              <label>Font
                <select bind:value={t.fontId} onchange={edit} disabled={live}>
                  <optgroup label="Single-line plotter fonts">{#each fonts.filter((f) => f.kind === 'stroke') as f}<option value={f.id}>{f.name}</option>{/each}</optgroup>
                  {#if fonts.some((f) => f.kind === 'outline')}<optgroup label="Installed fonts (outlines)">{#each fonts.filter((f) => f.kind === 'outline') as f}<option value={f.id}>{f.name}</option>{/each}</optgroup>{/if}
                </select>
              </label>
              <label>Size (mm)<input type="number" min="2" max="40" step="0.5" bind:value={t.sizeMm} oninput={edit} disabled={live} /></label>
            </div>
            {#if t.fontId.startsWith('sys:')}<p class="mute small">Installed fonts are filled shapes; the pen traces their outlines. Single-line fonts are faster and look more like handwriting.</p>{/if}
            {#if fontErr}<p class="err small">{fontErr}</p>{/if}
            <div class="row">
              <div class="seg" role="group" aria-label="Alignment">
                {#each [['left', 'Left'], ['center', 'Centre'], ['right', 'Right']] as [v, n]}<button class:on={t.align === v} onclick={() => { t.align = v as 'left'; edit(); }} disabled={live}>{n}</button>{/each}
              </div>
              {#if mode === 'text'}<label>Line spacing<input type="number" min="0.5" max="4" step="0.1" bind:value={p.lineSpacing} oninput={edit} disabled={live} /></label>{/if}
            </div>
          {/if}

          {#if mode === 'lined'}
            <h3>The ruled page</h3>
            <label>Paper style
              <select value={NOTEBOOK.find((n) => n.v === p.lined.spacingMm)?.v ?? 'custom'} onchange={(e) => { const v = e.currentTarget.value; if (v !== 'custom') { p.lined.spacingMm = +v; edit(); } }} disabled={live}>
                {#each NOTEBOOK as n}<option value={n.v}>{n.n}</option>{/each}
                <option value="custom">Custom spacing</option>
              </select>
            </label>
            <div class="row">
              <label>Line spacing (mm)<input type="number" min="4" max="20" step="0.1" bind:value={p.lined.spacingMm} oninput={edit} disabled={live} /></label>
              <label>Write on<select bind:value={p.lined.writeEvery} onchange={edit} disabled={live}><option value={1}>every line</option><option value={2}>every 2nd line</option><option value={3}>every 3rd line</option></select></label>
            </div>
            {#if p.lined.sizeMm > spacingForLimit}<p class="small warn">Text will be limited to {spacingForLimit.toFixed(1)} mm so it fits between the lines.</p>{/if}
            <div class="row">
              <label class="check"><input type="checkbox" bind:checked={p.lined.autoLines} onchange={edit} disabled={live} /> Fill the page with lines</label>
              {#if !p.lined.autoLines}<label>Number of lines<input type="number" min="1" max="80" bind:value={p.lined.lineCount} oninput={edit} disabled={live} /></label>{/if}
            </div>
            <label>Line visibility
              <select bind:value={p.lined.lineStyle} onchange={edit} disabled={live}>
                <option value="solid">Solid lines</option><option value="dashed">Light dashed lines (slower)</option><option value="hidden">Don't draw lines (write on invisible lines)</option>
              </select>
            </label>
            <div class="row">
              <label class="check"><input type="checkbox" bind:checked={p.lined.border} onchange={edit} disabled={live || p.lined.lineStyle === 'hidden'} /> Draw page margins</label>
              <label class="check"><input type="checkbox" bind:checked={p.lined.marginLine} onchange={edit} disabled={live || p.lined.lineStyle === 'hidden'} /> Vertical margin line</label>
            </div>
            {#if p.lined.marginLine}<label>Margin line, mm from the left margin<input type="number" min="5" max="60" bind:value={p.lined.marginLineOffsetMm} oninput={edit} disabled={live} /></label>{/if}
          {/if}

          {#if mode === 'image'}
            <label>Drawing style
              <select bind:value={p.img.style} onchange={edit} disabled={live}>
                <option value="outline">Edge tracing</option><option value="threshold">Outlines of dark shapes</option><option value="detail">Line art (centre lines)</option>
                <option value="shade">Hatching and shading</option><option value="crosshatch">Cross-hatching (dark areas)</option>
              </select>
            </label>
            <p class="small mute">SVG files are drawn as their own vector paths. Styles apply to photos and pictures.</p>
            <label>Detail <input type="range" min="0.1" max="1" step="0.05" bind:value={p.img.detail} oninput={edit} disabled={live} /></label>
            <label>Size <input type="range" min="0.1" max="1" step="0.05" bind:value={p.img.scale} oninput={edit} disabled={live} /></label>
            <label>Rotate: {p.img.rotation}°
              <input type="range" min="-180" max="180" step="1" bind:value={p.img.rotation} oninput={edit} disabled={live} />
            </label>
            <div class="row"><button disabled={live} onclick={() => { p.img.rotation = ((p.img.rotation + 270 + 180) % 360) - 180; edit(); }}>↶ 90°</button><button disabled={live} onclick={() => { p.img.rotation = ((p.img.rotation + 90 + 180) % 360) - 180; edit(); }}>↷ 90°</button><button disabled={live} onclick={() => { p.img.rotation = 0; edit(); }}>Reset</button></div>
            <div class="row">
              <label>Left ↔ right<input type="range" min="0" max="1" step="0.02" bind:value={p.img.x} oninput={edit} disabled={live} /></label>
              <label>Top ↔ bottom<input type="range" min="0" max="1" step="0.02" bind:value={p.img.y} oninput={edit} disabled={live} /></label>
            </div>
            <label class="check"><input type="checkbox" bind:checked={p.img.clean} onchange={edit} disabled={live} /> Remove a plain background</label>
            <label class="check"><input type="checkbox" bind:checked={p.img.invert} onchange={edit} disabled={live} /> Swap light and dark</label>
          {/if}

          <div class="row">
            <div class="seg" role="group" aria-label="Page orientation">
              {#each [['portrait', 'Portrait'], ['landscape', 'Landscape']] as [v, n]}
                <button class:on={orient === v} disabled={live} onclick={() => { if (mode === 'text') p.orient = v as 'portrait'; else if (mode === 'lined') p.lined.orient = v as 'portrait'; else p.img.orient = v as 'portrait'; edit(); }}>{n}</button>
              {/each}
            </div>
          </div>
          <details>
            <summary class="mute small">Page margins (mm)</summary>
            {#each [mode === 'text' ? p.margins : mode === 'lined' ? p.lined.margins : p.img.margins] as m}
              <div class="row">
                {#each [['left', 'Left'], ['right', 'Right'], ['top', 'Top'], ['bottom', 'Bottom']] as [k, n]}
                  <label>{n}<input type="number" min={floor[k as 'left']} max="100" bind:value={m[k as 'left']} oninput={edit} disabled={live} /></label>
                {/each}
              </div>
            {/each}
            <p class="small mute">Never smaller than the frame's {sheet.marginMm} mm safe margin or the plotter's reach.</p>
          </details>
        </div>

        <div class="drawpanel">
          {#if st.phase === 'error' && st.error}
            <div class="state bad">
              <b>Something went wrong</b>
              <p class="small">{st.error.message}{st.error.detail ? ' ' + st.error.detail : ''}</p>
              <div class="row"><button onclick={() => act('dismiss')}>OK</button><button class="ghost" onclick={() => (showTech = true)}>Technical view</button></div>
            </div>
          {:else if st.phase === 'preparing'}
            <div class="state"><b>Checking and starting…</b><p class="small mute">The plotter's own safety check is running.</p></div>
          {:else if job && (job.live || job.state === 'paused')}
            <div class="state">
              <div class="prog"><div class="bar"><div style="width:{job.percent}%"></div></div><b class="pct">{job.percent.toFixed(0)}%</b></div>
              <p class="op">{job.operation}</p>
              <p class="small mute">{job.state === 'paused' ? 'Paused' : st.controller.reachable ? 'Drawing' : 'Lost contact with the plotter'} · elapsed {mmss(job.elapsedS)}{eta && job.state !== 'paused' ? ` · about ${eta} left` : ''}</p>
              {#if !st.controller.reachable}<p class="small err">The connection to the plotter dropped. It finishes or stops by itself. Do not touch it; this page will reconnect.</p>{/if}
              <div class="row">
                {#if job.state === 'paused'}<button class="primary" onclick={() => act('resume')}>Resume</button>{:else}<button disabled={job.state !== 'running'} onclick={() => act('pause')}>Pause</button>{/if}
                <button disabled={job.state === 'stopping'} onclick={() => act('stop')}>Cancel</button>
              </div>
            </div>
          {:else if st.phase === 'finished'}
            <div class="state good">
              <b>✓ {st.message}</b>
              {#if pageIdx < pages.length - 1}<p class="small">Put a fresh sheet in the frame, then draw sheet {pageIdx + 1} of {pages.length}.</p>{/if}
              <button onclick={() => act('dismiss')}>OK</button>
            </div>
          {:else}
            <button class="primary big" disabled={!!problem || !st.canDraw} onclick={go}>{drawLabel}</button>
            {#if page && !problem}<p class="small mute center-t">About {planTime}{pages.length > 1 ? ` for this sheet` : ''}</p>{/if}
            {#if problem}<p class="small {contentProblem ? 'mute' : 'warn'} center-t">{problem}</p>{/if}
            {#if st.phase === 'ready' && st.message === 'Stopped.'}<p class="small mute center-t">Cancelled. The pen is up.</p>{/if}
          {/if}
          {#if actionErr}<p class="small err">{actionErr}</p>{/if}
          <div class="tools">
            <button class="estop" onclick={estop} disabled={!st.controller.reachable} title="Stops all motion at once. Shift+Esc">Emergency stop</button>
          </div>
        </div>
        <LiveCamera />
      </aside>
    </main>

    {#if showCal}<Calibration onclose={() => (showCal = false)} />{/if}
    {#if showSetup}<Setup onclose={() => (showSetup = false)} oncalibrate={() => { showSetup = false; showCal = true; }} ontech={() => { showSetup = false; showTech = true; }} />{/if}
    {#if showTech}<Technical onclose={() => (showTech = false)} />{/if}
  </div>
{/if}

<style>
  .app { display: flex; flex-direction: column; height: 100%; }
  .center { height: 100%; display: grid; place-items: center; text-align: center; }
  .top { display: flex; align-items: center; gap: 18px; padding: 10px 18px; border-bottom: 1px solid var(--line); background: var(--surface); flex-wrap: wrap; }
  .brand { display: flex; align-items: center; gap: 8px; font-size: 16px; letter-spacing: -0.01em; }
  .modes { display: flex; gap: 6px; flex: 1; justify-content: center; min-width: 0; flex-wrap: wrap; }
  .modes button { display: grid; gap: 0; text-align: left; padding: 8px 16px; min-width: 150px; background: transparent; border-color: var(--line); }
  .modes button b { font-size: 14px; } .modes button span { font-size: 11px; color: var(--mute); }
  .modes button.on { background: var(--accent); border-color: var(--accent); color: var(--accent-ink); } .modes button.on span { color: inherit; opacity: .85; }
  .chips { display: flex; gap: 6px; align-items: center; }
  .chip { display: flex; align-items: center; gap: 7px; font-size: 12.5px; padding: 5px 10px; border-color: var(--line); color: var(--ink); }
  .gear { font-size: 17px; padding: 4px 9px; }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--mute); display: inline-block; } .dot.ok { background: var(--ok); } .dot.bad { background: var(--danger); } .dot.warnc { background: var(--warn); }
  .banner { display: flex; gap: 14px; align-items: center; justify-content: space-between; padding: 10px 18px; background: var(--accent-soft); border-bottom: 1px solid var(--line); flex-wrap: wrap; font-size: 13px; }
  main { flex: 1; min-height: 0; display: grid; grid-template-columns: minmax(0, 1fr) 380px; }
  .stage { padding: 16px 20px; display: flex; flex-direction: column; min-height: 0; min-width: 0; }
  .stagebar { display: flex; gap: 16px; align-items: center; justify-content: center; flex-wrap: wrap; margin-top: 8px; }
  .stagebar label { margin: 0; }
  .pager { display: flex; align-items: center; gap: 6px; }
  .note { text-align: center; margin: 4px 0 0; }
  .side { border-left: 1px solid var(--line); background: var(--surface); padding: 14px 16px; overflow: auto; display: flex; flex-direction: column; gap: 12px; }
  .controls h3 { margin: 12px 0 6px; }
  textarea { resize: vertical; font-size: 15px; line-height: 1.5; min-height: 120px; }
  .seg { display: flex; border: 1px solid var(--line); border-radius: 8px; overflow: hidden; margin-bottom: 10px; align-self: end; }
  .seg button { flex: 1; border: 0; border-radius: 0; background: transparent; padding: 7px 10px; } .seg button.on { background: var(--ink); color: var(--bg); }
  .drop { border: 1.5px dashed var(--line); border-radius: var(--radius); padding: 22px 12px; text-align: center; cursor: pointer; word-break: break-all; display: grid; gap: 2px; color: var(--ink); }
  .drop:hover, .drop.dragging { border-color: var(--accent); background: var(--accent-soft); } .drop span { color: var(--mute); } .drop input { display: none; }
  details { margin-bottom: 8px; }
  .drawpanel { position: sticky; bottom: -14px; background: var(--surface); padding: 12px 0 6px; border-top: 1px solid var(--line); margin-top: auto; display: grid; gap: 8px; }
  .state { border: 1px solid var(--line); border-radius: var(--radius); padding: 12px; display: grid; gap: 6px; background: var(--bg); }
  .state.bad { border-color: var(--danger); } .state.good { border-color: var(--ok); }
  .state p { margin: 0; }
  .prog { display: flex; gap: 10px; align-items: center; } .bar { flex: 1; height: 10px; background: var(--soft); border-radius: 6px; overflow: hidden; } .bar > div { height: 100%; background: var(--accent); transition: width .4s; }
  .pct { font-variant-numeric: tabular-nums; min-width: 3.2em; text-align: right; }
  .op { font-weight: 600; }
  .center-t { text-align: center; margin: 0; }
  .tools { display: flex; justify-content: flex-end; }
  .estop { border-color: var(--danger); color: var(--danger); background: transparent; font-size: 12.5px; padding: 5px 10px; }
  @media (max-width: 900px) {
    main { grid-template-columns: 1fr; overflow: auto; display: block; }
    .stage { min-height: 70vh; } .side { border-left: 0; border-top: 1px solid var(--line); overflow: visible; }
    .drawpanel { bottom: 0; }
    .modes button { min-width: 0; flex: 1; padding: 8px 10px; }
    .top { gap: 10px; padding: 8px 12px; } .chips { flex-wrap: wrap; }
  }
</style>
