//! Small planar geometry kit: 3x3 homographies (DLT), point mapping, convex-quad ordering.

pub type H = [[f64; 3]; 3];

pub fn apply(h: &H, p: [f64; 2]) -> Option<[f64; 2]> {
    let w = h[2][0] * p[0] + h[2][1] * p[1] + h[2][2];
    if w.abs() < 1e-12 || !w.is_finite() {
        return None;
    }
    Some([(h[0][0] * p[0] + h[0][1] * p[1] + h[0][2]) / w, (h[1][0] * p[0] + h[1][1] * p[1] + h[1][2]) / w])
}

/// Solve A x = b (n x n) by Gaussian elimination with partial pivoting.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for c in 0..n {
        let piv = (c..n).max_by(|&i, &j| a[i][c].abs().partial_cmp(&a[j][c].abs()).unwrap())?;
        if a[piv][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, piv);
        b.swap(c, piv);
        for r in c + 1..n {
            let f = a[r][c] / a[c][c];
            for k in c..n {
                a[r][k] -= f * a[c][k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let s: f64 = (r + 1..n).map(|k| a[r][k] * x[k]).sum();
        x[r] = (b[r] - s) / a[r][r];
    }
    Some(x)
}

/// Least-squares homography mapping `src[i]` -> `dst[i]` (h22 = 1). Needs >= 4 non-degenerate points.
/// Inputs are normalised (centre + scale) for conditioning.
pub fn fit(src: &[[f64; 2]], dst: &[[f64; 2]]) -> Option<H> {
    let n = src.len();
    if n < 4 || dst.len() != n {
        return None;
    }
    let norm = |pts: &[[f64; 2]]| {
        let cx = pts.iter().map(|p| p[0]).sum::<f64>() / n as f64;
        let cy = pts.iter().map(|p| p[1]).sum::<f64>() / n as f64;
        let d = pts.iter().map(|p| ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt()).sum::<f64>() / n as f64;
        let s = if d > 1e-12 { std::f64::consts::SQRT_2 / d } else { 1.0 };
        ([[s, 0.0, -s * cx], [0.0, s, -s * cy], [0.0, 0.0, 1.0]], s, cx, cy)
    };
    let (ts, ..) = norm(src);
    let (td, ..) = norm(dst);
    let ap = |t: &H, p: [f64; 2]| [t[0][0] * p[0] + t[0][2], t[1][1] * p[1] + t[1][2]];
    // normal equations of the 2n x 8 system
    let mut ata = vec![vec![0.0; 8]; 8];
    let mut atb = vec![0.0; 8];
    for i in 0..n {
        let [x, y] = ap(&ts, src[i]);
        let [u, v] = ap(&td, dst[i]);
        let rows = [([x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y], u), ([0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y], v)];
        for (r, rhs) in rows {
            for a in 0..8 {
                for b in 0..8 {
                    ata[a][b] += r[a] * r[b];
                }
                atb[a] += r[a] * rhs;
            }
        }
    }
    let h = solve(ata, atb)?;
    let hn: H = [[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], 1.0]];
    // H = Td^-1 * Hn * Ts
    let td_inv = invert(&td)?;
    let full = mul(&mul(&td_inv, &hn), &ts);
    let k = full[2][2];
    if k.abs() < 1e-15 {
        return None;
    }
    let mut out = full;
    for r in out.iter_mut() {
        for v in r.iter_mut() {
            *v /= k;
        }
    }
    Some(out)
}

pub fn mul(a: &H, b: &H) -> H {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

pub fn invert(m: &H) -> Option<H> {
    let c = |r: usize, k: usize| m[(r + 1) % 3][(k + 1) % 3] * m[(r + 2) % 3][(k + 2) % 3] - m[(r + 1) % 3][(k + 2) % 3] * m[(r + 2) % 3][(k + 1) % 3];
    let det = m[0][0] * c(0, 0) + m[0][1] * c(0, 1) + m[0][2] * c(0, 2);
    if det.abs() < 1e-18 || !det.is_finite() {
        return None;
    }
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = c(j, i) / det;
        }
    }
    Some(o)
}

/// RMS reprojection error of `h` over the correspondences (in dst units).
pub fn rms(h: &H, src: &[[f64; 2]], dst: &[[f64; 2]]) -> f64 {
    let s: f64 = src.iter().zip(dst).map(|(a, b)| apply(h, *a).map(|p| (p[0] - b[0]).powi(2) + (p[1] - b[1]).powi(2)).unwrap_or(1e12)).sum();
    (s / src.len().max(1) as f64).sqrt()
}

/// Worst leave-one-out error: fit without point i, predict point i. Honest accuracy estimate for redundant data.
pub fn loo_max(src: &[[f64; 2]], dst: &[[f64; 2]]) -> Option<f64> {
    if src.len() < 5 {
        return None;
    }
    let mut worst: f64 = 0.0;
    for i in 0..src.len() {
        let s: Vec<_> = src.iter().enumerate().filter(|(k, _)| *k != i).map(|(_, p)| *p).collect();
        let d: Vec<_> = dst.iter().enumerate().filter(|(k, _)| *k != i).map(|(_, p)| *p).collect();
        let h = fit(&s, &d)?;
        let p = apply(&h, src[i])?;
        worst = worst.max(((p[0] - dst[i][0]).powi(2) + (p[1] - dst[i][1]).powi(2)).sqrt());
    }
    Some(worst)
}

pub fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Order four points counter-clockwise (in a y-up frame) starting from the one at `start_angle`-nearest edge direction:
/// returns [BL, BR, TR, TL] such that BL->BR points closest to `heading_deg` (0 = +X, 90 = +Y).
pub fn order_quad(p: &[[f64; 2]; 4], heading_deg: f64) -> Option<[usize; 4]> {
    let cx = p.iter().map(|q| q[0]).sum::<f64>() / 4.0;
    let cy = p.iter().map(|q| q[1]).sum::<f64>() / 4.0;
    let mut idx: [usize; 4] = [0, 1, 2, 3];
    idx.sort_by(|&a, &b| (p[a][1] - cy).atan2(p[a][0] - cx).partial_cmp(&(p[b][1] - cy).atan2(p[b][0] - cx)).unwrap());
    // polygon must be convex and non-degenerate
    let mut sign = 0.0f64;
    for i in 0..4 {
        let (a, b, c) = (p[idx[i]], p[idx[(i + 1) % 4]], p[idx[(i + 2) % 4]]);
        let cr = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if cr.abs() < 1e-6 || (sign != 0.0 && cr.signum() != sign) {
            return None;
        }
        sign = cr.signum();
    }
    let target = heading_deg.to_radians();
    let mut best = (f64::MAX, 0);
    for k in 0..4 {
        let (a, b) = (p[idx[k]], p[idx[(k + 1) % 4]]);
        let mut d = (b[1] - a[1]).atan2(b[0] - a[0]) - target;
        while d > std::f64::consts::PI {
            d -= 2.0 * std::f64::consts::PI;
        }
        while d < -std::f64::consts::PI {
            d += 2.0 * std::f64::consts::PI;
        }
        if d.abs() < best.0 {
            best = (d.abs(), k);
        }
    }
    let k = best.1;
    Some([idx[k], idx[(k + 1) % 4], idx[(k + 2) % 4], idx[(k + 3) % 4]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn truth() -> H {
        [[1.9, 0.2, 30.0], [-0.1, 2.1, 12.0], [1e-4, -2e-4, 1.0]]
    }
    fn grid() -> Vec<[f64; 2]> {
        vec![[0., 0.], [100., 0.], [100., 80.], [0., 80.], [50., 40.], [20., 70.]]
    }

    #[test]
    fn fit_recovers_exact_homography() {
        let src = grid();
        let dst: Vec<_> = src.iter().map(|p| apply(&truth(), *p).unwrap()).collect();
        let h = fit(&src, &dst).unwrap();
        for p in [[10., 10.], [90., 60.], [33., 71.]] {
            let a = apply(&h, p).unwrap();
            let b = apply(&truth(), p).unwrap();
            assert!(dist(a, b) < 1e-6, "{a:?} {b:?}");
        }
        assert!(rms(&h, &src, &dst) < 1e-6);
    }

    #[test]
    fn invert_round_trips() {
        let h = truth();
        let i = invert(&h).unwrap();
        let p = apply(&i, apply(&h, [12.0, 34.0]).unwrap()).unwrap();
        assert!(dist(p, [12.0, 34.0]) < 1e-9);
    }

    #[test]
    fn fit_rejects_degenerate_and_loo_flags_outlier() {
        assert!(fit(&[[0., 0.], [1., 1.], [2., 2.], [3., 3.]], &[[0., 0.], [1., 1.], [2., 2.], [3., 3.]]).is_none());
        let src = grid();
        let mut dst: Vec<_> = src.iter().map(|p| apply(&truth(), *p).unwrap()).collect();
        assert!(loo_max(&src, &dst).unwrap() < 1e-5);
        dst[4][0] += 8.0;
        assert!(loo_max(&src, &dst).unwrap() > 2.0);
    }

    #[test]
    fn quad_order_follows_heading() {
        // unordered rectangle corners, rotated 0 deg: BL must be (0,0)
        let q = [[10., 20.], [0., 0.], [10., 0.], [0., 20.]];
        let o = order_quad(&q, 0.0).unwrap();
        assert_eq!(q[o[0]], [0., 0.]);
        assert_eq!(q[o[1]], [10., 0.]);
        assert_eq!(q[o[3]], [0., 20.]);
        // heading 180: BL->BR points to -X, so BL is the top-right corner
        let o = order_quad(&q, 180.0).unwrap();
        assert_eq!(q[o[0]], [10., 20.]);
        // concave quad rejected
        assert!(order_quad(&[[0., 0.], [10., 0.], [10., 10.], [9., 1.]], 0.0).is_none());
    }
}
