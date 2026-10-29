import { describe, expect, it } from 'vitest';
import { DEFAULT_JOY, deflection, fineStep, stepFromStick } from './joystick';
import { initialSteps, marksOf, nextEdgeSteps, cornerReachable } from './calibsteps';
import { placeholderFrame } from './frame';

describe('joystick', () => {
  it('does nothing inside the dead zone and in a released (NaN) state', () => {
    expect(stepFromStick(0.05, 0.05, DEFAULT_JOY)).toBeNull();
    expect(stepFromStick(NaN, 0, DEFAULT_JOY)).toBeNull();
  });
  it('up on the screen is +Y (away from you), right is +X', () => {
    const up = stepFromStick(0, -1, DEFAULT_JOY)!, right = stepFromStick(1, 0, DEFAULT_JOY)!;
    expect(up.dy).toBeGreaterThan(0); expect(Math.abs(up.dx)).toBeLessThan(1e-9);
    expect(right.dx).toBeGreaterThan(0); expect(Math.abs(right.dy)).toBeLessThan(1e-9);
  });
  it('a step never exceeds the cap, however fast the setting', () => {
    const s = stepFromStick(1, 0, { ...DEFAULT_JOY, speedMmS: 500 })!;
    expect(Math.hypot(s.dx, s.dy)).toBeLessThanOrEqual(DEFAULT_JOY.maxStepMm + 1e-9);
  });
  it('small deflection gives small steps (precise positioning)', () => {
    const small = stepFromStick(0.25, 0, DEFAULT_JOY)!, full = stepFromStick(1, 0, DEFAULT_JOY)!;
    expect(small.dx).toBeLessThan(full.dx / 5);
    expect(full.dx).toBeCloseTo(12 * 0.14, 6); // speed x tick at full deflection
  });
  it('flipping axes reverses the machine direction', () => {
    expect(stepFromStick(1, 0, { ...DEFAULT_JOY, flipX: true })!.dx).toBeLessThan(0);
    expect(fineStep('forward', 1, { flipY: true }).dy).toBe(-1);
    expect(fineStep('left', 0.1)).toEqual({ dx: -0.1, dy: 0 });
  });
  it('clamps the pointer to the circle', () => {
    const d = deflection(500, 100, 100, 100, 50);
    expect(Math.hypot(d.vx, d.vy)).toBeCloseTo(1, 9);
    expect(d.vy).toBeCloseTo(0, 9);
  });
});

describe('calibration steps', () => {
  it('starts with the four corners in the order top-left, top-right, bottom-right, bottom-left', () => {
    expect(initialSteps(null, null).map((s) => s.name)).toEqual(['tl', 'tr', 'br', 'bl']);
  });
  it('flags corners the guessed position puts out of reach', () => {
    const f = { ...placeholderFrame(), originPen: [-9, 45] as [number, number] };
    const reach: [number, number, number, number] = [30, 280, 30, 308];
    expect(cornerReachable('br', f, reach)).toBe(true);
    expect(cornerReachable('tl', f, reach)).toBe(false);
    expect(initialSteps(f, reach).filter((s) => s.likelyUnreachable).map((s) => s.name).sort()).toEqual(['bl', 'tl', 'tr']);
  });
  it('offers the edge reference points in order, once each', () => {
    const steps = initialSteps(null, null);
    const more = nextEdgeSteps(steps);
    expect(more.map((s) => s.name)).toEqual(['right', 'right', 'front', 'front', 'left', 'back']);
    expect(nextEdgeSteps([...steps, ...more.slice(0, 2)]).length).toBe(4);
  });
  it('collects only completed marks', () => {
    const s = initialSteps(null, null);
    s[2].status = 'done'; s[2].mark = { kind: 'corner', name: 'br', x: 200, y: 45 };
    s[0].status = 'skipped';
    expect(marksOf(s)).toEqual([{ kind: 'corner', name: 'br', x: 200, y: 45 }]);
  });
});

import { DEFAULT_Z, zStepFromStick } from './joystick';
describe('vertical stick', () => {
  it('up raises (positive Z), down lowers', () => {
    expect(zStepFromStick(1, 30, 15.3, DEFAULT_Z)!).toBeGreaterThan(0);
    expect(zStepFromStick(-1, 30, 15.3, DEFAULT_Z)!).toBeLessThan(0);
  });
  it('does nothing in the dead zone', () => expect(zStepFromStick(0.05, 30, 15.3, DEFAULT_Z)).toBeNull());
  it('steps down in small increments near the paper', () => {
    const far = Math.abs(zStepFromStick(-1, 25, 15.3, DEFAULT_Z)!), near = Math.abs(zStepFromStick(-1, 17, 15.3, DEFAULT_Z)!);
    expect(far).toBeCloseTo(1.0, 6); expect(near).toBeLessThanOrEqual(0.3 + 1e-9);
  });
  it('never asks to go below the verified minimum', () => {
    expect(13.55 + (zStepFromStick(-1, 13.55, 15.3, DEFAULT_Z) ?? 0)).toBeGreaterThanOrEqual(13.5 - 1e-9);
    const s = zStepFromStick(-1, 13.8, 15.3, DEFAULT_Z)!; expect(13.8 + s).toBeGreaterThanOrEqual(13.5 - 1e-9);
    expect(zStepFromStick(-1, 13.5, 15.3, DEFAULT_Z)).toBeNull();
  });
  it('raising is allowed from the lowest height', () => expect(zStepFromStick(1, 13.5, 15.3, DEFAULT_Z)!).toBeGreaterThan(0));
});
