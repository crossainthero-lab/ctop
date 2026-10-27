//! Path planner: ordering and joining of pen strokes in PAPER space.
//! Pure geometry; knows nothing about the machine.

use serde::{Deserialize, Serialize};

pub type Pt = [f64; 2];
pub type Stroke = Vec<Pt>;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Order {
    /// Exactly the order the document produced.
    AsDrawn,
    /// Nearest-neighbour with stroke reversal to cut pen-up travel.
    #[default]
    Optimized,
    /// Strokes sorted by their top edge (good for writing; hand never smudges fresh lines).
    TopToBottom,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct PlanOptions {
    pub order: Order,
    /// Consecutive strokes whose gap is below this are merged into one pen-down path.
    pub join_gap_mm: f64,
    /// Strokes shorter than this are dropped.
    pub min_length_mm: f64,
}

impl Default for PlanOptions {
    fn default() -> Self {
        PlanOptions { order: Order::Optimized, join_gap_mm: 0.2, min_length_mm: 0.1 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub strokes: Vec<Stroke>,
    pub draw_length_mm: f64,
    pub travel_length_mm: f64,
    pub pen_lifts: usize,
}

fn dist(a: Pt, b: Pt) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

pub fn stroke_length(s: &[Pt]) -> f64 {
    s.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// Remove non-finite points and consecutive duplicates.
fn clean(s: &Stroke) -> Stroke {
    let mut out: Stroke = Vec::with_capacity(s.len());
    for p in s {
        if !p[0].is_finite() || !p[1].is_finite() {
            continue;
        }
        if out.last().map_or(true, |l| dist(*l, *p) > 1e-4) {
            out.push(*p);
        }
    }
    out
}

pub fn plan(input: &[Stroke], opts: &PlanOptions, start: Pt) -> Plan {
    let mut strokes: Vec<Stroke> = input
        .iter()
        .map(clean)
        .filter(|s| s.len() >= 2 && stroke_length(s) >= opts.min_length_mm.max(1e-4))
        .collect();

    match opts.order {
        Order::AsDrawn => {}
        Order::TopToBottom => {
            let top = |s: &Stroke| s.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
            // stable sort keeps document order within the same row
            strokes.sort_by(|a, b| top(a).partial_cmp(&top(b)).unwrap_or(std::cmp::Ordering::Equal));
        }
        Order::Optimized => strokes = nearest_neighbour(strokes, start),
    }

    // join consecutive strokes that meet (saves a pen lift without changing the picture)
    let mut joined: Vec<Stroke> = Vec::with_capacity(strokes.len());
    for s in strokes {
        match joined.last_mut() {
            Some(last) if dist(*last.last().unwrap(), s[0]) <= opts.join_gap_mm => {
                if dist(*last.last().unwrap(), s[0]) > 1e-4 {
                    last.push(s[0]);
                }
                last.extend_from_slice(&s[1..]);
            }
            _ => joined.push(s),
        }
    }

    let draw: f64 = joined.iter().map(|s| stroke_length(s)).sum();
    let mut travel = 0.0;
    let mut cur = start;
    for s in &joined {
        travel += dist(cur, s[0]);
        cur = *s.last().unwrap();
    }
    Plan { pen_lifts: joined.len(), strokes: joined, draw_length_mm: draw, travel_length_mm: travel }
}

fn nearest_neighbour(mut rest: Vec<Stroke>, start: Pt) -> Vec<Stroke> {
    let mut out = Vec::with_capacity(rest.len());
    let mut cur = start;
    while !rest.is_empty() {
        let mut best = (0usize, false, f64::INFINITY);
        for (i, s) in rest.iter().enumerate() {
            let df = dist(cur, s[0]);
            let dr = dist(cur, *s.last().unwrap());
            if df < best.2 {
                best = (i, false, df);
            }
            if dr < best.2 {
                best = (i, true, dr);
            }
        }
        let mut s = rest.swap_remove(best.0);
        if best.1 {
            s.reverse();
        }
        cur = *s.last().unwrap();
        out.push(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(a: Pt, b: Pt) -> Stroke {
        vec![a, b]
    }

    #[test]
    fn as_drawn_keeps_order() {
        let s = vec![line([50.0, 0.0], [60.0, 0.0]), line([0.0, 0.0], [10.0, 0.0])];
        let p = plan(&s, &PlanOptions { order: Order::AsDrawn, ..Default::default() }, [0.0, 0.0]);
        assert_eq!(p.strokes[0][0], [50.0, 0.0]);
    }

    #[test]
    fn optimized_reduces_travel_and_preserves_ink() {
        let s = vec![
            line([100.0, 0.0], [110.0, 0.0]),
            line([0.0, 0.0], [10.0, 0.0]),
            line([50.0, 0.0], [60.0, 0.0]),
            line([20.0, 0.0], [10.5, 0.0]),
        ];
        let a = plan(&s, &PlanOptions { order: Order::AsDrawn, join_gap_mm: 0.0, ..Default::default() }, [0.0, 0.0]);
        let o = plan(&s, &PlanOptions { join_gap_mm: 0.0, ..Default::default() }, [0.0, 0.0]);
        assert!(o.travel_length_mm < a.travel_length_mm);
        assert!((o.draw_length_mm - a.draw_length_mm).abs() < 1e-9);
        assert_eq!(o.strokes.len(), 4);
    }

    #[test]
    fn reverses_stroke_when_end_is_closer() {
        let s = vec![line([10.0, 0.0], [1.0, 0.0])];
        let p = plan(&s, &PlanOptions::default(), [0.0, 0.0]);
        assert_eq!(p.strokes[0][0], [1.0, 0.0]);
    }

    #[test]
    fn joins_touching_strokes() {
        let s = vec![line([0.0, 0.0], [10.0, 0.0]), line([10.1, 0.0], [20.0, 0.0])];
        let p = plan(&s, &PlanOptions { order: Order::AsDrawn, ..Default::default() }, [0.0, 0.0]);
        assert_eq!(p.strokes.len(), 1);
        assert_eq!(p.pen_lifts, 1);
    }

    #[test]
    fn drops_degenerate_and_nonfinite() {
        let s = vec![vec![[1.0, 1.0]], vec![[1.0, 1.0], [1.0, 1.0]], vec![[0.0, 0.0], [f64::NAN, 0.0], [5.0, 0.0]]];
        let p = plan(&s, &PlanOptions::default(), [0.0, 0.0]);
        assert_eq!(p.strokes.len(), 1);
        assert!(p.strokes[0].iter().all(|q| q[0].is_finite()));
    }

    #[test]
    fn top_to_bottom_sorts_rows() {
        let s = vec![line([0.0, 30.0], [5.0, 30.0]), line([0.0, 10.0], [5.0, 10.0])];
        let p = plan(&s, &PlanOptions { order: Order::TopToBottom, join_gap_mm: 0.0, ..Default::default() }, [0.0, 0.0]);
        assert_eq!(p.strokes[0][0][1], 10.0);
    }
}
