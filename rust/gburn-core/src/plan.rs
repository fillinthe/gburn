//! The cut plan: every laser-on stroke, in the order the machine runs them, with its layer's settings, plus travel
//! and a time estimate. Line layers follow the outline; Fill layers scan the inside with parallel lines. Shapes of a
//! fill layer are filled together (even-odd), so a ring stays a ring.

use crate::{bounds, flatten, geom, pt, Contour, Document, Mode, Pt};

/// one laser-on polyline
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub pts: Vec<Pt>,
    pub layer: usize,
    /// power per segment as a fraction of the layer's (None = full all along): overscan dead runs, greyscale
    pub pw: Option<Vec<f32>>,
    /// dot mode: a single point fired for this long (ms); 0 = a normal stroke
    pub dwell_ms: f32,
    /// a closed loop that starts where its shape says (ordering doesn't move its start)
    pub keep_start: bool,
}

impl Stroke {
    pub fn new(pts: Vec<Pt>, layer: usize) -> Stroke {
        Stroke { pts, layer, pw: None, dwell_ms: 0.0, keep_start: false }
    }
    pub fn dot(p: Pt, layer: usize, ms: f64) -> Stroke {
        Stroke { pts: vec![p, p], layer, pw: None, dwell_ms: ms as f32, keep_start: false }
    }
}

/// Points `step` apart along a polyline (the first always, the last when the leftover is more than half a step).
pub fn points_along(p: &[Pt], step: f64) -> Vec<Pt> {
    let step = step.max(0.01);
    let mut out = vec![p[0]];
    let mut carry = 0.0;
    for w in p.windows(2) {
        let l = dist(w[0], w[1]);
        let mut d = step - carry;
        while d <= l {
            out.push(pt(w[0].x + (w[1].x - w[0].x) * d / l, w[0].y + (w[1].y - w[0].y) * d / l));
            d += step;
        }
        carry = l - (d - step);
    }
    // a closed loop: the point that lands back on the start is the start
    if out.len() > 1 && dist(out[0], *out.last().unwrap()) < 1e-6 {
        out.pop();
    }
    out
}

/// The convex hull of points (monotone chain), counter-clockwise, first point repeated at the end.
pub fn convex_hull(pts: &[Pt]) -> Vec<Pt> {
    let mut v: Vec<Pt> = pts.to_vec();
    v.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap().then(a.y.partial_cmp(&b.y).unwrap()));
    v.dedup_by(|a, b| (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9);
    if v.len() < 3 {
        return v;
    }
    let cross = |o: Pt, a: Pt, b: Pt| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut lower: Vec<Pt> = Vec::new();
    for p in &v {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], *p) <= 0.0 {
            lower.pop();
        }
        lower.push(*p);
    }
    let mut upper: Vec<Pt> = Vec::new();
    for p in v.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], *p) <= 0.0 {
            upper.pop();
        }
        upper.push(*p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower.push(lower[0]);
    lower
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub strokes: Vec<Stroke>,
    /// mm with the laser on / off
    pub cut_mm: f64,
    pub travel_mm: f64,
    pub seconds: f64,
    /// per stroke: seconds when its jump starts, when its cut starts, when its cut ends (the simulation scrubs by time)
    pub times: Vec<[f64; 3]>,
    /// seconds per layer (index = layer)
    pub layer_seconds: Vec<f64>,
    /// per stroke: Z from the job's start height (mm); empty when no layer moves Z
    pub z: Vec<f64>,
}

/// Where the head is at one moment of the simulated job.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadAt {
    pub stroke: usize,
    pub pos: Pt,
    /// cutting (else travelling to the stroke)
    pub on: bool,
    /// points of the stroke already cut: `pts[..done]` then `pos`
    pub done: usize,
}

impl Plan {
    /// The head at `t` seconds into the job (clamped to the job's length); None for an empty plan.
    pub fn head_at(&self, t: f64) -> Option<HeadAt> {
        if self.strokes.is_empty() {
            return None;
        }
        let i = self.times.partition_point(|t3| t3[2] <= t).min(self.strokes.len() - 1);
        let s = &self.strokes[i];
        let [t0, tc, t1] = self.times[i];
        let from = if i == 0 { pt(0.0, 0.0) } else { *self.strokes[i - 1].pts.last().unwrap() };
        let len: f64 = s.pts.windows(2).map(|w| dist(w[0], w[1])).sum();
        if t < tc {
            let k = if tc > t0 { ((t - t0) / (tc - t0)).clamp(0.0, 1.0) } else { 1.0 };
            return Some(HeadAt { stroke: i, pos: pt(from.x + (s.pts[0].x - from.x) * k, from.y + (s.pts[0].y - from.y) * k), on: false, done: 0 });
        }
        let mut along = if t1 > tc { ((t - tc) / (t1 - tc)).clamp(0.0, 1.0) * len } else { len };
        for (k, w) in s.pts.windows(2).enumerate() {
            let d = dist(w[0], w[1]);
            if along <= d || k == s.pts.len() - 2 {
                let u = if d > 0.0 { (along / d).clamp(0.0, 1.0) } else { 1.0 };
                return Some(HeadAt { stroke: i, pos: pt(w[0].x + (w[1].x - w[0].x) * u, w[0].y + (w[1].y - w[0].y) * u), on: true, done: if u >= 1.0 { k + 2 } else { k + 1 } });
            }
            along -= d;
        }
        Some(HeadAt { stroke: i, pos: *s.pts.last().unwrap(), on: true, done: s.pts.len() })
    }
}

/// Machine numbers the estimate needs. Hardware is never ideal: leave the knobs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Machine {
    /// mm/s travel speed with the laser off
    pub rapid: f64,
    /// mm/s² (GRBL $120 / $121; Ruida's own)
    pub accel: f64,
    /// Line layers: nearest-first with holes before outlines (true) or in drawing order (false)
    pub optimize: bool,
    /// Line layers: a stretch already cut by an earlier line of the same layer is skipped (shared edges cut once)
    pub remove_overlap: bool,
    /// LightBurn's Order by Group: each group (or lone shape) gets all its layers before the next one starts,
    /// nearest first
    pub by_group: bool,
    /// LightBurn's Choose corners: closed shapes start at a sharp corner
    pub corners: bool,
    /// Cut in direction order: 0 = nearest first, 1 top → bottom, 2 bottom → top, 3 left → right, 4 right → left
    /// (rows 3 mm deep, nearest first within a row)
    pub sweep: u8,
    /// Reduce direction changes: the next cut is chosen as much for carrying on the same way as for being near
    pub fewer_turns: bool,
    /// Hide backlash: closed shapes start at their lowest-left point and run anticlockwise, so every start and end is
    /// met moving the same way
    pub backlash: bool,
    /// time estimate only: acceleration for fills and pictures (0 = `accel`), and each axis's top speed (mm/s, 0 = none)
    pub accel_fill: f64,
    pub max_xy: [f64; 2],
    /// time estimate only: GRBL's junction deviation ($11, mm): corners slow the head down (0 = corners free)
    pub corner_dev: f64,
}

impl Default for Machine {
    fn default() -> Self {
        Machine {
            rapid: 200.0,
            accel: 2500.0,
            optimize: true,
            remove_overlap: false,
            by_group: false,
            corners: false,
            sweep: 0,
            fewer_turns: false,
            backlash: false,
            accel_fill: 0.0,
            max_xy: [0.0, 0.0],
            corner_dev: 0.0,
        }
    }
}

/// curve flattening tolerance, mm
pub const TOL: f64 = 0.02;

pub fn plan(doc: &Document, m: Machine) -> Plan {
    let (strokes, z) = if m.by_group {
        // units = groups and lone shapes, nearest first from where the last one ended
        let mut units: Vec<(Vec<i32>, crate::Rect)> = Vec::new();
        let mut of_group: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
        for s in &doc.shapes {
            match of_group.get(&s.group).filter(|_| s.group != 0) {
                Some(&u) => {
                    units[u].0.push(s.id);
                    units[u].1.add_rect(&bounds(&s.contour));
                }
                None => {
                    of_group.insert(s.group, units.len());
                    units.push((vec![s.id], bounds(&s.contour)));
                }
            }
        }
        let (mut strokes, mut z) = (Vec::new(), Vec::new());
        let mut at = pt(0.0, 0.0);
        while !units.is_empty() {
            let near = |r: &crate::Rect| dist(at, pt(at.x.clamp(r.x0, r.x1), at.y.clamp(r.y0, r.y1)));
            let i = if m.optimize { (0..units.len()).min_by(|&a, &b| near(&units[a].1).total_cmp(&near(&units[b].1))).unwrap_or(0) } else { 0 };
            let (ids, _) = units.remove(i);
            let (s, zz) = job_strokes(doc, m, &|s| ids.contains(&s.id));
            if let Some(last) = s.last() {
                at = *last.pts.last().unwrap();
            }
            strokes.extend(s);
            z.extend(zz);
        }
        (strokes, z)
    } else {
        // cut order priority: each priority's shapes (all their layers) before the next; one pass when all are 0
        let mut pri: Vec<i32> = doc.shapes.iter().map(|s| s.priority).collect();
        pri.sort_unstable();
        pri.dedup();
        if pri.len() <= 1 {
            job_strokes(doc, m, &|_| true)
        } else {
            let (mut strokes, mut z) = (Vec::new(), Vec::new());
            for p in pri {
                let (s, zz) = job_strokes(doc, m, &|s| s.priority == p);
                // Z per stroke stays aligned when only some buckets move Z
                if !zz.is_empty() || !z.is_empty() {
                    z.resize(strokes.len(), 0.0);
                    z.extend(if zz.is_empty() { vec![0.0; s.len()] } else { zz });
                }
                strokes.extend(s);
            }
            (strokes, z)
        }
    };
    timed(doc, m, strokes, z)
}

/// Every output layer's strokes (passes included) for the shapes `keep` lets through, and the Z of each (empty when
/// no layer moves Z).
/// More passes than this (a damaged or hostile file) would only hang the planner.
pub const MAX_PASSES: u32 = 1000;

fn job_strokes(doc: &Document, m: Machine, keep: &dyn Fn(&crate::Shape) -> bool) -> (Vec<Stroke>, Vec<f64>) {
    let mut strokes = Vec::new();
    let mut z = Vec::new();
    let uses_z = doc.layers.iter().any(|l| l.output && (l.z_offset != 0.0 || l.z_step != 0.0));
    for (li, layer) in doc.layers.iter().enumerate() {
        if !layer.output {
            continue;
        }
        let shapes: Vec<&crate::Shape> = doc.shapes.iter().filter(|s| s.layer == li && s.contour.nodes.len() >= 2 && keep(s)).collect();
        let layer_strokes = layer_strokes(&shapes, li, layer, m, layer.angle);
        if layer_strokes.is_empty() {
            continue;
        }
        for k in 0..layer.passes.clamp(1, MAX_PASSES) {
            if layer.mode == Mode::Image && layer.dither == crate::raster::Dither::Slices {
                // pass k of n burns what is darker than level (k + ½) / n: the brightness shift moves the picture's
                // threshold there (ponytail: brightness comes before gamma, so with a gamma the levels aren't even)
                let n = layer.passes.clamp(1, MAX_PASSES) as f64;
                let lift = ((k as f64 + 0.5) / n - 0.5) * 100.0;
                let owned: Vec<crate::Shape> = shapes
                    .iter()
                    .map(|s| {
                        let mut s = (*s).clone();
                        if let Some(r) = &mut s.raster {
                            r.brightness = (r.brightness + lift).clamp(-100.0, 100.0);
                        }
                        s
                    })
                    .collect();
                let refs: Vec<&crate::Shape> = owned.iter().collect();
                let mut l = layer.clone();
                l.dither = crate::raster::Dither::Threshold;
                let this = self::layer_strokes(&refs, li, &l, m, layer.angle);
                // cleanup: every n passes, this pass again at a share of the power (dead runs stay dead)
                let clean = layer.cleanup_every > 0 && (k + 1) % layer.cleanup_every == 0;
                let again: Vec<Stroke> = if clean {
                    let share = (layer.cleanup_power / 100.0).clamp(0.01, 1.0) as f32;
                    this.iter()
                        .map(|s| {
                            let segs = s.pts.len().saturating_sub(1);
                            let pw = (0..segs).map(|i| s.pw.as_ref().and_then(|v| v.get(i)).copied().unwrap_or(1.0).min(1.0) * share).collect();
                            Stroke { pw: Some(pw), ..s.clone() }
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                strokes.extend(this);
                strokes.extend(again);
            } else if k > 0 && layer.angle_step.abs() > 1e-9 && matches!(layer.mode, Mode::Fill | Mode::FillLine) {
                strokes.extend(self::layer_strokes(&shapes, li, layer, m, layer.angle + layer.angle_step * k as f64));
            } else {
                strokes.extend(layer_strokes.iter().cloned());
            }
            if uses_z {
                z.resize(strokes.len(), layer.z_offset + layer.z_step * k as f64);
            }
        }
    }
    (strokes, z)
}

/// The plan with its travel, times and totals.
fn timed(doc: &Document, m: Machine, strokes: Vec<Stroke>, z: Vec<f64>) -> Plan {
    let mut p = Plan { strokes, z, ..Default::default() };
    let mut at = pt(0.0, 0.0);
    // a move's speed along a direction: the asked speed, or less where one axis would go past its top speed
    let capped = |v: f64, a: Pt, b: Pt| -> f64 {
        let (l, mut v) = (dist(a, b), v);
        if l > 1e-12 {
            for (lim, d) in [(m.max_xy[0], (b.x - a.x).abs()), (m.max_xy[1], (b.y - a.y).abs())] {
                if lim > 0.0 && d > 1e-12 {
                    v = v.min(lim * l / d);
                }
            }
        }
        v.max(0.1)
    };
    for s in &p.strokes {
        let layer = &doc.layers[s.layer];
        let speed = layer.speed.max(0.1);
        let accel = if m.accel_fill > 0.0 && matches!(layer.mode, Mode::Fill | Mode::FillLine | Mode::Image | Mode::Offset) { m.accel_fill } else { m.accel };
        let len: f64 = s.pts.windows(2).map(|w| dist(w[0], w[1])).sum();
        let cut_t: f64 = s.pts.windows(2).map(|w| dist(w[0], w[1]) / capped(speed, w[0], w[1])).sum::<f64>() + corner_time(&s.pts, speed, accel, m.corner_dev);
        let jump = dist(at, s.pts[0]);
        p.cut_mm += len;
        p.travel_mm += jump;
        // ponytail: trapezoid per stroke and per jump (one accel + one decel each), corners inside a stroke free
        let t0 = p.seconds;
        let tc = t0 + jump / capped(m.rapid, at, s.pts[0]) + m.rapid / m.accel;
        p.seconds = tc + if s.dwell_ms > 0.0 { s.dwell_ms as f64 / 1000.0 } else { cut_t + speed / accel };
        p.times.push([t0, tc, p.seconds]);
        if p.layer_seconds.len() <= s.layer {
            p.layer_seconds.resize(s.layer + 1, 0.0);
        }
        p.layer_seconds[s.layer] += p.seconds - t0;
        at = *s.pts.last().unwrap();
    }
    p
}

/// Extra seconds the corners of a cut cost: GRBL's junction speed √(a·δ·sin(θ/2) / (1 − sin(θ/2))) (θ = the angle
/// between the reversed way in and the way out), slowing from `v` and back = (v − vj)² / (a·v) per corner, at most
/// the time of crossing half of each side at vj instead of v.
fn corner_time(pts: &[Pt], v: f64, a: f64, dev: f64) -> f64 {
    if dev <= 0.0 || pts.len() < 3 {
        return 0.0;
    }
    pts.windows(3)
        .map(|w| {
            let (l1, l2) = (dist(w[0], w[1]), dist(w[1], w[2]));
            if l1 < 1e-9 || l2 < 1e-9 {
                return 0.0;
            }
            // cos of the angle between the reversed way in and the way out
            let c = -((w[1].x - w[0].x) * (w[2].x - w[1].x) + (w[1].y - w[0].y) * (w[2].y - w[1].y)) / (l1 * l2);
            let sh = ((1.0 - c) / 2.0).max(0.0).sqrt(); // sin(θ/2)
            if sh > 0.999_999 {
                return 0.0; // straight on
            }
            let vj = (a * dev * sh / (1.0 - sh)).sqrt().min(v);
            let near = (l1 + l2) / 2.0;
            ((v - vj).powi(2) / (a * v)).min(near / vj.max(1e-3) - near / v)
        })
        .sum()
}

fn dist(a: Pt, b: Pt) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// Nearest-neighbour ordering from the origin; a stroke is reversed when its far end is nearer. Closed strokes are
/// rotated to start at the nearest vertex.
/// `corners`: closed loops start at their nearest sharp corner (the start mark hides there), not the nearest point
fn order(strokes: &mut Vec<Stroke>, m: &Machine, start: Pt) {
    let corners = m.corners;
    let mut out = Vec::with_capacity(strokes.len());
    let mut at = start;
    let mut heading: Option<Pt> = None; // the last cut's final direction (unit)
    let mut rest: Vec<Stroke> = std::mem::take(strokes);
    // direction order: a stroke's place along the sweep (smaller = sooner)
    let key = |s: &Stroke| -> f64 {
        let (mut lo, mut hi) = (pt(f64::INFINITY, f64::INFINITY), pt(f64::NEG_INFINITY, f64::NEG_INFINITY));
        for p in &s.pts {
            (lo.x, lo.y, hi.x, hi.y) = (lo.x.min(p.x), lo.y.min(p.y), hi.x.max(p.x), hi.y.max(p.y));
        }
        match m.sweep {
            1 => -hi.y,
            2 => lo.y,
            3 => lo.x,
            4 => -hi.x,
            _ => 0.0,
        }
    };
    let unit = |a: Pt, b: Pt| {
        let l = dist(a, b);
        (l > 1e-9).then(|| pt((b.x - a.x) / l, (b.y - a.y) / l))
    };
    // ponytail: nearest-first is O(n²); past BIG strokes (dot fields, huge imports) they're sorted into serpentine rows
    // once and each pick only looks at the next WINDOW of them. A spatial grid would keep true nearest-first.
    const BIG: usize = 20_000;
    const WINDOW: usize = 64;
    let big = rest.len() > BIG;
    if big {
        let (mut lo, mut hi) = (pt(f64::INFINITY, f64::INFINITY), pt(f64::NEG_INFINITY, f64::NEG_INFINITY));
        for p in rest.iter().map(|s| s.pts[0]) {
            (lo.x, lo.y, hi.x, hi.y) = (lo.x.min(p.x), lo.y.min(p.y), hi.x.max(p.x), hi.y.max(p.y));
        }
        let cell = (((hi.x - lo.x) * (hi.y - lo.y)) / rest.len() as f64).sqrt().max(1e-3);
        let place = |s: &Stroke| {
            let row = ((s.pts[0].y - lo.y) / cell) as i64;
            (row, if row % 2 == 0 { s.pts[0].x } else { -s.pts[0].x })
        };
        // reversed: picks are taken from the end
        rest.sort_by(|a, b| place(b).partial_cmp(&place(a)).unwrap_or(std::cmp::Ordering::Equal));
    }
    while !rest.is_empty() {
        let band = if m.sweep > 0 && !big { rest.iter().map(key).fold(f64::INFINITY, f64::min) + 3.0 } else { f64::INFINITY };
        let mut best = (f64::INFINITY, 0, false);
        let first = if big { rest.len().saturating_sub(WINDOW) } else { 0 };
        for (i, s) in rest.iter().enumerate().skip(first) {
            if m.sweep > 0 && key(s) > band {
                continue;
            }
            let n = s.pts.len();
            for (rev, from, next) in [(false, s.pts[0], s.pts[1.min(n - 1)]), (true, s.pts[n - 1], s.pts[n.saturating_sub(2)])] {
                let mut c = dist(at, from);
                if m.fewer_turns {
                    // up to half as far again for turning right round
                    if let (Some(h), Some(u)) = (heading, unit(from, next)) {
                        c *= 1.0 + 0.25 * (1.0 - (h.x * u.x + h.y * u.y));
                    }
                }
                if c < best.0 {
                    best = (c, i, rev);
                }
            }
        }
        let mut s = rest.swap_remove(best.1);
        if best.2 {
            s.pts.reverse();
        }
        if m.backlash && s.pts.first() == s.pts.last() && s.pts.len() > 3 && !s.keep_start {
            // anticlockwise from the lowest-left point
            if signed_area(&s.pts) < 0.0 {
                s.pts.reverse();
            }
            let n = s.pts.len() - 1;
            let k = (0..n).min_by(|&i, &j| (s.pts[i].y, s.pts[i].x).partial_cmp(&(s.pts[j].y, s.pts[j].x)).unwrap()).unwrap();
            let mut r: Vec<Pt> = s.pts[k..n].to_vec();
            r.extend_from_slice(&s.pts[..=k]);
            s.pts = r;
        } else if s.pts.first() == s.pts.last() && s.pts.len() > 3 && !s.keep_start {
            // closed: start at the vertex nearest to where we are (or the nearest corner that turns 30° or more)
            let n = s.pts.len() - 1;
            let sharp = |i: usize| {
                let (p, c, q) = (s.pts[(i + n - 1) % n], s.pts[i], s.pts[(i + 1) % n]);
                let (a1, a2) = ((c.y - p.y).atan2(c.x - p.x), (q.y - c.y).atan2(q.x - c.x));
                let mut t = (a2 - a1).abs() % (2.0 * std::f64::consts::PI);
                if t > std::f64::consts::PI {
                    t = 2.0 * std::f64::consts::PI - t;
                }
                t >= 30f64.to_radians()
            };
            let any_sharp = corners && (0..n).any(sharp);
            let k = (0..n).filter(|&i| !any_sharp || sharp(i)).min_by(|&i, &j| dist(at, s.pts[i]).partial_cmp(&dist(at, s.pts[j])).unwrap()).unwrap();
            let mut r: Vec<Pt> = s.pts[k..n].to_vec();
            r.extend_from_slice(&s.pts[..=k]);
            s.pts = r;
        }
        at = *s.pts.last().unwrap();
        let n = s.pts.len();
        heading = (n > 1).then(|| unit(s.pts[n - 2], s.pts[n - 1])).flatten();
        out.push(s);
    }
    *strokes = out;
}

/// A closed polyline cut in `n` places: `n` gaps of `tab_len` spread evenly round it (bridges that hold the part).
/// Returns the pieces between the gaps (the first and last joined when the loop closes on itself).
pub fn with_tabs(p: &[Pt], n: usize, tab_len: f64) -> Vec<Vec<Pt>> {
    let total: f64 = p.windows(2).map(|w| dist(w[0], w[1])).sum();
    tabs_at(p, &(0..n).map(|k| (k as f64 + 0.5) * total / n.max(1) as f64).collect::<Vec<_>>(), tab_len)
}

/// A closed polyline (first point repeated) cut with a `tab_len` gap centred at each distance along it in `centres`
/// (ascending, within the loop); the pieces between the gaps.
pub fn tabs_at(p: &[Pt], centres: &[f64], tab_len: f64) -> Vec<Vec<Pt>> {
    let total: f64 = p.windows(2).map(|w| dist(w[0], w[1])).sum();
    let n = centres.len();
    if n == 0 || tab_len <= 0.0 || total <= tab_len * n as f64 * 1.5 {
        return vec![p.to_vec()];
    }
    // gap k spans [c_k - tab/2, c_k + tab/2] along the loop
    // ponytail: a gap over the loop's start is slid just past it, and gaps closer than 1½ tabs merge into one
    let mut cs: Vec<f64> = centres.iter().map(|c| c.clamp(tab_len * 0.51, total - tab_len * 0.51)).collect();
    cs.dedup_by(|b, a| *b - *a < tab_len * 1.5);
    let n = cs.len();
    let gaps: Vec<(f64, f64)> = cs.iter().map(|c| (c - tab_len / 2.0, c + tab_len / 2.0)).collect();
    let at = |d: f64| -> Pt {
        let mut left = d;
        for w in p.windows(2) {
            let l = dist(w[0], w[1]);
            if left <= l {
                let u = if l > 0.0 { left / l } else { 0.0 };
                return pt(w[0].x + (w[1].x - w[0].x) * u, w[0].y + (w[1].y - w[0].y) * u);
            }
            left -= l;
        }
        *p.last().unwrap()
    };
    let mut pieces = Vec::new();
    // piece i runs from the end of gap i to the start of gap i+1 (the last wraps to the first)
    for i in 0..n {
        let (from, to) = (gaps[i].1, if i + 1 < n { gaps[i + 1].0 } else { gaps[0].0 + total });
        let mut pts = vec![at(from)];
        let mut d = 0.0;
        for w in p.windows(2) {
            let l = dist(w[0], w[1]);
            let vertex_at = d + l; // the end of this segment
            if vertex_at > from && vertex_at < to {
                pts.push(w[1]);
            }
            d += l;
        }
        if to > total {
            // wrapped: the vertices from the start of the loop up to the first gap
            let mut d = 0.0;
            for w in p.windows(2) {
                let l = dist(w[0], w[1]);
                if d + l < to - total {
                    pts.push(w[1]);
                }
                d += l;
            }
        }
        pts.push(at(if to > total { to - total } else { to }));
        pieces.push(pts);
    }
    pieces
}

/// The circle through three points (centre, radius); None when they lie on a line (LightBurn's Center Finder).
pub fn circle_through(a: Pt, b: Pt, c: Pt) -> Option<(Pt, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-9 {
        return None;
    }
    let (a2, b2, c2) = (a.x * a.x + a.y * a.y, b.x * b.x + b.y * b.y, c.x * c.x + c.y * c.y);
    let o = pt((a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d, (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d);
    Some((o, dist(o, a)))
}

/// The distances along polyline `p` (ascending) of the points in `pts` lying within `reach` of it.
pub fn tab_centres(p: &[Pt], pts: &[Pt], reach: f64) -> Vec<f64> {
    let mut out: Vec<f64> = pts
        .iter()
        .filter_map(|q| {
            let (mut best, mut at, mut d0) = (f64::INFINITY, 0.0, 0.0);
            for w in p.windows(2) {
                let l = dist(w[0], w[1]);
                let u = if l < 1e-12 { 0.0 } else { (((q.x - w[0].x) * (w[1].x - w[0].x) + (q.y - w[0].y) * (w[1].y - w[0].y)) / (l * l)).clamp(0.0, 1.0) };
                let d = dist(*q, pt(w[0].x + u * (w[1].x - w[0].x), w[0].y + u * (w[1].y - w[0].y)));
                if d < best {
                    (best, at) = (d, d0 + u * l);
                }
                d0 += l;
            }
            (best <= reach).then_some(at)
        })
        .collect();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out
}

/// A polyline cut as dashes: `cut` mm on, `skip` mm off, along its length (LightBurn's perforation mode).
pub fn perforate(p: &[Pt], cut: f64, skip: f64) -> Vec<Vec<Pt>> {
    let total: f64 = p.windows(2).map(|w| dist(w[0], w[1])).sum();
    if cut <= 0.0 || skip <= 0.0 || total <= cut {
        return vec![p.to_vec()];
    }
    let at = |d: f64| -> Pt {
        let mut left = d;
        for w in p.windows(2) {
            let l = dist(w[0], w[1]);
            if left <= l {
                let u = if l > 0.0 { left / l } else { 0.0 };
                return pt(w[0].x + (w[1].x - w[0].x) * u, w[0].y + (w[1].y - w[0].y) * u);
            }
            left -= l;
        }
        *p.last().unwrap()
    };
    let mut out = Vec::new();
    let mut from = 0.0;
    while from < total {
        let to = (from + cut).min(total);
        let mut pts = vec![at(from)];
        let mut d = 0.0;
        for w in p.windows(2) {
            let l = dist(w[0], w[1]);
            if d + l > from && d + l < to {
                pts.push(w[1]);
            }
            d += l;
        }
        pts.push(at(to));
        out.push(pts);
        from += cut + skip;
    }
    out
}

/// Print & Cut: the transform that takes design marks `d1`, `d2` onto where the laser found them, `m1`, `m2`:
/// turn about d1, scale only when `scale`, then move d1 onto m1. The second mark then lands on m2 exactly when
/// scaling (or when the two distances already match).
pub fn print_cut_transform(d1: Pt, d2: Pt, m1: Pt, m2: Pt, scale: bool) -> gcarve_core::geom::Affine {
    use gcarve_core::geom::Affine;
    let (dv, mv) = (pt(d2.x - d1.x, d2.y - d1.y), pt(m2.x - m1.x, m2.y - m1.y));
    let turn = mv.y.atan2(mv.x) - dv.y.atan2(dv.x);
    let k = if scale { dist(m1, m2) / dist(d1, d2).max(1e-9) } else { 1.0 };
    Affine::translate(m1.x, m1.y).then(&Affine::rotate(turn)).then(&Affine::scale(k, k)).then(&Affine::translate(-d1.x, -d1.y))
}

/// One layer's strokes for one pass; `angle` = the hatch angle (degrees) for Fill modes.
fn layer_strokes(shapes: &[&crate::Shape], li: usize, layer: &crate::Layer, m: Machine, angle: f64) -> Vec<Stroke> {
    match layer.mode {
        Mode::Line => {
            // kerf: closed shapes grow / shrink by half the beam; a kerf makes polylines of curves (clipper)
            let contours: Vec<Contour> = shapes.iter().map(|s| s.contour.clone()).collect();
            let cut: Vec<Contour> = if layer.kerf.abs() > 1e-6 {
                let (closed, open): (Vec<Contour>, Vec<Contour>) = contours.into_iter().partition(|c| c.closed);
                let mut v = gcarve_core::vecops::offset_contours(&orient_nested(&closed), layer.kerf / 2.0, true);
                v.extend(open);
                v
            } else {
                contours
            };
            // inner shapes first: deeper inside other shapes = earlier (a part must not drop before its holes are cut)
            let mut polys: Vec<Vec<Pt>> = cut.iter().map(|c| flatten(c, TOL)).collect();
            // shapes with a start point: their loop (or its kerf offset) begins at the outline point nearest it
            let starts: Vec<Pt> = shapes.iter().filter_map(|s| s.start.map(|p| pt(p[0], p[1]))).collect();
            let tab_pts: Vec<Pt> = shapes.iter().flat_map(|s| s.tabs.iter().map(|p| pt(p[0], p[1]))).collect();
            let mut fixed = vec![false; polys.len()];
            for (k, (p, c)) in polys.iter_mut().zip(&cut).enumerate() {
                if !c.closed || p.len() < 3 {
                    continue;
                }
                let Some((i, d2)) = starts.iter().flat_map(|s| p.iter().enumerate().map(move |(i, q)| (i, dist(*s, *q)))).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap()) else { continue };
                if d2 <= layer.kerf.abs() + 0.5 {
                    if p.first() == p.last() {
                        p.pop();
                    }
                    let n = p.len();
                    p.rotate_left(i % n);
                    fixed[k] = true;
                }
            }
            // how many closed outlines each one sits inside (odd = a hole)
            let nest: Vec<usize> = if (layer.inner_first && m.optimize) || layer.tabs_skip_inner {
                polys
                    .iter()
                    .enumerate()
                    .map(|(i, p)| polys.iter().enumerate().filter(|(j, q)| *j != i && cut[*j].closed && q.len() > 2 && geom::point_in_poly(p[0], q.len(), |k| (q[k].x, q[k].y))).count())
                    .collect()
            } else {
                vec![0; polys.len()]
            };
            let depth: Vec<usize> = if layer.inner_first && m.optimize { nest.clone() } else { vec![0; polys.len()] };
            let max_depth = depth.iter().copied().max().unwrap_or(0);
            let mut out = Vec::new();
            for d in (0..=max_depth).rev() {
                let mut group: Vec<Stroke> = polys
                    .iter()
                    .zip(&cut)
                    .zip(&depth)
                    .zip(&fixed)
                    .zip(&nest)
                    .filter(|(((_, dd), _), _)| **dd == d)
                    .flat_map(|((((p, c), _), keep), nd)| {
                        let mut p = p.clone();
                        if c.closed && p.first() != p.last() {
                            p.push(p[0]);
                        }
                        // hand-placed tabs on this loop (within the kerf + 0.5 mm of it), else the layer's even ones
                        let mine = if c.closed { tab_centres(&p, &tab_pts, layer.kerf.abs() + 0.5) } else { Vec::new() };
                        let pieces = if !mine.is_empty() {
                            tabs_at(&p, &mine, layer.tab_len)
                        } else if c.closed && layer.tabs > 0 && !(layer.tabs_skip_inner && nd % 2 == 1) {
                            with_tabs(&p, layer.tabs as usize, layer.tab_len)
                        } else {
                            vec![p]
                        };
                        if layer.dot_ms > 0.0 {
                            pieces.into_iter().flat_map(|p| points_along(&p, layer.dot_mm)).map(|q| Stroke::dot(q, li, layer.dot_ms)).collect::<Vec<_>>()
                        } else if layer.perf_cut > 0.0 && layer.perf_skip > 0.0 {
                            pieces.into_iter().flat_map(|p| perforate(&p, layer.perf_cut, layer.perf_skip)).map(|pts| Stroke::new(pts, li)).collect::<Vec<_>>()
                        } else {
                            pieces
                                .into_iter()
                                .map(|pts| Stroke { keep_start: *keep, ..Stroke::new(if layer.wobble_mm > 0.0 { wobble(&pts, layer.wobble_mm, layer.wobble_step) } else { pts }, li) })
                                .collect::<Vec<_>>()
                        }
                    })
                    .collect();
                if m.optimize {
                    order(&mut group, &m, out.last().and_then(|s: &Stroke| s.pts.last().copied()).unwrap_or(pt(0.0, 0.0)));
                }
                // closed loops: direction, then the overcut and the lead-in (after ordering, so they stay put);
                // the waste is outside a part (even depth) and inside a hole (odd depth)
                if layer.direction > 0 || layer.lead_in > 0.0 || layer.overcut > 0.0 || layer.lead_out > 0.0 {
                    for st in group.iter_mut().filter(|s| s.dwell_ms <= 0.0 && s.pts.len() > 3 && s.pts.first() == s.pts.last()) {
                        let ccw = signed_area(&st.pts) > 0.0;
                        if layer.direction == 1 && ccw || layer.direction == 2 && !ccw {
                            st.pts.reverse();
                        }
                        let ccw = signed_area(&st.pts) > 0.0;
                        // the lead-in leaves along the bisector of the two edges at the start (a corner start then
                        // points diagonally into the waste instead of along the next edge)
                        let n = st.pts.len();
                        let left_of = |a: Pt, b: Pt| {
                            let l = dist(a, b).max(1e-9);
                            pt(-(b.y - a.y) / l, (b.x - a.x) / l)
                        };
                        let (n1, n2) = (left_of(st.pts[n - 2], st.pts[0]), left_of(st.pts[0], st.pts[1]));
                        let bis = pt(n1.x + n2.x, n1.y + n2.y);
                        let bl = (bis.x * bis.x + bis.y * bis.y).sqrt();
                        let left = if bl > 1e-6 { pt(bis.x / bl, bis.y / bl) } else { n2 };
                        let into_waste = if (d % 2 == 0) == ccw { -1.0 } else { 1.0 };
                        let start = st.pts[0];
                        if layer.overcut > 0.0 {
                            let extra = path_prefix(&st.pts, layer.overcut);
                            st.pts.extend(extra.into_iter().skip(1));
                        }
                        // a quarter circle on the waste side: from its far end round onto the path, arriving along it
                        let arc = |at: Pt, along: Pt, side: Pt, r: f64, leaving: bool| -> Vec<Pt> {
                            let c = pt(at.x + side.x * r, at.y + side.y * r);
                            let k = if leaving { 1.0 } else { -1.0 };
                            (0..=8)
                                .map(|i| {
                                    let th = std::f64::consts::FRAC_PI_2 * i as f64 / 8.0;
                                    pt(c.x + r * (-th.cos() * side.x + k * th.sin() * along.x), c.y + r * (-th.cos() * side.y + k * th.sin() * along.y))
                                })
                                .collect()
                        };
                        let unit = |a: Pt, b: Pt| {
                            let l = dist(a, b).max(1e-9);
                            pt((b.x - a.x) / l, (b.y - a.y) / l)
                        };
                        let side = pt(left.x * into_waste, left.y * into_waste);
                        if layer.lead_in > 0.0 && layer.lead_arc {
                            let mut a = arc(start, unit(start, st.pts[1]), side, layer.lead_in, false);
                            a.reverse();
                            a.pop(); // the start itself is already there
                            st.pts.splice(0..0, a);
                        } else if layer.lead_in > 0.0 {
                            // out into the waste, turned back along the path by the lead angle
                            let (ca, sa) = (layer.lead_angle.to_radians().cos(), layer.lead_angle.clamp(0.0, 89.0).to_radians().sin());
                            let t = unit(start, st.pts[1]);
                            let d = unit(pt(0.0, 0.0), pt(side.x * ca - t.x * sa, side.y * ca - t.y * sa));
                            st.pts.insert(0, pt(start.x + d.x * layer.lead_in, start.y + d.y * layer.lead_in));
                        }
                        if layer.lead_out > 0.0 {
                            // off the end into the waste, so the beam stops away from the part: back at the start corner
                            // along its bisector (as the lead-in), else square to the last edge (an overcut ends mid-edge)
                            let k = st.pts.len();
                            let (a, b) = (st.pts[k - 2], st.pts[k - 1]);
                            let off = if dist(b, start) < 1e-9 { left } else { left_of(a, b) };
                            if layer.lead_arc {
                                st.pts.extend(arc(b, unit(a, b), pt(off.x * into_waste, off.y * into_waste), layer.lead_out, true).into_iter().skip(1));
                            } else {
                                // and the lead-out runs on along the path by the same angle
                                let (ca, sa) = (layer.lead_angle.to_radians().cos(), layer.lead_angle.clamp(0.0, 89.0).to_radians().sin());
                                let u = unit(a, b);
                                let d = unit(pt(0.0, 0.0), pt(off.x * into_waste * ca + u.x * sa, off.y * into_waste * ca + u.y * sa));
                                st.pts.push(pt(b.x + d.x * layer.lead_out, b.y + d.y * layer.lead_out));
                            }
                        }
                    }
                }
                out.extend(group);
            }
            if m.remove_overlap && layer.dot_ms <= 0.0 && layer.wobble_mm <= 0.0 {
                out = remove_overlaps(out, 0.05);
            }
            out
        }
        Mode::Offset => {
            // concentric insets: offset the layer's shapes inward by the interval until nothing is left
            let mut rings: Vec<Contour> = shapes.iter().filter(|s| s.contour.closed).map(|s| s.contour.clone()).collect();
            rings = orient_nested(&rings);
            let mut out: Vec<Stroke> = Vec::new();
            let step = layer.interval.max(0.01);
            let mut depth = 0;
            while !rings.is_empty() && depth < 5000 {
                for r in &rings {
                    let mut p = flatten(r, TOL);
                    if p.first() != p.last() {
                        p.push(p[0]);
                    }
                    out.push(Stroke::new(p, li));
                }
                rings = gcarve_core::vecops::offset_contours(&rings, -step, true);
                depth += 1;
            }
            order(&mut out, &m, pt(0.0, 0.0));
            out
        }
        Mode::Fill | Mode::FillLine => {
            let polys: Vec<Vec<Pt>> = shapes.iter().map(|s| flatten(&s.contour, TOL)).collect();
            let step = layer.interval.max(0.01);
            // outlines grown (> 0) or shrunk (< 0) as one set, so holes widen as parts shrink
            let inset = |p: &[Vec<Pt>], d: f64| -> Vec<Vec<Pt>> {
                let cs: Vec<Contour> = p.iter().filter(|p| p.len() > 2).map(|p| geom::make_polyline(p, true)).collect();
                gcarve_core::vecops::offset_contours(&orient_nested(&cs), d, true).iter().map(|c| flatten(c, TOL)).collect()
            };
            let gap = layer.loop_gap.max(0.01);
            let shrink = layer.hatch_inset.max(0.0) + layer.loops as f64 * gap;
            let fill = |p: &[Vec<Pt>], deg: f64| {
                let shrunk;
                let p = if shrink > 0.0 {
                    shrunk = inset(p, -shrink);
                    &shrunk[..]
                } else {
                    p
                };
                let step = if layer.even_lines { even_step(p, step, deg.to_radians()) } else { step };
                let h = hatch(p, step, deg.to_radians());
                if layer.flood {
                    flood(h, step, deg.to_radians())
                } else {
                    h
                }
            };
            let mut lines = if layer.fill_each {
                // part by part: each outermost outline with the holes inside it (a group, e.g. a word, as one part),
                // its lines together, then the next part
                let area = |p: &Vec<Pt>| signed_area(p).abs();
                let root = |i: usize| {
                    let q = polys[i].first().copied().unwrap_or(pt(0.0, 0.0));
                    (0..polys.len())
                        .filter(|&j| j != i && shapes[j].contour.closed && polys[j].len() > 2 && geom::point_in_poly(q, polys[j].len(), |k| (polys[j][k].x, polys[j][k].y)))
                        .max_by(|&a, &b| area(&polys[a]).total_cmp(&area(&polys[b])))
                        .unwrap_or(i)
                };
                let mut parts: Vec<(i64, Vec<Pt>)> = Vec::new();
                let keys: Vec<i64> = (0..polys.len())
                    .map(|i| {
                        let r = root(i);
                        if shapes[r].group != 0 {
                            -(shapes[r].group as i64)
                        } else {
                            r as i64
                        }
                    })
                    .collect();
                let mut order_seen: Vec<i64> = Vec::new();
                for (i, k) in keys.iter().enumerate() {
                    if !order_seen.contains(k) {
                        order_seen.push(*k);
                    }
                    parts.push((*k, polys[i].clone()));
                }
                let mut v = Vec::new();
                for k in order_seen {
                    let mine: Vec<Vec<Pt>> = parts.iter().filter(|(pk, _)| *pk == k).map(|(_, p)| p.clone()).collect();
                    v.extend(fill(&mine, angle));
                    if layer.cross {
                        v.extend(fill(&mine, angle + 90.0));
                    }
                }
                v
            } else {
                fill(&polys, angle)
            };
            if layer.cross && !layer.fill_each {
                lines.extend(fill(&polys, angle + 90.0));
            }
            if layer.one_way {
                // every line along its family's direction (a cross-hatch line goes along +90°)
                let (s, c) = angle.to_radians().sin_cos();
                for l in &mut lines {
                    let d = pt(l[1].x - l[0].x, l[1].y - l[0].y);
                    let (along, across) = (d.x * c + d.y * s, -d.x * s + d.y * c);
                    if (along.abs() >= across.abs() && along < 0.0) || (along.abs() < across.abs() && across < 0.0) {
                        l.reverse();
                    }
                }
            }
            let mut v: Vec<Stroke> = lines
                .into_iter()
                .map(|pts| {
                    if layer.overscan > 0.0 {
                        // dead runs before and after the span, along its direction
                        let (a, b) = (pts[0], pts[1]);
                        let d = dist(a, b).max(1e-9);
                        let u = pt((b.x - a.x) / d, (b.y - a.y) / d);
                        Stroke {
                            pts: vec![pt(a.x - u.x * layer.overscan, a.y - u.y * layer.overscan), a, b, pt(b.x + u.x * layer.overscan, b.y + u.y * layer.overscan)],
                            layer: li,
                            pw: Some(vec![0.0, 1.0, 0.0]),
                            dwell_ms: 0.0,
                            keep_start: false,
                        }
                    } else {
                        Stroke::new(pts, li)
                    }
                })
                .collect();
            for st in &mut v {
                dot_correct(st, layer.dot_width);
            }
            if layer.ramp > 0.0 {
                let floor = if layer.power_min > 0.0 && layer.power > 0.0 { (layer.power_min / layer.power).min(1.0) } else { 0.0 };
                let edges = SegGrid::new(&polys, layer.ramp);
                for st in &mut v {
                    ramp(st, &edges, layer.ramp, floor);
                }
            }
            if layer.loops > 0 {
                // the loops round the edge after the hatch, outermost first
                let mut rings: Vec<Stroke> = (0..layer.loops.min(100))
                    .flat_map(|k| if k == 0 { polys.iter().filter(|p| p.len() > 2).cloned().collect() } else { inset(&polys, -(k as f64) * gap) })
                    .map(|mut p| {
                        if p.first() != p.last() {
                            p.push(p[0]);
                        }
                        Stroke::new(p, li)
                    })
                    .collect();
                order(&mut rings, &m, v.last().and_then(|s| s.pts.last().copied()).unwrap_or(pt(0.0, 0.0)));
                v.extend(rings);
            }
            if layer.mode == Mode::FillLine {
                // then the outlines, nearest first from where the fill ends
                let mut outlines: Vec<Stroke> = polys
                    .iter()
                    .zip(shapes.iter())
                    .map(|(p, s)| {
                        let mut p = p.clone();
                        if s.contour.closed && p.first() != p.last() {
                            p.push(p[0]);
                        }
                        Stroke::new(p, li)
                    })
                    .collect();
                order(&mut outlines, &m, v.last().and_then(|s| s.pts.last().copied()).unwrap_or(pt(0.0, 0.0)));
                v.extend(outlines);
            }
            v
        }
        Mode::Image => shapes
            .iter()
            .filter_map(|s| {
                s.raster.as_ref().map(|r| {
                    crate::raster::raster_strokes_at(
                        r,
                        &bounds(&s.contour),
                        if layer.pass_through { bounds(&s.contour).h() / r.h.max(1) as f64 } else { layer.interval },
                        layer.dither,
                        layer.negative,
                        layer.overscan,
                        layer.one_way,
                        li,
                        angle,
                    )
                })
            })
            .flatten()
            .map(|mut st| {
                dot_correct(&mut st, layer.dot_width);
                st
            })
            .collect(),
    }
}

/// Closed contours turned so an offset grows parts and shrinks their holes: anticlockwise when inside an even number of
/// the others (a part), clockwise when inside an odd number (a hole, the hole's island again anticlockwise…). Open
/// contours are left. (gcarve's `same_direction` turns everything one way, which makes a hole more solid: an offset
/// then swallows it.)
pub fn orient_nested(cs: &[Contour]) -> Vec<Contour> {
    let polys: Vec<Vec<Pt>> = cs.iter().map(|c| flatten(c, TOL)).collect();
    cs.iter()
        .enumerate()
        .map(|(i, c)| {
            let mut c = c.clone();
            if c.closed && polys[i].len() > 2 {
                let q = polys[i][0];
                let depth = (0..cs.len()).filter(|&j| j != i && cs[j].closed && polys[j].len() > 2 && geom::point_in_poly(q, polys[j].len(), |k| (polys[j][k].x, polys[j][k].y))).count();
                if (signed_area(&polys[i]) > 0.0) != (depth % 2 == 0) {
                    geom::reverse(&mut c);
                }
            }
            c
        })
        .collect()
}

/// The plan drawn as a picture (PNG): white, the burnt moves dark by their power share, `px_per_mm` (≤ 4000 px a
/// side), a 2 mm margin. Its own line drawing (each segment stepped in half-pixel samples).
pub fn plan_png(plan: &Plan, job: &Document, px_per_mm: f64) -> Option<Vec<u8>> {
    let mut b = crate::Rect::default();
    plan.strokes.iter().flat_map(|s| s.pts.iter()).for_each(|p| b.add(*p));
    if b.is_empty() {
        return None;
    }
    let k = px_per_mm.min(4000.0 / (b.w() + 4.0)).min(4000.0 / (b.h() + 4.0)).max(0.01);
    let (w, h) = (((b.w() + 4.0) * k).ceil() as usize, ((b.h() + 4.0) * k).ceil() as usize);
    let mut px = vec![255u8; w * h];
    let to = |p: Pt| ((p.x - b.x0 + 2.0) * k, (b.y1 + 2.0 - p.y) * k);
    for s in &plan.strokes {
        let power = job.layers.get(s.layer).map_or(1.0, |l| l.power / 100.0).clamp(0.0, 1.0);
        for (i, seg) in s.pts.windows(2).enumerate() {
            let share = s.pw.as_ref().and_then(|v| v.get(i)).copied().unwrap_or(1.0) as f64;
            if share <= 0.0 {
                continue;
            }
            let v = (255.0 * (1.0 - (0.25 + 0.75 * power * share))) as u8;
            let ((x0, y0), (x1, y1)) = (to(seg[0]), to(seg[1]));
            let n = ((x1 - x0).abs().max((y1 - y0).abs()) * 2.0).ceil().max(1.0) as usize;
            for j in 0..=n {
                let t = j as f64 / n as f64;
                let (x, y) = ((x0 + (x1 - x0) * t) as usize, (y0 + (y1 - y0) * t) as usize);
                if x < w && y < h {
                    px[y * w + x] = px[y * w + x].min(v);
                }
            }
        }
    }
    let img = image::GrayImage::from_raw(w as u32, h as u32, px)?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// The rest of a job from stroke `k` on (Start Here): the same plan with the strokes before it dropped.
pub fn plan_from(plan: &Plan, k: usize) -> Plan {
    let k = k.min(plan.strokes.len());
    let t0 = plan.times.get(k).map_or(plan.seconds, |t| t[0]);
    Plan {
        strokes: plan.strokes[k..].to_vec(),
        z: if plan.z.is_empty() { Vec::new() } else { plan.z[k..].to_vec() },
        times: plan.times[k.min(plan.times.len())..].iter().map(|t| [t[0] - t0, t[1] - t0, t[2] - t0]).collect(),
        seconds: plan.seconds - t0,
        ..plan.clone()
    }
}

/// Circles touching all three given circles (centre, radius): Apollonius' problem, up to 8 answers. Each sign
/// choice (inside / outside each circle) gives a quadratic in the radius after the centre is solved linearly.
pub fn apollonius(c: [(Pt, f64); 3]) -> Vec<(Pt, f64)> {
    let mut out: Vec<(Pt, f64)> = Vec::new();
    for signs in 0..8 {
        let s: [f64; 3] = std::array::from_fn(|k| if signs >> k & 1 == 1 { -1.0 } else { 1.0 });
        // (x−xi)² + (y−yi)² = (r + si·ri)²; subtracting circle 0's from 1's and 2's leaves a·x + b·y + e·r = f
        let row = |i: usize| {
            let ((p0, r0), (pi, ri)) = (c[0], c[i]);
            let a = 2.0 * (pi.x - p0.x);
            let b = 2.0 * (pi.y - p0.y);
            let e = 2.0 * (s[i] * ri - s[0] * r0);
            let f = (pi.x * pi.x + pi.y * pi.y - ri * ri) - (p0.x * p0.x + p0.y * p0.y - r0 * r0);
            (a, b, e, f)
        };
        let ((a1, b1, e1, f1), (a2, b2, e2, f2)) = (row(1), row(2));
        let det = a1 * b2 - a2 * b1;
        if det.abs() < 1e-12 {
            continue; // centres in a line: no unique answer this way
        }
        // x = mx + nx·r, y = my + ny·r
        let (mx, nx) = ((f1 * b2 - f2 * b1) / det, (-e1 * b2 + e2 * b1) / det);
        let (my, ny) = ((a1 * f2 - a2 * f1) / det, (-a1 * e2 + a2 * e1) / det);
        let (p0, r0) = c[0];
        let (dx, dy) = (mx - p0.x, my - p0.y);
        let qa = nx * nx + ny * ny - 1.0;
        let qb = 2.0 * (dx * nx + dy * ny - s[0] * r0);
        let qc = dx * dx + dy * dy - r0 * r0;
        let roots: Vec<f64> = if qa.abs() < 1e-12 {
            if qb.abs() < 1e-12 {
                vec![]
            } else {
                vec![-qc / qb]
            }
        } else {
            let disc = qb * qb - 4.0 * qa * qc;
            if disc < 0.0 {
                vec![]
            } else {
                vec![(-qb + disc.sqrt()) / (2.0 * qa), (-qb - disc.sqrt()) / (2.0 * qa)]
            }
        };
        for r in roots.into_iter().filter(|r| *r > 1e-6) {
            let p = pt(mx + nx * r, my + ny * r);
            if !out.iter().any(|(q, rr)| geom::dist(*q, p) < 1e-6 && (rr - r).abs() < 1e-6) {
                out.push((p, r));
            }
        }
    }
    out
}

/// Stencil bridges: bars `width` wide every `spacing` mm at `angle_deg` (0 = across, 90 = up; one through the middle)
/// taken out of the closed outlines, holes respected (a ring loses the bar's width on both sides and its island
/// stays whole). Each result is (index of the outline it came from, piece). Open paths are left out.
pub fn bridges(cs: &[Contour], width: f64, spacing: f64, angle_deg: f64) -> Vec<(usize, Contour)> {
    use gcarve_core::vecops::{boolean_op, BoolOp};
    let polys: Vec<Vec<Pt>> = cs.iter().map(|c| flatten(c, TOL)).collect();
    let ok = |i: usize| cs[i].closed && polys[i].len() > 2;
    let inside = |i: usize, j: usize| j != i && ok(j) && geom::point_in_poly(polys[i][0], polys[j].len(), |k| (polys[j][k].x, polys[j][k].y));
    let depth: Vec<usize> = (0..cs.len()).map(|i| if ok(i) { (0..cs.len()).filter(|&j| inside(i, j)).count() } else { 0 }).collect();
    let mut b = crate::Rect::default();
    polys.iter().flatten().for_each(|p| b.add(*p));
    if b.is_empty() || width <= 0.0 || spacing <= width {
        return Vec::new();
    }
    let (c, r) = (pt((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0), 0.5 * b.w().hypot(b.h()) + 1.0);
    let (ux, uy) = (angle_deg.to_radians().cos(), angle_deg.to_radians().sin()); // along a bar
    let n = (r / spacing).ceil() as i64;
    let bars: Vec<Contour> = (-n..=n)
        .map(|k| {
            let t = k as f64 * spacing;
            let at = |u: f64, v: f64| pt(c.x + u * ux - v * uy, c.y + u * uy + v * ux);
            geom::make_polyline(&[at(-r, t - width / 2.0), at(r, t - width / 2.0), at(r, t + width / 2.0), at(-r, t + width / 2.0)], true)
        })
        .collect();
    let mut out = Vec::new();
    for i in (0..cs.len()).filter(|&i| ok(i) && depth[i].is_multiple_of(2)) {
        let holes = (0..cs.len()).filter(|&j| ok(j) && depth[j] == depth[i] + 1 && inside(j, i)).map(|j| cs[j].clone());
        let clips: Vec<Contour> = holes.chain(bars.iter().cloned()).collect();
        out.extend(boolean_op(BoolOp::Subtract, std::slice::from_ref(&cs[i]), &clips).into_iter().map(|p| (i, p)));
    }
    out
}

/// The outline segments in square buckets of `cell` mm, for "how far to the nearest edge, up to `cell`".
fn bounds_pts<'a>(ps: impl Iterator<Item = &'a Pt>) -> (Pt, Pt) {
    ps.fold((pt(f64::INFINITY, f64::INFINITY), pt(f64::NEG_INFINITY, f64::NEG_INFINITY)), |(lo, hi), p| (pt(lo.x.min(p.x), lo.y.min(p.y)), pt(hi.x.max(p.x), hi.y.max(p.y))))
}

struct SegGrid {
    cell: f64,
    map: std::collections::HashMap<(i64, i64), Vec<(Pt, Pt)>>,
}

impl SegGrid {
    /// `cell` is a lower bound: it grows so the outlines' box holds at most `MAX_CELLS` cells (a tiny ramp on a big
    /// design would otherwise fill memory); `dist` stays exact up to the cell, so callers capping at less don't notice.
    fn new(polys: &[Vec<Pt>], cell: f64) -> SegGrid {
        const MAX_CELLS: f64 = 1e6;
        let b = bounds_pts(polys.iter().flatten());
        let cell = cell.max(((b.1.x - b.0.x).max(0.0) * (b.1.y - b.0.y).max(0.0) / MAX_CELLS).sqrt()).max(1e-6);
        let mut map: std::collections::HashMap<(i64, i64), Vec<(Pt, Pt)>> = std::collections::HashMap::new();
        let key = |v: f64| (v / cell).floor() as i64;
        for p in polys {
            for k in 0..p.len() {
                let (a, b) = (p[k], p[(k + 1) % p.len()]);
                if a == b {
                    continue;
                }
                for i in key(a.x.min(b.x))..=key(a.x.max(b.x)) {
                    for j in key(a.y.min(b.y))..=key(a.y.max(b.y)) {
                        map.entry((i, j)).or_default().push((a, b));
                    }
                }
            }
        }
        SegGrid { cell, map }
    }
    /// the distance to the nearest outline, capped at `cell`
    fn dist(&self, p: Pt) -> f64 {
        let (ci, cj) = ((p.x / self.cell).floor() as i64, (p.y / self.cell).floor() as i64);
        let mut best = self.cell;
        for i in ci - 1..=ci + 1 {
            for j in cj - 1..=cj + 1 {
                for (a, b) in self.map.get(&(i, j)).into_iter().flatten() {
                    let (dx, dy) = (b.x - a.x, b.y - a.y);
                    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                    best = best.min(dist(p, pt(a.x + dx * t, a.y + dy * t)));
                }
            }
        }
        best
    }
}

/// Fill ramp: burning segments are cut into short pieces whose power rises from `floor` at an edge to full at
/// `len` mm from it (pieces of the same power joined again).
fn ramp(st: &mut Stroke, edges: &SegGrid, len: f64, floor: f64) {
    let step = (len / 6.0).max(0.05);
    let (mut pts, mut pw) = (vec![st.pts[0]], Vec::new());
    for (k, w) in st.pts.windows(2).enumerate() {
        let base = st.pw.as_ref().and_then(|v| v.get(k)).copied().unwrap_or(1.0);
        let n = if base > 0.0 { (dist(w[0], w[1]) / step).ceil().max(1.0) as usize } else { 1 };
        for j in 1..=n {
            let (t0, t1) = ((j - 1) as f64 / n as f64, j as f64 / n as f64);
            let at = |t: f64| pt(w[0].x + (w[1].x - w[0].x) * t, w[0].y + (w[1].y - w[0].y) * t);
            let f = if base > 0.0 { base * ((floor + (1.0 - floor) * edges.dist(at((t0 + t1) / 2.0)) / len).min(1.0) * 32.0).round() as f32 / 32.0 } else { base };
            if pw.last() == Some(&f) {
                *pts.last_mut().unwrap() = at(t1); // same power as the piece before: one longer piece
            } else {
                pts.push(at(t1));
                pw.push(f);
            }
        }
    }
    st.pts = pts;
    st.pw = Some(pw);
}

/// Dot width correction: each run of burning segments (power > 0) starts `w / 2` later and ends `w / 2` earlier
/// (at most 40 % of the run each end, so small dots shrink instead of vanishing); the trimmed bits become dead runs.
pub fn dot_correct(st: &mut Stroke, w: f64) {
    if w <= 0.0 || st.dwell_ms > 0.0 || st.pts.len() < 2 {
        return;
    }
    let n = st.pts.len() - 1;
    let p = |i: usize| st.pw.as_ref().map_or(1.0, |v| v[i]);
    let mut cum = vec![0.0];
    for k in 0..n {
        cum.push(cum[k] + dist(st.pts[k], st.pts[k + 1]));
    }
    let mut cuts: Vec<(f64, f64)> = Vec::new();
    let mut i = 0;
    while i < n {
        if p(i) > 0.0 {
            let mut j = i;
            while j < n && p(j) > 0.0 {
                j += 1;
            }
            let (s, e) = (cum[i], cum[j]);
            let d = (w / 2.0).min((e - s) * 0.4);
            cuts.push((s + d, e - d));
            i = j;
        } else {
            i += 1;
        }
    }
    let mut marks: Vec<f64> = cum.clone();
    marks.extend(cuts.iter().flat_map(|c| [c.0, c.1]));
    marks.sort_by(|a, b| a.partial_cmp(b).unwrap());
    marks.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let seg_at = |m: f64| cum.partition_point(|c| *c <= m).clamp(1, n) - 1;
    let at = |m: f64| {
        let k = seg_at(m);
        let t = if cum[k + 1] > cum[k] { (m - cum[k]) / (cum[k + 1] - cum[k]) } else { 0.0 };
        let (a, b) = (st.pts[k], st.pts[k + 1]);
        pt(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
    };
    let pts: Vec<Pt> = marks.iter().map(|m| at(*m)).collect();
    let pw: Vec<f32> = marks
        .windows(2)
        .map(|m| {
            let mid = (m[0] + m[1]) / 2.0;
            let c = cuts.partition_point(|c| c.1 < mid);
            if c < cuts.len() && cuts[c].0 <= mid {
                p(seg_at(mid))
            } else {
                0.0
            }
        })
        .collect();
    st.pts = pts;
    st.pw = Some(pw);
}

/// Each stroke's stretches that lie on a line already kept (within `tol` mm, either direction) are dropped; what
/// is left of a stroke stays one stroke while it is continuous. Strokes are taken in order, so the first cut of a
/// shared edge is the one that stays.
/// ponytail: every segment against every kept segment (bounding boxes first); a grid index if huge jobs lag
pub fn remove_overlaps(strokes: Vec<Stroke>, tol: f64) -> Vec<Stroke> {
    struct Seg {
        a: Pt,
        b: Pt,
        lo: Pt,
        hi: Pt,
    }
    let mut kept: Vec<Seg> = Vec::new();
    let mut out = Vec::new();
    for st in strokes {
        if st.pts.len() < 2 || st.dwell_ms > 0.0 {
            out.push(st);
            continue;
        }
        let mut cur: Vec<Pt> = Vec::new();
        let mut mine: Vec<Seg> = Vec::new(); // joins `kept` once the stroke is done, so its own overcut stays
        let flush = |cur: &mut Vec<Pt>, out: &mut Vec<Stroke>, layer: usize| {
            if cur.len() >= 2 {
                out.push(Stroke::new(std::mem::take(cur), layer));
            }
            cur.clear();
        };
        for w in st.pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = dist(a, b);
            if len < 1e-9 {
                continue;
            }
            let (ux, uy) = ((b.x - a.x) / len, (b.y - a.y) / len);
            // the parts of [0, len] along a→b that kept segments already cover
            let mut covered: Vec<(f64, f64)> = Vec::new();
            for k in &kept {
                if k.hi.x < a.x.min(b.x) - tol || k.lo.x > a.x.max(b.x) + tol || k.hi.y < a.y.min(b.y) - tol || k.lo.y > a.y.max(b.y) + tol {
                    continue;
                }
                let off = |p: Pt| ((p.x - a.x) * uy - (p.y - a.y) * ux).abs();
                if off(k.a) > tol || off(k.b) > tol {
                    continue;
                }
                let along = |p: Pt| (p.x - a.x) * ux + (p.y - a.y) * uy;
                let (t0, t1) = (along(k.a).min(along(k.b)).max(0.0), along(k.a).max(along(k.b)).min(len));
                if t1 - t0 > tol {
                    covered.push((t0, t1));
                }
            }
            covered.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
            // the uncovered pieces, in order
            let mut at = 0.0;
            let mut pieces: Vec<(f64, f64)> = Vec::new();
            for (c0, c1) in covered {
                if c0 > at + tol {
                    pieces.push((at, c0));
                }
                at = at.max(c1);
            }
            if len > at + tol {
                pieces.push((at, len));
            }
            let point = |t: f64| pt(a.x + ux * t, a.y + uy * t);
            for (t0, t1) in pieces {
                let (p0, p1) = (point(t0), point(t1));
                if cur.last().is_none_or(|l| dist(*l, p0) > 1e-9) {
                    flush(&mut cur, &mut out, st.layer);
                    cur.push(p0);
                }
                cur.push(p1);
            }
            if at > 0.0 && len - at <= tol {
                flush(&mut cur, &mut out, st.layer); // ended on a covered stretch
            }
            mine.push(Seg { a, b, lo: pt(a.x.min(b.x), a.y.min(b.y)), hi: pt(a.x.max(b.x), a.y.max(b.y)) });
        }
        flush(&mut cur, &mut out, st.layer);
        kept.append(&mut mine);
    }
    out
}

/// Shoelace area: positive = anticlockwise (Y up).
pub fn signed_area(p: &[Pt]) -> f64 {
    p.windows(2).map(|w| w[0].x * w[1].y - w[1].x * w[0].y).sum::<f64>() / 2.0
}

/// The first `len` mm of a path (from its start), cut at that length.
fn path_prefix(p: &[Pt], len: f64) -> Vec<Pt> {
    let mut out = vec![p[0]];
    let mut left = len;
    for w in p.windows(2) {
        let l = dist(w[0], w[1]);
        if l >= left {
            let u = if l > 0.0 { left / l } else { 0.0 };
            out.push(pt(w[0].x + (w[1].x - w[0].x) * u, w[0].y + (w[1].y - w[0].y) * u));
            return out;
        }
        out.push(w[1]);
        left -= l;
    }
    out
}

/// Wobble: the path walked as a run of small circles (`d` across, one every `step` mm along it).
pub fn wobble(p: &[Pt], d: f64, step: f64) -> Vec<Pt> {
    let r = d / 2.0;
    let step = step.max(0.01);
    // ~16 points per circle, never coarser than a tenth of the diameter
    let ds = (step / 16.0).min(d / 10.0).max(0.005);
    let mut out = Vec::new();
    let mut s = 0.0;
    for w in p.windows(2) {
        let l = dist(w[0], w[1]);
        if l <= 0.0 {
            continue;
        }
        let mut t = 0.0;
        while t < l {
            let (bx, by) = (w[0].x + (w[1].x - w[0].x) * t / l, w[0].y + (w[1].y - w[0].y) * t / l);
            let a = std::f64::consts::TAU * (s + t) / step;
            out.push(pt(bx + r * a.cos(), by + r * a.sin()));
            t += ds;
        }
        s += l;
    }
    if let Some(e) = p.last() {
        let a = std::f64::consts::TAU * s / step;
        out.push(pt(e.x + r * a.cos(), e.y + r * a.sin()));
    }
    out
}

/// LightBurn's Trim: the stretch of `path` (a flattened outline; a closed one doesn't repeat its first point) between
/// the two crossings with `others` (polylines; close a loop by repeating its first point) on either side of the point
/// nearest `at` is cut away. `None` = nothing crosses it (the whole shape goes); otherwise the open pieces left.
/// ponytail: the path's own self-crossings don't count, and the pieces come back as straight segments (curves are
/// flattened at TOL).
pub fn trim_path(path: &[Pt], closed: bool, others: &[Vec<Pt>], at: Pt) -> Option<Vec<Vec<Pt>>> {
    let n = path.len();
    if n < 2 {
        return None;
    }
    let segs: Vec<(Pt, Pt)> = (0..if closed { n } else { n - 1 }).map(|i| (path[i], path[(i + 1) % n])).collect();
    let mut s0 = Vec::with_capacity(segs.len() + 1);
    let mut total = 0.0;
    for (a, b) in &segs {
        s0.push(total);
        total += dist(*a, *b);
    }
    s0.push(total);
    // crossings, as distances along the path
    let mut cuts: Vec<f64> = Vec::new();
    for (i, (a, b)) in segs.iter().enumerate() {
        for o in others {
            for w in o.windows(2) {
                let (c, d) = (w[0], w[1]);
                let (r, q) = (pt(b.x - a.x, b.y - a.y), pt(d.x - c.x, d.y - c.y));
                let den = r.x * q.y - r.y * q.x;
                if den.abs() < 1e-12 {
                    continue;
                }
                let (ex, ey) = (c.x - a.x, c.y - a.y);
                let (u, v) = ((ex * q.y - ey * q.x) / den, (ex * r.y - ey * r.x) / den);
                if (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v) {
                    cuts.push(s0[i] + u * (s0[i + 1] - s0[i]));
                }
            }
        }
    }
    cuts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    if closed {
        // a crossing at the loop's start is the same as one at its end
        if cuts.len() > 1 && (cuts[cuts.len() - 1] - total).abs() < 1e-6 && cuts[0].abs() < 1e-6 {
            cuts.pop();
        }
    }
    if cuts.is_empty() || closed && cuts.len() < 2 {
        return None;
    }
    // where the click is along the path
    let (mut best, mut t) = (f64::INFINITY, 0.0);
    for (i, (a, b)) in segs.iter().enumerate() {
        let l2 = (b.x - a.x).powi(2) + (b.y - a.y).powi(2);
        let u = if l2 < 1e-18 { 0.0 } else { (((at.x - a.x) * (b.x - a.x) + (at.y - a.y) * (b.y - a.y)) / l2).clamp(0.0, 1.0) };
        let p = pt(a.x + u * (b.x - a.x), a.y + u * (b.y - a.y));
        if dist(p, at) < best {
            best = dist(p, at);
            t = s0[i] + u * (s0[i + 1] - s0[i]);
        }
    }
    let point_at = |d: f64| {
        let i = s0.partition_point(|s| *s <= d).saturating_sub(1).min(segs.len() - 1);
        let (a, b) = segs[i];
        let len = s0[i + 1] - s0[i];
        let u = if len < 1e-12 { 0.0 } else { ((d - s0[i]) / len).clamp(0.0, 1.0) };
        pt(a.x + u * (b.x - a.x), a.y + u * (b.y - a.y))
    };
    // the points from distance `from` to `to` along the path (from < to)
    let piece = |from: f64, to: f64| {
        let mut v = vec![point_at(from)];
        for (i, s) in s0.iter().enumerate().take(segs.len()).skip(1) {
            if *s > from + 1e-9 && *s < to - 1e-9 {
                v.push(segs[i].0);
            }
        }
        v.push(point_at(to));
        v
    };
    let mut out = Vec::new();
    let mut keep = |v: Vec<Pt>| {
        if v.windows(2).map(|w| dist(w[0], w[1])).sum::<f64>() > 1e-6 {
            out.push(v);
        }
    };
    let k = cuts.partition_point(|c| *c <= t);
    if !closed {
        // the stretch between the crossings round the click (or a path end) goes
        let (a, b) = (if k == 0 { 0.0 } else { cuts[k - 1] }, cuts.get(k).copied().unwrap_or(total));
        keep(piece(0.0, a));
        keep(piece(b, total));
    } else if k == 0 || k == cuts.len() {
        // the stretch over the loop's start goes: the rest is one piece
        keep(piece(cuts[0], cuts[cuts.len() - 1]));
    } else {
        // the loop opens there: from the next crossing round past the start to the previous one
        let mut v = piece(cuts[k], total);
        v.extend(piece(0.0, cuts[k - 1]).into_iter().skip(1));
        keep(v);
    }
    Some(out)
}

/// Flood fill: `hatch`'s spans (lines `step` apart at `angle`) regrouped into runs: a span joins the run of the span
/// on the line before when each is the other's only overlap, so a run has one span per line and goes back and forth
/// with no jumps. Runs go nearest first, each starting from its nearer end.
/// ponytail: nearest-first over runs is O(runs²); fine for the hundreds a design makes, a grid if it's ever thousands
pub fn flood(spans: Vec<Vec<Pt>>, step: f64, angle: f64) -> Vec<Vec<Pt>> {
    let (s, c) = angle.sin_cos();
    let to = |p: Pt| pt(c * p.x + s * p.y, -s * p.x + c * p.y);
    // scan space: (row, x lo, x hi, the span)
    let mut rows: Vec<(i64, f64, f64, Vec<Pt>)> = spans
        .into_iter()
        .filter(|v| v.len() == 2)
        .map(|v| {
            let (a, b) = (to(v[0]), to(v[1]));
            ((a.y / step).round() as i64, a.x.min(b.x), a.x.max(b.x), v)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let overlaps = |i: usize, j: usize| rows[i].1 < rows[j].2 && rows[j].1 < rows[i].2;
    // the spans of each row, by index
    let mut by_row: Vec<(i64, Vec<usize>)> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        match by_row.last_mut() {
            Some((k, v)) if *k == r.0 => v.push(i),
            _ => by_row.push((r.0, vec![i])),
        }
    }
    let mut run_of = vec![usize::MAX; rows.len()];
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for w in 0..by_row.len() {
        let prev: &[usize] = if w > 0 && by_row[w - 1].0 + 1 == by_row[w].0 { &by_row[w - 1].1 } else { &[] };
        for &j in &by_row[w].1 {
            let ups: Vec<usize> = prev.iter().copied().filter(|&i| overlaps(i, j)).collect();
            let joined = ups.len() == 1 && by_row[w].1.iter().filter(|&&k| overlaps(ups[0], k)).count() == 1;
            if joined {
                let r = run_of[ups[0]];
                runs[r].push(j);
                run_of[j] = r;
            } else {
                run_of[j] = runs.len();
                runs.push(vec![j]);
            }
        }
    }
    // nearest run first, from whichever end of its first span is closer; back and forth along the run
    let mut out = Vec::new();
    let mut at = pt(0.0, 0.0);
    let mut left: Vec<Vec<usize>> = runs;
    while !left.is_empty() {
        let (k, flip) = (0..left.len())
            .flat_map(|k| [(k, false), (k, true)])
            .min_by(|a, b| {
                let e = |(k, f): (usize, bool)| {
                    let v = &rows[left[k][0]].3;
                    dist(at, if f { v[1] } else { v[0] })
                };
                e(*a).total_cmp(&e(*b))
            })
            .unwrap();
        let run = left.swap_remove(k);
        let mut f = flip;
        for i in run {
            let v = &rows[i].3;
            // a span's own direction may be either way: put it the way this line goes
            let fwd = to(v[1]).x >= to(v[0]).x;
            let seg = if fwd != f { vec![v[0], v[1]] } else { vec![v[1], v[0]] };
            at = seg[1];
            out.push(seg);
            f = !f;
        }
    }
    out
}

/// Parallel scan lines `step` apart at `angle` across the even-odd inside of `polys`, alternating direction
/// (bidirectional). Each span is one stroke.
/// The interval nearest `step` that fits a whole number of lines across `polys` at `angle` (rad), so `hatch` spaces
/// them evenly with half a gap at each edge
pub fn even_step(polys: &[Vec<Pt>], step: f64, angle: f64) -> f64 {
    let (s, c) = angle.sin_cos();
    let (lo, hi) = polys.iter().flatten().map(|p| -s * p.x + c * p.y).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), y| (lo.min(y), hi.max(y)));
    let h = hi - lo;
    if !h.is_finite() || h <= 0.0 {
        return step;
    }
    h / (h / step).round().max(1.0)
}

pub fn hatch(polys: &[Vec<Pt>], step: f64, angle: f64) -> Vec<Vec<Pt>> {
    let (s, c) = angle.sin_cos();
    // rotate into scan space (lines horizontal), hatch, rotate back
    let to = |p: Pt| pt(c * p.x + s * p.y, -s * p.x + c * p.y);
    let back = |p: Pt| pt(c * p.x - s * p.y, s * p.x + c * p.y);
    let edges: Vec<(Pt, Pt)> = polys
        .iter()
        .flat_map(|poly| {
            let n = poly.len();
            (0..n).map(move |i| (to(poly[i]), to(poly[(i + 1) % n])))
        })
        .filter(|(a, b)| a.y != b.y)
        .collect();
    if edges.is_empty() {
        return Vec::new();
    }
    let (y0, y1) = edges.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (a, b)| (lo.min(a.y).min(b.y), hi.max(a.y).max(b.y)));
    let mut out = Vec::new();
    let mut y = y0 + step * 0.5;
    let mut flip = false;
    while y < y1 {
        let mut xs: Vec<f64> = edges.iter().filter(|(a, b)| (a.y > y) != (b.y > y)).map(|(a, b)| a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y)).collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut spans: Vec<(f64, f64)> = xs.chunks(2).filter(|c| c.len() == 2 && c[1] - c[0] > 1e-6).map(|c| (c[0], c[1])).collect();
        if flip {
            spans.reverse();
        }
        for (a, b) in spans {
            let (a, b) = if flip { (b, a) } else { (a, b) };
            out.push(vec![back(pt(a, y)), back(pt(b, y))]);
        }
        flip = !flip;
        y += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{geom, Layer, Mode};

    #[test]
    fn flood_fill_finishes_each_arm_before_crossing() {
        // a U: two arms joined at the bottom; flood = the bottom run, then each arm without hopping between them
        let u = vec![pt(0.0, 0.0), pt(30.0, 0.0), pt(30.0, 30.0), pt(20.0, 30.0), pt(20.0, 10.0), pt(10.0, 10.0), pt(10.0, 30.0), pt(0.0, 30.0)];
        let plain = hatch(std::slice::from_ref(&u), 1.0, 0.0);
        let f = flood(plain.clone(), 1.0, 0.0);
        assert_eq!(f.len(), plain.len(), "every span kept");
        let jumps = |v: &[Vec<Pt>]| v.windows(2).filter(|w| dist(w[0][1], w[1][0]) > 5.0).count();
        assert!(jumps(&plain) >= 19, "plain hops across the gap on every arm line: {}", jumps(&plain));
        assert!(jumps(&f) <= 2, "flood: {} long jumps", jumps(&f));
        let len = |v: &[Vec<Pt>]| v.iter().map(|s| dist(s[0], s[1])).sum::<f64>();
        assert!((len(&f) - len(&plain)).abs() < 1e-9);
        // through the planner with the layer switch
        let mut d = Document { layers: vec![Layer::new("Fill", [0, 0, 0], Mode::Fill, 100.0, 50.0)], ..Default::default() };
        (d.layers[0].interval, d.layers[0].flood) = (1.0, true);
        d.add(geom::make_polyline(&u, true), 0);
        let p = plan(&d, Machine::default());
        assert!(p.travel_mm < 200.0, "travel {}", p.travel_mm);
    }

    #[test]
    fn a_ring_fills_as_two_spans_per_line_and_a_square_cuts_once() {
        let mut d = Document { layers: vec![Layer::new("Fill", [0, 0, 0], Mode::Fill, 100.0, 50.0), Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        d.layers[0].interval = 1.0;
        d.add(geom::make_rect(0.0, 0.0, 20.0, 10.0), 0);
        d.add(geom::make_rect(5.0, 2.0, 15.0, 8.0), 0); // the hole
        d.add(geom::make_rect(30.0, 30.0, 40.0, 40.0), 1);
        let p = plan(&d, Machine::default());
        let fills = p.strokes.iter().filter(|s| s.layer == 0).count();
        // 10 lines: 4 whole (y in 0..2 and 8..10) + 6 split in two = 16 spans
        assert_eq!(fills, 16, "{:?}", p.strokes.iter().filter(|s| s.layer == 0).map(|s| (s.pts[0].y, s.pts.len())).collect::<Vec<_>>());
        let cuts: Vec<&Stroke> = p.strokes.iter().filter(|s| s.layer == 1).collect();
        assert_eq!(cuts.len(), 1);
        assert_eq!(cuts[0].pts.len(), 5);
        assert_eq!(cuts[0].pts[0], cuts[0].pts[4]);
        let fill_mm: f64 = 4.0 * 20.0 + 6.0 * 10.0;
        assert!((p.cut_mm - (fill_mm + 40.0)).abs() < 1e-6, "{}", p.cut_mm);
        assert!(p.seconds > 0.0 && p.seconds < 60.0);
        // passes repeat the strokes
        d.layers[1].passes = 3;
        assert_eq!(plan(&d, Machine::default()).strokes.iter().filter(|s| s.layer == 1).count(), 3);
        // an output-off layer is left out
        d.layers[0].output = false;
        assert!(plan(&d, Machine::default()).strokes.iter().all(|s| s.layer == 1));
    }

    #[test]
    fn slices_burn_darker_parts_on_more_passes() {
        let mut d = Document { layers: vec![Layer::new("Img", [0, 0, 0], Mode::Image, 100.0, 50.0)], ..Default::default() };
        d.layers[0].dither = crate::raster::Dither::Slices;
        d.layers[0].passes = 3;
        d.layers[0].interval = 1.0;
        let id = d.add(geom::make_rect(0.0, 0.0, 4.0, 1.0), 0);
        d.find_mut(id).unwrap().raster = Some(crate::raster::Raster::from_gray(4, 1, &[0, 85, 170, 255]));
        let p = plan(&d, Machine::default());
        // black on all 3 passes, dark grey on 2, light grey on 1, white never: 6 mm burnt
        assert!((p.cut_mm - 6.0).abs() < 1e-6, "{} {:?}", p.cut_mm, p.strokes.iter().map(|s| s.pts.clone()).collect::<Vec<_>>());
    }

    #[test]
    fn trim_cuts_the_stretch_between_crossings() {
        let cross = vec![vec![pt(5.0, -5.0), pt(5.0, 5.0)], vec![pt(8.0, -5.0), pt(8.0, 5.0)]];
        // an open line crossed at 5 and 8: a click at 6 leaves 0..5 and 8..10
        let line = [pt(0.0, 0.0), pt(10.0, 0.0)];
        let left = trim_path(&line, false, &cross, pt(6.0, 0.2)).unwrap();
        assert_eq!(left.len(), 2);
        assert!((left[0][1].x - 5.0).abs() < 1e-9 && (left[1][0].x - 8.0).abs() < 1e-9, "{left:?}");
        // a click past the last crossing takes the end off
        let left = trim_path(&line, false, &cross, pt(9.5, 0.0)).unwrap();
        assert_eq!(left.len(), 1);
        assert!((left[0].last().unwrap().x - 8.0).abs() < 1e-9);
        // nothing crosses: the whole shape goes
        assert!(trim_path(&line, false, &[vec![pt(0.0, 3.0), pt(9.0, 3.0)]], pt(1.0, 0.0)).is_none());
        // a 10 × 10 square crossed by a vertical line at x 5: clicking its left side leaves the right half, open
        let sq = [pt(0.0, -2.0), pt(10.0, -2.0), pt(10.0, 2.0), pt(0.0, 2.0)];
        let left = trim_path(&sq, true, &cross[..1], pt(0.0, 0.0)).unwrap();
        assert_eq!(left.len(), 1);
        let p = &left[0];
        assert!(p.iter().all(|q| q.x >= 5.0 - 1e-9), "{p:?}");
        let len: f64 = p.windows(2).map(|w| dist(w[0], w[1])).sum();
        assert!((len - 14.0).abs() < 1e-9, "{len}");
        // clicking the right side (inside the path's order, not over its start) leaves the left half
        let left = trim_path(&sq, true, &cross[..1], pt(10.0, 0.0)).unwrap();
        assert!(left[0].iter().all(|q| q.x <= 5.0 + 1e-9), "{:?}", left[0]);
    }

    #[test]
    fn one_way_fills_burn_every_line_the_same_direction() {
        for angle in [0.0, 30.0, 90.0] {
            let mut d = Document { layers: vec![Layer::new("Fill", [0, 0, 0], Mode::Fill, 100.0, 50.0)], ..Default::default() };
            d.layers[0].interval = 1.0;
            d.layers[0].angle = angle;
            d.layers[0].one_way = true;
            d.add(geom::make_rect(0.0, 0.0, 20.0, 10.0), 0);
            let (s, c) = angle.to_radians().sin_cos();
            let p = plan(&d, Machine::default());
            assert!(p.strokes.len() > 5);
            assert!(p.strokes.iter().all(|st| (st.pts[1].x - st.pts[0].x) * c + (st.pts[1].y - st.pts[0].y) * s > 0.0), "{angle}");
        }
    }

    #[test]
    fn strokes_run_nearest_first_and_closed_ones_start_near_the_head() {
        let mut d = Document { layers: vec![Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        d.add(geom::make_rect(100.0, 100.0, 110.0, 110.0), 0);
        d.add(geom::make_rect(10.0, 10.0, 20.0, 20.0), 0);
        d.add(geom::make_polyline(&[pt(50.0, 50.0), pt(30.0, 30.0)], false), 0);
        let p = plan(&d, Machine::default());
        assert_eq!(p.strokes[0].pts[0], pt(10.0, 10.0)); // nearest square, its nearest corner
        assert_eq!(p.strokes[1].pts[0], pt(30.0, 30.0)); // the line, reversed to start at its near end
        assert_eq!(p.strokes[2].pts[0], pt(100.0, 100.0));
    }

    #[test]
    fn the_head_moves_through_the_job_in_time_order() {
        let mut d = Document { layers: vec![Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        d.add(geom::make_polyline(&[pt(10.0, 0.0), pt(30.0, 0.0)], false), 0);
        d.add(geom::make_polyline(&[pt(30.0, 50.0), pt(30.0, 70.0)], false), 0);
        let p = plan(&d, Machine { rapid: 100.0, accel: 1e9, ..Machine::default() });
        assert_eq!(p.times.len(), 2);
        assert!((p.seconds - (0.1 + 2.0 + 0.5 + 2.0)).abs() < 1e-6, "{}", p.seconds);
        let h = p.head_at(0.0).unwrap();
        assert!(!h.on && h.pos == pt(0.0, 0.0));
        let h = p.head_at(1.1).unwrap(); // halfway along the first line
        assert!(h.on && h.stroke == 0 && (h.pos.x - 20.0).abs() < 1e-3, "{h:?}");
        let h = p.head_at(2.35).unwrap(); // halfway up the jump to the second
        assert!(!h.on && h.stroke == 1 && (h.pos.y - 25.0).abs() < 1e-3, "{h:?}");
        let h = p.head_at(99.0).unwrap(); // past the end: parked at the last point
        assert!(h.on && h.pos == pt(30.0, 70.0) && h.done == 2);
        assert!(Plan::default().head_at(1.0).is_none());
    }

    #[test]
    fn three_points_on_a_circle_find_its_centre() {
        let (o, r) = circle_through(pt(15.0, 7.0), pt(10.0, 12.0), pt(5.0, 7.0)).unwrap();
        assert!(dist(o, pt(10.0, 7.0)) < 1e-9 && (r - 5.0).abs() < 1e-9);
        assert!(circle_through(pt(0.0, 0.0), pt(1.0, 1.0), pt(2.0, 2.0)).is_none());
    }

    #[test]
    fn hand_placed_tabs_leave_bridges_where_they_are_and_move_with_the_shape() {
        let mut d = Document { layers: vec![Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        d.layers[0].tab_len = 2.0;
        let id = d.add(geom::make_rect(0.0, 0.0, 40.0, 20.0), 0);
        // two tabs: the middle of the bottom and of the top edge
        d.find_mut(id).unwrap().tabs = vec![[20.0, 0.0], [20.0, 20.0]];
        let p = plan(&d, Machine::default());
        assert_eq!(p.strokes.len(), 2);
        assert!((p.cut_mm - (120.0 - 4.0)).abs() < 1e-6, "{}", p.cut_mm);
        // the gaps are where the tabs are: no cut point between x 19 and 21 on y 0
        let near = |x: f64, y: f64| p.strokes.iter().flat_map(|s| s.pts.windows(2)).any(|w| w[0].y == y && w[1].y == y && w[0].x.min(w[1].x) < x && w[0].x.max(w[1].x) > x);
        assert!(!near(20.0, 0.0) && !near(20.0, 20.0) && near(10.0, 0.0));
        // moving the shape moves its tabs (and its start point) with it
        d.find_mut(id).unwrap().start = Some([0.0, 0.0]);
        d.transform(&[id], &geom::Affine::translate(100.0, 0.0));
        let s = d.find(id).unwrap();
        assert_eq!((s.tabs[0], s.start), ([120.0, 0.0], Some([100.0, 0.0])));
        let p = plan(&d, Machine::default());
        assert_eq!(p.strokes.len(), 2);
    }

    #[test]
    fn holes_cut_before_their_part_kerf_grows_the_outline_and_tabs_leave_bridges() {
        let mut d = Document { layers: vec![Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        d.add(geom::make_rect(0.0, 0.0, 40.0, 40.0), 0); // the part, listed first
        d.add(geom::make_rect(10.0, 10.0, 20.0, 20.0), 0); // a hole in it
        d.add(geom::make_rect(60.0, 0.0, 70.0, 10.0), 0); // another part, far away
        let p = plan(&d, Machine::default());
        assert_eq!(p.strokes.len(), 3);
        let b0 = bounds_of(&p.strokes[0].pts);
        assert!((b0.w() - 10.0).abs() < 1e-6, "the hole first: {b0:?}");
        // kerf 0.4 mm: the outside grows by 0.2 a side (the hole too, the same offset; LightBurn's rule for holes is
        // the direction, ours is the sign: a negative kerf shrinks)
        d.layers[0].kerf = 0.4;
        let p = plan(&d, Machine::default());
        let widest = p.strokes.iter().map(|s| bounds_of(&s.pts).w()).fold(0.0, f64::max);
        assert!((widest - 40.4).abs() < 0.05, "{widest}");
        d.layers[0].kerf = 0.0;
        d.layers[0].inner_first = false;
        let p = plan(&d, Machine::default());
        assert!((bounds_of(&p.strokes[0].pts).w() - 40.0).abs() < 1e-6, "nearest first again");
        // tabs: 2 bridges of 3 mm in a 40 mm square = 2 pieces, 160 - 6 = 154 mm cut
        d.layers[0].tabs = 2;
        d.layers[0].tab_len = 3.0;
        let p = plan(&d, Machine::default());
        let big: Vec<&Stroke> = p.strokes.iter().filter(|s| bounds_of(&s.pts).w() > 30.0).collect();
        assert_eq!(big.len(), 2, "{:?}", big.iter().map(|s| s.pts.len()).collect::<Vec<_>>());
        let cut: f64 = big.iter().map(|s| s.pts.windows(2).map(|w| dist(w[0], w[1])).sum::<f64>()).sum();
        assert!((cut - 154.0).abs() < 1e-6, "{cut}");
        // overscan on a fill: dead runs 3 mm past both ends at power 0
        d.layers[0].mode = Mode::Fill;
        d.layers[0].tabs = 0;
        d.layers[0].interval = 5.0;
        d.layers[0].overscan = 3.0;
        let p = plan(&d, Machine::default());
        let s = &p.strokes[0];
        assert_eq!(s.pts.len(), 4);
        assert_eq!(s.pw.as_deref(), Some(&[0.0f32, 1.0, 0.0][..]));
        assert!((dist(s.pts[0], s.pts[1]) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn dots_offset_fill_each_and_hull() {
        let sq = vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0), pt(0.0, 10.0), pt(0.0, 0.0)];
        let dots = points_along(&sq, 2.0);
        assert_eq!(dots.len(), 20, "{dots:?}");
        assert_eq!(dots[5], pt(10.0, 0.0));
        let mut d = Document { layers: vec![Layer::new("Dot", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        d.layers[0].dot_ms = 20.0;
        d.layers[0].dot_mm = 2.0;
        d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        let p = plan(&d, Machine::default());
        assert_eq!(p.strokes.len(), 20);
        assert!(p.strokes.iter().all(|s| s.dwell_ms == 20.0 && s.pts[0] == s.pts[1]));
        assert!((p.cut_mm).abs() < 1e-9 && p.seconds > 0.4);
        // offset fill: a 10 mm square at 1 mm steps = 5 rings (10, 8, 6, 4, 2)
        d.layers[0].dot_ms = 0.0;
        d.layers[0].mode = Mode::Offset;
        d.layers[0].interval = 1.0;
        let p = plan(&d, Machine::default());
        assert_eq!(p.strokes.len(), 5, "{:?}", p.strokes.iter().map(|s| s.pts.len()).collect::<Vec<_>>());
        // fill each: two squares get their lines grouped by shape
        d.layers[0].mode = Mode::Fill;
        d.layers[0].interval = 2.5;
        d.add(geom::make_rect(20.0, 0.0, 30.0, 10.0), 0);
        d.layers[0].fill_each = true;
        let p = plan(&d, Machine::default());
        assert_eq!(p.strokes.len(), 8);
        assert!(p.strokes[..4].iter().all(|s| s.pts[0].x <= 10.0) && p.strokes[4..].iter().all(|s| s.pts[0].x >= 20.0));
        // a hole stays empty part by part: a ring's lines stop at the hole (2 runs per line through it)
        let mut r = Document { layers: vec![Layer::new("F", [0, 0, 0], Mode::Fill, 100.0, 50.0)], ..Default::default() };
        r.layers[0].interval = 1.0;
        r.layers[0].fill_each = true;
        r.add(geom::make_rect(0.0, 0.0, 20.0, 20.0), 0);
        r.add(geom::make_rect(5.0, 5.0, 15.0, 15.0), 0);
        let p = plan(&r, Machine::default());
        let through = p.strokes.iter().filter(|s| s.pts[0].y > 6.0 && s.pts[0].y < 14.0).count();
        let cut: f64 = p.strokes.iter().map(|s| dist(s.pts[0], *s.pts.last().unwrap())).sum();
        assert!(through >= 16 && (cut - (400.0 - 100.0)).abs() < 2.0, "{through} lines, {cut} mm");
        let h = convex_hull(&[pt(0.0, 0.0), pt(5.0, 1.0), pt(10.0, 0.0), pt(10.0, 10.0), pt(0.0, 10.0), pt(5.0, 5.0)]);
        assert_eq!(h.len(), 5);
        assert!(!h.contains(&pt(5.0, 5.0)) && !h.contains(&pt(5.0, 1.0)));
    }

    #[test]
    fn lead_ins_start_in_the_waste_and_overcut_closes_the_loop() {
        let mut d = Document { layers: vec![Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        d.add(geom::make_rect(0.0, 0.0, 40.0, 40.0), 0); // the part
        d.add(geom::make_rect(15.0, 15.0, 25.0, 25.0), 0); // a hole in it
        d.layers[0].lead_in = 2.0;
        d.layers[0].overcut = 1.0;
        d.layers[0].direction = 1;
        let p = plan(&d, Machine::default());
        assert_eq!(p.strokes.len(), 2);
        let inside = |q: Pt, x0: f64, x1: f64| q.x > x0 && q.x < x1 && q.y > x0 && q.y < x1;
        // the hole goes first; its lead-in starts inside it, the part's starts outside it
        let (hole, part) = (&p.strokes[0], &p.strokes[1]);
        assert!(inside(hole.pts[0], 15.0, 25.0), "{:?}", hole.pts[0]);
        assert!(!inside(part.pts[0], 0.0, 40.0), "{:?}", part.pts[0]);
        assert!((dist(hole.pts[0], hole.pts[1]) - 2.0).abs() < 1e-9);
        // clockwise, then 1 mm on past the start
        for st in [hole, part] {
            let loop_pts = &st.pts[1..];
            assert!(signed_area(loop_pts) < 0.0 || signed_area(&loop_pts[..loop_pts.len() - 1]) < 0.0, "clockwise");
            let len: f64 = st.pts.windows(2).map(|w| dist(w[0], w[1])).sum();
            let perimeter = if std::ptr::eq(st, hole) { 40.0 } else { 160.0 };
            assert!((len - perimeter - 3.0).abs() < 1e-9, "{len}");
        }
    }

    #[test]
    fn a_lead_out_ends_in_the_waste() {
        let mut l = Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0);
        (l.lead_out, l.inner_first) = (2.0, true);
        let mut d = Document { layers: vec![l], ..Default::default() };
        d.add(geom::make_rect(0.0, 0.0, 40.0, 40.0), 0);
        d.add(geom::make_rect(10.0, 10.0, 30.0, 30.0), 0);
        let p = plan(&d, Machine::default());
        let end = |i: usize| *p.strokes[i].pts.last().unwrap();
        // the hole is cut first (inner first): it ends inside the hole; the part ends outside the part
        let (h, o) = (end(0), end(1));
        assert!(h.x > 10.0 && h.x < 30.0 && h.y > 10.0 && h.y < 30.0, "hole end {h:?}");
        assert!(o.x < 0.0 || o.x > 40.0 || o.y < 0.0 || o.y > 40.0, "part end {o:?}");
    }

    #[test]
    fn a_start_point_holds_through_ordering() {
        let mut d = Document { layers: vec![Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        let id = d.add(geom::make_rect(10.0, 10.0, 30.0, 30.0), 0);
        let p = plan(&d, Machine::default());
        assert_eq!(p.strokes[0].pts[0], pt(10.0, 10.0), "nearest to 0,0 without a start point");
        d.find_mut(id).unwrap().start = Some([30.2, 29.9]);
        let p = plan(&d, Machine::default());
        assert_eq!(p.strokes[0].pts[0], pt(30.0, 30.0));
        assert_eq!(p.strokes[0].pts.last(), Some(&pt(30.0, 30.0)), "still a closed loop");
    }

    #[test]
    fn shared_edges_are_cut_once() {
        let mut d = Document { layers: vec![Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        // two squares side by side share x = 10; a third square on top shares half of its bottom with them
        d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        d.add(geom::make_rect(10.0, 0.0, 20.0, 10.0), 0);
        d.add(geom::make_rect(5.0, 10.0, 15.0, 20.0), 0);
        let all = plan(&d, Machine::default()).cut_mm;
        assert!((all - 120.0).abs() < 1e-6, "{all}");
        let p = plan(&d, Machine { remove_overlap: true, ..Machine::default() });
        // 120 − the shared side (10) − the top square's bottom (10, already cut by the two tops)
        assert!((p.cut_mm - 100.0).abs() < 1e-6, "{}", p.cut_mm);
        assert!(p.strokes.iter().all(|s| s.pts.len() >= 2));
        // a stroke drawn twice is cut once
        let st = Stroke::new(vec![pt(0.0, 0.0), pt(5.0, 0.0)], 0);
        assert_eq!(remove_overlaps(vec![st.clone(), st], 0.05).len(), 1);
        // a loop's own overcut (back over its first side) stays
        let oc = Stroke::new(vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0), pt(0.0, 10.0), pt(0.0, 0.0), pt(2.0, 0.0)], 0);
        let len = |v: &[Stroke]| v.iter().flat_map(|s| s.pts.windows(2)).map(|w| dist(w[0], w[1])).sum::<f64>();
        assert!((len(&remove_overlaps(vec![oc], 0.05)) - 42.0).abs() < 1e-9);
    }

    #[test]
    fn a_huge_stroke_count_orders_in_rows_quickly() {
        // 30 000 dots on a 300 × 100 mm grid, shuffled: past the nearest-first limit they go row by row
        let n = 30_000;
        let mut v: Vec<Stroke> = (0..n).map(|i| (i * 7919) % n).map(|k| Stroke::new(vec![pt((k % 300) as f64, (k / 300) as f64), pt((k % 300) as f64 + 0.1, (k / 300) as f64)], 0)).collect();
        order(&mut v, &Machine::default(), pt(0.0, 0.0));
        assert_eq!(v.len(), n);
        let travel: f64 = v.windows(2).map(|w| dist(*w[0].pts.last().unwrap(), w[1].pts[0])).sum();
        assert!(travel < 2.0 * n as f64, "{travel}");
    }

    #[test]
    fn print_and_cut_lines_the_marks_up() {
        let (d1, d2) = (pt(10.0, 10.0), pt(110.0, 10.0));
        // the sheet lies 3° turned and moved on the bed, printed 1 % big
        let a = 3f64.to_radians();
        let m1 = pt(50.0, 40.0);
        let m2 = pt(m1.x + 101.0 * a.cos(), m1.y + 101.0 * a.sin());
        let t = print_cut_transform(d1, d2, m1, m2, true);
        let (p1, p2) = (t.apply(d1), t.apply(d2));
        assert!(dist(p1, m1) < 1e-9 && dist(p2, m2) < 1e-9, "{p1:?} {p2:?}");
        // without scaling the first mark is exact and the second lies on the right line, 1 mm short
        let t = print_cut_transform(d1, d2, m1, m2, false);
        let (p1, p2) = (t.apply(d1), t.apply(d2));
        assert!(dist(p1, m1) < 1e-9 && (dist(p2, m2) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn dot_width_trims_each_burn_run_at_its_ends_only() {
        let burnt = |st: &Stroke| st.pts.windows(2).enumerate().filter(|(k, _)| st.pw.as_ref().map_or(1.0, |v| v[*k]) > 0.0).map(|(_, w)| dist(w[0], w[1])).sum::<f64>();
        // a greyscale row: dead, two burning cells, dead: only the run's outer ends move in
        let mut st = Stroke { pw: Some(vec![0.0, 0.5, 0.8, 0.0]), ..Stroke::new(vec![pt(0.0, 0.0), pt(1.0, 0.0), pt(3.0, 0.0), pt(5.0, 0.0), pt(6.0, 0.0)], 0) };
        dot_correct(&mut st, 0.4);
        assert!((burnt(&st) - 3.6).abs() < 1e-9, "{}", burnt(&st));
        let first_burn = st.pw.as_ref().unwrap().iter().position(|w| *w > 0.0).unwrap();
        assert!((st.pts[first_burn].x - 1.2).abs() < 1e-9 && st.pw.as_ref().unwrap()[first_burn] == 0.5);
        // a tiny dot shrinks to 20 % instead of vanishing
        let mut dot = Stroke::new(vec![pt(0.0, 0.0), pt(0.1, 0.0)], 0);
        dot_correct(&mut dot, 0.4);
        assert!((burnt(&dot) - 0.02).abs() < 1e-9);
        // a whole fill: every 10 mm line burns 9.6 mm
        let mut d = Document { layers: vec![crate::Layer::new("F", [0, 0, 0], Mode::Fill, 100.0, 50.0)], ..Default::default() };
        d.layers[0].interval = 1.0;
        d.layers[0].dot_width = 0.4;
        d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        let p = plan(&d, Machine::default());
        assert!(!p.strokes.is_empty() && p.strokes.iter().all(|s| (burnt(s) - 9.6).abs() < 1e-6));
    }

    #[test]
    fn hatch_turns_each_pass_and_wobble_circles_the_path() {
        let mut d = Document { layers: vec![Layer::new("F", [0, 0, 0], Mode::Fill, 100.0, 50.0)], ..Default::default() };
        d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        d.layers[0].interval = 1.0;
        d.layers[0].passes = 2;
        d.layers[0].angle_step = 90.0;
        let p = plan(&d, Machine::default());
        let n = p.strokes.len() / 2;
        let first = &p.strokes[0].pts;
        let second = &p.strokes[n].pts;
        assert!((first[0].y - first[1].y).abs() < 1e-9, "pass 1 horizontal");
        assert!((second[0].x - second[1].x).abs() < 1e-9, "pass 2 vertical: {second:?}");
        let w = wobble(&[pt(0.0, 0.0), pt(10.0, 0.0)], 2.0, 0.5);
        assert!(w.len() > 300 && w.iter().all(|q| q.y.abs() <= 1.0 + 1e-9 && q.x >= -1.0 - 1e-9 && q.x <= 11.0 + 1e-9));
        assert!(w.iter().any(|q| q.y > 0.99) && w.iter().any(|q| q.y < -0.99));
        let mut d = Document { layers: vec![Layer::new("L", [0, 0, 0], Mode::Line, 10.0, 50.0)], ..Default::default() };
        d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        let plain = plan(&d, Machine::default()).cut_mm;
        d.layers[0].wobble_mm = 1.0;
        assert!(plan(&d, Machine::default()).cut_mm > plain * 3.0);
    }

    #[test]
    fn drawing_order_is_kept_when_not_optimising() {
        let mut d = Document { layers: vec![Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        d.add(geom::make_rect(100.0, 100.0, 110.0, 110.0), 0);
        d.add(geom::make_rect(10.0, 10.0, 20.0, 20.0), 0);
        let p = plan(&d, Machine { optimize: false, ..Machine::default() });
        assert_eq!(p.strokes[0].pts[0], pt(100.0, 100.0));
        let p = plan(&d, Machine::default());
        assert_eq!(p.strokes[0].pts[0], pt(10.0, 10.0));
    }

    #[test]
    fn order_by_group_finishes_each_part_first() {
        // two parts, each a score inside a cut, far apart; the far part was drawn first
        let mut d = Document { layers: vec![Layer::new("Score", [0, 0, 255], Mode::Line, 100.0, 20.0), Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        let parts: Vec<Vec<i32>> = [200.0, 0.0].iter().map(|&x| vec![d.add(geom::make_rect(x + 5.0, 5.0, x + 15.0, 15.0), 0), d.add(geom::make_rect(x, 0.0, x + 20.0, 20.0), 1)]).collect();
        for p in &parts {
            d.group(p);
        }
        let layers = |p: &Plan| p.strokes.iter().map(|s| s.layer).collect::<Vec<_>>();
        assert_eq!(layers(&plan(&d, Machine::default())), [0, 0, 1, 1]);
        let g = plan(&d, Machine { by_group: true, ..Machine::default() });
        assert_eq!(layers(&g), [0, 1, 0, 1]);
        assert!(g.strokes[0].pts[0].x < 100.0, "the near part first");
        // the Ruida file goes block by block and still has every cut
        let o = crate::ruida::RdOptions { bed: [600.0, 400.0], ..Default::default() };
        let (moves, ok) = crate::ruida::rd_decode(&crate::ruida::unswizzle(&crate::ruida::rd_file(&d, &g, &o), 0x88));
        assert!(ok && moves.iter().filter(|m| m.cut).count() == g.strokes.iter().map(|s| s.pts.len() - 1).sum::<usize>());
    }

    #[test]
    fn a_ramp_fades_the_fill_in_from_every_edge() {
        let mut l = Layer::new("Stamp", [0, 0, 0], Mode::Fill, 100.0, 80.0);
        l.ramp = 2.0;
        l.interval = 1.0;
        let mut d = Document { layers: vec![l], ..Default::default() };
        d.add(geom::make_rect(0.0, 0.0, 20.0, 20.0), 0);
        let p = plan(&d, Machine::default());
        let mid = p.strokes.iter().find(|s| (s.pts[0].y - 10.5).abs() < 0.01).expect("a line through the middle");
        let pw = mid.pw.as_ref().unwrap();
        assert!(pw[0] < 0.3 && *pw.last().unwrap() < 0.3, "faint at both edges: {pw:?}");
        assert!(pw.contains(&1.0) && pw.len() < 20, "full in the middle, pieces joined: {pw:?}");
        // the edge rows near the top are faint all along
        let top = p.strokes.iter().find(|s| (s.pts[0].y - 19.5).abs() < 0.01).unwrap();
        assert!(top.pw.as_ref().unwrap().iter().all(|w| *w <= 0.25 + 1e-6), "{:?}", top.pw);
    }

    #[test]
    fn choose_corners_starts_a_loop_on_a_sharp_corner() {
        // a D shape: a straight side and a round side; the nearest point to 0,0 is on the round side
        let mut d = Document { layers: vec![Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        let mut pts = vec![pt(20.0, 20.0), pt(20.0, 40.0)];
        pts.extend((1..20).map(|k| {
            let a = std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * k as f64 / 20.0;
            pt(20.0 + 10.0 * a.cos(), 30.0 + 10.0 * a.sin())
        }));
        d.add(geom::make_polyline(&pts, true), 0);
        let plain = plan(&d, Machine::default()).strokes[0].pts[0];
        let c = plan(&d, Machine { corners: true, ..Machine::default() }).strokes[0].pts[0];
        assert!(plain.x < 20.0 - 1e-6, "nearest point is on the curve: {plain:?}");
        assert!(c == pt(20.0, 20.0) || c == pt(20.0, 40.0), "a corner: {c:?}");
    }

    #[test]
    fn tabs_can_skip_the_holes() {
        let mut l = Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0);
        (l.tabs, l.tab_len, l.tabs_skip_inner) = (2, 1.0, true);
        let mut d = Document { layers: vec![l], ..Default::default() };
        d.add(geom::make_rect(0.0, 0.0, 40.0, 40.0), 0);
        d.add(geom::make_rect(10.0, 10.0, 30.0, 30.0), 0);
        // the part breaks into 2 pieces at its tabs, the hole stays one loop
        assert_eq!(plan(&d, Machine::default()).strokes.len(), 3);
        d.layers[0].tabs_skip_inner = false;
        assert_eq!(plan(&d, Machine::default()).strokes.len(), 4);
    }

    #[test]
    fn the_plan_draws_as_a_picture_and_resumes_part_way() {
        let d = crate::sample();
        let p = plan(&d, Machine::default());
        let png = plan_png(&p, &d, 2.0).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_luma8();
        assert!(img.pixels().any(|v| v.0[0] < 128) && img.pixels().filter(|v| v.0[0] == 255).count() > 1000);
        let half = plan_from(&p, p.strokes.len() / 2);
        assert_eq!(half.strokes.len(), p.strokes.len() - p.strokes.len() / 2);
        assert!(half.seconds < p.seconds && half.times[0][0].abs() < 1e-9);
    }

    #[test]
    fn axis_limits_and_fill_acceleration_slow_the_estimate() {
        let d = crate::sample();
        let base = plan(&d, Machine::default()).seconds;
        assert!(plan(&d, Machine { max_xy: [0.0, 20.0], ..Machine::default() }).seconds > base);
        assert!(plan(&d, Machine { accel_fill: 200.0, ..Machine::default() }).seconds > base);
        assert!((plan(&d, Machine { max_xy: [1e6, 1e6], ..Machine::default() }).seconds - base).abs() < 1e-6);
        // a square's 4 corners at GRBL's 0.01 mm: vj = √(2500·0.01·0.707/0.293) ≈ 7.8 mm/s from 100, 3 inner corners (the start / end one is not a junction)
        let sq = [pt(0.0, 0.0), pt(50.0, 0.0), pt(50.0, 50.0), pt(0.0, 50.0), pt(0.0, 0.0)];
        let t = corner_time(&sq, 100.0, 2500.0, 0.01);
        assert!((t - 3.0 * (100.0 - (2500.0f64 * 0.01 * 0.5f64.sqrt() / (1.0 - 0.5f64.sqrt())).sqrt()).powi(2) / 250000.0).abs() < 1e-9, "{t}");
        assert_eq!(corner_time(&[pt(0.0, 0.0), pt(1.0, 0.0), pt(2.0, 0.0)], 100.0, 2500.0, 0.01), 0.0);
        let mut d = d;
        let line = d.layers.iter().position(|l| l.mode == Mode::Line).unwrap();
        d.add(geom::make_rect(0.0, 0.0, 40.0, 40.0), line);
        let base = plan(&d, Machine::default()).seconds;
        assert!(plan(&d, Machine { corner_dev: 0.01, ..Machine::default() }).seconds > base);
    }

    #[test]
    fn an_angled_lead_in_comes_in_along_the_path() {
        let mut d = Document::default();
        d.layers[0].mode = Mode::Line;
        d.add(geom::make_rect(0.0, 0.0, 20.0, 20.0), 0);
        (d.layers[0].lead_in, d.layers[0].lead_angle) = (4.0, 60.0);
        let s = &plan(&d, Machine::default()).strokes[0];
        let (lead, start, next) = (s.pts[0], s.pts[1], s.pts[2]);
        assert!((geom::dist(lead, start) - 4.0).abs() < 1e-9);
        // 60° off square: the lead runs within 30° of the first edge's line
        let (u, v) = (pt(start.x - lead.x, start.y - lead.y), pt(next.x - start.x, next.y - start.y));
        let cos = (u.x * v.x + u.y * v.y) / (u.x.hypot(u.y) * v.x.hypot(v.y));
        assert!(cos > 0.5, "{cos}");
    }

    #[test]
    fn arc_leads_meet_the_path_along_it() {
        let mut d = Document::default();
        d.layers[0].mode = Mode::Line;
        d.add(geom::make_rect(0.0, 0.0, 20.0, 20.0), 0);
        (d.layers[0].lead_in, d.layers[0].lead_out, d.layers[0].lead_arc) = (3.0, 3.0, true);
        let s = &plan(&d, Machine::default()).strokes[0];
        // 8 arc points before the start, 8 after the end; every arc point 3 mm from its centre
        assert_eq!(s.pts.len(), 5 + 16);
        let (start, after) = (s.pts[8], s.pts[9]);
        let (before, at) = (s.pts[7], s.pts[8]);
        // arriving along the first edge: the last arc step and the first edge point the same way
        let (u, v) = (pt(at.x - before.x, at.y - before.y), pt(after.x - start.x, after.y - start.y));
        let cos = (u.x * v.x + u.y * v.y) / ((u.x.hypot(u.y)) * v.x.hypot(v.y));
        assert!(cos > 0.98, "{cos}");
        // and the lead stays outside the part
        assert!(s.pts[0].x < 0.0 || s.pts[0].y < 0.0 || s.pts[0].x > 20.0 || s.pts[0].y > 20.0, "{:?}", s.pts[0]);
    }

    #[test]
    fn slices_clean_up_every_n_passes_at_low_power() {
        let mut d = Document::default();
        let img = crate::raster::Raster::from_gray(4, 4, &[0u8; 16]);
        let id = d.add(geom::make_rect(0.0, 0.0, 4.0, 4.0), 0);
        d.find_mut(id).unwrap().raster = Some(img);
        let l = &mut d.layers[0];
        (l.mode, l.interval, l.dither, l.passes, l.cleanup_every, l.cleanup_power) = (Mode::Image, 1.0, crate::raster::Dither::Slices, 4, 2, 25.0);
        let p = plan(&d, Machine::default());
        // 4 passes of 4 rows, and 2 cleanups of 4 rows at a quarter power
        assert_eq!(p.strokes.len(), 24);
        assert_eq!(p.strokes.iter().filter(|s| s.pw.as_ref().is_some_and(|v| v.iter().any(|w| (*w - 0.25).abs() < 1e-6))).count(), 8);
    }

    #[test]
    fn priority_and_pass_through() {
        // two squares: the far one has priority 0, the near one 1, so the far one burns first
        let mut d = Document::default();
        d.layers[0].mode = Mode::Line;
        let near = d.add(geom::make_rect(0.0, 0.0, 5.0, 5.0), 0);
        d.add(geom::make_rect(100.0, 100.0, 105.0, 105.0), 0);
        d.find_mut(near).unwrap().priority = 1;
        let p = plan(&d, Machine::default());
        assert!(p.strokes[0].pts[0].x >= 100.0, "priority 0 first");
        // pass-through: a 4 px tall picture 20 mm tall scans 4 rows whatever the interval
        let mut d = Document::default();
        let img = crate::raster::Raster::from_gray(4, 4, &[0u8; 16]);
        let id = d.add(geom::make_rect(0.0, 0.0, 20.0, 20.0), 0);
        d.find_mut(id).unwrap().raster = Some(img);
        (d.layers[0].mode, d.layers[0].interval, d.layers[0].pass_through, d.layers[0].dither) = (Mode::Image, 0.1, true, crate::raster::Dither::Threshold);
        assert_eq!(plan(&d, Machine::default()).strokes.len(), 4);
    }

    #[test]
    fn direction_order_turns_and_backlash() {
        // a 3 × 3 grid of squares: top → bottom takes the top row first; left → right the left column
        let mut d = Document::default();
        for (x, y) in (0..9).map(|k| ((k % 3) as f64 * 20.0, (k / 3) as f64 * 20.0)) {
            d.add(geom::make_rect(x, y, x + 5.0, y + 5.0), 0);
        }
        d.layers[0].mode = Mode::Line;
        let y_of = |s: &Stroke| s.pts.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
        let x_of = |s: &Stroke| s.pts.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        let top = plan(&d, Machine { sweep: 1, ..Machine::default() });
        assert!(top.strokes[..3].iter().all(|s| (y_of(s) - 45.0).abs() < 1e-6), "the top row first");
        let left = plan(&d, Machine { sweep: 3, ..Machine::default() });
        assert!(left.strokes[..3].iter().all(|s| x_of(s).abs() < 1e-6), "the left column first");
        // backlash: every loop anticlockwise from its lowest-left corner
        let b = plan(&d, Machine { backlash: true, ..Machine::default() });
        for s in &b.strokes {
            assert!(signed_area(&s.pts) > 0.0);
            let lo = s.pts.iter().fold(pt(f64::INFINITY, f64::INFINITY), |a, p| if (p.y, p.x) < (a.y, a.x) { *p } else { a });
            assert_eq!(s.pts[0], lo);
        }
        // fewer turns still cuts everything
        assert_eq!(plan(&d, Machine { fewer_turns: true, ..Machine::default() }).strokes.len(), 9);
    }

    #[test]
    fn tangent_circles_touch_all_three() {
        let c = [(pt(0.0, 0.0), 1.0), (pt(6.0, 0.0), 1.0), (pt(3.0, 5.0), 1.5)];
        let found = apollonius(c);
        assert_eq!(found.len(), 8, "{found:?}");
        for (p, r) in &found {
            for (q, rq) in &c {
                let d = geom::dist(*p, *q);
                assert!((d - (r + rq)).abs() < 1e-6 || (d - (r - rq).abs()).abs() < 1e-6, "{p:?} {r} vs {q:?} {rq}");
            }
        }
    }

    #[test]
    fn bridges_cut_a_ring_and_keep_its_island() {
        // an O: a 20 mm square with a 10 mm hole; one 2 mm upright bar through the middle
        let o = [geom::make_rect(0.0, 0.0, 20.0, 20.0), geom::make_rect(5.0, 5.0, 15.0, 15.0)];
        let p = bridges(&o, 2.0, 100.0, 90.0);
        assert_eq!(p.len(), 2, "two halves of the ring");
        let area: f64 = p.iter().map(|(_, c)| Document::measure(c).1.abs()).sum();
        assert!((area - (400.0 - 100.0 - 2.0 * 2.0 * 5.0)).abs() < 0.1, "{area}");
        assert!(p.iter().all(|(i, _)| *i == 0));
    }

    #[test]
    fn a_kerf_grows_the_part_and_shrinks_its_hole() {
        let mut l = Layer::new("C", [255, 0, 0], Mode::Line, 10.0, 90.0);
        (l.kerf, l.interval) = (0.2, 3.0);
        let mut d = Document { layers: vec![l], ..Default::default() };
        d.add(geom::make_rect(0.0, 0.0, 40.0, 40.0), 0);
        d.add(geom::make_rect(10.0, 10.0, 30.0, 30.0), 0);
        let widths = |d: &Document| {
            let mut w: Vec<f64> = plan(d, Machine::default()).strokes.iter().map(|s| bounds(&geom::make_polyline(&s.pts, false)).w()).collect();
            w.sort_by(|a, b| a.total_cmp(b));
            w
        };
        let w = widths(&d);
        assert!(w.len() == 2 && (w[0] - 19.8).abs() < 1e-3 && (w[1] - 40.2).abs() < 1e-3, "{w:?}");
        // offset fill: rings shrink from the outside and grow from the hole, never inside the hole
        d.layers[0].mode = Mode::Offset;
        assert!(widths(&d).iter().all(|w| *w > 20.0 - 1e-6), "{:?}", widths(&d));
    }

    #[test]
    fn perforation_cuts_dashes_along_the_outline() {
        let sq = vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0), pt(0.0, 10.0), pt(0.0, 0.0)];
        let dashes = perforate(&sq, 3.0, 1.0);
        assert_eq!(dashes.len(), 10, "{dashes:?}"); // 40 / 4 = 10 dashes
        let cut: f64 = dashes.iter().map(|d| d.windows(2).map(|w| dist(w[0], w[1])).sum::<f64>()).sum();
        assert!((cut - 30.0).abs() < 1e-6, "{cut}");
        // a dash round a corner keeps the corner
        assert!(dashes.iter().any(|d| d.len() == 3));
        let mut doc = Document { layers: vec![Layer::new("Cut", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        doc.layers[0].perf_cut = 3.0;
        doc.layers[0].perf_skip = 1.0;
        doc.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        assert_eq!(plan(&doc, Machine::default()).strokes.len(), 10);
    }

    #[test]
    fn cross_hatch_doubles_the_lines_and_fill_line_adds_the_outline_last() {
        let mut d = Document { layers: vec![Layer::new("F", [0, 0, 0], Mode::Fill, 100.0, 50.0)], ..Default::default() };
        d.layers[0].interval = 1.0;
        d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        let n = plan(&d, Machine::default()).strokes.len();
        d.layers[0].cross = true;
        let p = plan(&d, Machine::default());
        assert_eq!(p.strokes.len(), 2 * n);
        assert!((p.layer_seconds[0] - p.seconds).abs() < 1e-9);
        d.layers[0].mode = Mode::FillLine;
        let p = plan(&d, Machine::default());
        assert_eq!(p.strokes.len(), 2 * n + 1);
        assert_eq!(p.strokes.last().unwrap().pts.len(), 5);
    }

    #[test]
    fn edge_loops_follow_the_fill_and_the_inset_keeps_it_off_the_edge() {
        let mut d = Document { layers: vec![Layer::new("F", [0, 0, 0], Mode::Fill, 100.0, 50.0)], ..Default::default() };
        (d.layers[0].interval, d.layers[0].loops, d.layers[0].loop_gap, d.layers[0].hatch_inset) = (0.5, 2, 0.5, 0.5);
        d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        d.add(geom::make_rect(4.0, 4.0, 6.0, 6.0), 0); // a hole: it widens by the same 1.5 mm
        let p = plan(&d, Machine::default());
        let (lines, rings): (Vec<&Stroke>, Vec<&Stroke>) = p.strokes.iter().partition(|s| s.pts.len() == 2);
        assert_eq!(rings.len(), 4, "two loops round the part and two round the hole");
        assert!(p.strokes.iter().rposition(|s| s.pts.len() == 2).unwrap() < p.strokes.iter().position(|s| s.pts.len() > 2).unwrap(), "loops after the hatch");
        for l in &lines {
            for q in &l.pts {
                assert!(q.x > 1.49 && q.x < 8.51 && q.y > 1.49 && q.y < 8.51, "{q:?}");
                let (dx, dy) = ((4.0 - q.x).max(q.x - 6.0).max(0.0), (4.0 - q.y).max(q.y - 6.0).max(0.0));
                assert!(dx.hypot(dy) > 1.49, "inside the widened hole: {q:?}");
            }
        }
        let len = |s: &Stroke| s.pts.windows(2).map(|w| dist(w[0], w[1])).sum::<f64>();
        let mut ls: Vec<f64> = rings.iter().map(|s| len(s)).collect();
        ls.sort_by(f64::total_cmp);
        for (got, want) in ls.iter().zip([8.0, 8.0 + std::f64::consts::PI, 36.0, 40.0]) {
            // the hole grows with round corners
            assert!((got - want).abs() < 0.5, "{ls:?}");
        }
    }

    #[test]
    fn even_lines_spread_edge_to_edge() {
        let sq = vec![vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.3), pt(0.0, 10.3)]];
        let st = even_step(&sq, 1.0, 0.0);
        assert!((st - 1.03).abs() < 1e-9, "{st}");
        let ys: Vec<f64> = hatch(&sq, st, 0.0).iter().map(|l| l[0].y).collect();
        assert_eq!(ys.len(), 10);
        assert!((ys[0] - 0.515).abs() < 1e-9 && (10.3 - ys[9] - 0.515).abs() < 1e-9, "{ys:?}");
        assert!((even_step(&sq, 1.0, std::f64::consts::FRAC_PI_2) - 1.0).abs() < 1e-9, "10 mm across at 90°");
    }

    fn bounds_of(pts: &[Pt]) -> crate::Rect {
        let mut r = crate::Rect::default();
        for p in pts {
            r.add(*p);
        }
        r
    }

    #[test]
    fn an_angled_hatch_covers_the_same_area() {
        let sq = vec![vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0), pt(0.0, 10.0)]];
        let flat: f64 = hatch(&sq, 0.5, 0.0).iter().map(|s| dist(s[0], s[1])).sum();
        let tilted: f64 = hatch(&sq, 0.5, 45f64.to_radians()).iter().map(|s| dist(s[0], s[1])).sum();
        assert!((flat - 200.0).abs() < 1e-6, "{flat}");
        assert!((tilted - 200.0).abs() < 8.0, "{tilted}"); // line count × spacing = area, within edge effects
        assert!(hatch(&[], 0.5, 0.0).is_empty());
    }
}
