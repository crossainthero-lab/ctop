import { describe, expect, it } from 'vitest';
import { applyH, fitHomography, invertH } from './homography';

describe('homography', () => {
  const src: [number, number][] = [[0, 0], [210, 0], [210, 297], [0, 297]];
  const dst: [number, number][] = [[400, 20], [880, 35], [900, 700], [380, 690]];

  it('fits 4 points exactly', () => {
    const h = fitHomography(src, dst)!;
    expect(h).not.toBeNull();
    src.forEach((p, i) => {
      const q = applyH(h, p)!;
      expect(q[0]).toBeCloseTo(dst[i][0], 6);
      expect(q[1]).toBeCloseTo(dst[i][1], 6);
    });
  });

  it('inverts (round trip)', () => {
    const h = fitHomography(src, dst)!;
    const hi = invertH(h)!;
    for (const p of [[10, 20], [105, 150], [200, 290]] as [number, number][]) {
      const r = applyH(hi, applyH(h, p)!)!;
      expect(r[0]).toBeCloseTo(p[0], 6);
      expect(r[1]).toBeCloseTo(p[1], 6);
    }
  });

  it('least squares over many points recovers a known homography', () => {
    const truth = [1.2, 0.1, 30, -0.05, 0.9, 12, 0.0004, 0.0002, 1];
    const pts: [number, number][] = [];
    for (let i = 0; i < 6; i++) for (let j = 0; j < 5; j++) pts.push([i * 40, j * 50]);
    const out = pts.map((p) => applyH(truth, p)!);
    const h = fitHomography(pts, out)!;
    truth.forEach((v, i) => expect(h[i]).toBeCloseTo(v, 5));
  });

  it('rejects degenerate input', () => {
    expect(fitHomography(src.slice(0, 3), dst.slice(0, 3))).toBeNull();
    expect(invertH([1, 2, 3, 2, 4, 6, 0, 0, 1])).toBeNull();
  });
});
