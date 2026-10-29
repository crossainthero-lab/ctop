// The stored A4 frame calibration, as the backend (src-tauri/src/frame.rs) defines it, plus the same coordinate maths
// so the preview checks exactly what the backend will check.
import type { Pt, Stroke } from './types';

export interface FrameProfile {
  name: string;
  created: number;
  updated: number;
  pageW: number;
  pageH: number;
  /** pen mm of the page's bottom-left corner */
  originPen: [number, number];
  rotation: 0 | 90 | 180 | 270;
  skewDeg: number;
  /** Z where the pen tip first touches the paper (measured) */
  surfaceZ: number;
  /** contact plane slopes (mm of Z per mm), reference point = page centre */
  planeGx?: number;
  planeGy?: number;
  marginMm: number;
  source: 'manual' | 'camera-assisted';
  verified: boolean;
  verifiedAt: number | null;
  pageToPen: number[][];
}

export const A4 = { w: 210, h: 297 };
/** [xmin, xmax, ymin, ymax] in pen mm */
export type Reach = [number, number, number, number];

export function matrixOf(p: Pick<FrameProfile, 'rotation' | 'skewDeg' | 'originPen'>): [[number, number, number], [number, number, number]] {
  const a = ((p.rotation + p.skewDeg) * Math.PI) / 180, c = Math.cos(a), s = Math.sin(a);
  return [[c, -s, p.originPen[0]], [s, c, p.originPen[1]]];
}

/** paper space (mm, origin top-left, +y down) -> pen mm */
export function paperToPen(p: FrameProfile, q: Pt): Pt {
  const m = matrixOf(p), px = q[0], py = p.pageH - q[1];
  return [m[0][0] * px + m[0][1] * py + m[0][2], m[1][0] * px + m[1][1] * py + m[1][2]];
}

/** pen mm -> paper space */
export function penToPaper(p: FrameProfile, w: Pt): Pt {
  const m = matrixOf(p), det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
  const dx = w[0] - m[0][2], dy = w[1] - m[1][2];
  const px = (m[1][1] * dx - m[0][1] * dy) / det, py = (-m[1][0] * dx + m[0][0] * dy) / det;
  return [px, p.pageH - py];
}

export const inReach = (w: Pt, r: Reach, tol = 0.5) => w[0] >= r[0] + tol && w[0] <= r[1] - tol && w[1] >= r[2] + tol && w[1] <= r[3] - tol;

/** The checks the backend applies before it will send anything: safe area (paper minus margin) and the plotter's reach. */
export function checkStrokes(strokes: Stroke[], p: FrameProfile, reach: Reach | null): { offPaper: number; offReach: number } {
  let offPaper = 0, offReach = 0;
  const m = p.marginMm;
  for (const s of strokes) for (const q of s) {
    if (q[0] < m || q[0] > p.pageW - m || q[1] < m || q[1] > p.pageH - m) offPaper++;
    if (reach && !inReach(paperToPen(p, q), reach)) offReach++;
  }
  return { offPaper, offReach };
}

/** Plotter reach drawn on the sheet: its four corners in paper space (a rotated rectangle when the sheet is skewed). */
export function reachOnPaper(p: FrameProfile, r: Reach): Pt[] {
  return ([[r[0], r[2]], [r[1], r[2]], [r[1], r[3]], [r[0], r[3]]] as Pt[]).map((w) => penToPaper(p, w));
}

/** A profile to preview against before any calibration exists: A4 flat, nothing is allowed to be drawn from it. */
export function placeholderFrame(): FrameProfile {
  return { name: '', created: 0, updated: 0, pageW: A4.w, pageH: A4.h, originPen: [0, 0], rotation: 0, skewDeg: 0, surfaceZ: 0, marginMm: 8, source: 'manual', verified: false, verifiedAt: null, pageToPen: [[1, 0, 0], [0, 1, 0]] };
}

/** Minimum inset from each physical sheet edge so that everything stays inside the plotter's reach (bounding box of the reach on the sheet). */
export function reachInsets(p: FrameProfile, r: Reach | null): { left: number; right: number; top: number; bottom: number } {
  const m = p.marginMm;
  if (!r) return { left: m, right: m, top: m, bottom: m };
  const c = reachOnPaper(p, r);
  const xs = c.map((q) => q[0]), ys = c.map((q) => q[1]);
  const pad = 1; // the backend keeps 0.5 mm off the limit
  return {
    left: Math.max(m, Math.min(...xs) + pad), right: Math.max(m, p.pageW - Math.max(...xs) + pad),
    top: Math.max(m, Math.min(...ys) + pad), bottom: Math.max(m, p.pageH - Math.max(...ys) + pad),
  };
}
