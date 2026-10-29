import { describe, expect, it } from 'vitest';
import { applyH, fitHomography, invertH } from './homography';
import { detectLines, manualLines, rectify, type Gray } from './lines';

const PPM = 3, WMM = 210, HMM = 297;

function rng(seed: number) {
  let s = seed >>> 0;
  return () => ((s = (Math.imul(s, 1664525) + 1013904223) >>> 0) / 4294967296);
}

interface Opts { spacing?: number; first?: number; angle?: number; noise?: number; rules?: boolean; text?: boolean }

/** Synthetic paper page raster (pixel row j covers y in [j, j+1)/PPM mm). */
function makePage(o: Opts = {}): { g: Gray; truth: number[] } {
  const { spacing = 8, first = 30, angle = 0, noise = 2, rules = true, text = true } = o;
  const w = WMM * PPM, h = HMM * PPM, rnd = rng(7);
  const data = new Float32Array(w * h).fill(255);
  const truth: number[] = [];
  const tan = Math.tan((angle * Math.PI) / 180);
  if (rules) {
    for (let y0 = first; y0 < HMM - 3; y0 += spacing) {
      truth.push(y0);
      for (let i = 0; i < w; i++) {
        const yc = (y0 + ((i + 0.5) / PPM - WMM / 2) * tan) * PPM; // in px
        for (let j = Math.floor(yc - 1.5); j <= Math.ceil(yc + 1.5); j++) {
          if (j < 0 || j >= h) continue;
          const cov = Math.max(0, Math.min(j + 1, yc + 0.45) - Math.max(j, yc - 0.45));
          data[j * w + i] -= cov * 65;
        }
      }
    }
    for (let j = 0; j < h; j++) for (let i = 25 * PPM; i < 25 * PPM + 1; i++) data[j * w + i] = Math.min(data[j * w + i], 160); // margin line
  }
  if (text) {
    for (let n = 0; n < 40; n++) {
      const bx = 30 * PPM + Math.floor(rnd() * 150 * PPM), by = 20 * PPM + Math.floor(rnd() * 250 * PPM);
      const bw = 6 + Math.floor(rnd() * 20), bh = 3 + Math.floor(rnd() * 12);
      for (let j = by; j < by + bh; j++) for (let i = bx; i < bx + bw; i++) data[j * w + i] = 70 + rnd() * 40;
    }
    // a handwritten-ish word band running along a rule
    for (let i = 40 * PPM; i < 90 * PPM; i++) for (let j = 0; j < 9; j++) if (((i >> 2) + j) % 3) data[(31 * PPM - 4 + j) * w + i] = 90;
  }
  for (let k = 0; k < data.length; k++) data[k] = Math.max(0, Math.min(255, data[k] + (rnd() + rnd() + rnd() - 1.5) * 2 * noise));
  return { g: { width: w, height: h, data }, truth };
}

function expectMatch(res: ReturnType<typeof detectLines>, truth: number[], tol: number) {
  expect(res.found).toBe(true);
  const usable = truth.filter((y) => y > HMM * 0.05 && y < HMM * 0.95);
  expect(res.baselinesMm.length).toBeGreaterThanOrEqual(Math.floor(usable.length * 0.9));
  for (const y of res.baselinesMm) {
    const e = Math.min(...truth.map((t) => Math.abs(t - y)));
    expect(e).toBeLessThan(tol);
  }
  expect(res.baselinesMm).toEqual([...res.baselinesMm].sort((a, b) => a - b));
}

describe('detectLines', () => {
  it('finds 8 mm rules with margin line and text blobs', () => {
    const { g, truth } = makePage();
    const t0 = performance.now();
    const res = detectLines(g, PPM);
    const dt = performance.now() - t0;
    expectMatch(res, truth, 0.3);
    expect(Math.abs(res.spacingMm - 8)).toBeLessThan(0.1);
    expect(res.confidence).toBeGreaterThan(0.8);
    expect(dt).toBeLessThan(250);
  });

  it('works through a perspective photo + rectify', () => {
    const { g, truth } = makePage();
    const pageToImg = fitHomography([[0, 0], [WMM, 0], [WMM, HMM], [0, HMM]], [[400, 20], [880, 35], [900, 700], [380, 690]])!;
    const imgToPage = invertH(pageToImg)!;
    const W = 1280, H = 720, img = new Uint8Array(W * H);
    for (let y = 0; y < H; y++) for (let x = 0; x < W; x++) {
      const p = applyH(imgToPage, [x, y])!;
      const u = p[0] * PPM - 0.5, v = p[1] * PPM - 0.5;
      if (u < 0 || v < 0 || u > g.width - 1 || v > g.height - 1) { img[y * W + x] = 110; continue; }
      const x0 = Math.floor(u), y0 = Math.floor(v), fx = u - x0, fy = v - y0, i = y0 * g.width + x0, D = g.data;
      img[y * W + x] = D[i] * (1 - fx) * (1 - fy) + D[i + 1] * fx * (1 - fy) + D[i + g.width] * (1 - fx) * fy + D[i + g.width + 1] * fx * fy;
    }
    const page = rectify({ width: W, height: H, data: img }, pageToImg, WMM, HMM, PPM);
    expect(page.width).toBe(630);
    expectMatch(detectLines(page, PPM), truth, 0.5);
  });

  it('returns found=false for blank noisy paper', () => {
    for (const noise of [2, 6]) {
      const { g } = makePage({ rules: false, text: false, noise });
      const res = detectLines(g, PPM);
      expect(res.found).toBe(false);
      expect(res.baselinesMm).toEqual([]);
      expect(res.reason).toBeTruthy();
    }
  });

  it('tolerates 1.5 degrees of residual rotation (both directions)', () => {
    for (const angle of [1.5, -1.5]) {
      const { g, truth } = makePage({ angle });
      expectMatch(detectLines(g, PPM), truth, 0.5);
    }
  });

  it('handles narrow-ruled 6.5 mm paper', () => {
    const { g, truth } = makePage({ spacing: 6.5, first: 22 });
    const res = detectLines(g, PPM);
    expectMatch(res, truth, 0.3);
    expect(Math.abs(res.spacingMm - 6.5)).toBeLessThan(0.1);
  });
});

describe('manualLines', () => {
  it('lists baselines inside the page', () => {
    const l = manualLines(210, 297, 8, 30);
    expect(l[0]).toBe(30);
    expect(l[l.length - 1]).toBeLessThanOrEqual(297);
    expect(l.length).toBe(34);
    expect(manualLines(210, 297, 8, -3)[0]).toBe(5);
    expect(manualLines(210, 297, 0, 5)).toEqual([]);
  });
});
