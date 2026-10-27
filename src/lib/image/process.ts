import type { RasterImage } from '../types';

/** Single-channel float image, values 0 (black) .. 1 (white). */
export interface Gray {
  width: number;
  height: number;
  data: Float32Array;
}

/** Luma of an RGBA image with alpha composited over white. */
export function toGray(img: RasterImage): Gray {
  const { width, height, data } = img;
  const out = new Float32Array(width * height);
  for (let i = 0; i < out.length; i++) {
    const a = data[i * 4 + 3] / 255;
    const y = (0.2126 * data[i * 4] + 0.7152 * data[i * 4 + 1] + 0.0722 * data[i * 4 + 2]) / 255;
    out[i] = y * a + (1 - a);
  }
  return { width, height, data: out };
}

/** brightness in [-1,1] added after contrast; contrast in [-1,1] (0 = unchanged). */
export function adjust(g: Gray, brightness = 0, contrast = 0): Gray {
  const k = contrast >= 0 ? 1 / (1 - Math.min(contrast, 0.99)) : 1 + contrast;
  const out = new Float32Array(g.data.length);
  for (let i = 0; i < out.length; i++) out[i] = Math.min(1, Math.max(0, (g.data[i] - 0.5) * k + 0.5 + brightness));
  return { ...g, data: out };
}

export function gaussianBlur(g: Gray, sigma: number): Gray {
  if (sigma < 0.3) return g;
  const r = Math.max(1, Math.ceil(sigma * 3));
  const kern = new Float32Array(2 * r + 1);
  let sum = 0;
  for (let i = -r; i <= r; i++) sum += kern[i + r] = Math.exp(-(i * i) / (2 * sigma * sigma));
  for (let i = 0; i < kern.length; i++) kern[i] /= sum;
  const { width: w, height: h } = g;
  const tmp = new Float32Array(w * h), out = new Float32Array(w * h);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    let a = 0;
    for (let i = -r; i <= r; i++) a += g.data[y * w + Math.min(w - 1, Math.max(0, x + i))] * kern[i + r];
    tmp[y * w + x] = a;
  }
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    let a = 0;
    for (let i = -r; i <= r; i++) a += tmp[Math.min(h - 1, Math.max(0, y + i)) * w + x] * kern[i + r];
    out[y * w + x] = a;
  }
  return { width: w, height: h, data: out };
}

/** Resample to exactly (w,h): area-average when shrinking, bilinear when enlarging. */
export function resize(g: Gray, w: number, h: number): Gray {
  w = Math.max(1, Math.round(w)); h = Math.max(1, Math.round(h));
  if (w === g.width && h === g.height) return g;
  const out = new Float32Array(w * h);
  const sx = g.width / w, sy = g.height / h;
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    if (sx > 1 || sy > 1) {
      const x0 = x * sx, x1 = (x + 1) * sx, y0 = y * sy, y1 = (y + 1) * sy;
      let acc = 0, wt = 0;
      for (let yy = Math.floor(y0); yy < Math.min(g.height, Math.ceil(y1)); yy++) {
        const wy = Math.min(yy + 1, y1) - Math.max(yy, y0);
        for (let xx = Math.floor(x0); xx < Math.min(g.width, Math.ceil(x1)); xx++) {
          const wgt = wy * (Math.min(xx + 1, x1) - Math.max(xx, x0));
          acc += g.data[yy * g.width + xx] * wgt; wt += wgt;
        }
      }
      out[y * w + x] = wt ? acc / wt : 1;
    } else out[y * w + x] = sample(g, (x + 0.5) * sx - 0.5, (y + 0.5) * sy - 0.5);
  }
  return { width: w, height: h, data: out };
}

/** Bilinear sample in pixel-index coordinates (clamped to edges). */
export function sample(g: Gray, x: number, y: number): number {
  x = Math.min(g.width - 1, Math.max(0, x)); y = Math.min(g.height - 1, Math.max(0, y));
  const x0 = Math.floor(x), y0 = Math.floor(y), x1 = Math.min(g.width - 1, x0 + 1), y1 = Math.min(g.height - 1, y0 + 1);
  const fx = x - x0, fy = y - y0, d = g.data, w = g.width;
  return (d[y0 * w + x0] * (1 - fx) + d[y0 * w + x1] * fx) * (1 - fy) + (d[y1 * w + x0] * (1 - fx) + d[y1 * w + x1] * fx) * fy;
}
