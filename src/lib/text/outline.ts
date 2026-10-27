import * as opentype from 'opentype.js';
import type { Pt, Stroke } from '../types';
import { layoutWithMetrics, type FontMetrics, type TextLayout, type TextOpts } from './layout';

export type OutlineFont = opentype.Font;
export type OutlineOpts = Omit<TextOpts, 'fontId'> & { fontId?: string; /** Bezier flattening tolerance in mm (default 0.05). */ toleranceMm?: number };

export function parseFont(buf: ArrayBuffer): OutlineFont {
  return opentype.parse(buf);
}

function flatten(cmds: opentype.PathCommand[], tol: number): Stroke[] {
  const out: Stroke[] = [];
  let cur: Stroke = [];
  let last: Pt = [0, 0], start: Pt = [0, 0];
  const close = () => {
    if (cur.length > 1) {
      const f = cur[0], l = cur[cur.length - 1];
      if (f[0] !== l[0] || f[1] !== l[1]) cur.push([f[0], f[1]]);
      out.push(cur);
    }
    cur = [];
  };
  // segments by chord-error bound: n = ceil(sqrt(dd / (8 tol)))
  const cubic = (p0: Pt, p1: Pt, p2: Pt, p3: Pt) => {
    const dd = Math.max(Math.hypot(p0[0] - 2 * p1[0] + p2[0], p0[1] - 2 * p1[1] + p2[1]), Math.hypot(p1[0] - 2 * p2[0] + p3[0], p1[1] - 2 * p2[1] + p3[1]));
    const n = Math.max(2, Math.min(64, Math.ceil(Math.sqrt((6 * dd) / (8 * tol)))));
    for (let i = 1; i <= n; i++) {
      const t = i / n, u = 1 - t;
      cur.push([
        u * u * u * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t * t * t * p3[0],
        u * u * u * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t * t * t * p3[1],
      ]);
    }
  };
  for (const c of cmds) {
    if (c.type === 'M') { close(); last = start = [c.x, c.y]; cur = [last]; }
    else if (c.type === 'L') { cur.push([c.x, c.y]); last = [c.x, c.y]; }
    else if (c.type === 'C') { const e: Pt = [c.x, c.y]; cubic(last, [c.x1, c.y1], [c.x2, c.y2], e); last = e; }
    else if (c.type === 'Q') {
      const q: Pt = [c.x1, c.y1], e: Pt = [c.x, c.y];
      cubic(last, [last[0] + (2 / 3) * (q[0] - last[0]), last[1] + (2 / 3) * (q[1] - last[1])], [e[0] + (2 / 3) * (q[0] - e[0]), e[1] + (2 / 3) * (q[1] - e[1])], e);
      last = e;
    } else if (c.type === 'Z') { close(); last = start; }
  }
  close();
  return out;
}

/** Outline-font text. sizeMm is the cap height (measured from the font's "H"). Origin top-left, y down. */
export function outlineText(font: OutlineFont, text: string, opts: OutlineOpts): TextLayout {
  const tol = opts.toleranceMm ?? 0.05;
  const hb = font.charToGlyph('H').getBoundingBox();
  const capUnits = hb.y2 - hb.y1 > 0 ? hb.y2 - hb.y1 : font.unitsPerEm * 0.7;
  const k = opts.sizeMm / capUnits; // mm per font unit
  const fm: FontMetrics = {
    advance(ch) {
      const g = font.charToGlyph(ch);
      if (!g || g.index === 0) return ch === ' ' ? (font.charToGlyph(' ').advanceWidth ?? font.unitsPerEm * 0.3) * k : null;
      return (g.advanceWidth ?? 0) * k;
    },
    glyph(ch, x, baselineY) {
      const g = font.charToGlyph(ch);
      // getPath(x, y, fontSize): font units scale = fontSize/unitsPerEm, y already flipped to y-down
      const path = g.getPath(x, baselineY, k * font.unitsPerEm);
      return flatten(path.commands, tol);
    },
  };
  return layoutWithMetrics(text, { fontId: opts.fontId ?? 'outline', ...opts }, fm);
}
