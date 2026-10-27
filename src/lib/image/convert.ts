import type { RasterImage, Stroke } from '../types';
import { clipStrokesToRect, dropShort, joinNearby, simplify } from '../geometry';
import { adjust, gaussianBlur, resize, toGray, type Gray } from './process';
import { cannyEdges, isoContours, skeletonize, traceChains } from './trace';
import { crosshatchTonePasses, hatchPasses, hatchTonePasses } from './hatch';

export type ConvertMode = 'outline' | 'lineart' | 'threshold' | 'hatch' | 'crosshatch';

export interface ConvertParams {
  /** -1..1 */ brightness: number;
  /** -1..1 */ contrast: number;
  /** 0..1: luminance below this counts as ink (lineart / threshold) */ threshold: number;
  /** 0..1: higher detects more edges (outline) */ edgeSensitivity: number;
  hatchAngleDeg: number;
  hatchSpacingMm: number;
  /** 0..1: processing resolution & smoothing; higher keeps finer detail */ detail: number;
  /** strokes shorter than this (mm) are dropped */ minFeatureMm: number;
  /** Douglas-Peucker tolerance (mm) */ simplifyMm: number;
  /** swap ink and paper (draw the light areas) */ invert?: boolean;
}

export const DEFAULT_CONVERT_PARAMS: ConvertParams = {
  brightness: 0, contrast: 0, threshold: 0.5, edgeSensitivity: 0.5, hatchAngleDeg: 45,
  hatchSpacingMm: 1.5, detail: 0.5, minFeatureMm: 1, simplifyMm: 0.1, invert: false,
};

export interface ConvertResult { strokes: Stroke[]; widthMm: number; heightMm: number }

/** Processing long-edge in pixels for a detail level (never upsamples beyond the source). */
export const processingSize = (detail: number) => Math.round(120 + 880 * Math.min(1, Math.max(0, detail)));

export function convertImage(img: RasterImage, mode: ConvertMode, params: Partial<ConvertParams>, outputWidthMm: number): ConvertResult {
  const p: ConvertParams = { ...DEFAULT_CONVERT_PARAMS, ...params };
  const src = toGray(img);
  const widthMm = outputWidthMm, heightMm = (outputWidthMm * src.height) / src.width;
  const long = Math.max(src.width, src.height), target = Math.min(long, processingSize(p.detail));
  const k = target / long;
  let g: Gray = resize(src, src.width * k, src.height * k);
  g = adjust(g, p.brightness, p.contrast);
  const sx = widthMm / g.width, sy = heightMm / g.height; // mm per processing pixel
  // darkness: 1 = ink
  const dark: Gray = { ...g, data: g.data.map((v) => (p.invert ? v : 1 - v)) };
  const sigma = 0.6 + (1 - p.detail) * 1.6;
  let px: Stroke[];

  if (mode === 'threshold' || mode === 'lineart') {
    const field = gaussianBlur(dark, sigma * 0.5);
    const level = 1 - p.threshold;
    if (mode === 'threshold') px = isoContours(field.data, g.width, g.height, level);
    else {
      const mask = new Uint8Array(g.width * g.height);
      for (let i = 0; i < mask.length; i++) mask[i] = field.data[i] >= level ? 1 : 0;
      px = traceChains(skeletonize(mask, g.width, g.height), g.width, g.height);
    }
  } else if (mode === 'outline') {
    const edgeSrc: Gray = p.invert ? { ...g, data: g.data.map((v) => 1 - v) } : g; // edges are symmetric; kept for clarity
    px = traceChains(cannyEdges(edgeSrc, sigma, p.edgeSensitivity), g.width, g.height);
  } else {
    const field = gaussianBlur(dark, Math.max(0.5, sigma * 0.6));
    const passes = (mode === 'hatch' ? hatchTonePasses : crosshatchTonePasses)(p.hatchAngleDeg, Math.max(0.2, p.hatchSpacingMm));
    return finish(hatchPasses(field, widthMm, heightMm, passes), widthMm, heightMm, p, true);
  }
  let strokes = px.map((s) => s.map(([x, y]): [number, number] => [x * sx, y * sy]));
  strokes = strokes.filter((s) => s.length >= 2);
  // stitch chains split at junctions / pixel gaps back together
  if (mode === 'outline' || mode === 'lineart') strokes = joinNearby(strokes, Math.max(sx, sy) * 1.5);
  return finish(strokes, widthMm, heightMm, p, false);
}

function finish(strokes: Stroke[], widthMm: number, heightMm: number, p: ConvertParams, hatch: boolean): ConvertResult {
  let s = hatch ? strokes : strokes.map((st) => simplify(st, p.simplifyMm));
  s = dropShort(s, p.minFeatureMm);
  s = clipStrokesToRect(s, { minX: 0, minY: 0, maxX: widthMm, maxY: heightMm });
  return { strokes: dropShort(s, 1e-6), widthMm, heightMm };
}
