// Application state: the backend state (polled), the user's preferences (saved in the backend settings, so they
// survive restarts and are the same in every browser), and small shared helpers.
import { call } from './rpc';
import type { AppState } from './types.app';
import type { Margins } from './write/pagelayout';
import type { Align } from './text/layout';
import type { ImageStyle } from './write/imagejob';
import type { LineStyle, Mode, Orient } from './plan/jobs';

export interface Prefs {
  mode: Mode;
  text: string;
  fontId: string;
  sizeMm: number;
  lineSpacing: number;
  align: Align;
  orient: Orient;
  margins: Margins;
  // lined paper
  lined: {
    text: string;
    fontId: string;
    sizeMm: number;
    align: Align;
    spacingMm: number;
    margins: Margins;
    border: boolean;
    marginLine: boolean;
    marginLineOffsetMm: number;
    lineStyle: LineStyle;
    autoLines: boolean;
    lineCount: number;
    writeEvery: number;
    orient: Orient;
  };
  // image
  img: { style: ImageStyle; detail: number; scale: number; rotation: number; x: number; y: number; invert: boolean; clean: boolean; margins: Margins; orient: Orient };
  showTravel: boolean;
}

export const DEFAULT_PREFS: Prefs = {
  mode: 'text', text: '', fontId: 'futural', sizeMm: 6, lineSpacing: 1, align: 'left', orient: 'portrait',
  margins: { left: 20, right: 20, top: 20, bottom: 20 },
  lined: {
    text: '', fontId: 'scripts', sizeMm: 4.5, align: 'left', spacingMm: 8, margins: { left: 15, right: 12, top: 18, bottom: 14 },
    border: true, marginLine: true, marginLineOffsetMm: 22, lineStyle: 'solid', autoLines: true, lineCount: 25, writeEvery: 1, orient: 'portrait',
  },
  img: { style: 'outline', detail: 0.5, scale: 1, rotation: 0, x: 0.5, y: 0.5, invert: false, clean: true, margins: { left: 15, right: 15, top: 15, bottom: 15 }, orient: 'portrait' },
  showTravel: false,
};

export const app = $state<{ st: AppState | null; backendError: string | null; prefs: Prefs; prefsLoaded: boolean; clientErrors: string[] }>({
  st: null, backendError: null, prefs: structuredClone(DEFAULT_PREFS), prefsLoaded: false, clientErrors: [],
});

export function noteClientError(msg: string) {
  app.clientErrors.push(`${new Date().toISOString()} ${msg}`);
  if (app.clientErrors.length > 50) app.clientErrors.shift();
  call('log_client', { msg }).catch(() => {});
}

export async function refresh() {
  try {
    app.st = await call<AppState>('state');
    app.backendError = null;
  } catch (e) {
    app.backendError = e instanceof Error ? e.message : String(e);
  }
}

let saveTimer: ReturnType<typeof setTimeout> | undefined;
export function savePrefsSoon() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => call('set_setting', { key: 'ui2', value: $state.snapshot(app.prefs) }).catch(() => {}), 700);
}

function merge<T extends object>(base: T, over: unknown): T {
  if (!over || typeof over !== 'object') return base;
  const out: Record<string, unknown> = { ...(base as Record<string, unknown>) };
  for (const [k, v] of Object.entries(over as Record<string, unknown>)) {
    const b = (base as Record<string, unknown>)[k];
    out[k] = b && typeof b === 'object' && !Array.isArray(b) ? merge(b as object, v) : v;
  }
  return out as T;
}

export async function startApp() {
  try {
    const s = await call<Record<string, unknown>>('get_settings');
    app.prefs = merge(structuredClone(DEFAULT_PREFS), s.ui2);
  } catch (e) {
    app.backendError = e instanceof Error ? e.message : String(e);
  }
  app.prefsLoaded = true;
  await refresh();
  setInterval(refresh, 500);
  window.addEventListener('error', (e) => noteClientError(`${e.message} @ ${e.filename}:${e.lineno}`));
  window.addEventListener('unhandledrejection', (e) => noteClientError(`unhandled rejection: ${e.reason?.stack ?? e.reason}`));
}

export const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));
