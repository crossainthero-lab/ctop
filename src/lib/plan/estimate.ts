// Time estimate for a stroke list. Mirrors src-tauri/src/writer/fops.rs `stroke_seconds` so the number shown before
// drawing and the one shown while drawing agree.
import type { Pt, Stroke } from '../types';

export const DRAW_FEED = 1000; // mm/min
export const TRAVEL_FEED = 3000; // mm/min
export const STROKE_OVERHEAD_S = 1.8; // pen down + up per stroke

export interface PlanStats { strokes: number; drawMm: number; travelMm: number; seconds: number }

export function planStats(strokes: Stroke[], feed = DRAW_FEED): PlanStats {
  let drawMm = 0, travelMm = 0, cur: Pt | null = null;
  for (const s of strokes) {
    for (let i = 1; i < s.length; i++) drawMm += Math.hypot(s[i][0] - s[i - 1][0], s[i][1] - s[i - 1][1]);
    if (cur) travelMm += Math.hypot(s[0][0] - cur[0], s[0][1] - cur[1]);
    cur = s[s.length - 1];
  }
  return { strokes: strokes.length, drawMm, travelMm, seconds: (drawMm / feed) * 60 + (travelMm / TRAVEL_FEED) * 60 + strokes.length * STROKE_OVERHEAD_S };
}

export function formatDuration(seconds: number | null | undefined): string {
  if (seconds == null || !isFinite(seconds)) return '';
  const s = Math.max(0, Math.round(seconds));
  if (s < 45) return 'under a minute';
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  return `${h} h ${String(m % 60).padStart(2, '0')} min`;
}
