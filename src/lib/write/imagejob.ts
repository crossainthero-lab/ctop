// Image / SVG -> pen strokes in PAPER space (mm, origin top-left, +y down), fitted into the writing area and ordered
// to minimise pen-up travel. Deterministic; no network, no LLM.
import type { Pt, RasterImage, Stroke } from '../types';
import { clipStrokesToRect, dropShort, simplify, strokeLength } from '../geometry';
import { convertImage, type ConvertMode } from '../image/convert';
import { importSvg, type SvgImportOptions } from '../svg/import';
import type { Margins } from './pagelayout';

export const MAX_DECODE_EDGE = 4000;
export const MAX_STROKES = 6000;
export const MAX_POINTS = 400_000;
export const MAX_DRAW_MM = 40_000;
export const FEED_DRAW = 1000; // mm/min
export const FEED_TRAVEL = 3000; // mm/min

export type SvgSource = { svgText: string };
export type ImageSource = RasterImage | SvgSource;
export type ImageStyle = 'outline' | 'detail' | 'shade' | 'crosshatch' | ConvertMode;
export interface RawFile { name: string; type: string; bytes: Uint8Array }

export const isSvgSource = (s: ImageSource): s is SvgSource => typeof (s as SvgSource).svgText === 'string';

// ---------------------------------------------------------------- decoding

export type FileKind = 'png' | 'jpeg' | 'webp' | 'svg';

/** Decide the file kind from magic bytes first, then mime type, then extension. null = unsupported. */
export function sniffKind(name: string, type: string, bytes: Uint8Array): FileKind | null {
  const b = bytes;
  if (b.length >= 8 && b[0] === 0x89 && b[1] === 0x50 && b[2] === 0x4e && b[3] === 0x47) return 'png';
  if (b.length >= 3 && b[0] === 0xff && b[1] === 0xd8 && b[2] === 0xff) return 'jpeg';
  if (b.length >= 12 && String.fromCharCode(b[0], b[1], b[2], b[3]) === 'RIFF' && String.fromCharCode(b[8], b[9], b[10], b[11]) === 'WEBP') return 'webp';
  const head = new TextDecoder().decode(b.subarray(0, Math.min(b.length, 2048))).toLowerCase();
  if (head.includes('<svg')) return 'svg';
  const t = type.toLowerCase(), ext = (name.toLowerCase().match(/\.([a-z0-9]+)$/) ?? [])[1] ?? '';
  if (t === 'image/svg+xml' || ext === 'svg') return head.includes('<svg') ? 'svg' : null;
  return null; // magic bytes decide for rasters; a mislabelled file is rejected
}

/** Scale (w,h) so the long edge is at most `max`. */
export function capSize(w: number, h: number, max = MAX_DECODE_EDGE): { width: number; height: number } {
  const k = Math.min(1, max / Math.max(w, h));
  return { width: Math.max(1, Math.round(w * k)), height: Math.max(1, Math.round(h * k)) };
}

/** Composite RGBA over white in place (result is fully opaque). */
export function compositeOnWhite(img: RasterImage): RasterImage {
  const d = img.data;
  for (let i = 0; i < d.length; i += 4) {
    const a = d[i + 3];
    if (a === 255) continue;
    const k = a / 255;
    d[i] = Math.round(d[i] * k + 255 * (1 - k));
    d[i + 1] = Math.round(d[i + 1] * k + 255 * (1 - k));
    d[i + 2] = Math.round(d[i + 2] * k + 255 * (1 - k));
    d[i + 3] = 255;
  }
  return img;
}

export type RasterDecoder = (bytes: Uint8Array, mime: string) => Promise<RasterImage>;

/** Browser raster decoder: createImageBitmap -> canvas (white backdrop) -> pixels, long edge capped. */
export const browserRasterDecoder: RasterDecoder = async (bytes, mime) => {
  const bmp = await createImageBitmap(new Blob([bytes as BlobPart], { type: mime }));
  const { width, height } = capSize(bmp.width, bmp.height);
  const cv: OffscreenCanvas | HTMLCanvasElement = typeof OffscreenCanvas !== 'undefined' ? new OffscreenCanvas(width, height) : Object.assign(document.createElement('canvas'), { width, height });
  const ctx = cv.getContext('2d', { willReadFrequently: true }) as CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D;
  ctx.fillStyle = '#fff';
  ctx.fillRect(0, 0, width, height);
  ctx.drawImage(bmp, 0, 0, width, height);
  bmp.close?.();
  const d = ctx.getImageData(0, 0, width, height);
  return compositeOnWhite({ width, height, data: d.data });
};

export async function decodeImageFile(file: File | RawFile, decoder: RasterDecoder = browserRasterDecoder): Promise<ImageSource> {
  const raw: RawFile = (file as RawFile).bytes instanceof Uint8Array ? (file as RawFile) : { name: file.name, type: file.type, bytes: new Uint8Array(await (file as File).arrayBuffer()) };
  if (!raw.bytes.length) throw new Error('The file is empty.');
  const kind = sniffKind(raw.name, raw.type, raw.bytes);
  if (!kind) throw new Error(`Unsupported file "${raw.name}". Use a PNG, JPG, WEBP or SVG image.`);
  if (kind === 'svg') return { svgText: new TextDecoder().decode(raw.bytes) };
  const mime = kind === 'jpeg' ? 'image/jpeg' : `image/${kind}`;
  let img: RasterImage;
  try { img = await decoder(raw.bytes, mime); } catch (e) { throw new Error(`Could not decode "${raw.name}": ${e instanceof Error ? e.message : String(e)}`); }
  return compositeOnWhite(img);
}

// ---------------------------------------------------------------- background cleanup

const median = (a: number[]) => { const s = [...a].sort((x, y) => x - y); return s[s.length >> 1]; };

export interface CleanOpts { /** colour distance (0..441) treated as backdrop */ tolerance?: number }

/**
 * If the image border is (nearly) one colour, flood-fill the backdrop from the border and paint it white so edge/line
 * tracing does not follow the backdrop or its shading. Non-uniform border: returns the input unchanged.
 */
export function cleanBackground(img: RasterImage, o: CleanOpts = {}): RasterImage {
  const { width: w, height: h } = img;
  if (w < 4 || h < 4) return img;
  const src = compositeOnWhite({ width: w, height: h, data: Uint8ClampedArray.from(img.data) });
  const d = src.data;
  const border: number[] = [];
  for (let x = 0; x < w; x++) { border.push(x, (h - 1) * w + x); }
  for (let y = 1; y < h - 1; y++) { border.push(y * w, y * w + w - 1); }
  const med = [0, 1, 2].map((c) => median(border.map((i) => d[i * 4 + c])));
  const dist = (i: number) => Math.hypot(d[i * 4] - med[0], d[i * 4 + 1] - med[1], d[i * 4 + 2] - med[2]);
  const tol = o.tolerance ?? 48;
  const near = border.filter((i) => dist(i) <= tol * 1.25).length / border.length;
  if (near < 0.85) return img; // busy border: not a flat backdrop
  const bg = new Uint8Array(w * h), stack: number[] = [];
  const push = (i: number) => { if (!bg[i] && dist(i) <= tol) { bg[i] = 1; stack.push(i); } };
  for (const i of border) push(i);
  while (stack.length) {
    const i = stack.pop()!, x = i % w, y = (i / w) | 0;
    if (x > 0) push(i - 1);
    if (x < w - 1) push(i + 1);
    if (y > 0) push(i - w);
    if (y < h - 1) push(i + w);
  }
  // grow one pixel so the antialiased fringe of the backdrop does not become an edge
  const grown = Uint8Array.from(bg);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    const i = y * w + x;
    if (bg[i]) continue;
    if ((x > 0 && bg[i - 1] && dist(i) < tol * 1.6) || (x < w - 1 && bg[i + 1] && dist(i) < tol * 1.6) || (y > 0 && bg[i - w] && dist(i) < tol * 1.6) || (y < h - 1 && bg[i + w] && dist(i) < tol * 1.6)) grown[i] = 1;
  }
  for (let i = 0; i < grown.length; i++) if (grown[i]) { d[i * 4] = d[i * 4 + 1] = d[i * 4 + 2] = 255; }
  return src;
}

// ---------------------------------------------------------------- ordering

const d2 = (a: Pt, b: Pt) => (a[0] - b[0]) ** 2 + (a[1] - b[1]) ** 2;

/** Pen-up travel (mm) when drawing `strokes` in order, starting from `start`. */
export function travelLength(strokes: Stroke[], start: Pt = [0, 0]): number {
  let t = 0, cur = start;
  for (const s of strokes) { t += Math.sqrt(d2(cur, s[0])); cur = s[s.length - 1]; }
  return t;
}

/** Greedy nearest-neighbour ordering with stroke reversal. Returns new array; input untouched. */
export function orderStrokes(strokes: Stroke[], start: Pt = [0, 0]): Stroke[] {
  const n = strokes.length;
  const sx = new Float64Array(n), sy = new Float64Array(n), ex = new Float64Array(n), ey = new Float64Array(n);
  strokes.forEach((s, i) => { sx[i] = s[0][0]; sy[i] = s[0][1]; ex[i] = s[s.length - 1][0]; ey[i] = s[s.length - 1][1]; });
  const alive = new Int32Array(n).map((_, i) => i);
  let left = n, cx = start[0], cy = start[1];
  const out: Stroke[] = [];
  while (left > 0) {
    let best = Infinity, bi = 0, rev = false;
    for (let k = 0; k < left; k++) {
      const i = alive[k];
      const a = (sx[i] - cx) ** 2 + (sy[i] - cy) ** 2;
      if (a < best) { best = a; bi = k; rev = false; }
      const b = (ex[i] - cx) ** 2 + (ey[i] - cy) ** 2;
      if (b < best) { best = b; bi = k; rev = true; }
    }
    const i = alive[bi];
    alive[bi] = alive[--left];
    const s = rev ? strokes[i].slice().reverse() : strokes[i];
    out.push(s);
    const e = s[s.length - 1];
    cx = e[0]; cy = e[1];
  }
  return out;
}

// ---------------------------------------------------------------- job

export interface ImageJobArea { widthMm: number; heightMm: number; margins: Margins }
export interface ImageJobOpts {
  style: ImageStyle;
  /** 0..1 */ detail: number;
  /** 0.1..1 fraction of the largest box that fits the writing area (default 1) */ scale?: number;
  /** 0..1 anchor within the free space (default centred) */ offset?: { x: number; y: number };
  area: ImageJobArea;
  invert?: boolean;
  /** default true */ cleanBackground?: boolean;
  /** start position of the pen, paper space (default top-left of the paper) */ start?: Pt;
  /** inject an XML parser for SVG outside the browser */ svgParser?: SvgImportOptions['parser'];
}
export interface JobStats { strokes: number; points: number; drawMm: number; travelMm: number; estMinutes: number }
export interface Placement { xMm: number; yMm: number; widthMm: number; heightMm: number }
export interface ImageJob { strokes: Stroke[]; widthMm: number; heightMm: number; placement: Placement; warnings: string[]; stats: JobStats }

const MODE: Record<string, ConvertMode> = { outline: 'outline', detail: 'lineart', shade: 'hatch', crosshatch: 'crosshatch', lineart: 'lineart', threshold: 'threshold', hatch: 'hatch' };
const clamp = (v: number, a: number, b: number) => Math.min(b, Math.max(a, v));

export function jobStats(strokes: Stroke[], start: Pt = [0, 0]): JobStats {
  let points = 0, drawMm = 0;
  for (const s of strokes) { points += s.length; drawMm += strokeLength(s); }
  const travelMm = travelLength(strokes, start);
  return { strokes: strokes.length, points, drawMm, travelMm, estMinutes: drawMm / FEED_DRAW + travelMm / FEED_TRAVEL };
}

const overLimit = (s: Stroke[]) => {
  let pts = 0, len = 0;
  for (const st of s) { pts += st.length; len += strokeLength(st); }
  return s.length > MAX_STROKES || pts > MAX_POINTS || len > MAX_DRAW_MM;
};

const ESCALATE = { minFeature: [1, 2, 3.5, 5, 8], simplify: [0.1, 0.2, 0.4, 0.6, 1], spacing: [1, 1.3, 1.7, 2.2, 3], detail: [1, 0.8, 0.65, 0.5, 0.4] };

export function imageToStrokes(src: ImageSource, o: ImageJobOpts): ImageJob {
  const warnings: string[] = [];
  const m = o.area.margins;
  const boxW = o.area.widthMm - m.left - m.right, boxH = o.area.heightMm - m.top - m.bottom;
  if (!(boxW > 1 && boxH > 1)) throw new Error('The writing area is too small for an image.');
  const start: Pt = o.start ?? [0, 0];
  const mode = MODE[o.style] ?? 'outline';
  const detail = clamp(o.detail, 0, 1);

  let aspect: number; // width / height
  let svgNative: { strokes: Stroke[]; w: number; h: number } | null = null;
  let raster: RasterImage | null = null;
  if (isSvgSource(src)) {
    const r = importSvg(src.svgText, { parser: o.svgParser });
    warnings.push(...r.warnings);
    if (!r.strokes.length || !(r.widthMm > 0) || !(r.heightMm > 0)) throw new Error('The SVG has no drawable lines.');
    svgNative = { strokes: r.strokes, w: r.widthMm, h: r.heightMm };
    aspect = r.widthMm / r.heightMm;
  } else {
    raster = o.cleanBackground === false ? src : cleanBackground(src);
    aspect = raster.width / raster.height;
  }
  const fit = Math.min(boxW, boxH * aspect); // largest width that fits
  const widthMm = fit * clamp(o.scale ?? 1, 0.1, 1), heightMm = widthMm / aspect;
  const ox = clamp(o.offset?.x ?? 0.5, 0, 1), oy = clamp(o.offset?.y ?? 0.5, 0, 1);
  const x0 = m.left + (boxW - widthMm) * ox, y0 = m.top + (boxH - heightMm) * oy;
  const placement: Placement = { xMm: x0, yMm: y0, widthMm, heightMm };
  const rect = { minX: 0, minY: 0, maxX: widthMm, maxY: heightMm };

  let local: Stroke[] = [];
  let attempt = 0;
  for (; attempt < ESCALATE.minFeature.length; attempt++) {
    const minF = ESCALATE.minFeature[attempt], simp = ESCALATE.simplify[attempt];
    if (svgNative) {
      const k = widthMm / svgNative.w;
      local = svgNative.strokes.map((s) => simplify(s.map(([x, y]): Pt => [x * k, y * k]), simp * 0.5));
      local = dropShort(clipStrokesToRect(local, rect), 0.05 * (attempt + 1));
    } else {
      local = convertImage(raster!, mode, {
        detail: detail * ESCALATE.detail[attempt],
        edgeSensitivity: 0.3 + 0.5 * detail,
        hatchSpacingMm: (2.5 - 1.5 * detail) * ESCALATE.spacing[attempt],
        minFeatureMm: minF, simplifyMm: simp, invert: !!o.invert,
      }, widthMm).strokes;
    }
    if (!overLimit(local)) break;
  }
  if (attempt > 0) warnings.push(overLimit(local) ? 'The drawing is still very complex even after simplifying; consider a smaller image or lower detail.' : 'The image was very detailed, so fine detail was reduced automatically.');
  if (!local.length) warnings.push('No lines were found in this image. Try a different style or more detail.');

  const eps = 1e-6;
  const placed = local
    .map((s) => s.map(([x, y]): Pt => [clamp(x, 0, widthMm) + x0, clamp(y, 0, heightMm) + y0]))
    .filter((s) => s.length >= 2 && strokeLength(s) > eps);
  const ordered = orderStrokes(placed, start);
  const stats = jobStats(ordered, start);
  if (stats.estMinutes > 90) warnings.push(`Estimated drawing time is long (${Math.round(stats.estMinutes)} min).`);
  return { strokes: ordered, widthMm, heightMm, placement, warnings, stats };
}
