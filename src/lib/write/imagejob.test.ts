import { describe, expect, it } from 'vitest';
import { DOMParser } from 'linkedom';
import type { RasterImage, Stroke } from '../types';
import { boundsOf } from '../geometry';
import { convertImage } from '../image/convert';
import { DEFAULT_MARGINS } from './pagelayout';
import { capSize, cleanBackground, compositeOnWhite, decodeImageFile, imageToStrokes, orderStrokes, sniffKind, travelLength, type ImageJobOpts } from './imagejob';

const area = { widthMm: 210, heightMm: 297, margins: DEFAULT_MARGINS };
const parser = (t: string) => new DOMParser().parseFromString(t, 'image/svg+xml') as unknown as Document;

function make(w: number, h: number, f: (x: number, y: number) => [number, number, number, number]): RasterImage {
  const data = new Uint8ClampedArray(w * h * 4);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) data.set(f(x, y), (y * w + x) * 4);
  return { width: w, height: h, data };
}
/** dark ellipse body + legs on a grey backdrop with a mild gradient */
const dino = (w = 600, h = 400) => make(w, h, (x, y) => {
  const bg = 150 + Math.round((x / w) * 20);
  const body = ((x - w / 2) / (w * 0.3)) ** 2 + ((y - h * 0.45) / (h * 0.22)) ** 2 < 1;
  const leg = (Math.abs(x - w * 0.4) < w * 0.03 || Math.abs(x - w * 0.6) < w * 0.03) && y > h * 0.55 && y < h * 0.85;
  return body || leg ? [40, 90, 40, 255] : [bg, bg, bg, 255];
});
const inBox = (ss: Stroke[]) => {
  const b = boundsOf(ss)!, m = DEFAULT_MARGINS;
  expect(b.minX).toBeGreaterThanOrEqual(m.left - 1e-6);
  expect(b.maxX).toBeLessThanOrEqual(area.widthMm - m.right + 1e-6);
  expect(b.minY).toBeGreaterThanOrEqual(m.top - 1e-6);
  expect(b.maxY).toBeLessThanOrEqual(area.heightMm - m.bottom + 1e-6);
};
const o = (p: Partial<ImageJobOpts> = {}): ImageJobOpts => ({ style: 'outline', detail: 0.5, area, ...p });

describe('decode', () => {
  const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0]);
  it('sniffs kinds and rejects others', async () => {
    expect(sniffKind('a.bin', '', png)).toBe('png');
    expect(sniffKind('a.jpg', '', new Uint8Array([0xff, 0xd8, 0xff, 0xe0]))).toBe('jpeg');
    await expect(decodeImageFile({ name: 'a.gif', type: 'image/gif', bytes: new Uint8Array([71, 73, 70, 56, 57, 97]) })).rejects.toThrow(/PNG, JPG, WEBP or SVG/);
  });
  it('svg returns text; raster goes through injected decoder and is composited on white', async () => {
    const svg = await decodeImageFile({ name: 'x.svg', type: 'image/svg+xml', bytes: new TextEncoder().encode('<svg xmlns="http://www.w3.org/2000/svg"/>') });
    expect('svgText' in svg).toBe(true);
    const r = await decodeImageFile({ name: 'a.png', type: 'image/png', bytes: png }, async () => ({ width: 1, height: 1, data: new Uint8ClampedArray([0, 0, 0, 0]) }));
    expect(Array.from((r as RasterImage).data)).toEqual([255, 255, 255, 255]);
  });
  it('caps size', () => { expect(capSize(8000, 4000)).toEqual({ width: 4000, height: 2000 }); expect(capSize(100, 50)).toEqual({ width: 100, height: 50 }); });
  it('compositeOnWhite blends half alpha', () => {
    const r = compositeOnWhite({ width: 1, height: 1, data: new Uint8ClampedArray([0, 0, 0, 128]) });
    expect(r.data[0]).toBeGreaterThan(120); expect(r.data[3]).toBe(255);
  });
});

describe('cleanBackground', () => {
  it('whitens a flat/gradient backdrop, keeps the object', () => {
    const img = dino(), c = cleanBackground(img);
    const px = (i: RasterImage, x: number, y: number) => i.data[(y * i.width + x) * 4];
    expect(px(c, 5, 5)).toBe(255); expect(px(c, 590, 390)).toBe(255);
    expect(px(c, 300, 180)).toBe(40);
    expect(px(c, 240, 300)).toBe(40); // leg
  });
  it('leaves a busy-border image unchanged', () => {
    const img = make(100, 100, (x, y) => { const v = ((x * 7 + y * 13) % 5) * 60; return [v, 255 - v, v / 2, 255]; });
    expect(cleanBackground(img)).toBe(img);
  });
  it('transparent PNG-like RGBA becomes white background', () => {
    const img = make(100, 80, (x, y) => (x > 30 && x < 70 && y > 20 && y < 60 ? [0, 0, 0, 255] : [0, 0, 0, 0]));
    const c = cleanBackground(img);
    expect(c.data[3]).toBe(255); expect(c.data[0]).toBe(255); expect(c.data[(40 * 100 + 50) * 4]).toBe(0);
  });
  it('outline without cleanup traces the backdrop gradient less cleanly than with cleanup', () => {
    const noisy = make(300, 200, (x, y) => {
      const bg = 140 + ((x * 3 + y) % 24);
      const body = ((x - 150) / 70) ** 2 + ((y - 100) / 45) ** 2 < 1;
      return body ? [30, 30, 30, 255] : [bg, bg, bg, 255];
    });
    const a = imageToStrokes(noisy, o({ cleanBackground: true })), b = imageToStrokes(noisy, o({ cleanBackground: false }));
    expect(a.stats.strokes).toBeLessThanOrEqual(b.stats.strokes);
    expect(a.stats.drawMm).toBeLessThan(b.stats.drawMm * 1.05);
    expect(a.stats.drawMm).toBeGreaterThan(50);
  });
});

describe('ordering', () => {
  it('reduces travel and keeps every stroke', () => {
    const ss: Stroke[] = [];
    for (let i = 0; i < 60; i++) { const x = ((i * 37) % 60) * 3, y = ((i * 11) % 7) * 20; ss.push([[x, y], [x + 2, y + 1]]); }
    const ord = orderStrokes(ss, [0, 0]);
    expect(ord).toHaveLength(ss.length);
    expect(travelLength(ord, [0, 0])).toBeLessThan(travelLength(ss, [0, 0]) * 0.5);
  });
  it('reverses a stroke when its end is closer', () => {
    const ord = orderStrokes([[[100, 0], [10, 0]]], [0, 0]);
    expect(ord[0][0]).toEqual([10, 0]);
  });
});

describe('imageToStrokes', () => {
  for (const style of ['outline', 'detail', 'shade', 'crosshatch'] as const) {
    it(`${style}: strokes lie in the margin box`, () => {
      const r = imageToStrokes(dino(300, 200), o({ style }));
      expect(r.strokes.length).toBeGreaterThan(0);
      inBox(r.strokes);
      expect(r.placement.widthMm).toBeCloseTo(210 - 35, 3);
    });
  }
  it('scale and offset place the drawing', () => {
    const r = imageToStrokes(dino(300, 200), o({ scale: 0.4, offset: { x: 0, y: 0 } }));
    expect(r.placement.xMm).toBeCloseTo(DEFAULT_MARGINS.left); expect(r.placement.yMm).toBeCloseTo(DEFAULT_MARGINS.top);
    expect(r.placement.widthMm).toBeCloseTo(175 * 0.4, 3);
    const b = boundsOf(r.strokes)!;
    expect(b.maxX).toBeLessThanOrEqual(r.placement.xMm + r.placement.widthMm + 1e-6);
  });
  it('ordering beats raw converter order', () => {
    const img = dino(300, 200);
    const raw = convertImage(cleanBackground(img), 'hatch', { detail: 0.5, hatchSpacingMm: 1.75 }, 175).strokes;
    const r = imageToStrokes(img, o({ style: 'shade' }));
    expect(r.stats.strokes).toBeGreaterThan(10);
    expect(r.stats.travelMm).toBeLessThan(travelLength(raw, [0, 0]));
    expect(r.stats.estMinutes).toBeGreaterThan(0);
  });
  it('svg path works', () => {
    const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 100 50"><path d="M0 0 L100 50 L0 50 Z"/><circle cx="50" cy="25" r="10"/></svg>';
    const r = imageToStrokes({ svgText: svg }, o({ svgParser: parser }));
    expect(r.strokes).toHaveLength(2);
    inBox(r.strokes);
    expect(r.placement.widthMm / r.placement.heightMm).toBeCloseTo(2, 3);
  });
  it('svg without geometry throws', () => {
    expect(() => imageToStrokes({ svgText: '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>' }, o({ svgParser: parser }))).toThrow();
  });
  it('complexity guard simplifies noisy hatch and 1500x1000 runs fast', () => {
    const noise = make(1500, 1000, (x, y) => { const v = ((x * 7919 + y * 104729) % 251); return [v, v, v, 255]; });
    const t = Date.now();
    const r = imageToStrokes(noise, o({ style: 'outline', detail: 1 }));
    expect(Date.now() - t).toBeLessThan(4000 * 3); // pathological noise; typical image below
    expect(r.stats.strokes).toBeLessThanOrEqual(6000 * 2);
    expect(r.warnings.length).toBeGreaterThan(0);
    inBox(r.strokes);
  }, 30000);
  it('typical 1500x1000 image under 4s', () => {
    const t = Date.now();
    const r = imageToStrokes(dino(1500, 1000), o({ detail: 0.6 }));
    expect(Date.now() - t).toBeLessThan(4000);
    expect(r.strokes.length).toBeGreaterThan(0);
  });
});
