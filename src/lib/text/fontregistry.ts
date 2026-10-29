// Font registry: bundled Hershey stroke fonts + installed system outline fonts (via the `fonts` / `font_bytes` commands).
// layoutPage is synchronous, so outline fonts are fetched + parsed ONCE by `ensureFont` and then served from a cache.
import { listStrokeFonts, strokeFontMetrics } from './strokefont';
import { outlineMetrics, parseFont, type OutlineFont } from './outline';
import type { FontMetrics } from './layout';

export type CallFn = <T = unknown>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
export interface FontListEntry { id: string; name: string; kind: 'stroke' | 'outline' }
interface SysFont { id: string; family: string; style: string }

export const SYS_PREFIX = 'sys:';
const loaded = new Map<string, OutlineFont>();
const inflight = new Map<string, Promise<void>>();
let lastCall: CallFn | null = null;

export const isOutlineId = (fontId: string) => fontId.startsWith(SYS_PREFIX);

export async function loadFontList(call: CallFn): Promise<FontListEntry[]> {
  lastCall = call;
  const stroke: FontListEntry[] = listStrokeFonts().map((f) => ({ id: f.id, name: f.name, kind: 'stroke' }));
  let sys: SysFont[] = [];
  try { sys = (await call<SysFont[]>('fonts')) ?? []; } catch { /* no system fonts available */ }
  const outline: FontListEntry[] = sys.map((f) => ({
    id: SYS_PREFIX + f.id,
    name: f.style && f.style !== 'Regular' ? `${f.family} ${f.style}` : f.family,
    kind: 'outline',
  }));
  return [...stroke, ...outline];
}

function b64ToBuffer(b64: string): ArrayBuffer {
  const bin = atob(b64);
  const u = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i);
  return u.buffer;
}

/** Fetch + parse the font once (no-op for stroke fonts and already loaded fonts). Rejects if it cannot be loaded. */
export function ensureFont(fontId: string, call?: CallFn): Promise<void> {
  if (!isOutlineId(fontId) || loaded.has(fontId)) return Promise.resolve();
  let p = inflight.get(fontId);
  if (p) return p;
  const c = call ?? lastCall;
  if (!c) return Promise.reject(new Error('font registry not initialised (call loadFontList first)'));
  p = (async () => {
    const b64 = await c<string>('font_bytes', { id: fontId.slice(SYS_PREFIX.length) });
    loaded.set(fontId, parseFont(b64ToBuffer(b64)));
  })().finally(() => inflight.delete(fontId));
  inflight.set(fontId, p);
  return p;
}

/** Sync metrics: stroke metrics for Hershey, outline metrics for loaded system fonts, null if not (yet) available. */
export function fontMetricsFor(fontId: string, sizeMm: number): FontMetrics | null {
  if (isOutlineId(fontId)) {
    const f = loaded.get(fontId);
    return f ? outlineMetrics(f, sizeMm) : null;
  }
  try { return strokeFontMetrics(fontId, sizeMm); } catch { return null; }
}

export async function getMetrics(fontId: string, sizeMm: number): Promise<FontMetrics | null> {
  try { await ensureFont(fontId); } catch { return null; }
  return fontMetricsFor(fontId, sizeMm);
}

export const isFontLoaded = (fontId: string) => !isOutlineId(fontId) || loaded.has(fontId);
/** test helper */
export function _resetFontRegistry() { loaded.clear(); inflight.clear(); lastCall = null; }
