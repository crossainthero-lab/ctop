// Shared geometry contract. All geometry in the TS engines is in PAPER space:
// millimetres, origin at the top-left of the sheet, +x right, +y DOWN (as the
// sheet is viewed). The Rust backend owns the paper->machine transform.

/** A point in millimetres. */
export type Pt = [number, number];

/** A pen-down polyline (>= 2 points). Strokes are never implicitly closed;
 *  a closed contour repeats its first point at the end. */
export type Stroke = Pt[];

export interface Bounds {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

/** Row-major RGBA image as produced by canvas getImageData. */
export interface RasterImage {
  width: number;
  height: number;
  data: Uint8ClampedArray | Uint8Array;
}
