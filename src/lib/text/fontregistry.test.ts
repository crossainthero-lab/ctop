import { describe, expect, it, vi, beforeEach } from 'vitest';
import { execSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

vi.mock('./outline', async (orig) => {
  const m = await orig<typeof import('./outline')>();
  return { ...m, parseFont: vi.fn(m.parseFont) };
});
import * as outline from './outline';
import { loadFontList, ensureFont, fontMetricsFor, getMetrics, _resetFontRegistry } from './fontregistry';
import { layoutPage, type PageOpts } from '../write/pagelayout';
import { boundsOf } from '../geometry';

function findFont(): string | null {
  try {
    const out = execSync(`fc-list ':family=DejaVu Sans' file`, { encoding: 'utf8' }) + execSync('fc-list file', { encoding: 'utf8' });
    return out.split('\n').map((l) => l.replace(/:\s*$/, '').trim()).find((f) => /\.(ttf|otf)$/i.test(f)) ?? null;
  } catch { return null; }
}
const file = findFont();
const d = file ? describe : describe.skip;

const base: PageOpts = {
  widthMm: 210, heightMm: 297, fontId: 'sys:abc', sizeMm: 6, lineSpacing: 1, align: 'left',
  margins: { left: 20, right: 15, top: 20, bottom: 15 }, rules: null,
};

d('outline fonts via registry', () => {
  const b64 = file ? readFileSync(file).toString('base64') : '';
  const fake = vi.fn(async (cmd: string) => (cmd === 'fonts' ? [{ id: 'abc', family: 'Test', style: 'Regular' }] : b64));
  beforeEach(() => { _resetFontRegistry(); vi.mocked(outline.parseFont).mockClear(); fake.mockClear(); });

  it('lists stroke and outline fonts', async () => {
    const l = await loadFontList(fake as never);
    expect(l.some((f) => f.kind === 'stroke')).toBe(true);
    expect(l.find((f) => f.id === 'sys:abc')).toMatchObject({ kind: 'outline', name: 'Test' });
  });

  it('lays out Hello inside the margins with closed contours; metrics cached', async () => {
    await loadFontList(fake as never);
    expect(fontMetricsFor('sys:abc', 6)).toBeNull();
    await Promise.all([ensureFont('sys:abc'), ensureFont('sys:abc')]);
    await ensureFont('sys:abc'); await getMetrics('sys:abc', 6);
    const l = layoutPage('Hello', base);
    expect(l.fontMissing).toBeUndefined();
    expect(l.strokes.length).toBeGreaterThan(4);
    for (const s of l.strokes) { expect(s.length).toBeGreaterThanOrEqual(3); expect(s[0]).toEqual(s[s.length - 1]); }
    const bb = boundsOf(l.strokes)!;
    expect(bb.minX).toBeGreaterThanOrEqual(20 - 1e-6);
    expect(bb.maxX).toBeLessThanOrEqual(195);
    expect(bb.minY).toBeGreaterThanOrEqual(20 - 1e-6);
    // cap height of H == sizeMm
    const H = layoutPage('H', base);
    const hb = boundsOf(H.strokes)!;
    expect(hb.maxY - hb.minY).toBeCloseTo(6, 0);
    const pts = l.strokes.reduce((n, s) => n + s.length, 0);
    expect(pts).toBeLessThan(400);
    expect(outline.parseFont).toHaveBeenCalledTimes(1);
    expect(fake.mock.calls.filter((c) => c[0] === 'font_bytes').length).toBe(1);
  });

  it('wraps long text', async () => {
    await loadFontList(fake as never); await ensureFont('sys:abc');
    const l = layoutPage('Hello world, this is a long line of outline text that must wrap. '.repeat(3), base);
    expect(l.lines.length).toBeGreaterThan(1);
    const bb = boundsOf(l.strokes)!;
    expect(bb.maxX).toBeLessThanOrEqual(195 + 1e-6);
  });

  it('falls back to the default stroke font when not loaded', () => {
    const l = layoutPage('Hello', { ...base, fontId: 'sys:notloaded' });
    expect(l.fontMissing).toBe(true);
    expect(l.strokes.length).toBeGreaterThan(0);
  });
});
