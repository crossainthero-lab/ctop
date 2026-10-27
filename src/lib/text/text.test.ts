import { describe, expect, it } from 'vitest';
import { readFileSync, existsSync } from 'node:fs';
import { listStrokeFonts, layoutText } from './strokefont';
import { outlineText, parseFont } from './outline';
import { boundsOf } from '../geometry';

describe('stroke fonts', () => {
  it('lists curated fonts', () => {
    const ids = listStrokeFonts().map((f) => f.id);
    expect(ids).toContain('futural');
    expect(ids).toContain('scripts');
    expect(ids.length).toBeGreaterThanOrEqual(8);
  });
  it('capital H is cap-height tall, upright (y down), top at y=0', () => {
    const l = layoutText('H', { fontId: 'futural', sizeMm: 10 });
    const b = boundsOf(l.strokes)!;
    expect(b.minY).toBeCloseTo(0, 5);
    expect(b.maxY).toBeCloseTo(10, 5);
    expect(l.strokes).toHaveLength(3); // two stems + crossbar
    // crossbar roughly mid-height
    const bar = l.strokes.find((s) => s[0][1] === s[1][1] && s[0][1] > 0 && s[0][1] < 10)!;
    expect(bar[0][1]).toBeGreaterThan(3); expect(bar[0][1]).toBeLessThan(7);
  });
  it('descenders go below baseline, connected glyph stays one stroke', () => {
    const l = layoutText('g', { fontId: 'futural', sizeMm: 10 });
    expect(boundsOf(l.strokes)!.maxY).toBeGreaterThan(10);
    const o = layoutText('o', { fontId: 'futural', sizeMm: 10 });
    expect(o.strokes).toHaveLength(1);
  });
  it('glyphs do not overlap neighbours', () => {
    const l = layoutText('HI', { fontId: 'futural', sizeMm: 10 });
    const xs = (s: number[][][]) => s.flat().map((p) => p[0]);
    const h = layoutText('H', { fontId: 'futural', sizeMm: 10 });
    expect(Math.min(...xs(l.strokes.slice(h.strokes.length)))).toBeGreaterThan(Math.max(...xs(h.strokes)));
  });
  it('multiline, spacing and alignment', () => {
    const base = { fontId: 'futural', sizeMm: 8 };
    const l = layoutText('Hello\nHi', base);
    expect(l.heightMm).toBeCloseTo(8 + 8 * 1.5);
    const left = layoutText('Hello\nHi', { ...base, align: 'left' });
    const right = layoutText('Hello\nHi', { ...base, align: 'right' });
    const rx = boundsOf(right.strokes)!.maxX; // ink edge sits within the glyph side-bearing of the box edge
    expect(rx).toBeLessThanOrEqual(right.widthMm + 1e-6); expect(rx).toBeGreaterThan(right.widthMm - 2);
    expect(left.widthMm).toBeCloseTo(right.widthMm);
    const wide = layoutText('Hello', { ...base, letterSpacingMm: 2 });
    expect(wide.widthMm).toBeCloseTo(layoutText('Hello', base).widthMm + 8, 5);
    const tall = layoutText('a\nb', { ...base, lineSpacing: 2 });
    expect(tall.heightMm).toBeCloseTo(8 + 8 * 1.5 * 2);
  });
  it('centre alignment centres short lines', () => {
    const l = layoutText('MMMMMM\nI', { fontId: 'futural', sizeMm: 10, align: 'center' });
    const lastLine = l.strokes.filter((s) => s.every((p) => p[1] > 12));
    const b = boundsOf(lastLine)!;
    expect((b.minX + b.maxX) / 2).toBeCloseTo(l.widthMm / 2, 0);
  });
  it('wraps at maxWidth and hard-wraps long words', () => {
    const t = 'the quick brown fox jumps over';
    const wrapped = layoutText(t, { fontId: 'futural', sizeMm: 6, maxWidthMm: 40 });
    expect(wrapped.heightMm).toBeGreaterThan(6);
    expect(boundsOf(wrapped.strokes)!.maxX).toBeLessThanOrEqual(40.01);
    const hard = layoutText('Supercalifragilisticexpialidocious', { fontId: 'futural', sizeMm: 6, maxWidthMm: 30 });
    expect(hard.heightMm).toBeGreaterThan(6 * 1.5);
    expect(boundsOf(hard.strokes)!.maxX).toBeLessThanOrEqual(30.01);
  });
  it('reports missing characters', () => {
    const l = layoutText('A€B', { fontId: 'futural', sizeMm: 6 });
    expect(l.missing).toEqual(['€']);
  });
  it('rotation by 90deg swaps extents', () => {
    const a = boundsOf(layoutText('Hello', { fontId: 'futural', sizeMm: 10 }).strokes)!;
    const r = boundsOf(layoutText('Hello', { fontId: 'futural', sizeMm: 10, rotationDeg: 90 }).strokes)!;
    expect(r.maxY - r.minY).toBeCloseTo(a.maxX - a.minX, 4);
  });
  it('every curated font renders printable ASCII', () => {
    const ascii = Array.from({ length: 94 }, (_, i) => String.fromCharCode(33 + i)).join('');
    for (const f of listStrokeFonts()) expect(layoutText(ascii, { fontId: f.id, sizeMm: 5 }).missing).toEqual([]);
  });
});

const TTF = ['/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf', '/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf'].find(existsSync);
describe.skipIf(!TTF)('outline fonts', () => {
  const font = () => {
    const b = readFileSync(TTF!);
    return parseFont(b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength) as ArrayBuffer);
  };
  it('H is cap-height tall, upright, closed contours', () => {
    const l = outlineText(font(), 'H', { sizeMm: 10 });
    const b = boundsOf(l.strokes)!;
    expect(b.minY).toBeCloseTo(0, 0); expect(b.maxY).toBeCloseTo(10, 0);
    for (const s of l.strokes) expect(s[0]).toEqual(s[s.length - 1]);
  });
  it('O has outer and inner contour; curves are flattened finely', () => {
    const l = outlineText(font(), 'O', { sizeMm: 20, toleranceMm: 0.02 });
    expect(l.strokes).toHaveLength(2);
    expect(l.strokes[0].length).toBeGreaterThan(20);
  });
  it('multiline/wrap/letter spacing', () => {
    const f = font();
    expect(outlineText(f, 'a\nb', { sizeMm: 8 }).heightMm).toBeCloseTo(8 + 12);
    const w = outlineText(f, 'hello world foo bar', { sizeMm: 6, maxWidthMm: 30 });
    expect(boundsOf(w.strokes)!.maxX).toBeLessThanOrEqual(31);
    expect(outlineText(f, 'ab', { sizeMm: 6, letterSpacingMm: 3 }).widthMm).toBeCloseTo(outlineText(f, 'ab', { sizeMm: 6 }).widthMm + 3, 5);
  });
});
