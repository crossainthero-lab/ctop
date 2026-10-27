// Document → strokes → machine plan (via the Rust planner). Debounced; stale results are dropped.
import { api, type PlanPreview } from './api';
import { clipStrokesToRect, strokeBounds } from './geometry';
import type { Stroke } from './types';
import { paperStrokes, localGeom } from './render.svelte';
import { project } from './doc.svelte';

export const pipe = $state<{
  strokes: Stroke[]; outside: number; notes: string[]; plan: PlanPreview | null; busy: boolean; error: string | null;
}>({ strokes: [], outside: 0, notes: [], plan: null, busy: false, error: null });

let timer: ReturnType<typeof setTimeout> | undefined;
let seq = 0;

export function collect(): { strokes: Stroke[]; outside: number; notes: string[] } {
  const p = project.paper, m = p.margin;
  const rect = { minX: m, minY: m, maxX: p.w - m, maxY: p.h - m };
  let all: Stroke[] = [];
  const notes: string[] = [];
  for (const o of project.objects) {
    if (o.hidden) continue;
    all.push(...paperStrokes(o, project.assets));
    for (const n of localGeom(o, project.assets).notes) if (!/Decoding|Loading/.test(n)) notes.push(`${o.name}: ${n}`);
  }
  let outside = 0;
  for (const s of all) {
    const b = strokeBounds(s);
    if (b && (b.minX < rect.minX - 1e-6 || b.minY < rect.minY - 1e-6 || b.maxX > rect.maxX + 1e-6 || b.maxY > rect.maxY + 1e-6)) outside++;
  }
  if (project.settings.clipToMargins) all = clipStrokesToRect(all, rect);
  return { strokes: all, outside, notes };
}

export function schedule(delay = 180) {
  clearTimeout(timer);
  timer = setTimeout(run, delay);
}

async function run() {
  const my = ++seq;
  const c = collect();
  pipe.strokes = c.strokes; pipe.outside = c.outside; pipe.notes = c.notes;
  if (!c.strokes.length) { pipe.plan = null; pipe.error = null; return; }
  pipe.busy = true;
  try {
    const plan = await api.planPreview(c.strokes, project.settings.plan, project.settings.gcode);
    if (my === seq) { pipe.plan = plan; pipe.error = null; }
  } catch (e) {
    if (my === seq) { pipe.error = String(e); pipe.plan = null; }
  } finally {
    if (my === seq) pipe.busy = false;
  }
}
