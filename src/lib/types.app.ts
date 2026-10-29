// Shapes returned by the backend `state` command (src-tauri/src/writer.rs).
import type { FrameProfile } from './frame';

export type Phase = 'ready' | 'preparing' | 'writing' | 'paused' | 'finished' | 'error';

export interface PaperGeom {
  cornersImg: [number, number][];
  cornersPen: [number, number][];
  widthMm: number;
  heightMm: number;
  pageToPen: number[][];
}

export interface JobView {
  id: string;
  state: 'running' | 'pausing' | 'paused' | 'stopping' | 'stopped' | 'completed' | 'failed';
  kind: 'draw' | 'test' | 'marks' | 'write';
  done: number;
  total: number;
  percent: number;
  etaS: number | null;
  elapsedS: number;
  operation: string;
  stage: string;
  live: boolean;
  resumable: boolean;
}

export interface FrameInfo {
  active: FrameProfile | null;
  activeName: string | null;
  problem: string | null;
  reach: { usableHeightMm: number; fraction: number } | null;
  prior: [number, number] | null;
  ready: boolean;
}

export interface AppState {
  phase: Phase;
  message: string;
  note: string | null;
  progress: { done: number; total: number } | null;
  error: { stage: string; message: string; detail: string; atMs: number } | null;
  controller: { reachable: boolean; error: string | null; ready: boolean; blockers: string[]; pos: number[] | null; box: number[] | null; estop: boolean; sock: string };
  camera: { url: string; reachable: boolean; error: string | null; paperSeen: boolean; lastOkMs: number | null; streaming?: boolean; hubState?: string | null };
  cal: { rmsMm: number; looMm: number | null; pose: number[]; created: number; poseDeltaMm: number | null; poseOk: boolean } | null;
  defaultPose: number[];
  paper: { state: 'uncalibrated' | 'nocamera' | 'none' | 'invalid' | 'moving' | 'ready'; geom: PaperGeom | null; lockId: number };
  services?: { cameraHub: string | null; cameraStreaming: boolean; controllerNote: string | null; motionCheck: unknown; completionCheck: unknown };
  frame: FrameInfo;
  job: JobView | null;
  canDraw: boolean;
  drawBlockers: string[];
  setup: string[];
  blockers: string[];
  canWrite: boolean;
  edgeMm: number;
  settings: Record<string, unknown>;
  web: string | null;
  surfaceZ: { value: number | null; fromSetting: boolean };
  version: string;
}
