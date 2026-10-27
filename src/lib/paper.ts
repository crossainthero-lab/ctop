export interface PaperPreset { id: string; name: string; w: number; h: number }
export const PAPER_PRESETS: PaperPreset[] = [
  { id: 'a3', name: 'A3', w: 297, h: 420 },
  { id: 'a4', name: 'A4', w: 210, h: 297 },
  { id: 'a5', name: 'A5', w: 148, h: 210 },
  { id: 'a6', name: 'A6', w: 105, h: 148 },
  { id: 'letter', name: 'Letter', w: 215.9, h: 279.4 },
  { id: 'legal', name: 'Legal', w: 215.9, h: 355.6 },
  { id: 'custom', name: 'Custom', w: 200, h: 200 },
];
