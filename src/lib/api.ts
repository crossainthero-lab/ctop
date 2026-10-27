// Typed wrappers over the Rust backend (Tauri commands + events).
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { Stroke } from './types';

export type Mode = 'none' | 'simulation' | 'hardware';
export type Link = 'disconnected' | 'connecting' | 'ready' | 'halted' | 'lost';
export type ArmMode = 'envelope' | 'firmwareOnly';
export type Busy = 'idle' | 'moving' | 'homing' | 'syncing' | 'job';
export type JobState = 'running' | 'paused' | 'cancelling' | 'completed' | 'failed' | 'cancelled' | 'stopped' | 'interrupted';
export type JobStatus = 'queued' | 'active' | 'completed' | 'failed' | 'cancelled' | 'stopped' | 'interrupted';
export type Xyz = [number, number, number];

export interface FirmwareInfo { name: string; machineType: string; isMarlin: boolean; emergencyParser: boolean | null; raw: string }
export interface FwLimits { xMin: number; xMax: number; yMin: number; yMax: number; zMin: number; zMax: number }
export interface JobProgress {
  id: string; name: string; state: JobState; totalLines: number; sent: number; accepted: number;
  confirmedStrokes: number; totalStrokes: number; elapsedS: number; estimateS: number;
  message: string; error: string | null; warnings: string[]; currentStroke: number | null;
}
export interface Snapshot {
  session: number; mode: Mode; link: Link; device: string; firmware: FirmwareInfo | null; fwLimits: FwLimits | null;
  armed: ArmMode | null; axesKnown: [boolean, boolean, boolean]; position: Xyz | null; positionSource: string;
  feed: number; activity: string; busy: Busy; lastError: string | null; attention: string | null;
  pending: number; txCount: number; rxCount: number; job: JobProgress | null;
}
export interface LogEntry { tMs: number; dir: 'tx' | 'rx' | 'info' | 'warn' | 'error'; text: string }

export interface Margins { left: number; right: number; top: number; bottom: number }
export interface Calibration {
  pencilOffsetX: number | null; pencilOffsetY: number | null;
  zDown: number | null; zUp: number | null; zSafe: number | null;
  paperOriginX: number | null; paperOriginY: number | null; paperWidth: number | null; paperHeight: number | null;
  flipX: boolean; flipY: boolean; swapXy: boolean; zIncreasesUp: boolean;
  margins: Margins;
  envelope: { xMin: number | null; xMax: number | null; yMin: number | null; yMax: number | null; zMin: number | null; zMax: number | null };
  verified: { offsets: boolean; z: boolean; paper: boolean; envelope: boolean; orientation: boolean };
  notes: string;
}
export interface Issue { severity: 'error' | 'warning'; field: string; message: string }
export interface Resolved {
  offset: [number, number]; zDown: number; zUp: number; zSafe: number; origin: [number, number]; paper: [number, number];
  flipX: boolean; flipY: boolean; swapXy: boolean; zIncreasesUp: boolean; margins: Margins;
  envelope: { xMin: number; xMax: number; yMin: number; yMax: number; zMin: number; zMax: number };
}
export interface CalibrationStatus { issues: Issue[]; readyForDrawing: boolean; reasons: string[]; envelopeOk: boolean; resolved: Resolved | null }
export interface Profile { id: string; name: string; kind: 'simulator' | 'hardware'; calibration: Calibration; serialPath: string | null; expectedFirmware: string }
export interface ProfilesFile { active: string; profiles: Profile[] }

export type Order = 'asDrawn' | 'optimized' | 'topToBottom';
export interface PlanOptions { order: Order; joinGapMm: number; minLengthMm: number }
export interface GcodeSettings { drawFeed: number; travelFeed: number; zFeed: number; penSettleMs: number; accelMmS2: number; minSegmentMm: number }
export interface ProgramStats { strokes: number; lines: number; drawLengthMm: number; travelLengthMm: number; penLifts: number; estimatedSeconds: number }
export interface PlanPreview { plan: { strokes: Stroke[]; drawLengthMm: number; travelLengthMm: number; penLifts: number }; stats: ProgramStats; zAssumed: boolean; gcodeBlockers: string[] }
export interface JobSummary {
  id: string; name: string; createdAt: number; endedAt: number | null; status: JobStatus; stats: ProgramStats;
  progress: JobProgress | null; error: string | null; paper: [number, number];
}
export interface JobDetail { summary: JobSummary; strokes: Stroke[] }
export interface PortInfo { path: string; realPath: string; byId: boolean; label: string }
export interface PortCheck { path: string; realPath: string | null; available: boolean; holders: { pid: number; command: string; how: string }[]; message: string }
export interface SimTruth {
  bed: Xyz; pencilOffset: [number, number]; paperContactZ: number; paperRect: [number, number, number, number];
  start: Xyz; emergencyParser: boolean; firmware: string; machineType: string;
}
export interface SimSnapshot {
  phys: Xyz; logical: Xyz; tip: [number, number]; homed: [boolean, boolean, boolean]; penDown: boolean; plannerDepth: number;
  halted: boolean; stalled: boolean; speed: number; truth: SimTruth; trailLen: number;
  collisions: { atMs: number; message: string }[]; commandsSeen: number;
}
export type Fault =
  | { kind: 'disconnect' } | { kind: 'reset' } | { kind: 'halt' } | { kind: 'errorNext' }
  | { kind: 'stall'; seconds: number } | { kind: 'skipSteps'; axis: number; mm: number };
export interface FontFile { family: string; style: string; path: string }

const c = <T>(cmd: string, args?: Record<string, unknown>) => invoke<T>(cmd, args);

export const api = {
  listPorts: () => c<PortInfo[]>('list_serial_ports'),
  checkPort: (path: string) => c<PortCheck>('check_port', { path }),
  connectSimulation: () => c<void>('connect_simulation'),
  connectHardware: (path?: string) => c<void>('connect_hardware', { path }),
  disconnect: () => c<void>('disconnect'),
  machineState: () => c<Snapshot>('machine_state'),
  recentLog: () => c<LogEntry[]>('recent_log'),
  arm: (mode: ArmMode, acknowledged = false) => c<void>('arm', { mode, acknowledged }),
  disarm: () => c<void>('disarm'),
  jog: (axis: number, delta: number, feed: number) => c<void>('jog', { axis, delta, feed }),
  goto: (x: number | null, y: number | null, z: number | null, feed: number) => c<void>('goto_position', { x, y, z, feed }),
  home: (x: boolean, y: boolean, z: boolean) => c<void>('home', { x, y, z }),
  readPosition: () => c<void>('read_position'),
  verifyPosition: () => c<void>('verify_position'),
  pauseJob: () => c<void>('pause_job'),
  resumeJob: () => c<void>('resume_job'),
  cancelJob: () => c<void>('cancel_job'),
  quickStop: () => c<void>('quick_stop'),
  emergencyStop: () => c<void>('emergency_stop'),
  profiles: () => c<ProfilesFile>('get_profiles'),
  selectProfile: (id: string) => c<void>('select_profile', { id }),
  updateProfile: (id: string, name?: string, serialPath?: string) => c<void>('update_profile', { id, name, serialPath }),
  setCalibration: (calibration: Calibration) => c<CalibrationStatus>('set_calibration', { calibration }),
  calibrationStatus: () => c<CalibrationStatus>('calibration_status'),
  planPreview: (strokes: Stroke[], plan: PlanOptions, settings: GcodeSettings) => c<PlanPreview>('plan_preview', { strokes, plan, settings }),
  createJob: (request: { name: string; strokes: Stroke[]; plan: PlanOptions; settings: GcodeSettings; paper: [number, number] }) => c<JobSummary>('create_job', { request }),
  listJobs: () => c<JobSummary[]>('list_jobs'),
  jobDetail: (id: string) => c<JobDetail>('job_detail', { id }),
  deleteJob: (id: string) => c<void>('delete_job', { id }),
  jobGcode: (id: string) => c<string>('job_gcode', { id }),
  exportJob: (id: string, path: string) => c<void>('export_job', { id, path }),
  startJob: (id: string, paperConfirmed: boolean) => c<void>('start_job', { id, paperConfirmed }),
  simState: () => c<SimSnapshot>('sim_state'),
  simTrail: (from: number) => c<[number, [number, number, number, number][]]>('sim_trail', { from }),
  simFault: (fault: Fault) => c<void>('sim_fault', { fault }),
  simSpeed: (speed: number) => c<void>('sim_speed', { speed }),
  simClear: () => c<void>('sim_clear'),
  simSetTruth: (truth: SimTruth) => c<void>('sim_set_truth', { truth }),
  settings: () => c<Record<string, unknown>>('get_settings'),
  setSetting: (key: string, value: unknown) => c<void>('set_setting', { key, value }),
  readTextFile: (path: string) => c<string>('read_text_file', { path }),
  writeTextFile: (path: string, content: string) => c<void>('write_text_file', { path, content }),
  readBinaryFile: async (path: string) => (await invoke<ArrayBuffer>('read_binary_file', { path })),
  systemFonts: () => c<FontFile[]>('list_system_fonts'),
};

export const onMachine = (f: (s: Snapshot) => void): Promise<UnlistenFn> => listen<Snapshot>('ctop://machine', (e) => f(e.payload));
export const onLog = (f: (l: LogEntry[]) => void): Promise<UnlistenFn> => listen<LogEntry[]>('ctop://log', (e) => f(e.payload));

export const DEFAULT_PLAN: PlanOptions = { order: 'optimized', joinGapMm: 0.2, minLengthMm: 0.1 };
export const DEFAULT_GCODE: GcodeSettings = { drawFeed: 1800, travelFeed: 4800, zFeed: 600, penSettleMs: 0, accelMmS2: 400, minSegmentMm: 0.05 };

export function fmtDuration(s: number): string {
  if (!isFinite(s) || s < 0) return '—';
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = Math.round(s % 60);
  return h ? `${h}h ${String(m).padStart(2, '0')}m` : m ? `${m}m ${String(sec).padStart(2, '0')}s` : `${sec}s`;
}
