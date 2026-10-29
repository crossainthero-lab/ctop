// Deterministic ruled-line detection for a rectified photo of lined paper. Pure, dependency-free.
import type { H } from './homography';

export interface Gray {
  width: number;
  height: number;
  data: Uint8Array | Uint8ClampedArray | Float32Array;
}
export interface LineResult {
  found: boolean;
  confidence: number;
  spacingMm: number;
  /** y from the paper TOP in mm, ascending, centre of each rule at the page's horizontal centre */
  baselinesMm: number[];
  reason?: string;
}

export function rgbaToGray(rgba: ArrayLike<number>, w: number, h: number): Gray {
  const data = new Uint8Array(w * h);
  for (let i = 0; i < w * h; i++) data[i] = 0.299 * rgba[i * 4] + 0.587 * rgba[i * 4 + 1] + 0.114 * rgba[i * 4 + 2] + 0.5;
  return { width: w, height: h, data };
}

/** Bilinear sample, pixel centres at integer coords; outside -> `out`. */
function bil(d: ArrayLike<number>, w: number, h: number, x: number, y: number, out: number): number {
  if (x < 0 || y < 0 || x > w - 1 || y > h - 1) return out;
  const x0 = x | 0, y0 = y | 0, x1 = x0 + 1 < w ? x0 + 1 : x0, y1 = y0 + 1 < h ? y0 + 1 : y0;
  const fx = x - x0, fy = y - y0;
  const a = d[y0 * w + x0], b = d[y0 * w + x1], c = d[y1 * w + x0], e = d[y1 * w + x1];
  return a + (b - a) * fx + (c - a) * fy + (a - b - c + e) * fx * fy;
}

/** Inverse-warp the photo into a paper-plane raster (x right, y down from top-left, pxPerMm px per mm). */
export function rectify(gray: Gray, pageToImg: H, widthMm: number, heightMm: number, pxPerMm = 3): Gray {
  const w = Math.round(widthMm * pxPerMm), h = Math.round(heightMm * pxPerMm);
  const out = new Float32Array(w * h);
  const m = pageToImg;
  for (let j = 0; j < h; j++) {
    const y = (j + 0.5) / pxPerMm;
    for (let i = 0; i < w; i++) {
      const x = (i + 0.5) / pxPerMm;
      const q = m[6] * x + m[7] * y + m[8];
      out[j * w + i] = Math.abs(q) < 1e-12 ? 255 : bil(gray.data, gray.width, gray.height, (m[0] * x + m[1] * y + m[2]) / q, (m[3] * x + m[4] * y + m[5]) / q, 255);
    }
  }
  return { width: w, height: h, data: out };
}

export function manualLines(widthMm: number, heightMm: number, spacingMm: number, firstMm: number): number[] {
  void widthMm;
  const out: number[] = [];
  if (!(spacingMm > 0.5) || !Number.isFinite(firstMm)) return out;
  let k = firstMm < 0 ? Math.ceil(-firstMm / spacingMm) : 0;
  for (; out.length < 10000; k++) {
    const y = firstMm + k * spacingMm;
    if (y > heightMm) break;
    out.push(y);
  }
  return out;
}

const MAX_D = 80; // clamp so dark text can't dominate
const THR = 10; // darkness (grey levels below local background) counted as ink

/** Darkness = local background (large box blur) minus pixel, clamped to [0, MAX_D]. */
function darkness(g: Gray, ppm: number): Float32Array {
  const { width: w, height: h, data } = g;
  const R = Math.max(2, Math.round(7.5 * ppm));
  const W1 = w + 1;
  const I = new Float64Array(W1 * (h + 1));
  for (let y = 0; y < h; y++) {
    let row = 0;
    for (let x = 0; x < w; x++) {
      row += data[y * w + x];
      I[(y + 1) * W1 + x + 1] = I[y * W1 + x + 1] + row;
    }
  }
  const d = new Float32Array(w * h);
  for (let y = 0; y < h; y++) {
    const y0 = Math.max(0, y - R), y1 = Math.min(h, y + R + 1);
    for (let x = 0; x < w; x++) {
      const x0 = Math.max(0, x - R), x1 = Math.min(w, x + R + 1);
      const bg = (I[y1 * W1 + x1] - I[y0 * W1 + x1] - I[y1 * W1 + x0] + I[y0 * W1 + x0]) / ((x1 - x0) * (y1 - y0));
      const v = bg - data[y * w + x];
      d[y * w + x] = v < 0 ? 0 : v > MAX_D ? MAX_D : v;
    }
  }
  return d;
}

interface Ctx { d: Float32Array; w: number; h: number; x0: number; x1: number; r0: number; r1: number }

/** Sharpness of the row profile when rows are sheared by slope t (about the page centre). */
function sharpness(c: Ctx, t: number): number {
  const cx = (c.w - 1) / 2;
  let acc = 0;
  for (let r = c.r0; r < c.r1; r++) {
    let s = 0, n = 0;
    for (let x = c.x0; x < c.x1; x += 2, n++) s += bil(c.d, c.w, c.h, x, r + (x - cx) * t, 0);
    acc += (s / n) * (s / n);
  }
  return acc;
}

const fail = (reason: string, confidence = 0): LineResult => ({ found: false, confidence, spacingMm: 0, baselinesMm: [], reason });

export function detectLines(page: Gray, pxPerMm: number): LineResult {
  const ppm = pxPerMm, { width: w, height: h } = page;
  if (w < 20 || h < 20) return fail('The page image is too small.');
  const d = darkness(page, ppm);
  const bx = Math.ceil(w * 0.04), by = Math.ceil(h * 0.04);
  const c: Ctx = { d, w, h, x0: bx, x1: w - bx, r0: by, r1: h - by };
  const cx = (w - 1) / 2;

  // 1. residual skew: coarse then fine shear search, maximise row-profile sharpness
  let best = 0, bestS = -1;
  const tryA = (a: number) => {
    const s = sharpness(c, Math.tan((a * Math.PI) / 180));
    if (s > bestS) { bestS = s; best = a; }
  };
  for (let a = -2; a <= 2.001; a += 0.5) tryA(a);
  const centre = best;
  for (let a = centre - 0.4; a <= centre + 0.401; a += 0.1) if (Math.abs(a - centre) > 1e-9 && Math.abs(a) <= 2.5) tryA(a);
  const t = Math.tan((best * Math.PI) / 180);

  // 2. per-row score from long horizontal runs (gaps up to 2 mm bridged)
  const W = c.x1 - c.x0, gapMax = Math.round(2 * ppm);
  const strongMin = 0.4 * W, weakMin = 0.2 * W;
  const strong = new Float32Array(h), weak = new Float32Array(h), ink = new Float32Array(h);
  for (let r = c.r0; r < c.r1; r++) {
    let start = -1, last = -1, rs = 0;
    const close = () => {
      const span = last - start + 1;
      if (span >= weakMin) { weak[r] += span / W; }
      if (span >= strongMin) { strong[r] += span / W; ink[r] += rs; }
    };
    for (let x = c.x0; x < c.x1; x++) {
      const v = bil(d, w, h, x, r + (x - cx) * t, 0);
      if (v <= THR) continue;
      if (start >= 0 && x - last - 1 > gapMax) { close(); start = -1; rs = 0; }
      if (start < 0) start = x;
      last = x; rs += v;
    }
    if (start >= 0) close();
  }

  // 3. peaks: contiguous strong rows, sub-pixel centre of mass; reject groups too thick to be a rule
  const peaks: number[] = []; // mm
  const maxThick = 2 * ppm;
  for (let r = c.r0; r < c.r1; ) {
    if (strong[r] <= 0) { r++; continue; }
    let e = r, sw = 0, sp = 0;
    while (e < c.r1 && strong[e] > 0) { sw += ink[e]; sp += ink[e] * e; e++; }
    if (e - r <= maxThick && sw > 0) peaks.push((sp / sw + 0.5) / ppm);
    r = e;
  }
  if (peaks.length < 4) return fail('No ruled lines were found on the page.');

  // 4. dominant spacing: triangular-weighted pair votes over 4..15 mm, then prefer a multiple that fits as well
  const vote = (s: number) => {
    let v = 0;
    for (let i = 0; i < peaks.length; i++) for (let j = i + 1; j < peaks.length; j++) {
      const dd = peaks[j] - peaks[i], k = Math.round(dd / s);
      if (k >= 1) v += Math.max(0, 1 - Math.abs(dd - k * s) / 0.25);
    }
    return v;
  };
  const search = (lo: number, hi: number, step: number) => {
    let bs = lo, bv = -1;
    for (let s = lo; s <= hi + 1e-9; s += step) { const v = vote(s); if (v > bv) { bv = v; bs = s; } }
    return { s: bs, v: bv };
  };
  let { s: sp, v: bv } = search(4, 15, 0.05);
  for (let m = 4; m >= 2; m--) {
    if (sp * m > 15.2) continue;
    const c2 = search(Math.max(4, sp * m - 0.3), Math.min(15, sp * m + 0.3), 0.05);
    if (c2.v >= 0.9 * bv) { sp = c2.s; bv = Math.max(bv, c2.v); break; }
  }
  sp = search(Math.max(4, sp - 0.06), Math.min(15, sp + 0.06), 0.005).s;

  // 5. comb: circular-mean phase, then iterative least squares on inliers (|residual| < 15% of spacing)
  let sx = 0, sy = 0;
  for (const p of peaks) { sx += Math.cos((2 * Math.PI * p) / sp); sy += Math.sin((2 * Math.PI * p) / sp); }
  let off = ((Math.atan2(sy, sx) / (2 * Math.PI)) * sp);
  let inl: { k: number; p: number }[] = [];
  for (let it = 0; it < 5; it++) {
    const byK = new Map<number, number>();
    for (const p of peaks) {
      const k = Math.round((p - off) / sp), e = Math.abs(p - off - k * sp);
      if (e < 0.15 * sp && (!byK.has(k) || e < Math.abs(byK.get(k)! - off - k * sp))) byK.set(k, p);
    }
    inl = [...byK].map(([k, p]) => ({ k, p })).sort((a, b) => a.k - b.k);
    if (inl.length < 4) return fail('The ruled lines are not evenly spaced enough to trust.');
    const n = inl.length, mk = inl.reduce((s, q) => s + q.k, 0) / n, mp = inl.reduce((s, q) => s + q.p, 0) / n;
    let num = 0, den = 0;
    for (const q of inl) { num += (q.k - mk) * (q.p - mp); den += (q.k - mk) ** 2; }
    if (den <= 0) return fail('The ruled lines are not evenly spaced enough to trust.');
    sp = num / den; off = mp - sp * mk;
    if (sp < 3.5 || sp > 16) return fail('The line spacing is outside the expected range.');
  }
  const rms = Math.sqrt(inl.reduce((s, q) => s + (q.p - off - q.k * sp) ** 2, 0) / inl.length);
  const regularity = Math.max(0, 1 - (rms / (0.2 * sp)) ** 2);

  // 6. supported lines: inliers, plus comb positions with some evidence (interior, or <= 1 spacing beyond the ends)
  const have = new Set(inl.map((q) => q.k));
  const kmin = inl[0].k, kmax = inl[inl.length - 1].k;
  const evidence = (mm: number) => {
    const row = mm * ppm - 0.5, rad = Math.ceil(1.0 * ppm);
    for (let r = Math.max(0, Math.round(row) - rad); r <= Math.min(h - 1, Math.round(row) + rad); r++) if (weak[r] > 0) return true;
    return false;
  };
  const ys = (k: number) => off + k * sp;
  const inPage = (mm: number) => mm * ppm >= c.r0 && mm * ppm <= c.r1;
  for (let k = kmin - 1; k <= kmax + 1; k++) {
    if (have.has(k) || !inPage(ys(k))) continue;
    if (evidence(ys(k))) have.add(k);
  }
  const ks = [...have].sort((a, b) => a - b);
  let supportedInterior = 0;
  for (const k of ks) if (k >= kmin && k <= kmax) supportedInterior++;
  const confidence = Math.min(1, supportedInterior / (kmax - kmin + 1)) * regularity;
  if (ks.length < 4) return fail('Fewer than four ruled lines could be confirmed.', confidence);
  if (confidence < 0.5) return fail('The ruled lines are too faint or irregular to trust.', confidence);
  return { found: true, confidence, spacingMm: sp, baselinesMm: ks.map(ys) };
}
