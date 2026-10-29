// Joystick maths for the calibration page. Pure functions so the behaviour (dead zone, speed curve, the hard cap on a single
// step, screen-to-machine direction) can be tested without a browser. Seen from the FRONT of the printer (LCD side):
// right = +X, away from you = +Y, which is "up" on the screen.

export interface JoyOpts {
  /** top speed at full deflection, mm per second */
  speedMmS: number;
  /** time between steps, ms */
  tickMs: number;
  /** fraction of the radius that does nothing */
  deadzone: number;
  /** one step is never longer than this (the backend refuses more than 6) */
  maxStepMm: number;
  flipX?: boolean;
  flipY?: boolean;
}

export const DEFAULT_JOY: JoyOpts = { speedMmS: 12, tickMs: 140, deadzone: 0.1, maxStepMm: 5 };

/** Stick deflection (vx right, vy DOWN on screen, each -1..1 as a fraction of the radius) -> one machine step, or null. */
export function stepFromStick(vx: number, vy: number, o: JoyOpts): { dx: number; dy: number } | null {
  const mag = Math.hypot(vx, vy);
  if (!isFinite(mag) || mag < o.deadzone) return null;
  const m = Math.min(1, mag);
  const curve = Math.pow((m - o.deadzone) / (1 - o.deadzone), 1.6); // gentle near the centre: precise positioning
  const dist = Math.min(o.maxStepMm, (o.speedMmS * o.tickMs) / 1000 * curve);
  if (dist < 0.005) return null;
  const sx = o.flipX ? -1 : 1, sy = o.flipY ? -1 : 1;
  return { dx: sx * (vx / mag) * dist, dy: sy * (-vy / mag) * dist };
}

export type Dir = 'left' | 'right' | 'forward' | 'back';
/** Fine-positioning button: forward = away from you (+Y). */
export function fineStep(dir: Dir, mm: number, o: Pick<JoyOpts, 'flipX' | 'flipY'> = {}): { dx: number; dy: number } {
  const sx = o.flipX ? -1 : 1, sy = o.flipY ? -1 : 1;
  switch (dir) {
    case 'left': return { dx: -sx * mm, dy: 0 };
    case 'right': return { dx: sx * mm, dy: 0 };
    case 'forward': return { dx: 0, dy: sy * mm };
    case 'back': return { dx: 0, dy: -sy * mm };
  }
}

/** Pointer position relative to the joystick's centre -> deflection, clamped to the circle. */
export function deflection(px: number, py: number, cx: number, cy: number, radius: number): { vx: number; vy: number } {
  const dx = (px - cx) / radius, dy = (py - cy) / radius;
  const m = Math.hypot(dx, dy);
  return m > 1 ? { vx: dx / m, vy: dy / m } : { vx: dx, vy: dy };
}
