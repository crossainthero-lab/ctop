import type { Stroke } from '../types';
import { compose, applyStrokes } from '../geometry';

export type Align = 'left' | 'center' | 'right';

export interface TextOpts {
  fontId: string;
  /** Cap height of the text in mm (height of a capital letter, e.g. "H"). */
  sizeMm: number;
  /** Extra space added after every glyph, mm. */
  letterSpacingMm?: number;
  /** Multiplier on the base line pitch (1.5 x cap height). 1 = normal. */
  lineSpacing?: number;
  align?: Align;
  /** Wrap width in mm. Words longer than this are hard-wrapped. */
  maxWidthMm?: number;
  /** Rotation in degrees (clockwise as seen on the sheet), about the text's top-left origin. */
  rotationDeg?: number;
}

export interface TextLayout {
  strokes: Stroke[];
  /** Unrotated block size in mm. Origin is top-left; first line's cap-top is at y = 0. */
  widthMm: number;
  heightMm: number;
  /** Distinct characters that the font cannot draw (skipped). */
  missing: string[];
}

/** What a font backend must provide for the shared line-layout engine. */
export interface FontMetrics {
  /** Advance in mm for the char at the given size, or null if unsupported. */
  advance(ch: string): number | null;
  /** Strokes for ch with its origin at (x, baselineY), y down. */
  glyph(ch: string, x: number, baselineY: number): Stroke[];
}

export const LINE_PITCH = 1.5; // line pitch / cap height at lineSpacing = 1

/** Break a paragraph into lines no wider than maxWidth (Infinity = no wrapping). */
export function wrapParagraph(par: string, width: (s: string) => number, maxWidth: number): string[] {
  if (!isFinite(maxWidth) || width(par) <= maxWidth) return [par];
  const lines: string[] = [];
  let cur = '';
  const flush = () => { lines.push(cur); cur = ''; };
  for (const word of par.split(/(?<= )/)) { // keep trailing space attached
    if (cur && width((cur + word).trimEnd()) > maxWidth) flush();
    if (width(word.trimEnd()) > maxWidth) { // hard wrap an overlong word
      for (const ch of word) {
        if (cur && width((cur + ch).trimEnd()) > maxWidth) flush();
        cur += ch;
      }
    } else cur += word;
  }
  flush();
  return lines.map((l) => l.trimEnd());
}

export function layoutWithMetrics(text: string, opts: TextOpts, fm: FontMetrics): TextLayout {
  const { sizeMm, letterSpacingMm = 0, lineSpacing = 1, align = 'left', maxWidthMm, rotationDeg = 0 } = opts;
  const missing = new Set<string>();
  const adv = (ch: string) => {
    const a = fm.advance(ch);
    if (a === null) { missing.add(ch); return 0; }
    return a + letterSpacingMm;
  };
  // width excludes the trailing letter-spacing so alignment is optically exact
  const width = (s: string) => {
    let w = 0, n = 0;
    for (const ch of s) { w += adv(ch); n++; }
    return n ? w - letterSpacingMm : 0;
  };
  const limit = maxWidthMm && maxWidthMm > 0 ? maxWidthMm : Infinity;
  const lines = text.replace(/\r\n?/g, '\n').split('\n').flatMap((p) => wrapParagraph(p, width, limit));
  const widths = lines.map(width);
  const boxW = isFinite(limit) ? limit : Math.max(0, ...widths);
  const pitch = sizeMm * LINE_PITCH * lineSpacing;
  const strokes: Stroke[] = [];
  lines.forEach((line, i) => {
    const baseline = sizeMm + i * pitch;
    let x = align === 'center' ? (boxW - widths[i]) / 2 : align === 'right' ? boxW - widths[i] : 0;
    for (const ch of line) {
      const a = fm.advance(ch);
      if (a !== null) strokes.push(...fm.glyph(ch, x, baseline));
      x += adv(ch);
    }
  });
  const heightMm = sizeMm + (lines.length - 1) * pitch;
  const out = rotationDeg ? applyStrokes(compose({ rotationDeg }), strokes) : strokes;
  return { strokes: out, widthMm: boxW, heightMm, missing: [...missing].filter((c) => c.trim() !== '' || c !== ' ') };
}
