// App-wide reactive state: machine snapshot, log, calibration status, toasts, navigation.
import { api, onLog, onMachine, type CalibrationStatus, type JobSummary, type LogEntry, type Snapshot } from './api';

export type View = 'workspace' | 'manual' | 'calibration' | 'jobs' | 'settings';

export const ui = $state<{ view: View; settings: Record<string, unknown> }>({ view: 'workspace', settings: {} });

export const machine = $state<{ s: Snapshot }>({
  s: {
    session: 0, mode: 'none', link: 'disconnected', device: '', firmware: null, fwLimits: null, armed: null,
    axesKnown: [false, false, false], position: null, positionSource: 'unknown', feed: 0, activity: 'Not connected',
    busy: 'idle', lastError: null, attention: null, pending: 0, txCount: 0, rxCount: 0, job: null,
  },
});

export const logs = $state<{ lines: LogEntry[] }>({ lines: [] });
export const cal = $state<{ status: CalibrationStatus | null }>({ status: null });
export const jobs = $state<{ list: JobSummary[] }>({ list: [] });
export const toasts = $state<{ items: { id: number; kind: 'info' | 'error' | 'ok'; text: string }[] }>({ items: [] });

let toastId = 0;
export function toast(text: string, kind: 'info' | 'error' | 'ok' = 'info', ms = 5000) {
  const id = ++toastId;
  toasts.items.push({ id, kind, text });
  setTimeout(() => (toasts.items = toasts.items.filter((t) => t.id !== id)), kind === 'error' ? Math.max(ms, 8000) : ms);
}

/** Run a backend call and surface failures as toasts. Returns true on success. */
export async function act(fn: () => Promise<unknown>): Promise<boolean> {
  try {
    await fn();
    return true;
  } catch (e) {
    toast(String(e), 'error');
    return false;
  }
}

export async function refreshCalibration() {
  try { cal.status = await api.calibrationStatus(); } catch { /* backend not ready */ }
}
export async function refreshJobs() {
  try { jobs.list = await api.listJobs(); } catch { /* ignore */ }
}

let started = false;
export async function startState() {
  if (started) return;
  started = true;
  try {
    ui.settings = await api.settings();
    machine.s = await api.machineState();
    logs.lines = await api.recentLog();
  } catch { /* running outside Tauri */ }
  await refreshCalibration();
  await refreshJobs();
  await onMachine((s) => {
    const prevJob = machine.s.job?.state;
    machine.s = s;
    if (s.job && s.job.state !== prevJob) refreshJobs();
    if (!s.job || s.job.state !== 'running') { /* keep list fresh at low rate */ }
  });
  await onLog((batch) => {
    logs.lines.push(...batch);
    if (logs.lines.length > 2500) logs.lines.splice(0, logs.lines.length - 2000);
  });
  setInterval(refreshJobs, 1500);
  setInterval(refreshCalibration, 4000);
}

export const isConnected = () => machine.s.link === 'ready';
export const modeLabel = () => (machine.s.mode === 'simulation' ? 'SIMULATION' : machine.s.mode === 'hardware' ? 'HARDWARE' : 'OFFLINE');
