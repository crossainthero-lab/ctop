import type { Bounds, Pt, Stroke } from '../types';
import { clipStrokesToRect } from '../geometry';
import { sample, type Gray } from './process';

/** One family of parallel lines, drawn only where darkness >= level. */
export interface HatchPass { angleDeg: number; spacingMm: number; offsetMm: number; level: number }

/**
 * Hatch a darkness field (1 = darkest). `darkness` is sampled in pixel space and mapped to a
 * widthMm x heightMm sheet. Lines are generated in mm, clipped to the sheet, and split wherever
 * the local darkness falls below the pass level.
 */
export function hatchPasses(darkness: Gray, widthMm: number, heightMm: number, passes: HatchPass[]): Stroke[] {
  const rect: Bounds = { minX: 0, minY: 0, maxX: widthMm, maxY: heightMm };
  const sx = darkness.width / widthMm, sy = darkness.height / heightMm;
  const step = Math.max(0.05, Math.min(widthMm / darkness.width, heightMm / darkness.height) * 0.5);
  const diag = Math.hypot(widthMm, heightMm), cx = widthMm / 2, cy = heightMm / 2;
  const out: Stroke[] = [];
  for (const p of passes) {
    if (p.spacingMm <= 0) continue;
    const a = (p.angleDeg * Math.PI) / 180, dx = Math.cos(a), dy = Math.sin(a), nx = -dy, ny = dx;
    // offsets aligned to a global lattice so passes with different phase interleave exactly
    const kMin = Math.ceil((-diag / 2 - p.offsetMm) / p.spacingMm), kMax = Math.floor((diag / 2 - p.offsetMm) / p.spacingMm);
    for (let k = kMin; k <= kMax; k++) {
      const off = k * p.spacingMm + p.offsetMm;
      const a0: Pt = [cx + nx * off - dx * diag, cy + ny * off - dy * diag];
      const a1: Pt = [cx + nx * off + dx * diag, cy + ny * off + dy * diag];
      for (const seg of clipStrokesToRect([[a0, a1]], rect)) {
        const [s, e] = [seg[0], seg[seg.length - 1]], len = Math.hypot(e[0] - s[0], e[1] - s[1]);
        const n = Math.max(1, Math.ceil(len / step));
        let runStart = -1;
        for (let i = 0; i <= n; i++) {
          const t = i / n, x = s[0] + (e[0] - s[0]) * t, y = s[1] + (e[1] - s[1]) * t;
          const dark = sample(darkness, x * sx - 0.5, y * sy - 0.5) >= p.level;
          if (dark && runStart < 0) runStart = i;
          if ((!dark || i === n) && runStart >= 0) {
            const endI = dark ? i : i - 1;
            if (endI > runStart) {
              const t0 = runStart / n, t1 = endI / n;
              out.push([[s[0] + (e[0] - s[0]) * t0, s[1] + (e[1] - s[1]) * t0], [s[0] + (e[0] - s[0]) * t1, s[1] + (e[1] - s[1]) * t1]]);
            }
            runStart = -1;
          }
        }
      }
    }
  }
  return out;
}

/** Single-direction hatching: density grows with tone (spacing, spacing/2, spacing/4 effective). */
export function hatchTonePasses(angleDeg: number, spacingMm: number): HatchPass[] {
  return [
    { angleDeg, spacingMm, offsetMm: 0, level: 0.2 },
    { angleDeg, spacingMm, offsetMm: spacingMm / 2, level: 0.5 },
    { angleDeg, spacingMm: spacingMm / 2, offsetMm: spacingMm / 4, level: 0.75 },
  ];
}

/** Crosshatching: second and further directions join as the tone darkens. */
export function crosshatchTonePasses(angleDeg: number, spacingMm: number): HatchPass[] {
  return [
    { angleDeg, spacingMm, offsetMm: 0, level: 0.2 },
    { angleDeg: angleDeg + 90, spacingMm, offsetMm: 0, level: 0.4 },
    { angleDeg: angleDeg + 45, spacingMm: spacingMm * 1.2, offsetMm: 0, level: 0.62 },
    { angleDeg: angleDeg + 135, spacingMm: spacingMm * 1.2, offsetMm: 0, level: 0.82 },
  ];
}
