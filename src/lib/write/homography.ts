// 3x3 homography helpers (row-major, 9 values). Pure, dependency-free.
export type H = number[];
type P = [number, number];

const mul = (a: H, b: H): H => {
  const o = new Array(9).fill(0);
  for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) for (let k = 0; k < 3; k++) o[i * 3 + j] += a[i * 3 + k] * b[k * 3 + j];
  return o;
};

/** Hartley normalisation: centroid to origin, mean distance sqrt(2). */
function normT(pts: P[]): H {
  const n = pts.length;
  const cx = pts.reduce((s, p) => s + p[0], 0) / n;
  const cy = pts.reduce((s, p) => s + p[1], 0) / n;
  const md = pts.reduce((s, p) => s + Math.hypot(p[0] - cx, p[1] - cy), 0) / n;
  const s = md > 1e-12 ? Math.SQRT2 / md : 1;
  return [s, 0, -s * cx, 0, s, -s * cy, 0, 0, 1];
}

/** Solve A x = b (square) by Gaussian elimination with partial pivoting. */
function solve(A: number[][], b: number[]): number[] | null {
  const n = b.length;
  const M = A.map((r, i) => [...r, b[i]]);
  for (let c = 0; c < n; c++) {
    let p = c;
    for (let r = c + 1; r < n; r++) if (Math.abs(M[r][c]) > Math.abs(M[p][c])) p = r;
    if (Math.abs(M[p][c]) < 1e-12) return null;
    [M[c], M[p]] = [M[p], M[c]];
    for (let r = c + 1; r < n; r++) {
      const f = M[r][c] / M[c][c];
      for (let k = c; k <= n; k++) M[r][k] -= f * M[c][k];
    }
  }
  const x = new Array(n).fill(0);
  for (let r = n - 1; r >= 0; r--) {
    let s = M[r][n];
    for (let k = r + 1; k < n; k++) s -= M[r][k] * x[k];
    x[r] = s / M[r][r];
  }
  return x;
}

export function invertH(h: H): H | null {
  const [a, b, c, d, e, f, g, i, j] = h;
  const A = e * j - f * i, B = -(d * j - f * g), C = d * i - e * g;
  const det = a * A + b * B + c * C;
  if (Math.abs(det) < 1e-18) return null;
  const inv = [A, -(b * j - c * i), b * f - c * e, B, a * j - c * g, -(a * f - c * d), C, -(a * i - b * g), a * e - b * d].map((v) => v / det);
  return inv;
}

/** Least-squares homography mapping src -> dst (>= 4 correspondences), normalised DLT with h22 = 1. */
export function fitHomography(src: P[], dst: P[]): H | null {
  const n = src.length;
  if (n < 4 || dst.length !== n) return null;
  const Ts = normT(src), Td = normT(dst);
  const AtA = Array.from({ length: 8 }, () => new Array(8).fill(0));
  const Atb = new Array(8).fill(0);
  const add = (row: number[], v: number) => {
    for (let i = 0; i < 8; i++) {
      Atb[i] += row[i] * v;
      for (let j = 0; j < 8; j++) AtA[i][j] += row[i] * row[j];
    }
  };
  for (let k = 0; k < n; k++) {
    const x = Ts[0] * src[k][0] + Ts[2], y = Ts[4] * src[k][1] + Ts[5];
    const u = Td[0] * dst[k][0] + Td[2], v = Td[4] * dst[k][1] + Td[5];
    add([x, y, 1, 0, 0, 0, -u * x, -u * y], u);
    add([0, 0, 0, x, y, 1, -v * x, -v * y], v);
  }
  const s = solve(AtA, Atb);
  const Tdi = invertH(Td);
  if (!s || !Tdi) return null;
  const h = mul(mul(Tdi, [...s, 1]), Ts);
  if (!h.every(Number.isFinite) || Math.abs(h[8]) < 1e-12) return null;
  return h.map((v) => v / h[8]);
}

export function applyH(h: H, [x, y]: P): P | null {
  const w = h[6] * x + h[7] * y + h[8];
  if (Math.abs(w) < 1e-12) return null;
  return [(h[0] * x + h[1] * y + h[2]) / w, (h[3] * x + h[4] * y + h[5]) / w];
}
