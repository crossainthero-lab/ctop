// The four-corner calibration as a list of steps, adapted to what the plotter can physically reach.
import { paperToPen, inReach, type FrameProfile, type Reach } from './frame';

export type MarkKind = 'corner' | 'edge';
export interface Mark { kind: MarkKind; name: string; x: number; y: number }
export interface Step {
  id: string;
  kind: MarkKind;
  name: string;
  title: string;
  /** where to look, in words anyone can follow standing at the front of the printer */
  where: string;
  status: 'todo' | 'done' | 'skipped';
  mark?: Mark;
  /** the guess puts this spot out of the machine's reach */
  likelyUnreachable?: boolean;
}

export const CORNERS: Omit<Step, 'status' | 'id'>[] = [
  { kind: 'corner', name: 'tl', title: 'Top-left corner', where: 'the corner at the BACK-LEFT of the sheet: farthest from you, on your left' },
  { kind: 'corner', name: 'tr', title: 'Top-right corner', where: 'the corner at the BACK-RIGHT of the sheet: farthest from you, on your right' },
  { kind: 'corner', name: 'br', title: 'Bottom-right corner', where: 'the corner at the FRONT-RIGHT of the sheet: nearest the LCD screen, on your right' },
  { kind: 'corner', name: 'bl', title: 'Bottom-left corner', where: 'the corner at the FRONT-LEFT of the sheet: nearest the LCD screen, on your left' },
];

/** Reference points along an edge, for sheets whose corners are beyond the machine's travel. */
export const EDGE_STEPS: Omit<Step, 'status' | 'id'>[] = [
  { kind: 'edge', name: 'right', title: 'Right edge: a point near the front', where: 'the RIGHT-hand edge of the sheet, close to the LCD end' },
  { kind: 'edge', name: 'right', title: 'Right edge: a point further back', where: 'the RIGHT-hand edge again, as far toward the back as the pen will go (at least 100 mm from the first point)' },
  { kind: 'edge', name: 'front', title: 'Front edge: a point on the right', where: 'the FRONT edge (the short edge nearest the LCD), toward your right' },
  { kind: 'edge', name: 'front', title: 'Front edge: a point further left', where: 'the FRONT edge again, as far to your left as the pen will go (at least 60 mm from the first point)' },
  { kind: 'edge', name: 'left', title: 'Left edge: any point', where: 'the LEFT-hand edge of the sheet' },
  { kind: 'edge', name: 'back', title: 'Back edge: any point', where: 'the BACK edge of the sheet (the short edge farthest from you)' },
];

const CORNER_PAGE: Record<string, [number, number]> = { bl: [0, 0], br: [1, 0], tr: [1, 1], tl: [0, 1] };

/** Is the guessed position of this corner inside the machine's reach (with slack for a wrong guess)? */
export function cornerReachable(name: string, f: FrameProfile | null, reach: Reach | null, slackMm = 12): boolean | null {
  if (!f || !reach) return null;
  const [u, v] = CORNER_PAGE[name];
  const w = paperToPen(f, [u * f.pageW, f.pageH - v * f.pageH]);
  return inReach(w, [reach[0] - slackMm, reach[1] + slackMm, reach[2] - slackMm, reach[3] + slackMm], 0);
}

export function initialSteps(f: FrameProfile | null, reach: Reach | null): Step[] {
  const steps: Step[] = CORNERS.map((c) => ({ ...c, id: c.name, status: 'todo' as const }));
  for (const s of steps) {
    if (cornerReachable(s.name, f, reach) === false) { s.likelyUnreachable = true; }
  }
  return steps;
}

/** After the corners, which edge reference points to ask for next: every edge step not yet done, in order. */
export function nextEdgeSteps(steps: Step[]): Step[] {
  const have = steps.filter((s) => s.kind === 'edge').length;
  const out: Step[] = [];
  for (let i = have; i < EDGE_STEPS.length; i++) out.push({ ...EDGE_STEPS[i], id: `edge${i}`, status: 'todo' });
  return out;
}

export const marksOf = (steps: Step[]): Mark[] => steps.filter((s) => s.status === 'done' && s.mark).map((s) => s.mark!);
