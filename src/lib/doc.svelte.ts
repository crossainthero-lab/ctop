// Document model with undo/redo. Assets (image bytes, SVG text) live outside the history snapshots.
import { DEFAULT_GCODE, DEFAULT_PLAN, type GcodeSettings, type PlanOptions } from './api';
import type { Stroke } from './types';
import { DEFAULT_CONVERT_PARAMS } from './image/convert';
import { DEFAULT_STROKE_FONT } from './text/strokefont';
import { localGeom, scaleObjectSize, rasters, clearCaches, type Asset, type Obj, type TextObj, type ImageObj, type SvgObj, type PathObj } from './render.svelte';
import { decodeDataUrl } from './imageio';
import { simplify } from './geometry';

export interface PaperSetup { name: string; w: number; h: number; orientation: 'portrait' | 'landscape'; margin: number }
export interface ProjectSettings { plan: PlanOptions; gcode: GcodeSettings; strokeWidthMm: number; clipToMargins: boolean }

export const project = $state<{
  name: string; path: string | null; dirty: boolean;
  paper: PaperSetup; objects: Obj[]; assets: Record<string, Asset>; settings: ProjectSettings;
}>({
  name: 'Untitled', path: null, dirty: false,
  paper: { name: 'A4', w: 210, h: 297, orientation: 'portrait', margin: 10 },
  objects: [], assets: {},
  settings: { plan: { ...DEFAULT_PLAN }, gcode: { ...DEFAULT_GCODE }, strokeWidthMm: 0.5, clipToMargins: true },
});

export const sel = $state<{ ids: string[] }>({ ids: [] });
export const hist = $state({ past: [] as string[], future: [] as string[] });

let idn = 0;
export const newId = (p = 'o') => `${p}${Date.now().toString(36)}${(idn++).toString(36)}`;

const snap = () => JSON.stringify({ paper: project.paper, objects: project.objects, settings: project.settings });
let lastKey = '', lastAt = 0;

/** Call BEFORE mutating. Calls with the same key within 900 ms coalesce into one undo step. */
export function checkpoint(key = '') {
  const now = Date.now();
  if (key && key === lastKey && now - lastAt < 900) { lastAt = now; return; }
  lastKey = key; lastAt = now;
  const s = snap();
  if (hist.past[hist.past.length - 1] !== s) hist.past.push(s);
  if (hist.past.length > 200) hist.past.shift();
  hist.future = [];
  project.dirty = true;
}

function restore(s: string) {
  const d = JSON.parse(s);
  project.paper = d.paper; project.objects = d.objects; project.settings = d.settings;
  sel.ids = sel.ids.filter((i) => project.objects.some((o) => o.id === i));
  project.dirty = true;
}
export function undo() { const p = hist.past.pop(); if (p === undefined) return; hist.future.push(snap()); restore(p); lastKey = ''; }
export function redo() { const f = hist.future.pop(); if (f === undefined) return; hist.past.push(snap()); restore(f); lastKey = ''; }

export const selected = () => project.objects.filter((o) => sel.ids.includes(o.id));
export const byId = (id: string) => project.objects.find((o) => o.id === id);

export function setPaper(w: number, h: number, name: string, orientation: 'portrait' | 'landscape') {
  checkpoint();
  project.paper.name = name; project.paper.orientation = orientation;
  const [a, b] = [Math.min(w, h), Math.max(w, h)];
  project.paper.w = orientation === 'portrait' ? a : b;
  project.paper.h = orientation === 'portrait' ? b : a;
}

export function addObject<T extends Obj>(o: T): T {
  checkpoint();
  project.objects.push(o);
  sel.ids = [o.id];
  return o;
}

function centered(w: number, h: number) {
  return { x: Math.max(0, (project.paper.w - w) / 2), y: Math.max(0, (project.paper.h - h) / 2) };
}

export function addText(text = 'Hello, world.', at?: [number, number]): TextObj {
  const o: TextObj = {
    id: newId(), name: 'Text', type: 'text', text, mode: 'stroke', fontId: DEFAULT_STROKE_FONT, fontPath: '', sizeMm: 8,
    letterSpacingMm: 0, lineSpacing: 1, align: 'left', maxWidthMm: 0, x: at?.[0] ?? project.paper.margin, y: at?.[1] ?? project.paper.margin, rotation: 0,
  };
  return addObject(o);
}

export async function addImageFromDataUrl(dataUrl: string, name: string): Promise<ImageObj> {
  const dec = await decodeDataUrl(dataUrl);
  const id = newId('a');
  project.assets[id] = { id, kind: 'image', name, dataUrl, pxW: dec.width, pxH: dec.height };
  rasters.set(id, dec.raster);
  const maxW = project.paper.w - 2 * project.paper.margin;
  const maxH = project.paper.h - 2 * project.paper.margin;
  const widthMm = Math.max(10, Math.min(maxW, maxH * (dec.width / dec.height), 120));
  const heightMm = widthMm * dec.height / dec.width;
  const o: ImageObj = { id: newId(), name, type: 'image', asset: id, widthMm, mode: 'outline', params: { ...DEFAULT_CONVERT_PARAMS }, ...centered(widthMm, heightMm), rotation: 0 };
  return addObject(o);
}

export function addSvg(svgText: string, name: string, natural: { w: number; h: number }): SvgObj {
  const id = newId('a');
  project.assets[id] = { id, kind: 'svg', name, svgText };
  const k = Math.min(1, (project.paper.w - 2 * project.paper.margin) / natural.w, (project.paper.h - 2 * project.paper.margin) / natural.h);
  const o: SvgObj = { id: newId(), name, type: 'svg', asset: id, scale: k, ...centered(natural.w * k, natural.h * k), rotation: 0 };
  return addObject(o);
}

export function addFreehand(points: [number, number][]) {
  if (points.length < 2) return;
  const s: Stroke = simplify(points, 0.15);
  if (s.length < 2) return;
  let minX = Infinity, minY = Infinity;
  for (const p of s) { minX = Math.min(minX, p[0]); minY = Math.min(minY, p[1]); }
  addObject<PathObj>({ id: newId(), name: 'Sketch', type: 'path', strokes: [s], scale: 1, x: minX, y: minY, rotation: 0 });
}

export function removeSelected() {
  if (!sel.ids.length) return;
  checkpoint();
  const ids = new Set(sel.ids);
  project.objects = project.objects.filter((o) => !ids.has(o.id));
  sel.ids = [];
}

export function duplicateSelected() {
  if (!sel.ids.length) return;
  checkpoint();
  const copies = selected().map((o) => ({ ...JSON.parse(JSON.stringify(o)), id: newId(), x: o.x + 5, y: o.y + 5 }) as Obj);
  project.objects.push(...copies);
  sel.ids = copies.map((c) => c.id);
}

export function reorder(dir: 'front' | 'back') {
  checkpoint();
  const ids = new Set(sel.ids);
  const picked = project.objects.filter((o) => ids.has(o.id));
  const rest = project.objects.filter((o) => !ids.has(o.id));
  project.objects = dir === 'front' ? [...rest, ...picked] : [...picked, ...rest];
}

export function nudge(dx: number, dy: number) {
  if (!sel.ids.length) return;
  checkpoint('nudge');
  for (const o of selected()) { o.x += dx; o.y += dy; }
}

export function align(kind: 'left' | 'hcenter' | 'right' | 'top' | 'vcenter' | 'bottom' | 'page-h' | 'page-v') {
  const sels = selected();
  if (!sels.length) return;
  checkpoint();
  const geo = sels.map((o) => ({ o, l: localGeom(o, project.assets) }));
  const minX = Math.min(...geo.map((g) => g.o.x)), maxX = Math.max(...geo.map((g) => g.o.x + g.l.w));
  const minY = Math.min(...geo.map((g) => g.o.y)), maxY = Math.max(...geo.map((g) => g.o.y + g.l.h));
  for (const { o, l } of geo) {
    if (kind === 'left') o.x = minX;
    if (kind === 'right') o.x = maxX - l.w;
    if (kind === 'hcenter') o.x = (minX + maxX) / 2 - l.w / 2;
    if (kind === 'top') o.y = minY;
    if (kind === 'bottom') o.y = maxY - l.h;
    if (kind === 'vcenter') o.y = (minY + maxY) / 2 - l.h / 2;
    if (kind === 'page-h') o.x = (project.paper.w - l.w) / 2;
    if (kind === 'page-v') o.y = (project.paper.h - l.h) / 2;
  }
}

/** Scale (about pivot), rotate (degrees, about pivot) and translate a set of objects. */
export function transformObjects(objs: Obj[], start: Map<string, Obj>, s: number, dTheta: number, pivot: [number, number], dx = 0, dy = 0) {
  const c = Math.cos(dTheta * Math.PI / 180), si = Math.sin(dTheta * Math.PI / 180);
  for (const o of objs) {
    const o0 = start.get(o.id)!;
    const l0 = localGeom(o0, project.assets);
    const cx = o0.x + l0.w / 2 - pivot[0], cy = o0.y + l0.h / 2 - pivot[1];
    Object.assign(o, JSON.parse(JSON.stringify(o0)));
    if (s !== 1) scaleObjectSize(o, s);
    const l1 = localGeom(o, project.assets);
    const nx = pivot[0] + s * (c * cx - si * cy) + dx, ny = pivot[1] + s * (si * cx + c * cy) + dy;
    o.x = nx - l1.w / 2; o.y = ny - l1.h / 2;
    o.rotation = ((o0.rotation + dTheta) % 360 + 360) % 360;
  }
}

// ── persistence ──
export function serialize(): string {
  const used = new Set(project.objects.flatMap((o) => ('asset' in o ? [o.asset] : [])));
  const assets = Object.fromEntries(Object.entries(project.assets).filter(([k]) => used.has(k)));
  return JSON.stringify({ format: 'ctop-project', version: 1, name: project.name, paper: project.paper, objects: project.objects, assets, settings: project.settings }, null, 1);
}

export async function load(text: string, path: string | null) {
  const d = JSON.parse(text);
  if (d.format !== 'ctop-project') throw new Error('Not a CtoP project file');
  clearCaches(); rasters.clear();
  project.name = d.name ?? 'Untitled'; project.path = path;
  project.paper = d.paper; project.objects = d.objects; project.assets = d.assets ?? {};
  project.settings = { plan: { ...DEFAULT_PLAN }, gcode: { ...DEFAULT_GCODE }, strokeWidthMm: 0.5, clipToMargins: true, ...(d.settings ?? {}) };
  sel.ids = []; hist.past = []; hist.future = [];
  for (const a of Object.values(project.assets)) {
    if (a.kind === 'image' && a.dataUrl) rasters.set(a.id, (await decodeDataUrl(a.dataUrl)).raster);
  }
  clearCaches();
  project.dirty = false;
}

export function newProject() {
  clearCaches(); rasters.clear();
  project.name = 'Untitled'; project.path = null; project.objects = []; project.assets = {};
  project.paper = { name: 'A4', w: 210, h: 297, orientation: 'portrait', margin: 10 };
  project.settings = { plan: { ...DEFAULT_PLAN }, gcode: { ...DEFAULT_GCODE }, strokeWidthMm: 0.5, clipToMargins: true };
  sel.ids = []; hist.past = []; hist.future = []; project.dirty = false;
}
