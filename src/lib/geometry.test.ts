import { describe, expect, it } from 'vitest';
import { applyPt, boundsOf, clipStrokesToRect, compose, dropShort, invert, joinNearby, resample, simplify, strokeLength } from './geometry';
import type { Stroke } from './types';

describe('geometry', () => {
  it('rotates about a centre', () => {
    const m = compose({ rotationDeg: 90, cx: 10, cy: 10 });
    const [x, y] = applyPt(m, [20, 10]);
    expect(x).toBeCloseTo(10); expect(y).toBeCloseTo(20); // clockwise on screen (y down)
  });
  it('scales about a centre then translates; invert round-trips', () => {
    const m = compose({ sx: 2, cx: 5, cy: 5, tx: 3, ty: -1 });
    expect(applyPt(m, [5, 5])).toEqual([8, 4]);
    const p = applyPt(invert(m), applyPt(m, [7, 9]));
    expect(p[0]).toBeCloseTo(7); expect(p[1]).toBeCloseTo(9);
  });
  it('bounds and length', () => {
    expect(boundsOf([[[0, 0], [3, 4]], [[-1, 2], [1, 8]]])).toEqual({ minX: -1, minY: 0, maxX: 3, maxY: 8 });
    expect(strokeLength([[0, 0], [3, 4], [3, 10]])).toBe(11);
  });
  it('simplify removes collinear points but keeps corners', () => {
    const s: Stroke = [[0, 0], [1, 0], [2, 0], [3, 0], [3, 3]];
    expect(simplify(s, 0.01)).toEqual([[0, 0], [3, 0], [3, 3]]);
  });
  it('dropShort', () => {
    expect(dropShort([[[0, 0], [0.1, 0]], [[0, 0], [5, 0]]], 1)).toHaveLength(1);
  });
  it('joinNearby merges in either orientation', () => {
    const j = joinNearby([[[0, 0], [10, 0]], [[20, 0], [10.2, 0]], [[50, 50], [60, 60]]], 0.5);
    expect(j).toHaveLength(2);
    expect(j.find((s) => s.length === 4)).toEqual([[0, 0], [10, 0], [10.2, 0], [20, 0]]);
  });
  it('resample bounds step size', () => {
    const r = resample([[0, 0], [10, 0]], 2);
    expect(r).toHaveLength(6);
  });
  it('clips and splits strokes', () => {
    const out = clipStrokesToRect([[[-5, 5], [15, 5], [15, 8], [5, 8], [5, 20]]], { minX: 0, minY: 0, maxX: 10, maxY: 10 });
    expect(out).toHaveLength(2);
    expect(out[0]).toEqual([[0, 5], [10, 5]]);
    expect(out[1]).toEqual([[10, 8], [5, 8], [5, 10]]);
    const b = boundsOf(out)!;
    expect(b.minX).toBeGreaterThanOrEqual(0); expect(b.maxX).toBeLessThanOrEqual(10);
  });
});
