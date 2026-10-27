<script lang="ts">
  import { project, sel, selected, checkpoint, addText, addFreehand, transformObjects, nudge, byId } from '../lib/doc.svelte';
  import { frameCorners, localGeom, pathData, paperStrokes, type Obj } from '../lib/render.svelte';
  import { pipe } from '../lib/pipeline.svelte';
  import { cal } from '../lib/state.svelte';
  import { applyPt } from '../lib/geometry';

  export type Tool = 'select' | 'text' | 'pen' | 'hand';
  let { tool = $bindable<Tool>('select'), showPath = false, scrub = 1, ontextcreated }: { tool?: Tool; showPath?: boolean; scrub?: number; ontextcreated?: () => void } = $props();

  const RULER = 22;
  let el: SVGSVGElement | undefined = $state();
  let W = $state(800), H = $state(600);
  let view = $state({ cx: 105, cy: 148, k: 2.2 }); // k = px per mm
  let fitted = false;

  const toMM = (px: number, py: number): [number, number] => [(px - RULER - (W - RULER) / 2) / view.k + view.cx, (py - RULER - (H - RULER) / 2) / view.k + view.cy];
  const toPX = (x: number, y: number): [number, number] => [(x - view.cx) * view.k + RULER + (W - RULER) / 2, (y - view.cy) * view.k + RULER + (H - RULER) / 2];
  function local(e: { clientX: number; clientY: number }): [number, number] { const r = el!.getBoundingClientRect(); return [e.clientX - r.left, e.clientY - r.top]; }

  export function fit() {
    const pad = 36;
    const k = Math.min((W - RULER - pad * 2) / project.paper.w, (H - RULER - pad * 2) / project.paper.h);
    view = { cx: project.paper.w / 2, cy: project.paper.h / 2, k: Math.max(0.3, k) };
  }
  export function zoomBy(f: number) { view.k = Math.min(60, Math.max(0.3, view.k * f)); }
  $effect(() => { W; H; if (!fitted && W > 100) { fit(); fitted = true; } });
  $effect(() => { project.paper.w; project.paper.h; fit(); });

  // rulers & grid
  const niceStep = $derived.by(() => { for (const s of [0.5, 1, 2, 5, 10, 20, 50, 100]) if (s * view.k >= 7) return s; return 100; });
  const majorEvery = $derived(niceStep >= 10 ? 5 : niceStep === 1 || niceStep === 0.5 ? 10 : 5);
  const ticksX = $derived.by(() => {
    const [a] = toMM(RULER, 0), [b] = toMM(W, 0), out: { v: number; major: boolean; px: number }[] = [];
    for (let v = Math.floor(a / niceStep) * niceStep; v <= b; v += niceStep) out.push({ v, major: Math.round(v / niceStep) % majorEvery === 0, px: toPX(v, 0)[0] });
    return out;
  });
  const ticksY = $derived.by(() => {
    const [, a] = toMM(0, RULER), [, b] = toMM(0, H), out: { v: number; major: boolean; px: number }[] = [];
    for (let v = Math.floor(a / niceStep) * niceStep; v <= b; v += niceStep) out.push({ v, major: Math.round(v / niceStep) % majorEvery === 0, px: toPX(0, v)[1] });
    return out;
  });
  const lbl = (v: number) => (Math.abs(v - Math.round(v)) < 1e-6 ? String(Math.round(v)) : v.toFixed(1));

  // selection frame
  const selObjs = $derived(project.objects.filter((o) => sel.ids.includes(o.id)));
  const frame = $derived.by(() => {
    if (!selObjs.length) return null;
    if (selObjs.length === 1) { const c = frameCorners(selObjs[0], project.assets); return { c, rot: selObjs[0].rotation }; }
    const pts = selObjs.flatMap((o) => frameCorners(o, project.assets));
    const xs = pts.map((p) => p[0]), ys = pts.map((p) => p[1]);
    const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
    return { c: [[x0, y0], [x1, y0], [x1, y1], [x0, y1]] as [number, number][], rot: 0 };
  });
  const frameCenter = $derived(frame ? [(frame.c[0][0] + frame.c[2][0]) / 2, (frame.c[0][1] + frame.c[2][1]) / 2] as [number, number] : [0, 0] as [number, number]);
  const rotHandle = $derived.by(() => {
    if (!frame) return null;
    const mid: [number, number] = [(frame.c[0][0] + frame.c[1][0]) / 2, (frame.c[0][1] + frame.c[1][1]) / 2];
    const dx = mid[0] - frameCenter[0], dy = mid[1] - frameCenter[1], len = Math.hypot(dx, dy) || 1;
    const off = 22 / view.k;
    return { mid, pos: [mid[0] + (dx / len) * off, mid[1] + (dy / len) * off] as [number, number] };
  });

  // interaction
  type Drag =
    | { kind: 'pan'; sx: number; sy: number; cx: number; cy: number }
    | { kind: 'move'; start: Map<string, Obj>; p0: [number, number]; moved: boolean }
    | { kind: 'scale'; start: Map<string, Obj>; anchor: [number, number]; d0: number }
    | { kind: 'rotate'; start: Map<string, Obj>; c: [number, number]; a0: number }
    | { kind: 'marquee'; p0: [number, number]; p1: [number, number]; add: boolean }
    | { kind: 'pen'; pts: [number, number][] };
  let drag = $state<Drag | null>(null);
  let spaceDown = false;
  let n = 0;

  const snapshot = (): Map<string, Obj> => new Map(selected().map((o) => [o.id, JSON.parse(JSON.stringify(o)) as Obj]));

  function down(e: PointerEvent) {
    if (!el) return;
    el.setPointerCapture(e.pointerId);
    const [px, py] = local(e);
    if (px < RULER || py < RULER) return;
    const p = toMM(px, py);
    if (e.button === 1 || tool === 'hand' || spaceDown) { drag = { kind: 'pan', sx: px, sy: py, cx: view.cx, cy: view.cy }; return; }
    if (tool === 'pen') { drag = { kind: 'pen', pts: [p] }; return; }
    if (tool === 'text') {
      const t = addText('Text', p);
      tool = 'select'; ontextcreated?.(); void t; return;
    }
    const t = e.target as Element;
    const handle = t.closest('[data-handle]')?.getAttribute('data-handle');
    if (handle && frame) {
      checkpoint('xf' + ++n);
      const start = snapshot();
      if (handle === 'rot') drag = { kind: 'rotate', start, c: frameCenter, a0: Math.atan2(p[1] - frameCenter[1], p[0] - frameCenter[0]) };
      else {
        const idx = ['nw', 'ne', 'se', 'sw'].indexOf(handle);
        const anchor = frame.c[(idx + 2) % 4];
        drag = { kind: 'scale', start, anchor, d0: Math.hypot(frame.c[idx][0] - anchor[0], frame.c[idx][1] - anchor[1]) };
      }
      return;
    }
    const id = t.closest('[data-id]')?.getAttribute('data-id');
    if (id) {
      if (e.shiftKey) sel.ids = sel.ids.includes(id) ? sel.ids.filter((i) => i !== id) : [...sel.ids, id];
      else if (!sel.ids.includes(id)) sel.ids = [id];
      if (sel.ids.includes(id)) { checkpoint('mv' + ++n); drag = { kind: 'move', start: snapshot(), p0: p, moved: false }; }
      return;
    }
    if (!e.shiftKey) sel.ids = [];
    drag = { kind: 'marquee', p0: p, p1: p, add: e.shiftKey };
  }

  function move(e: PointerEvent) {
    if (!drag || !el) return;
    const [px, py] = local(e);
    const p = toMM(px, py);
    switch (drag.kind) {
      case 'pan': view.cx = drag.cx - (px - drag.sx) / view.k; view.cy = drag.cy - (py - drag.sy) / view.k; break;
      case 'pen': drag.pts.push(p); break;
      case 'marquee': drag.p1 = p; break;
      case 'move': {
        let dx = p[0] - drag.p0[0], dy = p[1] - drag.p0[1];
        if (e.shiftKey) { if (Math.abs(dx) > Math.abs(dy)) dy = 0; else dx = 0; }
        drag.moved = true;
        for (const o of selected()) { const s = drag.start.get(o.id)!; o.x = s.x + dx; o.y = s.y + dy; }
        break;
      }
      case 'scale': {
        let s = Math.hypot(p[0] - drag.anchor[0], p[1] - drag.anchor[1]) / drag.d0;
        s = Math.min(40, Math.max(0.05, s));
        transformObjects(selected(), drag.start, s, 0, drag.anchor);
        break;
      }
      case 'rotate': {
        let d = (Math.atan2(p[1] - drag.c[1], p[0] - drag.c[0]) - drag.a0) * 180 / Math.PI;
        if (e.shiftKey) d = Math.round(d / 15) * 15;
        transformObjects(selected(), drag.start, 1, d, drag.c);
        break;
      }
    }
  }

  function up() {
    if (!drag) return;
    if (drag.kind === 'pen') addFreehand(drag.pts);
    if (drag.kind === 'marquee') {
      const [a, b] = [drag.p0, drag.p1];
      const r = { x0: Math.min(a[0], b[0]), x1: Math.max(a[0], b[0]), y0: Math.min(a[1], b[1]), y1: Math.max(a[1], b[1]) };
      if (r.x1 - r.x0 > 0.5 || r.y1 - r.y0 > 0.5) {
        const hit = project.objects.filter((o) => { const c = frameCorners(o, project.assets); return c.every((q) => q[0] >= r.x0 && q[0] <= r.x1 && q[1] >= r.y0 && q[1] <= r.y1) || c.some((q) => q[0] >= r.x0 && q[0] <= r.x1 && q[1] >= r.y0 && q[1] <= r.y1); }).map((o) => o.id);
        sel.ids = drag.add ? [...new Set([...sel.ids, ...hit])] : hit;
      }
    }
    drag = null;
  }

  function wheel(e: WheelEvent) {
    e.preventDefault();
    const [px, py] = local(e);
    if (e.ctrlKey || e.metaKey) {
      const before = toMM(px, py);
      view.k = Math.min(60, Math.max(0.3, view.k * Math.exp(-e.deltaY * 0.0022)));
      const after = toMM(px, py);
      view.cx += before[0] - after[0]; view.cy += before[1] - after[1];
    } else if (e.shiftKey) view.cx += (e.deltaY + e.deltaX) / view.k;
    else { view.cx += e.deltaX / view.k; view.cy += e.deltaY / view.k; }
  }

  function keydown(e: KeyboardEvent) { if (e.code === 'Space' && !isTyping(e)) { spaceDown = true; if (el) el.style.cursor = 'grab'; } }
  function keyup(e: KeyboardEvent) { if (e.code === 'Space') { spaceDown = false; if (el) el.style.cursor = ''; } }
  const isTyping = (e: KeyboardEvent) => ['INPUT', 'TEXTAREA', 'SELECT'].includes((e.target as HTMLElement)?.tagName);

  // machine path overlay with scrub
  const pathView = $derived.by(() => {
    const plan = pipe.plan?.plan.strokes;
    if (!showPath || !plan?.length) return null;
    type Seg = { len: number; pts: [number, number][]; down: boolean };
    const segs: Seg[] = [];
    let cur: [number, number] = [0, 0];
    for (const s of plan) {
      const tl = Math.hypot(s[0][0] - cur[0], s[0][1] - cur[1]);
      if (tl > 1e-6) segs.push({ len: tl, pts: [cur, s[0]], down: false });
      let l = 0; for (let i = 1; i < s.length; i++) l += Math.hypot(s[i][0] - s[i - 1][0], s[i][1] - s[i - 1][1]);
      segs.push({ len: l, pts: s, down: true });
      cur = s[s.length - 1];
    }
    const total = segs.reduce((a, s) => a + s.len, 0);
    let left = total * scrub, down = '', up = '', head: [number, number] | null = null;
    const part = (pts: [number, number][], rem: number) => {
      const out: [number, number][] = [pts[0]];
      for (let i = 1; i < pts.length; i++) {
        const d = Math.hypot(pts[i][0] - pts[i - 1][0], pts[i][1] - pts[i - 1][1]);
        if (d >= rem) { const t = d ? rem / d : 0; out.push([pts[i - 1][0] + (pts[i][0] - pts[i - 1][0]) * t, pts[i - 1][1] + (pts[i][1] - pts[i - 1][1]) * t]); break; }
        out.push(pts[i]); rem -= d;
      }
      return out;
    };
    for (const sg of segs) {
      if (left <= 0) break;
      const pts = left >= sg.len ? sg.pts : part(sg.pts, left);
      (sg.down ? (down += pathData([pts])) : (up += pathData([pts])));
      head = pts[pts.length - 1];
      left -= sg.len;
    }
    const all = pathData(plan);
    let allTravel = '';
    cur = [0, 0];
    for (const s of plan) { allTravel += `M${cur[0].toFixed(1)} ${cur[1].toFixed(1)}L${s[0][0].toFixed(1)} ${s[0][1].toFixed(1)}`; cur = s[s.length - 1]; }
    const starts = plan.map((s) => s[0]);
    return { down, up, head, all, allTravel, starts };
  });

  const cursor = $derived(tool === 'pen' ? 'crosshair' : tool === 'text' ? 'text' : tool === 'hand' ? 'grab' : 'default');
  const pen = $derived(drag?.kind === 'pen' ? pathData([drag.pts]) : '');
  const marq = $derived(drag?.kind === 'marquee' ? drag : null);
  const calPaper = $derived(cal.status?.resolved ? cal.status.resolved.paper : null);
  void byId; void nudge; void applyPt; void localGeom; void paperStrokes;
</script>

<svelte:window onkeydown={keydown} onkeyup={keyup} onpointerup={up} />

<div class="wrap" bind:clientWidth={W} bind:clientHeight={H}>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <svg bind:this={el} width={W} height={H} style="cursor:{cursor}" onpointerdown={down} onpointermove={move} onwheel={wheel} role="application" aria-label="Paper canvas">
    <rect width={W} height={H} class="bg" />
    <g transform="translate({toPX(0, 0)[0]} {toPX(0, 0)[1]}) scale({view.k})">
      <rect x="1.5" y="2" width={project.paper.w} height={project.paper.h} class="shadow" />
      <rect width={project.paper.w} height={project.paper.h} class="sheet" />
      {#each ticksX.filter((t) => t.v >= 0 && t.v <= project.paper.w) as t}<line x1={t.v} y1="0" x2={t.v} y2={project.paper.h} class={t.major ? 'gM' : 'gm'} vector-effect="non-scaling-stroke" />{/each}
      {#each ticksY.filter((t) => t.v >= 0 && t.v <= project.paper.h) as t}<line x1="0" y1={t.v} x2={project.paper.w} y2={t.v} class={t.major ? 'gM' : 'gm'} vector-effect="non-scaling-stroke" />{/each}
      <rect x={project.paper.margin} y={project.paper.margin} width={project.paper.w - 2 * project.paper.margin} height={project.paper.h - 2 * project.paper.margin} class="margin" vector-effect="non-scaling-stroke" />
      {#if calPaper}<rect width={calPaper[0]} height={calPaper[1]} class="calpaper" vector-effect="non-scaling-stroke" />{/if}

      {#each project.objects as o (o.id)}
        {#if !o.hidden}
          {@const l = localGeom(o, project.assets)}
          <g transform="translate({o.x} {o.y}) rotate({o.rotation} {l.w / 2} {l.h / 2})" opacity={showPath ? 0.22 : 1}>
            <path d={pathData(l.strokes)} class="ink" style="stroke-width:{project.settings.strokeWidthMm}" />
            <rect data-id={o.id} width={l.w} height={l.h} class="hit" class:on={sel.ids.includes(o.id)} />
          </g>
        {/if}
      {/each}

      {#if pathView}
        <path d={pathView.allTravel} class="trAll" vector-effect="non-scaling-stroke" />
        <path d={pathView.all} class="dnAll" style="stroke-width:{project.settings.strokeWidthMm}" />
        <path d={pathView.up} class="trDone" vector-effect="non-scaling-stroke" />
        <path d={pathView.down} class="dnDone" style="stroke-width:{project.settings.strokeWidthMm * 1.15}" />
        {#each pathView.starts as s}<circle cx={s[0]} cy={s[1]} r={2 / view.k * 1.2} class="st" />{/each}
        {#if pathView.head}<circle cx={pathView.head[0]} cy={pathView.head[1]} r={5 / view.k} class="head" /><circle cx={pathView.head[0]} cy={pathView.head[1]} r={1.6 / view.k} class="headc" />{/if}
      {/if}

      {#if pen}<path d={pen} class="penline" style="stroke-width:{project.settings.strokeWidthMm}" />{/if}
      {#if marq}<rect x={Math.min(marq.p0[0], marq.p1[0])} y={Math.min(marq.p0[1], marq.p1[1])} width={Math.abs(marq.p1[0] - marq.p0[0])} height={Math.abs(marq.p1[1] - marq.p0[1])} class="marq" vector-effect="non-scaling-stroke" />{/if}

      {#if frame && tool === 'select'}
        <polygon points={frame.c.map((p) => p.join(',')).join(' ')} class="frame" vector-effect="non-scaling-stroke" />
        {#if rotHandle}<line x1={rotHandle.mid[0]} y1={rotHandle.mid[1]} x2={rotHandle.pos[0]} y2={rotHandle.pos[1]} class="frame" vector-effect="non-scaling-stroke" />
          <circle data-handle="rot" cx={rotHandle.pos[0]} cy={rotHandle.pos[1]} r={6 / view.k} class="hnd rot" />{/if}
        {#each frame.c as c, i}<rect data-handle={['nw', 'ne', 'se', 'sw'][i]} x={c[0] - 4.5 / view.k} y={c[1] - 4.5 / view.k} width={9 / view.k} height={9 / view.k} class="hnd" />{/each}
      {/if}
    </g>

    <!-- rulers -->
    <rect x="0" y="0" width={W} height={RULER} class="rul" /><rect x="0" y="0" width={RULER} height={H} class="rul" />
    {#each ticksX as t}
      {#if t.px > RULER}<line x1={t.px} y1={t.major ? 6 : 14} x2={t.px} y2={RULER} class="rt" />{#if t.major}<text x={t.px + 3} y="11" class="rl">{lbl(t.v)}</text>{/if}{/if}
    {/each}
    {#each ticksY as t}
      {#if t.px > RULER}<line x1={t.major ? 6 : 14} y1={t.px} x2={RULER} y2={t.px} class="rt" />{#if t.major}<text transform="translate(10 {t.px - 3}) rotate(-90)" class="rl">{lbl(t.v)}</text>{/if}{/if}
    {/each}
    <rect x="0" y="0" width={RULER} height={RULER} class="corner" /><text x="4" y="14" class="rl">mm</text>
  </svg>
  <div class="zoombar">
    <button class="btn sm" onclick={() => zoomBy(1 / 1.25)}>−</button>
    <span class="mono">{Math.round(view.k / 3.78 * 100)}%</span>
    <button class="btn sm" onclick={() => zoomBy(1.25)}>+</button>
    <button class="btn sm" onclick={fit}>Fit</button>
  </div>
</div>

<style>
  .wrap { position: relative; width: 100%; height: 100%; overflow: hidden; background: #d8d4c6; }
  svg { display: block; touch-action: none; user-select: none; }
  .bg { fill: #d8d4c6; }
  .shadow { fill: rgba(0,0,0,.22); filter: blur(2px); }
  .sheet { fill: #fbfaf6; }
  .gm { stroke: #ebe7da; stroke-width: 1; } .gM { stroke: #ddd8c8; stroke-width: 1; }
  .margin { fill: none; stroke: #b9b4a2; stroke-width: 1; stroke-dasharray: 4 3; }
  .calpaper { fill: none; stroke: var(--ok); stroke-width: 1.2; stroke-dasharray: 2 4; }
  .ink { fill: none; stroke: #1b1b19; stroke-linecap: round; stroke-linejoin: round; }
  .hit { fill: transparent; stroke: none; pointer-events: all; }
  .hit:hover { fill: rgba(27,27,25,.04); }
  .hit.on { fill: rgba(27,27,25,.05); }
  .frame { fill: none; stroke: #1b1b19; stroke-width: 1; stroke-dasharray: 5 3; }
  .hnd { fill: #fff; stroke: #1b1b19; stroke-width: 1.2; vector-effect: non-scaling-stroke; cursor: nwse-resize; }
  .hnd.rot { cursor: grab; fill: #1b1b19; }
  .marq { fill: rgba(27,27,25,.06); stroke: #1b1b19; stroke-dasharray: 3 3; }
  .penline { fill: none; stroke: #1b1b19; stroke-linecap: round; stroke-linejoin: round; }
  .trAll { fill: none; stroke: var(--danger); stroke-width: .8; stroke-dasharray: 3 3; opacity: .28; }
  .dnAll { fill: none; stroke: #8a877b; stroke-linecap: round; stroke-linejoin: round; opacity: .55; }
  .trDone { fill: none; stroke: var(--danger); stroke-width: 1.1; stroke-dasharray: 3 3; }
  .dnDone { fill: none; stroke: #111; stroke-linecap: round; stroke-linejoin: round; }
  .st { fill: #fff; stroke: var(--ok); stroke-width: .3; }
  .head { fill: none; stroke: var(--danger); stroke-width: .35; } .headc { fill: var(--danger); }
  .rul { fill: var(--paper); } .corner { fill: var(--paper-2); stroke: var(--line-strong); stroke-width: .5; }
  .rt { stroke: #6f6c61; stroke-width: 1; } .rl { font: 9px var(--mono); fill: #4b4a45; }
  .zoombar { position: absolute; right: 12px; bottom: 12px; display: flex; gap: 6px; align-items: center; background: var(--paper); border: 1px solid var(--line-strong); padding: 4px 6px; }
</style>
