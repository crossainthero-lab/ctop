import { describe, expect, it } from 'vitest';
import { layoutPage, strokesKey, RULE_LIFT_MM, type PageOpts } from './pagelayout';
import { boundsOf } from '../geometry';

const base: PageOpts = {
  widthMm: 210, heightMm: 297, fontId: 'futural', sizeMm: 5, lineSpacing: 1, align: 'left',
  margins: { left: 20, right: 15, top: 20, bottom: 15 }, rules: null,
};
const rules = Array.from({ length: 33 }, (_, i) => 30 + i * 8); // 30 .. 286

describe('blank paper', () => {
  it('wraps inside the margins and stacks lines at the font pitch', () => {
    const l = layoutPage('The quick brown fox jumps over the lazy dog. '.repeat(6), base);
    expect(l.mode).toBe('blank');
    expect(l.lines.length).toBeGreaterThan(2);
    const b = boundsOf(l.strokes)!;
    expect(b.minX).toBeGreaterThanOrEqual(20 - 1e-6);
    expect(b.maxX).toBeLessThanOrEqual(210 - 15 + 1e-6);
    expect(b.minY).toBeGreaterThanOrEqual(20 - 1e-6);
    const d = l.lines[1].baselineMm - l.lines[0].baselineMm;
    expect(d).toBeCloseTo(5 * 1.5, 6);
    expect(l.fits).toBe(true);
  });
  it('keeps paragraphs and blank lines', () => {
    const l = layoutPage('one\n\ntwo', base);
    expect(l.lines.map((x) => x.text)).toEqual(['one', '', 'two']);
    expect(l.lines[2].baselineMm - l.lines[0].baselineMm).toBeCloseTo(2 * 7.5, 6);
  });
  it('reports overflow instead of drawing past the bottom margin', () => {
    const l = layoutPage('line\n'.repeat(80), base);
    expect(l.overflowLines).toBeGreaterThan(0);
    expect(l.fits).toBe(false);
    expect(boundsOf(l.strokes)!.maxY).toBeLessThanOrEqual(297 - 15 + 1e-6);
    expect(l.lines.every((x) => x.baselineMm <= 297 - 15 - 5 * 0.35 + 1e-6)).toBe(true);
  });
  it('aligns right and center', () => {
    const r = layoutPage('hi', { ...base, align: 'right' });
    expect(r.lines[0].xMm + r.lines[0].widthMm).toBeCloseTo(195, 5);
    const c = layoutPage('hi', { ...base, align: 'center' });
    expect(c.lines[0].xMm + c.lines[0].widthMm / 2).toBeCloseTo(20 + (175 / 2), 5);
  });
  it('hard-wraps a word longer than the line and survives empty text', () => {
    const l = layoutPage('x'.repeat(200), base);
    expect(l.lines.length).toBeGreaterThan(1);
    expect(layoutPage('', base).strokes).toEqual([]);
    expect(layoutPage('   \n \n', base).lines).toEqual([]);
  });
  it('skips characters the font lacks and says so', () => {
    const l = layoutPage('a中b', base);
    expect(l.missing).toContain('中');
  });
});

describe('ruled paper', () => {
  it('puts every line on a rule (just above its centre) and never above the top margin', () => {
    const l = layoutPage('Dear diary, today was a good day and I wrote it on lined paper.\nSecond paragraph.', { ...base, rules, sizeMm: 4 });
    expect(l.mode).toBe('ruled');
    for (const ln of l.lines) {
      const rule = rules.find((r) => Math.abs(r - RULE_LIFT_MM - ln.baselineMm) < 1e-9);
      expect(rule).toBeDefined();
    }
    // glyph bottoms (non-descender letters) end on the baseline
    const first = layoutPage('H', { ...base, rules, sizeMm: 4 });
    expect(boundsOf(first.strokes)!.maxY).toBeCloseTo(first.lines[0].baselineMm, 5);
    expect(first.lines[0].baselineMm - 4.4).toBeGreaterThanOrEqual(20 - 1e-6);
  });
  it('limits text size to the rule spacing', () => {
    const l = layoutPage('big', { ...base, rules, sizeMm: 12 });
    expect(l.sizeLimited).toBe(true);
    expect(l.sizeMm).toBeCloseTo(8 * 0.8, 6);
  });
  it('writes on every second rule when asked', () => {
    const l = layoutPage('a\nb\nc', { ...base, rules, sizeMm: 4, lineSpacing: 2 });
    expect(l.lines[1].baselineMm - l.lines[0].baselineMm).toBeCloseTo(16, 6);
  });
  it('overflows when there are fewer rules than lines', () => {
    const l = layoutPage('x\n'.repeat(60), { ...base, rules, sizeMm: 4 });
    expect(l.overflowLines).toBeGreaterThan(0);
    expect(l.lines.length).toBeLessThan(35);
  });
});

describe('key', () => {
  it('is stable and sensitive', () => {
    const a = layoutPage('hello', base).strokes;
    expect(strokesKey(a)).toBe(strokesKey(layoutPage('hello', base).strokes));
    expect(strokesKey(a)).not.toBe(strokesKey(layoutPage('hellp', base).strokes));
  });
});
