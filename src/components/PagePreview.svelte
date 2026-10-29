<script lang="ts">
  // The page exactly as it will be drawn: the same stroke arrays that are sent to the plotter, on the A4 sheet, with the
  // area the plotter cannot reach hatched and the safe margin outlined.
  import type { PagePlan } from '../lib/plan/jobs';
  import { fromSheetPt, type Orient } from '../lib/plan/jobs';
  import { reachOnPaper, type FrameProfile, type Reach } from '../lib/frame';
  import type { Pt } from '../lib/types';

  let { page, orient, sheet, frame, reach, showTravel = false, progress = null, label = '' }: {
    page: PagePlan | null; orient: Orient; sheet: { w: number; h: number; marginMm: number };
    frame: FrameProfile | null; reach: Reach | null; showTravel?: boolean; progress?: { done: number; total: number } | null; label?: string;
  } = $props();

  let wrap: HTMLDivElement | undefined = $state();
  let cvs: HTMLCanvasElement | undefined = $state();
  let box = $state({ w: 600, h: 700 });
  const rw = $derived(orient === 'portrait' ? sheet.w : sheet.h);
  const rh = $derived(orient === 'portrait' ? sheet.h : sheet.w);
  const k = $derived(Math.max(0.5, Math.min((box.w - 8) / rw, (box.h - 8) / rh))); // px per mm
  const cssW = $derived(Math.round(rw * k)), cssH = $derived(Math.round(rh * k));

  $effect(() => {
    if (!wrap) return;
    const ro = new ResizeObserver(() => { box = { w: wrap!.clientWidth, h: wrap!.clientHeight }; });
    ro.observe(wrap);
    return () => ro.disconnect();
  });

  function hatchPattern(ctx: CanvasRenderingContext2D, color: string): CanvasPattern {
    const c = document.createElement('canvas'); c.width = c.height = 10;
    const g = c.getContext('2d')!; g.strokeStyle = color; g.lineWidth = 1.2;
    g.beginPath(); g.moveTo(0, 10); g.lineTo(10, 0); g.moveTo(-5, 5); g.lineTo(5, -5); g.moveTo(5, 15); g.lineTo(15, 5); g.stroke();
    return ctx.createPattern(c, 'repeat')!;
  }

  $effect(() => {
    if (!cvs) return;
    const dpr = window.devicePixelRatio || 1;
    cvs.width = Math.round(cssW * dpr); cvs.height = Math.round(cssH * dpr);
    const ctx = cvs.getContext('2d')!;
    const css = getComputedStyle(cvs);
    const accent = css.getPropertyValue('--accent').trim() || '#0f766e', danger = css.getPropertyValue('--danger').trim() || '#b42318';
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssW, cssH);
    // paper (always light, like paper)
    ctx.fillStyle = '#fffefb'; ctx.fillRect(0, 0, cssW, cssH);
    const T = (q: Pt): Pt => { const r = fromSheetPt(q, sheet, orient); return [r[0] * k, r[1] * k]; };
    // unreachable part of the sheet
    if (frame && reach) {
      const poly = reachOnPaper(frame, reach).map(T);
      ctx.save();
      ctx.beginPath(); ctx.rect(0, 0, cssW, cssH);
      ctx.moveTo(poly[0][0], poly[0][1]); for (const p of poly.slice(1)) ctx.lineTo(p[0], p[1]); ctx.closePath();
      ctx.clip('evenodd');
      ctx.fillStyle = hatchPattern(ctx, 'rgba(180,35,24,.28)'); ctx.fillRect(0, 0, cssW, cssH);
      ctx.restore();
    }
    // safe margin
    const m = sheet.marginMm, c1 = T([m, m]), c2 = T([sheet.w - m, sheet.h - m]);
    ctx.setLineDash([4, 4]); ctx.strokeStyle = 'rgba(100,100,100,.55)'; ctx.lineWidth = 1;
    ctx.strokeRect(Math.min(c1[0], c2[0]), Math.min(c1[1], c2[1]), Math.abs(c2[0] - c1[0]), Math.abs(c2[1] - c1[1]));
    ctx.setLineDash([]);
    if (!page) return;
    // strokes, in the order they are drawn
    const draw = (s: Pt[]) => { const a = T(s[0]); ctx.moveTo(a[0], a[1]); for (let i = 1; i < s.length; i++) { const b = T(s[i]); ctx.lineTo(b[0], b[1]); } };
    ctx.lineCap = 'round'; ctx.lineJoin = 'round';
    const penW = Math.max(0.9, 0.45 * k);
    let idx = 0;
    const live = progress && progress.total === page.strokes.length ? progress : null;
    for (const layer of page.layers) {
      const base = layer.kind === 'ruling' ? '#5b7a99' : '#1b1c1f';
      const faded = layer.kind === 'ruling' ? 'rgba(91,122,153,.35)' : 'rgba(27,28,31,.28)';
      ctx.lineWidth = penW;
      // drawn so far, and still to come, are two paths so a running job shows progress on the page itself
      const split = live ? Math.max(0, Math.min(layer.strokes.length, live.done - idx)) : layer.strokes.length;
      ctx.beginPath(); for (const s of layer.strokes.slice(0, split)) draw(s); ctx.strokeStyle = live ? accent : base; ctx.stroke();
      if (split < layer.strokes.length) { ctx.beginPath(); for (const s of layer.strokes.slice(split)) draw(s); ctx.strokeStyle = faded; ctx.stroke(); }
      idx += layer.strokes.length;
    }
    if (showTravel) {
      ctx.beginPath(); ctx.setLineDash([3, 3]); ctx.lineWidth = 0.8; ctx.strokeStyle = danger;
      for (let i = 1; i < page.strokes.length; i++) { const a = T(page.strokes[i - 1][page.strokes[i - 1].length - 1]), b = T(page.strokes[i][0]); ctx.moveTo(a[0], a[1]); ctx.lineTo(b[0], b[1]); }
      ctx.stroke(); ctx.setLineDash([]);
    }
  });
</script>

<div class="wrap" bind:this={wrap}>
  <div class="sheet" style="width:{cssW}px;height:{cssH}px">
    <div role="img" aria-label="Preview of the page as the plotter will draw it"><canvas bind:this={cvs} style="width:{cssW}px;height:{cssH}px"></canvas></div>
  </div>
</div>
<p class="cap small mute">{label || `A4 ${orient} · ${sheet.w} × ${sheet.h} mm`}{#if frame && reach} · <span class="hatchkey"></span> out of the plotter's reach{/if} · dashed line = safe margin</p>

<style>
  .wrap { flex: 1; min-height: 280px; display: grid; place-items: center; overflow: hidden; }
  .sheet { background: #fffefb; box-shadow: var(--shadow); border-radius: 2px; }
  canvas { display: block; }
  .cap { text-align: center; margin: 6px 0 0; }
  .hatchkey { display: inline-block; width: 14px; height: 10px; vertical-align: -1px; background: repeating-linear-gradient(135deg, rgba(180,35,24,.6) 0 1px, transparent 1px 4px); border: 1px solid rgba(180,35,24,.4); }
</style>
