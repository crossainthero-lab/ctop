// Document objects → strokes. Local geometry is cached per object; the paper-space transform is cheap.
import type { Stroke, RasterImage } from './types';
import { compose, applyStrokes, applyPt, boundsOf, type Matrix } from './geometry';
import { layoutText } from './text/strokefont';
import { outlineText, parseFont, type OutlineFont } from './text/outline';
import { convertImage, type ConvertMode, type ConvertParams } from './image/convert';
import { importSvg } from './svg/import';
import { api } from './api';

export interface BaseObj { id: string; name: string; x: number; y: number; rotation: number; locked?: boolean; hidden?: boolean }
export interface TextObj extends BaseObj {
  type: 'text'; text: string; mode: 'stroke' | 'outline'; fontId: string; fontPath: string; sizeMm: number;
  letterSpacingMm: number; lineSpacing: number; align: 'left' | 'center' | 'right'; maxWidthMm: number;
}
export interface ImageObj extends BaseObj { type: 'image'; asset: string; widthMm: number; mode: ConvertMode; params: ConvertParams }
export interface SvgObj extends BaseObj { type: 'svg'; asset: string; scale: number }
export interface PathObj extends BaseObj { type: 'path'; strokes: Stroke[]; scale: number }
export type Obj = TextObj | ImageObj | SvgObj | PathObj;

export interface Asset { id: string; kind: 'image' | 'svg'; name: string; dataUrl?: string; svgText?: string; pxW?: number; pxH?: number }

export interface Local { strokes: Stroke[]; w: number; h: number; notes: string[] }

// Non-reactive caches
export const rasters = new Map<string, RasterImage>();
const outlineFonts = new Map<string, OutlineFont | 'loading' | 'failed'>();
const svgCache = new Map<string, { strokes: Stroke[]; w: number; h: number; warnings: string[] }>();
const localCache = new Map<string, { key: string; local: Local }>();
/** Bumped when async resources (fonts, rasters) arrive so the UI recomputes. */
export const resourceTick = $state({ n: 0 });

export function requestOutlineFont(path: string) {
  if (!path || outlineFonts.has(path)) return;
  outlineFonts.set(path, 'loading');
  api.readBinaryFile(path)
    .then((buf) => { outlineFonts.set(path, parseFont(buf)); resourceTick.n++; })
    .catch(() => { outlineFonts.set(path, 'failed'); resourceTick.n++; });
}

export function clearCaches() { localCache.clear(); svgCache.clear(); }

function svgFor(a: Asset) {
  let r = svgCache.get(a.id);
  if (!r) {
    const out = importSvg(a.svgText ?? '');
    r = { strokes: out.strokes, w: out.widthMm, h: out.heightMm, warnings: out.warnings };
    svgCache.set(a.id, r);
  }
  return r;
}

export function localGeom(o: Obj, assets: Record<string, Asset>): Local {
  void resourceTick.n;
  let key: string;
  switch (o.type) {
    case 'text': key = JSON.stringify([o.text, o.mode, o.fontId, o.fontPath, o.sizeMm, o.letterSpacingMm, o.lineSpacing, o.align, o.maxWidthMm, outlineFonts.get(o.fontPath) ? 1 : 0, resourceTick.n]); break;
    case 'image': key = JSON.stringify([o.asset, o.widthMm, o.mode, o.params, rasters.has(o.asset)]); break;
    case 'svg': key = JSON.stringify([o.asset, o.scale]); break;
    case 'path': key = `${o.strokes.length}:${o.strokes[0]?.length}:${o.scale}`; break;
  }
  const hit = localCache.get(o.id);
  if (hit && hit.key === key) return hit.local;
  const local = compute(o, assets);
  localCache.set(o.id, { key, local });
  return local;
}

function compute(o: Obj, assets: Record<string, Asset>): Local {
  const notes: string[] = [];
  switch (o.type) {
    case 'text': {
      const opts = { sizeMm: o.sizeMm, letterSpacingMm: o.letterSpacingMm, lineSpacing: o.lineSpacing, align: o.align, maxWidthMm: o.maxWidthMm > 0 ? o.maxWidthMm : undefined };
      if (o.mode === 'outline') {
        const f = outlineFonts.get(o.fontPath);
        if (f && f !== 'loading' && f !== 'failed') {
          const t = outlineText(f, o.text, opts);
          return { strokes: t.strokes, w: Math.max(t.widthMm, 1), h: Math.max(t.heightMm, 1), notes: t.missing.length ? [`Missing glyphs: ${t.missing.join(' ')}`] : [] };
        }
        notes.push(f === 'failed' ? 'Font file could not be loaded — showing single-stroke fallback' : 'Loading font…');
        requestOutlineFont(o.fontPath);
      }
      const t = layoutText(o.text, { fontId: o.fontId, ...opts });
      if (t.missing.length) notes.push(`Unsupported characters skipped: ${t.missing.join(' ')}`);
      return { strokes: t.strokes, w: Math.max(t.widthMm, 1), h: Math.max(t.heightMm, 1), notes };
    }
    case 'image': {
      const r = rasters.get(o.asset);
      if (!r) return { strokes: [], w: o.widthMm, h: o.widthMm, notes: ['Decoding image…'] };
      const out = convertImage(r, o.mode, o.params, o.widthMm);
      return { strokes: out.strokes, w: out.widthMm, h: out.heightMm, notes: out.strokes.length === 0 ? ['No strokes at these settings'] : [] };
    }
    case 'svg': {
      const a = assets[o.asset];
      if (!a) return { strokes: [], w: 10, h: 10, notes: ['Missing asset'] };
      const s = svgFor(a);
      return { strokes: applyStrokesScale(s.strokes, o.scale), w: Math.max(s.w * o.scale, 0.5), h: Math.max(s.h * o.scale, 0.5), notes: s.warnings };
    }
    case 'path': {
      const b = boundsOf(o.strokes);
      if (!b) return { strokes: [], w: 1, h: 1, notes: [] };
      const m: Matrix = [o.scale, 0, 0, o.scale, -b.minX * o.scale, -b.minY * o.scale];
      return { strokes: applyStrokes(m, o.strokes), w: Math.max((b.maxX - b.minX) * o.scale, 0.5), h: Math.max((b.maxY - b.minY) * o.scale, 0.5), notes };
    }
  }
}

const applyStrokesScale = (ss: Stroke[], k: number) => (k === 1 ? ss : applyStrokes([k, 0, 0, k, 0, 0], ss));

export function objMatrix(o: Obj, l: Local): Matrix {
  return compose({ tx: o.x, ty: o.y, rotationDeg: o.rotation, cx: l.w / 2, cy: l.h / 2 });
}

export function paperStrokes(o: Obj, assets: Record<string, Asset>): Stroke[] {
  const l = localGeom(o, assets);
  return applyStrokes(objMatrix(o, l), l.strokes);
}

/** Corners (TL, TR, BR, BL) of the object's rotated frame in paper space. */
export function frameCorners(o: Obj, assets: Record<string, Asset>): [number, number][] {
  const l = localGeom(o, assets);
  const m = objMatrix(o, l);
  return [[0, 0], [l.w, 0], [l.w, l.h], [0, l.h]].map((p) => applyPt(m, p as [number, number]));
}

export function pathData(strokes: Stroke[]): string {
  let d = '';
  for (const s of strokes) {
    if (s.length < 2) continue;
    d += 'M' + s[0][0].toFixed(2) + ' ' + s[0][1].toFixed(2);
    for (let i = 1; i < s.length; i++) d += 'L' + s[i][0].toFixed(2) + ' ' + s[i][1].toFixed(2);
  }
  return d;
}

/** Apply a uniform size multiplier to an object's own size parameter. */
export function scaleObjectSize(o: Obj, k: number) {
  switch (o.type) {
    case 'text': o.sizeMm = Math.max(1, o.sizeMm * k); o.letterSpacingMm *= k; if (o.maxWidthMm > 0) o.maxWidthMm *= k; break;
    case 'image': o.widthMm = Math.max(2, o.widthMm * k); break;
    case 'svg': case 'path': o.scale = Math.max(0.01, o.scale * k); break;
  }
}
