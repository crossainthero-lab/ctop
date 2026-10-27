import { contours } from 'd3-contour';
import type { Pt, Stroke } from '../types';
import type { Gray } from './process';
import { gaussianBlur } from './process';

/** Canny edge detector -> 1px-wide edge mask (1 = edge). sensitivity in (0,1]: higher finds more edges. */
export function cannyEdges(g: Gray, sigma: number, sensitivity: number): Uint8Array {
  const b = gaussianBlur(g, sigma), { width: w, height: h } = b, d = b.data;
  const mag = new Float32Array(w * h), dir = new Uint8Array(w * h);
  const at = (x: number, y: number) => d[Math.min(h - 1, Math.max(0, y)) * w + Math.min(w - 1, Math.max(0, x))];
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    const gx = (at(x + 1, y - 1) + 2 * at(x + 1, y) + at(x + 1, y + 1) - at(x - 1, y - 1) - 2 * at(x - 1, y) - at(x - 1, y + 1)) / 4;
    const gy = (at(x - 1, y + 1) + 2 * at(x, y + 1) + at(x + 1, y + 1) - at(x - 1, y - 1) - 2 * at(x, y - 1) - at(x + 1, y - 1)) / 4;
    mag[y * w + x] = Math.hypot(gx, gy);
    let a = (Math.atan2(gy, gx) * 180) / Math.PI; if (a < 0) a += 180;
    dir[y * w + x] = a < 22.5 || a >= 157.5 ? 0 : a < 67.5 ? 1 : a < 112.5 ? 2 : 3;
  }
  const hi = 0.02 + 0.28 * Math.pow(1 - Math.min(1, Math.max(0, sensitivity)), 1.5), lo = hi * 0.4;
  const nms = new Float32Array(w * h);
  const off: [number, number][] = [[1, 0], [1, 1], [0, 1], [-1, 1]]; // gradient directions 0,45,90,135 (y down)
  for (let y = 1; y < h - 1; y++) for (let x = 1; x < w - 1; x++) {
    const m = mag[y * w + x], [dx, dy] = off[dir[y * w + x]];
    // ties resolved asymmetrically so plateaus thin to a single pixel
    if (m >= mag[(y + dy) * w + x + dx] && m > mag[(y - dy) * w + x - dx]) nms[y * w + x] = m;
  }
  const out = new Uint8Array(w * h), stack: number[] = [];
  for (let i = 0; i < nms.length; i++) if (nms[i] >= hi) { out[i] = 1; stack.push(i); }
  while (stack.length) {
    const i = stack.pop()!, x = i % w, y = (i / w) | 0;
    for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) {
      const nx = x + dx, ny = y + dy;
      if (nx < 0 || ny < 0 || nx >= w || ny >= h) continue;
      const j = ny * w + nx;
      if (!out[j] && nms[j] >= lo) { out[j] = 1; stack.push(j); }
    }
  }
  return out;
}

/** Zhang-Suen thinning of a binary mask (1 = ink). Returns a new mask. */
export function skeletonize(mask: Uint8Array, w: number, h: number): Uint8Array {
  const m = Uint8Array.from(mask);
  const p = (x: number, y: number) => (x < 0 || y < 0 || x >= w || y >= h ? 0 : m[y * w + x]);
  let changed = true;
  while (changed) {
    changed = false;
    for (let pass = 0; pass < 2; pass++) {
      const del: number[] = [];
      for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
        if (!m[y * w + x]) continue;
        const n = [p(x, y - 1), p(x + 1, y - 1), p(x + 1, y), p(x + 1, y + 1), p(x, y + 1), p(x - 1, y + 1), p(x - 1, y), p(x - 1, y - 1)];
        const B = n.reduce((a, b) => a + b, 0);
        if (B < 2 || B > 6) continue;
        let A = 0;
        for (let i = 0; i < 8; i++) if (!n[i] && n[(i + 1) % 8]) A++;
        if (A !== 1) continue;
        if (pass === 0 ? n[0] * n[2] * n[4] === 0 && n[2] * n[4] * n[6] === 0 : n[0] * n[2] * n[6] === 0 && n[0] * n[4] * n[6] === 0) del.push(y * w + x);
      }
      if (del.length) { changed = true; for (const i of del) m[i] = 0; }
    }
  }
  return m;
}

/** Trace a 1px-wide mask into polylines (pixel-centre coordinates). Splits at junctions, closes loops. */
export function traceChains(mask: Uint8Array, w: number, h: number): Stroke[] {
  const on = (x: number, y: number) => x >= 0 && y >= 0 && x < w && y < h && mask[y * w + x] === 1;
  const N4: [number, number][] = [[1, 0], [0, 1], [-1, 0], [0, -1]];
  const N8: [number, number][] = [...N4, [1, 1], [-1, 1], [-1, -1], [1, -1]];
  const nbrs = (i: number): number[] => {
    const x = i % w, y = (i / w) | 0, r: number[] = [];
    for (const [dx, dy] of N8) {
      if (!on(x + dx, y + dy)) continue;
      // drop a diagonal when it is already reachable via an orthogonal 2-step (removes staircase triangles)
      if (dx !== 0 && dy !== 0 && (on(x + dx, y) || on(x, y + dy))) continue;
      r.push((y + dy) * w + x + dx);
    }
    return r;
  };
  const adj = new Map<number, number[]>();
  for (let i = 0; i < mask.length; i++) if (mask[i]) adj.set(i, []);
  for (const i of adj.keys()) for (const j of nbrs(i)) if (adj.has(j) && !adj.get(j)!.includes(i)) { adj.get(i)!.push(j); adj.get(j)!.push(i); }
  const key = (a: number, b: number) => (a < b ? a * mask.length + b : b * mask.length + a);
  const seen = new Set<number>(), out: Stroke[] = [];
  const pt = (i: number): Pt => [(i % w) + 0.5, ((i / w) | 0) + 0.5];
  const walk = (start: number, first: number) => {
    const s: Stroke = [pt(start)];
    let prev = start, cur = first;
    seen.add(key(prev, cur));
    for (;;) {
      s.push(pt(cur));
      const nx = adj.get(cur)!;
      if (nx.length !== 2 || cur === start) break;
      const next = nx[0] === prev ? nx[1] : nx[0];
      if (seen.has(key(cur, next))) break;
      seen.add(key(cur, next));
      prev = cur; cur = next;
    }
    out.push(s);
  };
  const ids = [...adj.keys()].sort((a, b) => a - b);
  for (const i of ids) { // nodes first (endpoints, junctions)
    const nb = adj.get(i)!;
    if (nb.length === 2) continue;
    for (const j of nb) if (!seen.has(key(i, j))) walk(i, j);
  }
  for (const i of ids) for (const j of adj.get(i)!) if (!seen.has(key(i, j))) walk(i, j); // pure loops
  return out;
}

/** Iso-contours of a field (higher = ink) at `level`; sub-pixel accurate closed rings in pixel-index coordinates (pixel centre = +0.5). */
export function isoContours(field: Float32Array, w: number, h: number, level: number): Stroke[] {
  const pad = 1, W = w + 2 * pad, H = h + 2 * pad;
  const padded = new Float64Array(W * H); // zero border so shapes touching the edge still close
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) padded[(y + pad) * W + x + pad] = field[y * w + x];
  const mp = contours().size([W, H]).thresholds([level])(Array.from(padded))[0];
  const out: Stroke[] = [];
  for (const poly of mp?.coordinates ?? []) for (const ring of poly) out.push(ring.map(([x, y]): Pt => [x - pad, y - pad]));
  return out;
}
