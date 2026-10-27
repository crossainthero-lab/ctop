import svgpath from 'svgpath';
import type { Pt, Stroke } from '../types';
import { IDENTITY, applyPt, boundsOf, multiply, scale, translate, type Matrix } from '../geometry';

export interface SvgImport { strokes: Stroke[]; widthMm: number; heightMm: number; warnings: string[] }
export interface SvgImportOptions {
  /** Inject a parser outside the browser (tests use linkedom). Defaults to the global DOMParser. */
  parser?: (text: string) => Document;
  /** Curve flattening tolerance in mm. Default 0.05. */
  toleranceMm?: number;
}

const MM_PER: Record<string, number> = { mm: 1, cm: 10, in: 25.4, pt: 25.4 / 72, pc: 25.4 / 6, px: 25.4 / 96, '': 25.4 / 96 };
/** Parse an SVG length such as "210mm" or "100" (unitless = px). Percentages return null. */
export function parseLength(v: string | null | undefined): number | null {
  const m = /^\s*([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)\s*([a-z%]*)\s*$/i.exec(v ?? '');
  if (!m || m[2] === '%' || !(m[2].toLowerCase() in MM_PER)) return null;
  return parseFloat(m[1]) * MM_PER[m[2].toLowerCase()];
}

export function parseTransform(t: string | null | undefined): Matrix {
  let m = IDENTITY;
  const re = /(matrix|translate|scale|rotate|skewX|skewY)\s*\(([^)]*)\)/g;
  for (let r; (r = re.exec(t ?? ''));) {
    const a = r[2].split(/[\s,]+/).filter(Boolean).map(Number);
    let n: Matrix;
    switch (r[1]) {
      case 'matrix': n = a.length === 6 ? (a as Matrix) : IDENTITY; break;
      case 'translate': n = translate(a[0] ?? 0, a[1] ?? 0); break;
      case 'scale': n = scale(a[0] ?? 1, a[1] ?? a[0] ?? 1); break;
      case 'rotate': {
        const rad = ((a[0] ?? 0) * Math.PI) / 180, c = Math.cos(rad), s = Math.sin(rad);
        n = multiply(translate(a[1] ?? 0, a[2] ?? 0), multiply([c, s, -s, c, 0, 0], translate(-(a[1] ?? 0), -(a[2] ?? 0))));
        break;
      }
      case 'skewX': n = [1, 0, Math.tan(((a[0] ?? 0) * Math.PI) / 180), 1, 0, 0]; break;
      default: n = [1, Math.tan(((a[0] ?? 0) * Math.PI) / 180), 0, 1, 0, 0];
    }
    m = multiply(m, n);
  }
  return m;
}

const attr = (el: Element, name: string) => el.getAttribute(name) ?? el.getAttribute(name.toLowerCase());
const num = (el: Element, name: string, d = 0) => { const v = parseFloat(attr(el, name) ?? ''); return isFinite(v) ? v : d; };
const NON_RENDERING = new Set(['defs', 'clippath', 'mask', 'symbol', 'marker', 'pattern', 'metadata', 'title', 'desc', 'style', 'lineargradient', 'radialgradient', 'filter', 'script', 'namedview', 'sodipodi:namedview']);
const UNSUPPORTED = new Set(['text', 'image', 'foreignobject', 'use', 'switch']);
const SHAPES = new Set(['path', 'line', 'polyline', 'polygon', 'rect', 'circle', 'ellipse']);

function cubicPts(p0: Pt, p1: Pt, p2: Pt, p3: Pt, tol: number, out: Stroke) {
  const dd = Math.max(Math.hypot(p0[0] - 2 * p1[0] + p2[0], p0[1] - 2 * p1[1] + p2[1]), Math.hypot(p1[0] - 2 * p2[0] + p3[0], p1[1] - 2 * p2[1] + p3[1]));
  const n = Math.max(2, Math.min(200, Math.ceil(Math.sqrt((6 * dd) / (8 * tol)))));
  for (let i = 1; i <= n; i++) {
    const t = i / n, u = 1 - t;
    out.push([u * u * u * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t * t * t * p3[0], u * u * u * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t * t * t * p3[1]]);
  }
}

/** Flatten a path `d` after applying matrix m (result in mm). */
function flattenPath(d: string, m: Matrix, tol: number): Stroke[] {
  const out: Stroke[] = [];
  let cur: Stroke = [], start: Pt = [0, 0], last: Pt = [0, 0];
  const end = () => { if (cur.length > 1) out.push(cur); cur = []; };
  svgpath(d).abs().unarc().unshort().matrix(m).iterate((seg) => {
    const c = seg[0] as string, v = seg.slice(1) as number[];
    switch (c) {
      case 'M': end(); last = start = [v[0], v[1]]; cur = [last]; break;
      case 'L': if (!cur.length) cur = [last]; last = [v[0], v[1]]; cur.push(last); break;
      case 'H': case 'V': // svgpath keeps H/V for axis-aligned transforms (absolute after .abs())
        if (!cur.length) cur = [last];
        last = c === 'H' ? [v[0], last[1]] : [last[0], v[0]]; cur.push(last); break;
      case 'C': { if (!cur.length) cur = [last]; const e: Pt = [v[4], v[5]]; cubicPts(last, [v[0], v[1]], [v[2], v[3]], e, tol, cur); last = e; break; }
      case 'Q': {
        if (!cur.length) cur = [last];
        const q: Pt = [v[0], v[1]], e: Pt = [v[2], v[3]];
        cubicPts(last, [last[0] + (2 / 3) * (q[0] - last[0]), last[1] + (2 / 3) * (q[1] - last[1])], [e[0] + (2 / 3) * (q[0] - e[0]), e[1] + (2 / 3) * (q[1] - e[1])], e, tol, cur);
        last = e; break;
      }
      case 'Z': if (cur.length) { cur.push([start[0], start[1]]); end(); } last = start; break;
    }
  });
  end();
  return out;
}

function ellipsePts(cx: number, cy: number, rx: number, ry: number, m: Matrix, tol: number): Stroke {
  const k = Math.sqrt(Math.abs(m[0] * m[3] - m[1] * m[2])), R = Math.max(rx, ry) * k;
  const n = Math.max(12, Math.min(720, Math.ceil(Math.PI / Math.acos(Math.max(0, 1 - tol / Math.max(R, tol * 2))))));
  const s: Stroke = [];
  for (let i = 0; i < n; i++) { const a = (2 * Math.PI * i) / n; s.push(applyPt(m, [cx + rx * Math.cos(a), cy + ry * Math.sin(a)])); }
  s.push(s[0]);
  return s;
}

export function importSvg(svgText: string, opts: SvgImportOptions = {}): SvgImport {
  const tol = opts.toleranceMm ?? 0.05;
  const warnings: string[] = [];
  const parse = opts.parser ?? ((t: string) => new DOMParser().parseFromString(t, 'image/svg+xml'));
  const doc = parse(svgText);
  const root = doc.documentElement;
  if (!root || root.localName?.toLowerCase() !== 'svg') throw new Error('Not an SVG document');

  // root: size and viewBox -> mm
  const vb = (attr(root, 'viewBox') ?? '').split(/[\s,]+/).filter(Boolean).map(Number);
  const hasVB = vb.length === 4 && vb.every(isFinite) && vb[2] > 0 && vb[3] > 0;
  let w = parseLength(attr(root, 'width')), h = parseLength(attr(root, 'height'));
  if (attr(root, 'width')?.includes('%')) w = null;
  if (attr(root, 'height')?.includes('%')) h = null;
  let rootM: Matrix = IDENTITY, sized = false;
  if (hasVB) {
    const [vx, vy, vw, vh] = vb;
    if (w === null && h === null) { w = vw * MM_PER.px; h = vh * MM_PER.px; }
    else if (w === null) w = (h! * vw) / vh;
    else if (h === null) h = (w * vh) / vw;
    const s = Math.min(w! / vw, h! / vh);
    rootM = multiply(translate((w! - vw * s) / 2, (h! - vh * s) / 2), multiply(scale(s), translate(-vx, -vy)));
    sized = true;
  } else {
    rootM = scale(MM_PER.px);
    if (w !== null && h !== null) sized = true;
  }

  const strokes: Stroke[] = [];
  const warned = new Set<string>();
  const warnOnce = (k: string, msg: string) => { if (!warned.has(k)) { warned.add(k); warnings.push(msg); } };

  const hidden = (el: Element) => {
    const style = attr(el, 'style') ?? '';
    return attr(el, 'display') === 'none' || /display\s*:\s*none/.test(style) || attr(el, 'visibility') === 'hidden' || /visibility\s*:\s*hidden/.test(style);
  };

  const visit = (el: Element, parentM: Matrix) => {
    const tag = (el.localName ?? el.tagName).toLowerCase();
    if (NON_RENDERING.has(tag)) {
      if (tag === 'filter') warnOnce('filter', 'SVG filters are not supported and were ignored');
      return;
    }
    if (hidden(el)) return;
    const m = multiply(parentM, parseTransform(attr(el, 'transform')));
    if (UNSUPPORTED.has(tag)) {
      warnOnce(tag, tag === 'text' ? 'SVG <text> is not drawn — convert text to paths, or use a CtoP text object' : `SVG <${tag}> is not supported and was skipped`);
      return;
    }
    if (SHAPES.has(tag)) {
      const fill = attr(el, 'fill') ?? /fill\s*:\s*([^;]+)/.exec(attr(el, 'style') ?? '')?.[1];
      if (fill && fill.trim() !== 'none') warnOnce('fill', 'Filled shapes are drawn as outlines only');
      const before = strokes.length;
      switch (tag) {
        case 'path': strokes.push(...flattenPath(attr(el, 'd') ?? '', m, tol)); break;
        case 'line': strokes.push([applyPt(m, [num(el, 'x1'), num(el, 'y1')]), applyPt(m, [num(el, 'x2'), num(el, 'y2')])]); break;
        case 'polyline': case 'polygon': {
          const n = (attr(el, 'points') ?? '').split(/[\s,]+/).filter(Boolean).map(Number).filter(isFinite);
          const pts: Stroke = [];
          for (let i = 0; i + 1 < n.length; i += 2) pts.push(applyPt(m, [n[i], n[i + 1]]));
          if (tag === 'polygon' && pts.length > 2) pts.push(pts[0]);
          if (pts.length > 1) strokes.push(pts);
          break;
        }
        case 'rect': {
          const x = num(el, 'x'), y = num(el, 'y'), rw = num(el, 'width'), rh = num(el, 'height');
          if (rw <= 0 || rh <= 0) break;
          let rx = Math.min(num(el, 'rx', num(el, 'ry')), rw / 2), ry = Math.min(num(el, 'ry', rx), rh / 2);
          if (attr(el, 'rx') === null && attr(el, 'ry') !== null) rx = ry;
          if (rx <= 0 || ry <= 0) strokes.push([[x, y], [x + rw, y], [x + rw, y + rh], [x, y + rh], [x, y]].map((p) => applyPt(m, p as Pt)));
          else strokes.push(...flattenPath(`M${x + rx},${y} H${x + rw - rx} A${rx},${ry} 0 0 1 ${x + rw},${y + ry} V${y + rh - ry} A${rx},${ry} 0 0 1 ${x + rw - rx},${y + rh} H${x + rx} A${rx},${ry} 0 0 1 ${x},${y + rh - ry} V${y + ry} A${rx},${ry} 0 0 1 ${x + rx},${y} Z`, m, tol));
          break;
        }
        case 'circle': { const r = num(el, 'r'); if (r > 0) strokes.push(ellipsePts(num(el, 'cx'), num(el, 'cy'), r, r, m, tol)); break; }
        case 'ellipse': { const rx = num(el, 'rx'), ry = num(el, 'ry'); if (rx > 0 && ry > 0) strokes.push(ellipsePts(num(el, 'cx'), num(el, 'cy'), rx, ry, m, tol)); break; }
      }
      if (strokes.length === before && tag === 'path' && (attr(el, 'd') ?? '').trim()) warnOnce('emptypath', 'Some paths produced no drawable geometry');
      return;
    }
    if (tag !== 'svg' && tag !== 'g' && tag !== 'a') warnOnce(`el:${tag}`, `Unknown SVG element <${tag}> was skipped`);
    for (const child of Array.from(el.children)) visit(child, m);
  };
  for (const child of Array.from(root.children)) visit(child, rootM);

  if (sized) return { strokes, widthMm: w!, heightMm: h!, warnings };
  // no declared size: use the artwork bounds, moved to the origin
  const b = boundsOf(strokes);
  if (!b) return { strokes, widthMm: 0, heightMm: 0, warnings: [...warnings, 'SVG contains no drawable geometry'] };
  const mv = translate(-b.minX, -b.minY);
  return { strokes: strokes.map((s) => s.map((p) => applyPt(mv, p))), widthMm: b.maxX - b.minX, heightMm: b.maxY - b.minY, warnings };
}
