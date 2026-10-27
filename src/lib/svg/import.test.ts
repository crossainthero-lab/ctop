import { describe, expect, it } from 'vitest';
import { DOMParser } from 'linkedom';
import { importSvg, parseLength, parseTransform } from './import';
import { applyPt, boundsOf } from '../geometry';

const parser = (t: string) => new DOMParser().parseFromString(t, 'image/svg+xml') as unknown as Document;
const imp = (s: string) => importSvg(s, { parser });
const NS = 'xmlns="http://www.w3.org/2000/svg"';

describe('svg import', () => {
  it('parses units', () => {
    expect(parseLength('210mm')).toBe(210);
    expect(parseLength('2cm')).toBe(20);
    expect(parseLength('1in')).toBeCloseTo(25.4);
    expect(parseLength('72pt')).toBeCloseTo(25.4);
    expect(parseLength('96')).toBeCloseTo(25.4);
    expect(parseLength('50%')).toBeNull();
  });
  it('transform attribute parsing', () => {
    const [x, y] = applyPt(parseTransform('translate(10,20) scale(2)'), [1, 1]);
    expect([x, y]).toEqual([12, 22]);
    const [rx, ry] = applyPt(parseTransform('rotate(90 5 5)'), [10, 5]);
    expect(rx).toBeCloseTo(5); expect(ry).toBeCloseTo(10);
  });
  it('mm width/height + viewBox scale to mm, y down', () => {
    const r = imp(`<svg ${NS} width="100mm" height="50mm" viewBox="0 0 200 100"><path d="M0 0 L200 100"/></svg>`);
    expect([r.widthMm, r.heightMm]).toEqual([100, 50]);
    expect(r.strokes).toHaveLength(1);
    expect(r.strokes[0]).toEqual([[0, 0], [100, 50]]);
  });
  it('viewBox offset is moved to origin; unitless = px at 96 dpi', () => {
    const r = imp(`<svg ${NS} viewBox="100 100 96 96"><line x1="100" y1="100" x2="196" y2="196"/></svg>`);
    expect(r.widthMm).toBeCloseTo(25.4);
    expect(r.strokes[0][0]).toEqual([0, 0]);
    expect(r.strokes[0][1][0]).toBeCloseTo(25.4);
  });
  it('shapes: rect, rounded rect, circle, ellipse, polyline, polygon', () => {
    const r = imp(`<svg ${NS} width="100mm" height="100mm" viewBox="0 0 100 100">
      <rect x="10" y="10" width="20" height="10"/>
      <rect x="40" y="10" width="20" height="10" rx="3"/>
      <circle cx="50" cy="50" r="10"/>
      <ellipse cx="20" cy="80" rx="10" ry="5"/>
      <polyline points="0,0 10,0 10,10"/>
      <polygon points="70,70 90,70 80,90"/></svg>`);
    expect(r.strokes).toHaveLength(6);
    expect(r.strokes[0]).toHaveLength(5);
    expect(boundsOf([r.strokes[2]])).toEqual(expect.objectContaining({ minX: expect.closeTo(40, 1), maxX: expect.closeTo(60, 1) }));
    const rr = boundsOf([r.strokes[1]])!;
    expect(rr.minX).toBeCloseTo(40); expect(rr.maxX).toBeCloseTo(60);
    expect(r.strokes[1].length).toBeGreaterThan(8); // rounded corners flattened
    const poly = r.strokes[5]; expect(poly[0]).toEqual(poly[poly.length - 1]);
    expect(r.strokes[2][0]).toEqual(r.strokes[2][r.strokes[2].length - 1]);
  });
  it('path commands: relative, H/V, curves, arcs, close, multiple subpaths', () => {
    const r = imp(`<svg ${NS} width="100mm" height="100mm" viewBox="0 0 100 100">
      <path d="M10 10 h20 v20 h-20 z M50 50 c10 0 20 10 20 20 s-10 10 -20 0 q5 5 0 10 a5 5 0 0 1 10 0"/></svg>`);
    expect(r.strokes).toHaveLength(2);
    expect(r.strokes[0]).toEqual([[10, 10], [30, 10], [30, 30], [10, 30], [10, 10]]);
    const b = boundsOf([r.strokes[1]])!;
    expect(b.minX).toBeGreaterThanOrEqual(45); expect(b.maxY).toBeLessThan(100);
    expect(r.strokes[1].length).toBeGreaterThan(20);
  });
  it('nested group transforms compose', () => {
    const r = imp(`<svg ${NS} width="100mm" height="100mm" viewBox="0 0 100 100"><g transform="translate(10 10)"><g transform="scale(2)"><rect x="5" y="5" width="10" height="10"/></g></g></svg>`);
    const b = boundsOf(r.strokes)!;
    expect([b.minX, b.minY, b.maxX, b.maxY]).toEqual([20, 20, 40, 40]);
  });
  it('skips hidden, defs and clipPath; warns on text/image/filter', () => {
    const r = imp(`<svg ${NS} width="100mm" height="100mm" viewBox="0 0 100 100">
      <defs><path id="a" d="M0 0 L9 9"/></defs>
      <clipPath id="c"><rect width="10" height="10"/></clipPath>
      <rect width="5" height="5" display="none"/><rect width="5" height="5" style="display:none"/>
      <text x="1" y="1">hi</text><image href="x.png"/><filter id="f"/>
      <line x1="0" y1="0" x2="10" y2="10"/></svg>`);
    expect(r.strokes).toHaveLength(1);
    expect(r.warnings.some((w) => /text/.test(w))).toBe(true);
    expect(r.warnings.some((w) => /image/.test(w))).toBe(true);
    expect(r.warnings.some((w) => /filter/i.test(w))).toBe(true);
  });
  it('no declared size: bounds from artwork moved to origin', () => {
    const r = imp(`<svg ${NS}><rect x="50" y="60" width="96" height="48"/></svg>`);
    expect(r.widthMm).toBeCloseTo(25.4); expect(r.heightMm).toBeCloseTo(12.7);
    expect(boundsOf(r.strokes)!.minX).toBeCloseTo(0); expect(boundsOf(r.strokes)!.minY).toBeCloseTo(0);
  });
  it('empty svg warns; non-svg throws', () => {
    expect(imp(`<svg ${NS}/>`).warnings.join()).toMatch(/no drawable/);
    expect(() => imp('<html/>')).toThrow();
  });
  it('mm path with preserveAspect letterboxing keeps uniform scale', () => {
    const r = imp(`<svg ${NS} width="100mm" height="100mm" viewBox="0 0 200 100"><path d="M0 0 L200 100"/></svg>`);
    expect(r.strokes[0][1][0]).toBeCloseTo(100);
    expect(r.strokes[0][1][1]).toBeCloseTo(75); // centred vertically: 25 + 50
  });
});
