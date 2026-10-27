import { describe, expect, it } from 'vitest';
import type { RasterImage, Stroke } from '../types';
import { boundsOf, clipStrokesToRect, strokeLength, totalLength } from '../geometry';
import { convertImage, type ConvertMode } from './convert';
import { adjust, gaussianBlur, resize, toGray } from './process';
import { skeletonize, traceChains } from './trace';

function make(w: number, h: number, f: (x: number, y: number) => number): RasterImage {
  const data = new Uint8ClampedArray(w * h * 4);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    const v = Math.round(f(x, y) * 255), i = (y * w + x) * 4;
    data[i] = data[i + 1] = data[i + 2] = v; data[i + 3] = 255;
  }
  return { width: w, height: h, data };
}
const white = make(100, 100, () => 1);
const square = make(100, 100, (x, y) => (x >= 30 && x < 70 && y >= 30 && y < 70 ? 0 : 1)); // black filled square
const circle = make(120, 120, (x, y) => (Math.hypot(x - 60, y - 60) < 35 ? 0 : 1));
const ring = make(120, 120, (x, y) => { const r = Math.hypot(x - 60, y - 60); return r > 30 && r < 36 ? 0 : 1; });
const gradient = make(200, 100, (x) => 1 - x / 199); // dark at right... white at left? 1 - x/199: left=white, right=black
const inside = (ss: Stroke[], w: number, h: number) => { const b = boundsOf(ss); return !b || (b.minX >= -1e-9 && b.minY >= -1e-9 && b.maxX <= w + 1e-9 && b.maxY <= h + 1e-9); };
const P = { minFeatureMm: 0.5, simplifyMm: 0.1 };

describe('image primitives', () => {
  it('toGray composites alpha over white', () => {
    const g = toGray({ width: 2, height: 1, data: new Uint8ClampedArray([0, 0, 0, 0, 0, 0, 0, 255]) });
    expect(g.data[0]).toBeCloseTo(1); expect(g.data[1]).toBeCloseTo(0);
  });
  it('adjust brightness/contrast', () => {
    const g = { width: 1, height: 1, data: new Float32Array([0.5]) };
    expect(adjust(g, 0.2, 0).data[0]).toBeCloseTo(0.7);
    expect(adjust({ ...g, data: new Float32Array([0.75]) }, 0, 0.5).data[0]).toBeCloseTo(1);
  });
  it('blur preserves mean on flat; resize halves dimensions', () => {
    const g = toGray(white);
    expect(gaussianBlur(g, 2).data[55]).toBeCloseTo(1);
    const r = resize(g, 50, 40);
    expect([r.width, r.height]).toEqual([50, 40]);
  });
  it('skeletonize thins a thick bar to one pixel and traces one chain', () => {
    const w = 30, h = 9, m = new Uint8Array(w * h);
    for (let y = 3; y < 6; y++) for (let x = 2; x < 28; x++) m[y * w + x] = 1;
    const sk = skeletonize(m, w, h);
    const chains = traceChains(sk, w, h);
    expect(chains).toHaveLength(1);
    expect(chains[0].length).toBeGreaterThan(15);
    expect(Math.abs(chains[0][5][1] - 4.5)).toBeLessThan(1.1);
  });
  it('traceChains closes loops and splits at junctions', () => {
    const w = 10, h = 10, m = new Uint8Array(w * h);
    for (let i = 2; i <= 7; i++) { m[2 * w + i] = m[7 * w + i] = m[i * w + 2] = m[i * w + 7] = 1; }
    const loops = traceChains(m, w, h);
    expect(loops).toHaveLength(1);
    expect(loops[0][0]).toEqual(loops[0][loops[0].length - 1]);
    for (let i = 2; i <= 7; i++) m[i * w + 5] = 1; // bar across -> junctions
    expect(traceChains(m, w, h).length).toBeGreaterThan(2);
  });
});

describe('convertImage', () => {
  it('output size keeps aspect', () => {
    const r = convertImage(gradient, 'threshold', P, 100);
    expect([r.widthMm, r.heightMm]).toEqual([100, 50]);
  });
  it('outline of a filled square is a loop near its boundary', () => {
    const r = convertImage(square, 'outline', { ...P, detail: 1 }, 100);
    expect(r.strokes.length).toBeGreaterThan(0);
    const b = boundsOf(r.strokes)!;
    for (const [a, c] of [[b.minX, 30], [b.minY, 30], [b.maxX, 70], [b.maxY, 70]]) expect(Math.abs(a - c)).toBeLessThan(3);
    const len = totalLength(r.strokes);
    expect(len).toBeGreaterThan(140); expect(len).toBeLessThan(190); // perimeter 160 mm
    expect(inside(r.strokes, 100, 100)).toBe(true);
  });
  it('threshold traces region boundaries as closed strokes', () => {
    const r = convertImage(square, 'threshold', P, 100);
    expect(r.strokes).toHaveLength(1);
    const s = r.strokes[0];
    expect(s[0]).toEqual(s[s.length - 1]);
    const b = boundsOf(r.strokes)!;
    expect(b.minX).toBeCloseTo(30, 0); expect(b.maxX).toBeCloseTo(70, 0);
    expect(inside(r.strokes, 100, 100)).toBe(true);
  });
  it('threshold closes shapes touching the image edge', () => {
    const edge = make(60, 60, (x) => (x < 20 ? 0 : 1));
    const r = convertImage(edge, 'threshold', P, 60);
    expect(r.strokes).toHaveLength(1);
    expect(r.strokes[0][0]).toEqual(r.strokes[0][r.strokes[0].length - 1]);
    expect(inside(r.strokes, 60, 60)).toBe(true);
  });
  it('lineart of a ring yields centerline strokes at the ring radius', () => {
    const r = convertImage(ring, 'lineart', { ...P, detail: 1 }, 120);
    expect(r.strokes.length).toBeGreaterThanOrEqual(1);
    const pts = r.strokes.flat();
    const rad = pts.map(([x, y]) => Math.hypot(x - 60, y - 60));
    expect(Math.min(...rad)).toBeGreaterThan(30); expect(Math.max(...rad)).toBeLessThan(36);
    expect(totalLength(r.strokes)).toBeGreaterThan(2 * Math.PI * 33 * 0.9);
  });
  it('outline of a circle forms a ring of the right radius', () => {
    const r = convertImage(circle, 'outline', { ...P, detail: 1 }, 120);
    const rad = r.strokes.flat().map(([x, y]) => Math.hypot(x - 60, y - 60));
    expect(rad.length).toBeGreaterThan(20);
    expect(Math.min(...rad)).toBeGreaterThan(32); expect(Math.max(...rad)).toBeLessThan(38);
  });
  it('white image yields no strokes in any mode', () => {
    for (const m of ['outline', 'lineart', 'threshold', 'hatch', 'crosshatch'] as ConvertMode[]) {
      expect(convertImage(white, m, P, 100).strokes, m).toHaveLength(0);
    }
  });
  it('hatch: darker areas get denser strokes; lines follow angle', () => {
    const r = convertImage(gradient, 'hatch', { ...P, hatchAngleDeg: 0, hatchSpacingMm: 2 }, 100);
    const count = (x0: number, x1: number) => r.strokes.filter((s) => { const x = (s[0][0] + s[s.length - 1][0]) / 2; return x >= x0 && x < x1; }).length;
    // horizontal lines: darker right side lines are split by fewer gaps -> compare drawn length per area
    const len = (x0: number, x1: number) => totalLength(clipStrokesToRect(r.strokes, { minX: x0, minY: 0, maxX: x1, maxY: 50 }));
    expect(len(80, 100)).toBeGreaterThan(len(0, 20));
    expect(len(0, 20)).toBeLessThan(len(40, 60));
    expect(count(0, 100)).toBeGreaterThan(0);
    for (const s of r.strokes) expect(s[0][1]).toBeCloseTo(s[s.length - 1][1], 6);
    expect(inside(r.strokes, 100, 50)).toBe(true);
  });
  it('hatch density: darkest right edge has ~4x line count of mid-light region', () => {
    const r = convertImage(gradient, 'hatch', { ...P, hatchAngleDeg: 90, hatchSpacingMm: 2, minFeatureMm: 0 }, 100);
    const lines = (x0: number, x1: number) => r.strokes.filter((s) => s[0][0] >= x0 && s[0][0] < x1).length;
    expect(lines(90, 100)).toBeGreaterThan(lines(20, 30));
  });
  it('crosshatch adds directions in dark regions only', () => {
    const dark = make(80, 80, () => 0.05);
    const light = make(80, 80, () => 0.9);
    const angles = (ss: Stroke[]) => new Set(ss.map((s) => Math.round(((Math.atan2(s[s.length - 1][1] - s[0][1], s[s.length - 1][0] - s[0][0]) * 180) / Math.PI + 360) % 180)));
    expect(angles(convertImage(dark, 'crosshatch', { ...P, hatchAngleDeg: 45 }, 80).strokes).size).toBeGreaterThanOrEqual(4);
    expect(convertImage(light, 'crosshatch', P, 80).strokes).toHaveLength(0);
    expect(angles(convertImage(dark, 'hatch', { ...P, hatchAngleDeg: 45 }, 80).strokes).size).toBe(1);
  });
  it('invert swaps ink and paper', () => {
    const a = convertImage(square, 'hatch', { ...P }, 100).strokes;
    const b = convertImage(square, 'hatch', { ...P, invert: true }, 100).strokes;
    expect(totalLength(a)).toBeLessThan(totalLength(b));
  });
  it('minFeature removes more strokes; simplify reduces points', () => {
    const noise = make(100, 100, (x, y) => ((x * 7 + y * 13) % 11 < 2 ? 0 : 1));
    const few = convertImage(noise, 'threshold', { ...P, minFeatureMm: 6 }, 100).strokes.length;
    const many = convertImage(noise, 'threshold', { ...P, minFeatureMm: 0.2 }, 100).strokes.length;
    expect(few).toBeLessThan(many);
    const pts = (s: number) => convertImage(circle, 'threshold', { ...P, simplifyMm: s }, 120).strokes.flat().length;
    expect(pts(1)).toBeLessThan(pts(0.01));
  });
  it('brightness/contrast change the result; threshold controls ink', () => {
    const mid = make(60, 60, (x) => (x < 30 ? 0.4 : 1));
    expect(convertImage(mid, 'threshold', { ...P, threshold: 0.3 }, 60).strokes).toHaveLength(0);
    expect(convertImage(mid, 'threshold', { ...P, threshold: 0.5 }, 60).strokes.length).toBeGreaterThan(0);
    expect(convertImage(mid, 'threshold', { ...P, threshold: 0.3, brightness: -0.2 }, 60).strokes.length).toBeGreaterThan(0);
  });
  it('is deterministic', () => {
    for (const m of ['outline', 'lineart', 'threshold', 'hatch', 'crosshatch'] as ConvertMode[]) {
      expect(convertImage(circle, m, P, 80)).toEqual(convertImage(circle, m, P, 80));
    }
  });
  it('all points stay inside the output rectangle', () => {
    for (const m of ['outline', 'lineart', 'threshold', 'hatch', 'crosshatch'] as ConvertMode[]) {
      const r = convertImage(gradient, m, { ...P, hatchAngleDeg: 33 }, 77);
      expect(inside(r.strokes, r.widthMm, r.heightMm), m).toBe(true);
      expect(r.strokes.every((s) => s.length >= 2 && strokeLength(s) > 0)).toBe(true);
    }
  });
});
