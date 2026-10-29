// Turns what the user typed/uploaded into pen strokes for the fixed A4 frame. One function per mode; each returns pages
// whose strokes are in PAPER space of the physical sheet (mm, origin top-left, +y down), exactly what the preview
// draws and exactly what is sent to the plotter. Nothing is re-laid-out afterwards.
import type { Pt, Stroke } from '../types';
import { layoutPage, RULE_LIFT_MM, RULED_MAX_FILL, type Margins } from '../write/pagelayout';
import { orderStrokes, imageToStrokes, jobStats, isSvgSource, type ImageSource, type ImageStyle } from '../write/imagejob';
import { importSvg } from '../svg/import';
import { strokeLength } from '../geometry';
import type { Align } from '../text/layout';
import { planStats, type PlanStats } from './estimate';

export type Mode = 'text' | 'image' | 'lined';
export type Orient = 'portrait' | 'landscape';
export interface Stage { label: string; count: number }
/** drawing-ordered groups of strokes; `kind` colours the preview */
export interface Layer { label: string; kind: 'ruling' | 'ink'; strokes: Stroke[] }
export interface PagePlan {
  layers: Layer[];
  strokes: Stroke[];
  stages: Stage[];
  stats: PlanStats;
  /** reading-orientation page size (differs from the sheet for landscape) */
  readW: number;
  readH: number;
}
export interface JobPlan {
  pages: PagePlan[];
  notes: string[];
  /** text did not fit even after using up to MAX_PAGES pages */
  overflow: boolean;
  missing: string[];
  /** lined paper only: the ruling and, per page, the baseline of every written line (reading space) */
  ruling?: Ruling;
  baselines?: number[][];
}
export const MAX_PAGES = 30;

export interface Sheet { w: number; h: number; marginMm: number }

/** reading size of a page for an orientation */
export const readSize = (sheet: Sheet, o: Orient) => (o === 'portrait' ? { w: sheet.w, h: sheet.h } : { w: sheet.h, h: sheet.w });

/**
 * Reading space (what you see when you read the page) -> physical sheet. The sheet always stays portrait in the frame, so a
 * landscape page is turned a quarter turn clockwise: the left edge of the page lands on the top edge of the sheet.
 */
export function toSheet(strokes: Stroke[], sheet: Sheet, o: Orient): Stroke[] {
  if (o === 'portrait') return strokes;
  return strokes.map((s) => s.map(([u, v]): Pt => [sheet.w - v, u]));
}
export function fromSheetPt(q: Pt, sheet: Sheet, o: Orient): Pt {
  return o === 'portrait' ? q : [q[1], sheet.w - q[0]];
}

function finish(layers: Layer[], sheet: Sheet, o: Orient): PagePlan {
  const rs = readSize(sheet, o);
  const phys = layers.map((l) => ({ ...l, strokes: toSheet(l.strokes.filter((s) => s.length >= 2 && strokeLength(s) > 1e-6), sheet, o) })).filter((l) => l.strokes.length);
  const strokes = phys.flatMap((l) => l.strokes);
  return { layers: phys, strokes, stages: phys.map((l) => ({ label: l.label, count: l.strokes.length })), stats: planStats(strokes), readW: rs.w, readH: rs.h };
}

// ───────────────────────────── Mode A: text ─────────────────────────────

export interface TextOpts {
  fontId: string;
  sizeMm: number;
  lineSpacing: number;
  align: Align;
  margins: Margins;
  orient: Orient;
}

/** Keeps placing wrapped lines on successive pages until the text is used up. */
function paginate(text: string, place: (rest: string) => ReturnType<typeof layoutPage>): { pages: ReturnType<typeof layoutPage>[]; overflow: boolean; missing: string[] } {
  const pages: ReturnType<typeof layoutPage>[] = [];
  let rest = text;
  let overflow = false;
  const missing = new Set<string>();
  for (let i = 0; i < MAX_PAGES; i++) {
    const l = place(rest);
    l.missing.forEach((m) => missing.add(m));
    if (l.consumed === 0) { overflow = l.wrapped.length > 0; break; }
    pages.push(l);
    if (l.wrapped.length <= l.consumed) return { pages, overflow: false, missing: [...missing] };
    rest = l.wrapped.slice(l.consumed).join('\n').replace(/^\n+/, '');
    if (!rest.trim()) return { pages, overflow: false, missing: [...missing] };
  }
  return { pages, overflow: true, missing: [...missing] };
}

export function textJob(text: string, sheet: Sheet, o: TextOpts): JobPlan {
  const rs = readSize(sheet, o.orient);
  const notes: string[] = [];
  if (!text.trim()) return { pages: [], notes, overflow: false, missing: [] };
  const r = paginate(text, (rest) => layoutPage(rest, { widthMm: rs.w, heightMm: rs.h, fontId: o.fontId, sizeMm: o.sizeMm, lineSpacing: o.lineSpacing, align: o.align, margins: o.margins }));
  if (r.overflow) notes.push(`The text needs more than ${MAX_PAGES} pages; only the first ${MAX_PAGES} are planned.`);
  const pages = r.pages.map((l) => finish([{ label: 'Writing text', kind: 'ink', strokes: l.strokes }], sheet, o.orient));
  if (pages.length > 1) notes.push(`This takes ${pages.length} sheets. Draw them one at a time, with a fresh sheet in the frame each time.`);
  return { pages, notes, overflow: r.overflow, missing: r.missing };
}

// ───────────────────────────── Mode C: lined paper ─────────────────────────────

export type LineStyle = 'solid' | 'dashed' | 'hidden';
export interface LinedOpts {
  fontId: string;
  sizeMm: number;
  align: Align;
  orient: Orient;
  /** distance between writing lines */
  spacingMm: number;
  /** ruled area: distance from each sheet edge to the border */
  margins: Margins;
  border: boolean;
  marginLine: boolean;
  /** vertical margin line, mm right of the left border */
  marginLineOffsetMm: number;
  lineStyle: LineStyle;
  /** 0 = as many lines as fit */
  lineCount: number;
  /** write on every n-th line (1 = every line) */
  writeEvery: number;
}

export interface Ruling {
  /** y (reading space) of every writing line, ascending */
  ys: number[];
  x0: number;
  x1: number;
  top: number;
  bottom: number;
  /** x of the vertical margin line, or null */
  marginX: number | null;
  /** strokes in drawing order: border, margin line, then writing lines (serpentine to save pen-up travel) */
  strokes: Stroke[];
}

const DASH = 6, GAP = 3.5;
function dashed(a: Pt, b: Pt): Stroke[] {
  const len = Math.hypot(b[0] - a[0], b[1] - a[1]);
  const out: Stroke[] = [];
  const ux = (b[0] - a[0]) / len, uy = (b[1] - a[1]) / len;
  for (let d = 0; d < len - 0.5; d += DASH + GAP) {
    const e = Math.min(len, d + DASH);
    out.push([[a[0] + ux * d, a[1] + uy * d], [a[0] + ux * e, a[1] + uy * e]]);
  }
  return out;
}

/** The ruled page in reading space. Every y here is also the baseline reference for the text. */
export function buildRuling(rw: number, rh: number, o: Pick<LinedOpts, 'spacingMm' | 'margins' | 'border' | 'marginLine' | 'marginLineOffsetMm' | 'lineStyle' | 'lineCount'>): Ruling {
  const m = o.margins;
  const x0 = m.left, x1 = rw - m.right, top = m.top, bottom = rh - m.bottom;
  if (!(x1 - x0 > 20) || !(bottom - top > o.spacingMm * 2)) throw new Error('The margins leave no room for lines.');
  const ys: number[] = [];
  for (let k = 1; ; k++) {
    const y = top + k * o.spacingMm;
    if (y > bottom + 1e-6) break;
    ys.push(Math.round(y * 1000) / 1000);
    if (o.lineCount > 0 && ys.length >= o.lineCount) break;
  }
  const strokes: Stroke[] = [];
  const marginX = o.marginLine ? x0 + Math.max(5, o.marginLineOffsetMm) : null;
  if (o.lineStyle !== 'hidden') {
    const bottomEdge = ys.length ? ys[ys.length - 1] : top;
    const reachDown = o.border ? bottom : bottomEdge;
    if (o.border) strokes.push([[x0, top], [x1, top], [x1, bottom], [x0, bottom], [x0, top]]);
    if (marginX !== null) strokes.push([[marginX, top], [marginX, reachDown]]);
    ys.forEach((y, i) => {
      if (o.border && (Math.abs(y - top) < 0.3 || Math.abs(y - bottom) < 0.3)) return; // already part of the border
      const [a, b]: [Pt, Pt] = i % 2 === 0 ? [[x0, y], [x1, y]] : [[x1, y], [x0, y]]; // serpentine
      if (o.lineStyle === 'dashed') strokes.push(...dashed(a, b)); else strokes.push([a, b]);
    });
  }
  return { ys, x0, x1, top, bottom, marginX, strokes };
}

export const TEXT_PAD_MM = 3;

export function linedJob(text: string, sheet: Sheet, o: LinedOpts): JobPlan {
  const rs = readSize(sheet, o.orient);
  const ruling = buildRuling(rs.w, rs.h, o);
  const notes: string[] = [];
  const textLeft = (ruling.marginX ?? ruling.x0) + TEXT_PAD_MM;
  const textRightMargin = rs.w - ruling.x1 + TEXT_PAD_MM;
  const safe = sheet.marginMm;
  // the line positions come from the ruling; only the physical safe area limits which lines may carry text
  const place = (rest: string) => layoutPage(rest, {
    widthMm: rs.w, heightMm: rs.h, fontId: o.fontId, sizeMm: o.sizeMm, lineSpacing: o.writeEvery, align: o.align,
    margins: { left: textLeft, right: textRightMargin, top: safe, bottom: safe }, rules: ruling.ys,
  });
  const has = text.trim().length > 0;
  const r = has ? paginate(text, place) : { pages: [], overflow: false, missing: [] as string[] };
  if (r.overflow) notes.push(`The text needs more than ${MAX_PAGES} pages; only the first ${MAX_PAGES} are planned.`);
  const label = o.lineStyle === 'hidden' ? null : 'Ruling the page';
  const make = (ink: Stroke[]) => finish([...(label && ruling.strokes.length ? [{ label, kind: 'ruling' as const, strokes: ruling.strokes }] : []), ...(ink.length ? [{ label: 'Writing text', kind: 'ink' as const, strokes: ink }] : [])], sheet, o.orient);
  const pages = r.pages.length ? r.pages.map((l) => make(l.strokes)) : [make([])];
  if (r.pages.length > 1) notes.push(`This takes ${r.pages.length} sheets. Each gets its own ruled page.`);
  if (r.pages.some((l) => l.sizeLimited)) notes.push(`Text size limited to ${(o.spacingMm * RULED_MAX_FILL).toFixed(1)} mm for ${o.spacingMm} mm line spacing.`);
  return { pages: pages.filter((p) => p.strokes.length), notes, overflow: r.overflow, missing: r.missing, ruling, baselines: r.pages.map((l) => l.lines.map((x) => x.baselineMm)) };
}

// ───────────────────────────── Mode B: image ─────────────────────────────

export interface ImageOpts {
  style: ImageStyle;
  detail: number;
  /** 0.1..1 */ scale: number;
  /** degrees, clockwise on the sheet */ rotationDeg: number;
  offsetX: number;
  offsetY: number;
  invert: boolean;
  clean: boolean;
  margins: Margins;
  orient: Orient;
}

/** width / height of the artwork before rotation, without converting it */
export function sourceAspect(src: ImageSource): number {
  if (isSvgSource(src)) { const r = importSvg(src.svgText); return r.widthMm / r.heightMm; }
  return src.width / src.height;
}

/** Largest scale (<= 1) at which a w x h box turned by `deg` still fits in bw x bh. */
export function fitScaleForRotation(aspect: number, deg: number, bw: number, bh: number): number {
  const a = (deg * Math.PI) / 180, c = Math.abs(Math.cos(a)), s = Math.abs(Math.sin(a));
  const base = Math.min(bw, bh * aspect); // unrotated width that fits
  const w = base, h = base / aspect;
  const rw = w * c + h * s, rh = w * s + h * c;
  return Math.min(1, bw / rw, bh / rh);
}

export function imageJob(src: ImageSource, sheet: Sheet, o: ImageOpts): { plan: JobPlan; warnings: string[] } {
  const rs = readSize(sheet, o.orient);
  const m = o.margins;
  const bw = rs.w - m.left - m.right, bh = rs.h - m.top - m.bottom;
  const aspect = sourceAspect(src);
  const k = fitScaleForRotation(aspect, o.rotationDeg, bw, bh);
  const job = imageToStrokes(src, {
    style: o.style, detail: o.detail, scale: Math.min(1, o.scale * k), offset: { x: 0.5, y: 0.5 }, invert: o.invert, cleanBackground: o.clean,
    area: { widthMm: rs.w, heightMm: rs.h, margins: m },
  });
  // rotate about the artwork's centre, then place the turned box inside the free space by the offset sliders
  const cx = job.placement.xMm + job.placement.widthMm / 2, cy = job.placement.yMm + job.placement.heightMm / 2;
  const a = (o.rotationDeg * Math.PI) / 180, ca = Math.cos(a), sa = Math.sin(a);
  let strokes = job.strokes.map((s) => s.map(([x, y]): Pt => [cx + (x - cx) * ca - (y - cy) * sa, cy + (x - cx) * sa + (y - cy) * ca]));
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const s of strokes) for (const [x, y] of s) { minX = Math.min(minX, x); maxX = Math.max(maxX, x); minY = Math.min(minY, y); maxY = Math.max(maxY, y); }
  if (strokes.length && isFinite(minX)) {
    const tx = m.left + (bw - (maxX - minX)) * Math.min(1, Math.max(0, o.offsetX)) - minX;
    const ty = m.top + (bh - (maxY - minY)) * Math.min(1, Math.max(0, o.offsetY)) - minY;
    strokes = orderStrokes(strokes.map((s) => s.map(([x, y]): Pt => [x + tx, y + ty])), [m.left, m.top]);
  }
  const page = finish([{ label: 'Drawing image', kind: 'ink', strokes }], sheet, o.orient);
  const stats = jobStats(strokes, [m.left, m.top]);
  const warnings = [...job.warnings];
  if (stats.estMinutes > 90) warnings.push('This will take a long time. Lower the detail or the size to shorten it.');
  return { plan: { pages: strokes.length ? [page] : [], notes: [], overflow: false, missing: [] }, warnings };
}

/** Cheap stable key of a page's strokes (duplicate-job protection). */
export function pageKey(strokes: Stroke[], extra = ''): string {
  let h = 2166136261;
  for (const s of strokes) for (const p of s) {
    h = Math.imul(h ^ Math.round(p[0] * 100), 16777619);
    h = Math.imul(h ^ Math.round(p[1] * 100), 16777619);
  }
  return `${extra}${strokes.length}:${(h >>> 0).toString(16)}`;
}
