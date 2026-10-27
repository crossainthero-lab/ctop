import hershey from 'hersheytext/hersheytext.min.json';
import type { Pt, Stroke } from '../types';
import { layoutWithMetrics, type FontMetrics, type TextLayout, type TextOpts } from './layout';

export type { TextOpts, TextLayout } from './layout';

interface RawGlyph { d: string; o: number }
interface RawFont { name: string; chars: RawGlyph[] }

/** Hershey cap-height is 21 units (cap top y=1, baseline y=22); glyph origin x is 0, advance is 2*o. */
const CAP_UNITS = 21;
const BASELINE_UNITS = 22;
const SPACE_UNITS = 12;

const CURATED: { id: string; name: string }[] = [
  { id: 'futural', name: 'Sans (single stroke)' },
  { id: 'futuram', name: 'Sans bold' },
  { id: 'timesr', name: 'Serif' },
  { id: 'timesrb', name: 'Serif bold' },
  { id: 'timesi', name: 'Serif italic' },
  { id: 'scripts', name: 'Script (single stroke)' },
  { id: 'cursive', name: 'Script (alternate)' },
  { id: 'scriptc', name: 'Script medium' },
  { id: 'gothiceng', name: 'Gothic English' },
];

const fonts = hershey as unknown as Record<string, RawFont>;

export function listStrokeFonts(): { id: string; name: string }[] {
  return CURATED.filter((f) => fonts[f.id]);
}

export const DEFAULT_STROKE_FONT = 'futural';

const parsed = new Map<string, Map<string, { strokes: Stroke[]; advance: number }>>();

function parseGlyph(g: RawGlyph): { strokes: Stroke[]; advance: number } {
  const strokes: Stroke[] = [];
  for (const part of g.d.split('M')) {
    const nums = part.replace(/L/g, ' ').trim().split(/\s+/).filter(Boolean);
    const pts: Pt[] = nums.map((t) => t.split(',').map(Number) as Pt).filter((p) => p.length === 2 && p.every(isFinite));
    if (pts.length === 1) pts.push([pts[0][0], pts[0][1]]); // a dot: zero-length stroke
    if (pts.length >= 2) strokes.push(pts);
  }
  return { strokes, advance: g.o * 2 };
}

function glyphTable(fontId: string) {
  let t = parsed.get(fontId);
  if (!t) {
    const f = fonts[fontId];
    if (!f) throw new Error(`Unknown stroke font: ${fontId}`);
    t = new Map();
    f.chars.forEach((g, i) => t!.set(String.fromCharCode(33 + i), parseGlyph(g)));
    parsed.set(fontId, t);
  }
  return t;
}

export function strokeFontMetrics(fontId: string, sizeMm: number): FontMetrics {
  const table = glyphTable(fontId);
  const k = sizeMm / CAP_UNITS;
  return {
    advance(ch) {
      if (ch === ' ') return SPACE_UNITS * k;
      if (ch === '\t') return SPACE_UNITS * 4 * k;
      const g = table.get(ch);
      return g ? g.advance * k : null;
    },
    glyph(ch, x, baselineY) {
      const g = table.get(ch);
      if (!g) return [];
      return g.strokes.map((s) => s.map(([gx, gy]): Pt => [x + gx * k, baselineY + (gy - BASELINE_UNITS) * k]));
    },
  };
}

/** Lay out text in a single-stroke (Hershey) font. Origin top-left, y down, mm. */
export function layoutText(text: string, opts: TextOpts): TextLayout {
  return layoutWithMetrics(text, opts, strokeFontMetrics(opts.fontId, opts.sizeMm));
}
