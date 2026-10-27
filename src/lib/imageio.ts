import type { RasterImage } from './types';

export interface Decoded { raster: RasterImage; width: number; height: number }

export function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((res, rej) => {
    const im = new Image();
    im.onload = () => res(im);
    im.onerror = () => rej(new Error('Could not decode image'));
    im.src = src;
  });
}

/** Decode an image to RGBA pixels, downscaling very large images (conversion never needs more). */
export async function decodeDataUrl(src: string, maxDim = 1400): Promise<Decoded> {
  const im = await loadImage(src);
  const k = Math.min(1, maxDim / Math.max(im.naturalWidth, im.naturalHeight));
  const w = Math.max(1, Math.round(im.naturalWidth * k)), h = Math.max(1, Math.round(im.naturalHeight * k));
  const cv = document.createElement('canvas');
  cv.width = w; cv.height = h;
  const ctx = cv.getContext('2d', { willReadFrequently: true })!;
  ctx.fillStyle = '#fff';
  ctx.fillRect(0, 0, w, h);
  ctx.drawImage(im, 0, 0, w, h);
  const d = ctx.getImageData(0, 0, w, h);
  return { raster: { width: w, height: h, data: d.data }, width: im.naturalWidth, height: im.naturalHeight };
}

export function fileToDataUrl(f: Blob): Promise<string> {
  return new Promise((res, rej) => {
    const r = new FileReader();
    r.onload = () => res(String(r.result));
    r.onerror = () => rej(r.error);
    r.readAsDataURL(f);
  });
}
