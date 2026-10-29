import { describe, expect, it } from 'vitest';
import { buildRuling, fitScaleForRotation, fromSheetPt, imageJob, linedJob, readSize, textJob, toSheet, type LinedOpts, type Sheet } from './jobs';
import { RULE_LIFT_MM } from '../write/pagelayout';
import { planStats } from './estimate';
import type { Stroke } from '../types';

const sheet: Sheet = { w: 210, h: 297, marginMm: 8 };
const margins = { left: 15, right: 12, top: 18, bottom: 14 };
const lined: LinedOpts = { fontId: 'futural', sizeMm: 4.5, align: 'left', orient: 'portrait', spacingMm: 8, margins, border: true, marginLine: true, marginLineOffsetMm: 22, lineStyle: 'solid', lineCount: 0, writeEvery: 1 };

const bounds = (ss: Stroke[]) => {
  let a = [Infinity, Infinity, -Infinity, -Infinity];
  for (const s of ss) for (const [x, y] of s) a = [Math.min(a[0], x), Math.min(a[1], y), Math.max(a[2], x), Math.max(a[3], y)];
  return a;
};

describe('orientation', () => {
  it('landscape is a quarter turn on the portrait sheet and maps back', () => {
    const r = readSize(sheet, 'landscape');
    expect(r).toEqual({ w: 297, h: 210 });
    const st: Stroke[] = [[[0, 0], [297, 0]], [[0, 210], [100, 105]]];
    const phys = toSheet(st, sheet, 'landscape');
    // reading-left edge becomes the sheet's top edge; reading-top becomes the sheet's right edge
    expect(phys[0][0]).toEqual([210, 0]);
    expect(phys[0][1]).toEqual([210, 297]);
    expect(phys[1][0]).toEqual([0, 0]);
    for (const [i, q] of phys[1].entries()) expect(fromSheetPt(q, sheet, 'landscape')).toEqual(st[1][i]);
    expect(bounds(phys)).toEqual([0, 0, 210, 297]);
  });
});

describe('text mode', () => {
  it('wraps, paginates and keeps ink inside the margins on every page', () => {
    const text = Array.from({ length: 120 }, (_, i) => `Line ${i} of a long paragraph that wraps across the page width nicely.`).join('\n');
    const job = textJob(text, sheet, { fontId: 'futural', sizeMm: 5, lineSpacing: 1, align: 'left', margins, orient: 'portrait' });
    expect(job.pages.length).toBeGreaterThan(1);
    expect(job.overflow).toBe(false);
    for (const p of job.pages) {
      const b = bounds(p.strokes);
      expect(b[0]).toBeGreaterThanOrEqual(margins.left - 1e-6);
      expect(b[2]).toBeLessThanOrEqual(210 - margins.right + 1e-6);
      expect(b[1]).toBeGreaterThanOrEqual(margins.top - 1e-6);
      expect(b[3]).toBeLessThanOrEqual(297 - margins.bottom + 1e-6);
    }
  });
  it('nothing typed means nothing planned', () => {
    expect(textJob('  \n', sheet, { fontId: 'futural', sizeMm: 5, lineSpacing: 1, align: 'left', margins, orient: 'portrait' }).pages).toEqual([]);
  });
  it('landscape text stays on the sheet', () => {
    const job = textJob('Landscape text that runs along the long edge of the sheet. '.repeat(6), sheet, { fontId: 'futural', sizeMm: 5, lineSpacing: 1, align: 'left', margins, orient: 'landscape' });
    const b = bounds(job.pages[0].strokes);
    expect(b[0]).toBeGreaterThanOrEqual(margins.top - 1e-6); // reading-top margin is the sheet's right inset
    expect(b[2]).toBeLessThanOrEqual(210 - margins.top + 1e-6);
    expect(b[1] >= 0 && b[3] <= 297).toBe(true);
  });
  it('monospace stroke font gives every character the same advance', () => {
    const a = textJob('iiii', sheet, { fontId: 'mono:futural', sizeMm: 5, lineSpacing: 1, align: 'left', margins, orient: 'portrait' });
    const b = textJob('WWWW', sheet, { fontId: 'mono:futural', sizeMm: 5, lineSpacing: 1, align: 'left', margins, orient: 'portrait' });
    const xs = (p: Stroke[]) => bounds(p);
    // four glyph cells wide, within one cell of each other regardless of the letter shape
    expect(Math.abs(xs(a.pages[0].strokes)[2] - xs(b.pages[0].strokes)[2])).toBeLessThan(5 * (22 / 21));
  });
});

describe('lined paper: ruling and text share one coordinate system', () => {
  it('draws evenly spaced lines inside the margins, vertical margin line included', () => {
    const r = buildRuling(210, 297, lined);
    expect(r.ys[0]).toBeCloseTo(margins.top + 8, 9);
    r.ys.slice(1).forEach((y, i) => expect(y - r.ys[i]).toBeCloseTo(8, 9));
    expect(r.ys[r.ys.length - 1]).toBeLessThanOrEqual(297 - margins.bottom + 1e-6);
    expect(r.ys[r.ys.length - 1] + 8).toBeGreaterThan(297 - margins.bottom);
    expect(r.marginX).toBeCloseTo(margins.left + 22, 9);
    // border first, margin line second, then writing lines
    expect(r.strokes[0].length).toBe(5);
    expect(r.strokes[1]).toEqual([[r.marginX!, margins.top], [r.marginX!, 297 - margins.bottom]]);
    const b = bounds(r.strokes);
    expect(b).toEqual([margins.left, margins.top, 210 - margins.right, 297 - margins.bottom]);
  });
  it('honours fixed line count, hidden lines and the optional margin line', () => {
    expect(buildRuling(210, 297, { ...lined, lineCount: 5 }).ys.length).toBe(5);
    const off = buildRuling(210, 297, { ...lined, marginLine: false, border: false });
    expect(off.marginX).toBeNull();
    expect(off.strokes.every((s) => s.length === 2)).toBe(true);
    const hidden = buildRuling(210, 297, { ...lined, lineStyle: 'hidden' });
    expect(hidden.strokes).toEqual([]);
    expect(hidden.ys.length).toBeGreaterThan(20);
    expect(() => buildRuling(210, 297, { ...lined, margins: { left: 100, right: 100, top: 10, bottom: 10 } })).toThrow();
  });
  it('serpentine ruling reduces pen-up travel against all-left-to-right', () => {
    const r = buildRuling(210, 297, { ...lined, border: false, marginLine: false });
    const naive = r.strokes.map((s) => (s[0][0] > s[1][0] ? [s[1], s[0]] : s));
    expect(planStats(r.strokes).travelMm).toBeLessThan(planStats(naive as Stroke[]).travelMm * 0.2);
  });
  it('text baselines are exactly on the ruling lines (same y as the pen draws them)', () => {
    const job = linedJob('Hello lined world\nsecond line here', sheet, lined);
    expect(job.pages.length).toBe(1);
    const ys = job.ruling!.ys;
    const bl = job.baselines![0];
    expect(bl.length).toBe(2);
    bl.forEach((b, i) => expect(b + RULE_LIFT_MM).toBeCloseTo(ys[i], 9));
    // the bottom of a capital letter H is its baseline: it sits RULE_LIFT above the ruling line
    const ink = job.pages[0].layers.find((l) => l.kind === 'ink')!.strokes;
    const firstLineBottom = Math.max(...ink.filter((s) => s.every((p) => Math.abs(p[1] - (ys[0] - RULE_LIFT_MM)) < 6)).flat().map((p) => p[1]).filter((y) => y < ys[0] + 5));
    expect(firstLineBottom).toBeGreaterThan(ys[0] - RULE_LIFT_MM - 0.2);
    expect(firstLineBottom).toBeLessThan(ys[0] + 5);
  });
  it('draws the ruling before the text and reports both stages', () => {
    const job = linedJob('abc', sheet, lined);
    expect(job.pages[0].layers.map((l) => l.label)).toEqual(['Ruling the page', 'Writing text']);
    expect(job.pages[0].stages.reduce((a, s) => a + s.count, 0)).toBe(job.pages[0].strokes.length);
    expect(job.pages[0].stages[0].label).toBe('Ruling the page');
    const n = job.pages[0].stages[0].count;
    expect(job.pages[0].strokes.slice(0, n)).toEqual(job.pages[0].layers[0].strokes);
  });
  it('starts text to the right of the margin line and never above/below the ruled area', () => {
    const job = linedJob('Margin check '.repeat(40), sheet, lined);
    const ink = job.pages[0].layers.find((l) => l.kind === 'ink')!.strokes;
    const b = bounds(ink);
    expect(b[0]).toBeGreaterThanOrEqual(job.ruling!.marginX! + 3 - 1e-6);
    expect(b[2]).toBeLessThanOrEqual(job.ruling!.x1 - 3 + 1e-6 + 1);
    expect(b[1]).toBeGreaterThanOrEqual(job.ruling!.top);
    expect(b[3]).toBeLessThanOrEqual(sheet.h - sheet.marginMm + 1e-6);
  });
  it('writing on every second line doubles the spacing', () => {
    const job = linedJob('one\ntwo\nthree', sheet, { ...lined, writeEvery: 2 });
    const bl = job.baselines![0];
    expect(bl[1] - bl[0]).toBeCloseTo(16, 9);
  });
  it('long text continues on further sheets, each with its own ruling', () => {
    const job = linedJob('word '.repeat(900), sheet, lined);
    expect(job.pages.length).toBeGreaterThan(1);
    expect(job.pages.every((p) => p.layers[0].kind === 'ruling')).toBe(true);
  });
  it('lined landscape uses the same coordinates, turned onto the sheet', () => {
    const job = linedJob('turned', sheet, { ...lined, orient: 'landscape', margins: { left: 15, right: 15, top: 15, bottom: 15 } });
    const b = bounds(job.pages[0].strokes);
    expect(b[0]).toBeGreaterThanOrEqual(15 - 1e-6);
    expect(b[2]).toBeLessThanOrEqual(210 - 15 + 1e-6);
    expect(b[3]).toBeLessThanOrEqual(297 - 15 + 1e-6);
    // the ruling's first line (reading y = top + spacing) is the sheet x = 210 - y
    const ys = job.ruling!.ys;
    expect(job.pages[0].layers[0].strokes.flat().some((p) => Math.abs(p[0] - (210 - ys[0])) < 1e-9)).toBe(true);
  });
  it('hidden lines: text still follows the invisible lines, only ink is drawn', () => {
    const job = linedJob('invisible ruling', sheet, { ...lined, lineStyle: 'hidden' });
    expect(job.pages[0].layers.map((l) => l.kind)).toEqual(['ink']);
    expect(job.baselines![0][0] + RULE_LIFT_MM).toBeCloseTo(job.ruling!.ys[0], 9);
  });
});

describe('image mode', () => {
  const disc = (w: number, h: number) => {
    const data = new Uint8ClampedArray(w * h * 4).fill(255);
    for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
      const inside = ((x - w / 2) / (w / 2.5)) ** 2 + ((y - h / 2) / (h / 2.5)) ** 2 < 1;
      if (inside) { const i = (y * w + x) * 4; data[i] = data[i + 1] = data[i + 2] = 20; }
    }
    return { width: w, height: h, data };
  };
  const opts = { style: 'outline' as const, detail: 0.5, scale: 1, rotationDeg: 0, offsetX: 0.5, offsetY: 0.5, invert: false, clean: true, margins: { left: 15, right: 15, top: 15, bottom: 15 }, orient: 'portrait' as const };
  it('keeps the artwork inside the margins, also when rotated', () => {
    for (const deg of [0, 30, 90, 137]) {
      const { plan } = imageJob(disc(200, 120), sheet, { ...opts, rotationDeg: deg });
      expect(plan.pages.length).toBe(1);
      const b = bounds(plan.pages[0].strokes);
      expect(b[0]).toBeGreaterThanOrEqual(15 - 0.5);
      expect(b[2]).toBeLessThanOrEqual(195 + 0.5);
      expect(b[1]).toBeGreaterThanOrEqual(15 - 0.5);
      expect(b[3]).toBeLessThanOrEqual(282 + 0.5);
    }
  });
  it('position sliders move the artwork', () => {
    const left = bounds(imageJob(disc(200, 120), sheet, { ...opts, scale: 0.4, offsetX: 0, offsetY: 0 }).plan.pages[0].strokes);
    const right = bounds(imageJob(disc(200, 120), sheet, { ...opts, scale: 0.4, offsetX: 1, offsetY: 1 }).plan.pages[0].strokes);
    expect(right[0]).toBeGreaterThan(left[0] + 50);
    expect(right[1]).toBeGreaterThan(left[1] + 50);
  });
  it('ordering keeps pen-up travel small', () => {
    const { plan } = imageJob(disc(240, 240), sheet, { ...opts, style: 'shade' });
    const st = plan.pages[0].stats;
    expect(st.strokes).toBeGreaterThan(5);
    expect(st.travelMm).toBeLessThan(st.drawMm * 0.5);
  });
  it('rotation fit shrinks only when needed', () => {
    expect(fitScaleForRotation(1, 0, 100, 100)).toBe(1);
    expect(fitScaleForRotation(1, 45, 100, 100)).toBeCloseTo(1 / Math.SQRT2, 6);
  });
});

import { reachInsets, placeholderFrame, checkStrokes } from '../frame';
describe('reach limits', () => {
  it('insets widen where the plotter cannot reach and drawings then pass the check', () => {
    const f = { ...placeholderFrame(), originPen: [29.4, 39.1] as [number, number], surfaceZ: 20 };
    const reach: [number, number, number, number] = [30, 280, 30, 308];
    const ins = reachInsets(f, reach);
    expect(ins.top).toBeGreaterThan(27); // sheet top is at pen y 336, reach ends at 308
    expect(ins.bottom).toBe(8);
    const job = linedJob('hello '.repeat(50), sheet, { ...lined, margins: { left: ins.left, right: ins.right, top: ins.top, bottom: ins.bottom } });
    const chk = checkStrokes(job.pages[0].strokes, f, reach);
    expect(chk).toEqual({ offPaper: 0, offReach: 0 });
  });
});
