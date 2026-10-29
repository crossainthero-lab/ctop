// Typed text -> wrapped lines -> baselines on the page -> single-stroke glyphs.
// Everything is in PAPER space: mm, origin top-left, +y down. These exact strokes are both drawn as the preview and
// sent to the plotter; nothing is re-laid-out afterwards.
import type { Stroke } from '../types';
import { DEFAULT_STROKE_FONT } from '../text/strokefont';
import { fontMetricsFor } from '../text/fontregistry';
import { wrapParagraph, LINE_PITCH, type Align } from '../text/layout';

export interface Margins { left: number; right: number; top: number; bottom: number }

export interface PageOpts {
  widthMm: number;
  heightMm: number;
  fontId: string;
  /** cap height in mm; on ruled paper it is limited to RULED_MAX_FILL of the rule spacing */
  sizeMm: number;
  /** blank paper: multiplier on the normal line pitch. ruled paper: write on every n-th rule (rounded, min 1) */
  lineSpacing: number;
  align: Align;
  margins: Margins;
  /** ruled baselines (mm from the paper top, ascending); null/empty = blank paper */
  rules?: number[] | null;
  letterSpacingMm?: number;
  /** outline (filled) fonts: the pen traces only the glyph contour */
  outlineMode?: 'contour';
}

export interface PlacedLine { text: string; baselineMm: number; xMm: number; widthMm: number }

export interface PageLayout {
  strokes: Stroke[];
  lines: PlacedLine[];
  mode: 'ruled' | 'blank';
  sizeMm: number;
  sizeLimited: boolean;
  /** wrapped lines that found no baseline on the page (not drawn) */
  overflowLines: number;
  /** wrapped lines in total */
  totalLines: number;
  /** characters the font cannot draw (skipped) */
  missing: string[];
  /** text that is actually drawn fits entirely */
  fits: boolean;
  /** the requested (system) font was not loaded; the default stroke font was used instead */
  fontMissing?: boolean;
  /** every wrapped line (drawn or not) and how many of them were placed on this page, for continuing on the next page */
  wrapped: string[];
  consumed: number;
}

export const RULED_MAX_FILL = 0.8;
/** Sit the letters this far above the centre of the rule so the pen line does not merge into the printed one. */
export const RULE_LIFT_MM = 0.4;
export const DEFAULT_MARGINS: Margins = { left: 20, right: 15, top: 20, bottom: 15 };

export function layoutPage(text: string, o: PageOpts): PageLayout {
  const ruled = !!(o.rules && o.rules.length >= 2);
  let size = Math.max(1, o.sizeMm);
  let sizeLimited = false;
  if (ruled) {
    const spacing = median(diffs(o.rules!));
    const cap = spacing * RULED_MAX_FILL;
    if (size > cap) { size = cap; sizeLimited = true; }
  }
  let fm = fontMetricsFor(o.fontId, size);
  let fontMissing = false;
  if (!fm) { fontMissing = true; fm = fontMetricsFor(DEFAULT_STROKE_FONT, size)!; }
  const ls = o.letterSpacingMm ?? 0;
  const missing = new Set<string>();
  const adv = (ch: string) => {
    const a = fm.advance(ch);
    if (a === null) { missing.add(ch); return 0; }
    return a + ls;
  };
  const width = (s: string) => {
    let w = 0, n = 0;
    for (const ch of s) { w += adv(ch); n++; }
    return n ? w - ls : 0;
  };
  const boxX = o.margins.left;
  const boxW = Math.max(1, o.widthMm - o.margins.left - o.margins.right);
  const wrapped = text.replace(/\r\n?/g, '\n').replace(/\t/g, '    ').split('\n').flatMap((p) => wrapParagraph(p, width, boxW));
  // trailing blank lines carry no ink and need no baseline
  while (wrapped.length && wrapped[wrapped.length - 1].trim() === '') wrapped.pop();

  const baselines: number[] = [];
  // ink must stay inside the margins: ascenders poke ~10% above cap height, descenders ~35% below the baseline
  const minBase = o.margins.top + size * 1.1;
  const maxBase = o.heightMm - o.margins.bottom - size * 0.35;
  if (ruled) {
    const step = Math.max(1, Math.round(o.lineSpacing));
    const ok = o.rules!.filter((r) => r - RULE_LIFT_MM >= minBase && r - RULE_LIFT_MM <= maxBase);
    for (let i = 0; i < ok.length; i += step) baselines.push(ok[i] - RULE_LIFT_MM);
  } else {
    const pitch = size * LINE_PITCH * Math.max(0.5, o.lineSpacing);
    for (let b = minBase; b <= maxBase + 1e-9; b += pitch) baselines.push(b);
  }

  const strokes: Stroke[] = [];
  const lines: PlacedLine[] = [];
  const n = Math.min(wrapped.length, baselines.length);
  for (let i = 0; i < n; i++) {
    const line = wrapped[i];
    const w = width(line);
    let x = boxX + (o.align === 'center' ? (boxW - w) / 2 : o.align === 'right' ? boxW - w : 0);
    const x0 = x;
    for (const ch of line) {
      if (fm.advance(ch) !== null && ch.trim() !== '') strokes.push(...fm.glyph(ch, x, baselines[i]));
      x += adv(ch);
    }
    lines.push({ text: line, baselineMm: baselines[i], xMm: x0, widthMm: w });
  }
  const overflowLines = Math.max(0, wrapped.length - baselines.length);
  return {
    strokes, lines, mode: ruled ? 'ruled' : 'blank', sizeMm: size, sizeLimited, overflowLines,
    wrapped, consumed: n, totalLines: wrapped.length, missing: [...missing].filter((c) => c.trim() !== ''), fits: overflowLines === 0,
    ...(fontMissing ? { fontMissing: true } : {}),
  };
}

function diffs(a: number[]): number[] { return a.slice(1).map((v, i) => v - a[i]); }
function median(a: number[]): number {
  const s = [...a].sort((x, y) => x - y);
  return s.length ? s[Math.floor(s.length / 2)] : 8;
}

/** Cheap stable key of what would be written (for duplicate-job protection). */
export function strokesKey(strokes: Stroke[]): string {
  let h = 2166136261;
  for (const s of strokes) for (const p of s) {
    h = Math.imul(h ^ Math.round(p[0] * 100), 16777619);
    h = Math.imul(h ^ Math.round(p[1] * 100), 16777619);
  }
  return `${strokes.length}:${(h >>> 0).toString(16)}`;
}
