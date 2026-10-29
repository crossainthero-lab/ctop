import type { Bounds, Pt, Stroke } from './types';

/** 2D affine matrix [a b c d e f]: x' = a x + c y + e, y' = b x + d y + f */
export type Matrix = [number, number, number, number, number, number];

export const IDENTITY: Matrix = [1, 0, 0, 1, 0, 0];

/** Returns m1 * m2 (apply m2 first, then m1). */
export function multiply(m1: Matrix, m2: Matrix): Matrix {
  const [a1, b1, c1, d1, e1, f1] = m1;
  const [a2, b2, c2, d2, e2, f2] = m2;
  return [
    a1 * a2 + c1 * b2, b1 * a2 + d1 * b2,
    a1 * c2 + c1 * d2, b1 * c2 + d1 * d2,
    a1 * e2 + c1 * f2 + e1, b1 * e2 + d1 * f2 + f1,
  ];
}

export const translate = (tx: number, ty: number): Matrix => [1, 0, 0, 1, tx, ty];
export const scale = (sx: number, sy = sx): Matrix => [sx, 0, 0, sy, 0, 0];
export function rotate(deg: number): Matrix {
  const r = (deg * Math.PI) / 180, c = Math.cos(r), s = Math.sin(r);
  return [c, s, -s, c, 0, 0];
}

/** Scale, then rotate (degrees, clockwise on screen with y down), both about (cx,cy), then translate by (tx,ty). */
export function compose(o: { tx?: number; ty?: number; rotationDeg?: number; sx?: number; sy?: number; cx?: number; cy?: number }): Matrix {
  const { tx = 0, ty = 0, rotationDeg = 0, sx = 1, sy = sx, cx = 0, cy = 0 } = o;
  let m = translate(-cx, -cy);
  m = multiply(scale(sx, sy), m);
  m = multiply(rotate(rotationDeg), m);
  m = multiply(translate(cx + tx, cy + ty), m);
  return m;
}

export const applyPt = (m: Matrix, [x, y]: Pt): Pt => [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];
export const applyStroke = (m: Matrix, s: Stroke): Stroke => s.map((p) => applyPt(m, p));
export const applyStrokes = (m: Matrix, ss: Stroke[]): Stroke[] => ss.map((s) => applyStroke(m, s));

export function invert(m: Matrix): Matrix {
  const [a, b, c, d, e, f] = m;
  const det = a * d - b * c;
  if (Math.abs(det) < 1e-12) throw new Error('singular matrix');
  return [d / det, -b / det, -c / det, a / det, (c * f - d * e) / det, (b * e - a * f) / det];
}

export function strokeBounds(s: Stroke): Bounds | null {
  if (!s.length) return null;
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const [x, y] of s) {
    if (x < minX) minX = x;
    if (x > maxX) maxX = x;
    if (y < minY) minY = y;
    if (y > maxY) maxY = y;
  }
  return { minX, minY, maxX, maxY };
}

export function boundsOf(ss: Stroke[]): Bounds | null {
  let r: Bounds | null = null;
  for (const s of ss) {
    const b = strokeBounds(s);
    if (!b) continue;
    r = r ? { minX: Math.min(r.minX, b.minX), minY: Math.min(r.minY, b.minY), maxX: Math.max(r.maxX, b.maxX), maxY: Math.max(r.maxY, b.maxY) } : b;
  }
  return r;
}

export function strokeLength(s: Stroke): number {
  let l = 0;
  for (let i = 1; i < s.length; i++) l += Math.hypot(s[i][0] - s[i - 1][0], s[i][1] - s[i - 1][1]);
  return l;
}

export const totalLength = (ss: Stroke[]): number => ss.reduce((a, s) => a + strokeLength(s), 0);

function segDist(p: Pt, a: Pt, b: Pt): number {
  const dx = b[0] - a[0], dy = b[1] - a[1], l2 = dx * dx + dy * dy;
  if (l2 === 0) return Math.hypot(p[0] - a[0], p[1] - a[1]);
  const t = Math.max(0, Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2));
  return Math.hypot(p[0] - (a[0] + t * dx), p[1] - (a[1] + t * dy));
}

/** Douglas-Peucker (iterative). Endpoints always kept; closed loops stay closed. */
export function simplify(s: Stroke, tolerance: number): Stroke {
  if (s.length < 3 || tolerance <= 0) return s.slice();
  const keep = new Uint8Array(s.length);
  keep[0] = keep[s.length - 1] = 1;
  const stack: [number, number][] = [[0, s.length - 1]];
  while (stack.length) {
    const [i, j] = stack.pop()!;
    let max = -1, idx = -1;
    for (let k = i + 1; k < j; k++) {
      const d = segDist(s[k], s[i], s[j]);
      if (d > max) { max = d; idx = k; }
    }
    if (max > tolerance && idx > 0) {
      keep[idx] = 1;
      stack.push([i, idx], [idx, j]);
    }
  }
  return s.filter((_, i) => keep[i]);
}

export const simplifyAll = (ss: Stroke[], tol: number): Stroke[] => ss.map((s) => simplify(s, tol));

/** Drop strokes whose length is below minLen (and degenerate strokes). */
export const dropShort = (ss: Stroke[], minLen: number): Stroke[] =>
  ss.filter((s) => s.length >= 2 && strokeLength(s) >= minLen && strokeLength(s) > 0);

const dist = (a: Pt, b: Pt) => Math.hypot(a[0] - b[0], a[1] - b[1]);

/** Merge strokes whose end/start (either orientation) are within gap. Greedy, deterministic. (grid-indexed endpoints) */
export function joinNearby(ss: Stroke[], gap: number): Stroke[] {
  const pool = ss.filter((s) => s.length).map((s) => s.slice());
  const out: Stroke[] = [];
  const used = new Array(pool.length).fill(false);
  const cell = Math.max(gap, 1e-9);
  const key = (x: number, y: number) => Math.floor(x / cell) * 73856093 ^ Math.floor(y / cell) * 19349663;
  // cell -> [stroke index, 0 = start / 1 = end]
  const grid = new Map<number, [number, number][]>();
  const put = (x: number, y: number, j: number, e: number) => { const k = key(x, y), l = grid.get(k); if (l) l.push([j, e]); else grid.set(k, [[j, e]]); };
  pool.forEach((o, j) => { put(o[0][0], o[0][1], j, 0); put(o[o.length - 1][0], o[o.length - 1][1], j, 1); });
  const near = (p: Pt, f: (j: number, e: number, d: number) => void) => {
    const cx = Math.floor(p[0] / cell), cy = Math.floor(p[1] / cell);
    for (let dx = -1; dx <= 1; dx++) for (let dy = -1; dy <= 1; dy++) {
      const l = grid.get((cx + dx) * 73856093 ^ (cy + dy) * 19349663);
      if (!l) continue;
      for (const [j, e] of l) {
        if (used[j]) continue;
        const q = e === 0 ? pool[j][0] : pool[j][pool[j].length - 1];
        f(j, e, dist(p, q));
      }
    }
  };
  for (let i = 0; i < pool.length; i++) {
    if (used[i]) continue;
    used[i] = true;
    let cur = pool[i];
    for (;;) {
      // mode: 0 end->start,1 end->end,2 start->end,3 start->start; ties: lowest j, then lowest mode
      let best = -1, bestD = gap, mode = 0;
      const consider = (j: number, m: number, d: number) => {
        if (d > gap) return;
        if (best < 0 || d < bestD || (d === bestD && (j < best || (j === best && m < mode)))) { best = j; bestD = d; mode = m; }
      };
      near(cur[cur.length - 1], (j, e, d) => consider(j, e === 0 ? 0 : 1, d));
      near(cur[0], (j, e, d) => consider(j, e === 1 ? 2 : 3, d));
      if (best < 0) break;
      used[best] = true;
      const o = pool[best];
      if (mode === 0) cur = cur.concat(o);
      else if (mode === 1) cur = cur.concat(o.slice().reverse());
      else if (mode === 2) cur = o.concat(cur);
      else cur = o.slice().reverse().concat(cur);
    }
    out.push(cur);
  }
  return out;
}

/** Resample a polyline to points spaced at most `step` apart (keeps original vertices). */
export function resample(s: Stroke, step: number): Stroke {
  if (s.length < 2 || step <= 0) return s.slice();
  const out: Stroke = [s[0]];
  for (let i = 1; i < s.length; i++) {
    const a = s[i - 1], b = s[i], n = Math.ceil(dist(a, b) / step);
    for (let k = 1; k < n; k++) out.push([a[0] + ((b[0] - a[0]) * k) / n, a[1] + ((b[1] - a[1]) * k) / n]);
    out.push(b);
  }
  return out;
}

/** Liang-Barsky clip of a segment; returns clipped [p0,p1] or null. */
function clipSeg(a: Pt, b: Pt, r: Bounds): [Pt, Pt, number, number] | null {
  const dx = b[0] - a[0], dy = b[1] - a[1];
  let t0 = 0, t1 = 1;
  const p = [-dx, dx, -dy, dy];
  const q = [a[0] - r.minX, r.maxX - a[0], a[1] - r.minY, r.maxY - a[1]];
  for (let i = 0; i < 4; i++) {
    if (p[i] === 0) { if (q[i] < 0) return null; continue; }
    const t = q[i] / p[i];
    if (p[i] < 0) { if (t > t1) return null; if (t > t0) t0 = t; }
    else { if (t < t0) return null; if (t < t1) t1 = t; }
  }
  return [[a[0] + t0 * dx, a[1] + t0 * dy], [a[0] + t1 * dx, a[1] + t1 * dy], t0, t1];
}

/** Clip strokes to a rectangle, splitting strokes where they leave and re-enter it. */
export function clipStrokesToRect(ss: Stroke[], r: Bounds): Stroke[] {
  const out: Stroke[] = [];
  for (const s of ss) {
    let cur: Stroke | null = null;
    for (let i = 1; i < s.length; i++) {
      const c = clipSeg(s[i - 1], s[i], r);
      if (!c) { if (cur && cur.length > 1) out.push(cur); cur = null; continue; }
      const [p0, p1, t0, t1] = c;
      if (!cur || t0 > 0) { if (cur && cur.length > 1) out.push(cur); cur = [p0]; }
      cur.push(p1);
      if (t1 < 1) { if (cur.length > 1) out.push(cur); cur = null; }
    }
    if (cur && cur.length > 1) out.push(cur);
  }
  return out;
}
