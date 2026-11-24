//! GBurn's engine: the document (shapes on cut layers), the planner (what the laser does, in order) and the encoders
//! (G-code for GRBL, Ruida .rd). No UI and no I/O: strings and bytes in and out, so the same code runs on the desktop
//! and in the browser (wasm32). Units: **millimetres**, Y up, origin at the machine's front-left corner.

pub mod galvo;
pub mod gcode;
pub mod k40;
pub mod plan;
pub mod raster;
pub mod ruida;
pub mod shx;
pub mod text;

pub use gcarve_core::geom::{self, bounds, flatten, pt, transformed, Affine, Contour, Pt, Rect};
use serde::{Deserialize, Serialize};

pub const MM_PER_IN: f64 = 25.4;
/// PCB files: Gerber layers (KiCad, Eagle, Altium names) and Excellon drill files.
/// G-code files that open as their burn moves (Laser › Run a G-code File sends one as it is).
pub const GCODE_EXTS: [&str; 6] = ["gcode", "nc", "ngc", "gc", "g", "tap"];

/// The burning moves of a G-code file (laser on, not rapids) as paths in inches, one per unbroken run.
fn import_gcode(text: &str) -> gcarve_core::svg::ImportResult {
    let prog = gcarve_core::parse_program(text, 1000.0);
    let moves: Vec<_> = prog.motions.iter().filter(|m| m.kind != gcarve_core::MoveType::Rapid && !m.machine_coords).collect();
    // fan-pin lasers (M106) never turn the "spindle" on: then every feed move counts
    let on = moves.iter().any(|m| m.spindle_on);
    let mut paths: Vec<Vec<Pt>> = Vec::new();
    for m in moves.into_iter().filter(|m| m.spindle_on || !on) {
        let pts: Vec<Pt> = gcarve_core::gcode::motion_points(m, 0.002).iter().map(|v| pt(v.x, v.y)).collect();
        match paths.last_mut() {
            Some(p) if p.last().is_some_and(|q| geom::dist(*q, pts[0]) < 1e-6) => p.extend_from_slice(&pts[1..]),
            _ => paths.push(pts),
        }
    }
    let contours: Vec<Contour> = paths
        .into_iter()
        .filter(|p| p.len() > 1)
        .map(|mut p| {
            let closed = p.len() > 3 && geom::dist(p[0], p[p.len() - 1]) < 1e-6;
            if closed {
                p.pop();
            }
            geom::make_polyline(&p, closed)
        })
        .collect();
    let note = if contours.is_empty() { "no burning moves in the G-code" } else { "to send the file as it is: Laser › Run a G-code File" }.to_string();
    gcarve_core::svg::ImportResult { layers: vec!["G-code".into(); contours.len()], contours, note, ..Default::default() }
}

pub const GERBER_EXTS: [&str; 23] = ["gbr", "ger", "gtl", "gbl", "gts", "gbs", "gto", "gbo", "gtp", "gbp", "gko", "gm1", "gml", "cmp", "sol", "plc", "pls", "stc", "sts", "crc", "crs", "art", "pho"];
pub const DRILL_EXTS: [&str; 4] = ["drl", "xln", "exc", "drd"];

/// What a layer does to its shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Mode {
    /// follow the outline (cut or score)
    #[default]
    Line,
    /// scan the inside with parallel lines `interval` apart (engrave)
    Fill,
    /// a picture: dithered or greyscale scan lines
    Image,
    /// scan the inside, then follow the outline
    FillLine,
    /// concentric insets `interval` apart (less travel on round shapes)
    Offset,
}

/// A cut layer: colour + settings. Shapes point at a layer by index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub color: [u8; 3],
    pub mode: Mode,
    /// mm/s
    pub speed: f64,
    /// % of the machine's maximum
    pub power: f64,
    pub passes: u32,
    /// fill line spacing, mm (0.1 mm = 254 DPI)
    pub interval: f64,
    /// fill scan angle, degrees
    #[serde(default)]
    pub angle: f64,
    /// sent to the machine (off = design only; LightBurn's "Output")
    pub output: bool,
    /// drawn and selectable (LightBurn's "Show")
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub air: bool,
    /// Line: the beam's width to keep off the line, mm (+ grows the shape: outside cuts; − shrinks: holes)
    #[serde(default)]
    pub kerf: f64,
    /// Line: bridges left uncut in closed shapes, and their length (mm)
    #[serde(default)]
    pub tabs: u32,
    #[serde(default = "two")]
    pub tab_len: f64,
    /// Fill / Image: dead run past each scan line's ends so the head is at speed where the burn starts, mm
    #[serde(default)]
    pub overscan: f64,
    /// Image: how the picture becomes scan lines
    #[serde(default)]
    pub dither: raster::Dither,
    #[serde(default)]
    pub negative: bool,
    /// Line: cut shapes inside other shapes first (so parts don't drop before their holes are cut)
    #[serde(default = "yes")]
    pub inner_first: bool,
    /// Fill: a second set of scan lines at 90° to the first
    #[serde(default)]
    pub cross: bool,
    /// Line: perforation: cut `perf_cut` mm, skip `perf_skip` mm, repeat (0 = off)
    #[serde(default)]
    pub perf_cut: f64,
    #[serde(default)]
    pub perf_skip: f64,
    /// Ruida: power at the slow ends of a move (corners); 0 = same as `power`
    #[serde(default)]
    pub power_min: f64,
    /// galvo: pulse frequency, kHz (fibre Q-switch / CO2 modulation)
    #[serde(default = "thirty")]
    pub freq_khz: f64,
    /// galvo (MOPA fibre): pulse width, ns; 0 = the source's own
    #[serde(default)]
    pub pulse_ns: u32,
    /// Line: dot mode: fire `dot_ms` at points `dot_mm` apart instead of cutting the line (0 = off)
    #[serde(default)]
    pub dot_ms: f64,
    #[serde(default = "one")]
    pub dot_mm: f64,
    /// Fill: hatch each shape on its own (travel between shapes) instead of the whole layer together
    #[serde(default)]
    pub fill_each: bool,
    /// Fill: flood fill (LightBurn's): each connected run of lines is finished before the next, so the head doesn't
    /// cross gaps (the arms of a U, separate letters) on every line
    #[serde(default)]
    pub flood: bool,
    /// Fill / Image: dot width correction, mm: each burn run starts and ends half this later / earlier, so the
    /// beam's spot doesn't make fills and pictures grow (LightBurn's Dot Width Correction)
    #[serde(default)]
    pub dot_width: f64,
    /// Fill / Image: every scan line burns in the same direction (cleaner edges on loose machines, slower)
    #[serde(default)]
    pub one_way: bool,
    /// Image: one scan line per picture row, no resampling (a picture prepared at the right DPI elsewhere)
    #[serde(default)]
    pub pass_through: bool,
    /// G-code Z: the head moves this far from where the job started before the layer (mm, negative = down) …
    #[serde(default)]
    pub z_offset: f64,
    /// … and this much more each pass (mm, negative = down: cutting thick stock in steps)
    #[serde(default)]
    pub z_step: f64,
    /// G-code: this layer burns in constant power (M3) even when the machine uses dynamic power (M4)
    #[serde(default)]
    pub constant_power: bool,
    /// Fill: power fades in over this many mm from every edge (rubber stamps: sloped shoulders); G-code only
    #[serde(default)]
    pub ramp: f64,
    /// Line: no automatic tabs on holes (they fall out as waste)
    #[serde(default)]
    pub tabs_skip_inner: bool,
    /// Fill: each pass turns the hatch by this many degrees (galvo-style hatch families)
    #[serde(default)]
    pub angle_step: f64,
    /// Line: wobble = small circles along the path, this diameter (mm, 0 = off) …
    #[serde(default)]
    pub wobble_mm: f64,
    /// … advancing this far per circle (mm)
    #[serde(default = "wobble_step")]
    pub wobble_step: f64,
    /// Line: closed shapes start this far off the path, in the waste (outside a part, inside a hole); mm, 0 = off
    #[serde(default)]
    pub lead_in: f64,
    /// Line: closed shapes run on past their start by this much (mm), so the loop closes cleanly
    #[serde(default)]
    pub overcut: f64,
    /// Line: closed shapes end by leaving the path this far into the waste (mm, 0 = off)
    #[serde(default)]
    pub lead_out: f64,
    /// 3D slices: after every this many passes (0 = never) the last pass is gone over again at `cleanup_power` % of
    /// the layer's power to clear what the burning left
    #[serde(default)]
    pub cleanup_every: u32,
    #[serde(default = "thirty")]
    pub cleanup_power: f64,
    /// the layer with its sub-layers, all run this many times in turn (LightBurn's galvo Global Passes; 0 / 1 = once)
    #[serde(default)]
    pub global_passes: u32,
    /// Fill: the scan angle moves on by Turn per pass every time the job starts (galvo Auto-Rotate)
    #[serde(default)]
    pub auto_rotate: bool,
    /// stand still before / after the layer (ms; galvo and G-code)
    #[serde(default)]
    pub pause_before: f64,
    #[serde(default)]
    pub pause_after: f64,
    /// galvo: this layer's own laser on / off / corner / jump delays (µs) instead of Machine Settings'
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timings: Option<[f64; 4]>,
    /// Line: the straight leads' angle off square to the path, ° (0 = straight out into the waste, 90 = along the
    /// path's line)
    #[serde(default)]
    pub lead_angle: f64,
    /// Line: lead-in / lead-out as a quarter circle of that radius, meeting the path along it (else a straight step)
    #[serde(default)]
    pub lead_arc: bool,
    /// Line: stand still this long (ms) before each cut / after it (G-code); with `pierce` > 0 the beam is on at
    /// that power (%) while it waits: Cut Through for thick material
    #[serde(default)]
    pub pause_start: f64,
    #[serde(default)]
    pub pause_end: f64,
    #[serde(default)]
    pub pierce: f64,
    /// Line: closed shapes go 0 = either way (shortest), 1 = clockwise, 2 = anticlockwise
    #[serde(default)]
    pub direction: u8,
    /// Fill: `loops` outlines `loop_gap` apart (the first on the edge) burnt after the hatch, and the hatch kept
    /// `hatch_inset` inside the last loop (or the edge): a galvo's clean-edged fill
    #[serde(default)]
    pub loops: u32,
    #[serde(default)]
    pub loop_gap: f64,
    #[serde(default)]
    pub hatch_inset: f64,
    /// Fill: the line interval stretched a little per part so the lines spread evenly edge to edge (no thin last gap)
    #[serde(default)]
    pub even_lines: bool,
    /// a two-head Ruida (LightBurn's Dual Laser): 0 = both heads at this power, 1 = laser 1 only, 2 = laser 2 only
    #[serde(default)]
    pub head: u8,
    /// the Material Library entry this layer follows (its label); "" = none. Saving that entry again updates the
    /// layer; a hand edit of a setting drops the link
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub link: String,
    /// sub-layers: more operations on this layer's shapes, run right after it with their own settings
    /// (their own `subs` are ignored)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subs: Vec<Layer>,
}

fn wobble_step() -> f64 {
    0.2
}

fn one() -> f64 {
    1.0
}

fn thirty() -> f64 {
    30.0
}

fn two() -> f64 {
    2.0
}
fn yes() -> bool {
    true
}

impl Layer {
    pub fn new(name: &str, color: [u8; 3], mode: Mode, speed: f64, power: f64) -> Layer {
        Layer {
            name: name.into(),
            color,
            mode,
            speed,
            power,
            passes: 1,
            interval: 0.1,
            angle: 0.0,
            output: true,
            visible: true,
            air: false,
            kerf: 0.0,
            tabs: 0,
            tab_len: 2.0,
            overscan: 0.0,
            dither: raster::Dither::default(),
            negative: false,
            inner_first: true,
            cross: false,
            perf_cut: 0.0,
            perf_skip: 0.0,
            power_min: 0.0,
            freq_khz: 30.0,
            pulse_ns: 0,
            dot_ms: 0.0,
            dot_mm: 1.0,
            fill_each: false,
            flood: false,
            dot_width: 0.0,
            one_way: false,
            pass_through: false,
            z_offset: 0.0,
            z_step: 0.0,
            constant_power: false,
            ramp: 0.0,
            tabs_skip_inner: false,
            angle_step: 0.0,
            wobble_mm: 0.0,
            wobble_step: 0.2,
            lead_in: 0.0,
            lead_out: 0.0,
            lead_arc: false,
            lead_angle: 0.0,
            global_passes: 0,
            cleanup_every: 0,
            cleanup_power: 30.0,
            auto_rotate: false,
            pause_before: 0.0,
            pause_after: 0.0,
            timings: None,
            pause_start: 0.0,
            pause_end: 0.0,
            pierce: 0.0,
            overcut: 0.0,
            direction: 0,
            loops: 0,
            loop_gap: 0.1,
            hatch_inset: 0.0,
            even_lines: false,
            head: 0,
            link: String::new(),
            subs: Vec::new(),
        }
    }
}

/// LightBurn-like palette: a layer per colour (the first 8; more are added as needed).
pub const PALETTE: [[u8; 3]; 12] =
    [[0, 0, 0], [0, 0, 255], [255, 0, 0], [0, 224, 0], [208, 208, 0], [255, 128, 0], [0, 224, 224], [255, 0, 255], [180, 180, 180], [128, 0, 128], [0, 128, 128], [160, 80, 0]];

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub id: i32,
    /// mm, Y up
    #[serde(with = "contour_json")]
    pub contour: Contour,
    pub layer: usize,
    #[serde(default)]
    pub name: String,
    /// a picture filling the contour's box (Image layers; the contour is its frame)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raster: Option<raster::Raster>,
    /// shapes with the same group number select and move together (0 = none)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub group: i32,
    /// locked: drawn and burnt, not picked or moved
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// a glyph of a placed text: what the text was made from (every glyph of the group carries it)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<text::TextSpec>,
    /// a module / bar of a placed QR code or barcode: what it encodes (tags re-stamped at Start)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<text::CodeSpec>,
    /// Line layers: a closed shape's cut starts at the outline point nearest this (mm); None = wherever is nearest
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<[f64; 2]>,
    /// Line layers: tabs placed by hand (outline points, mm): the cut leaves a bridge of the layer's tab length at
    /// each; when there are none the layer's evenly spaced tabs apply
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<[f64; 2]>,
    /// power scale: % of its layer's power (LightBurn's per-shape Power Scale); None = 100
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<f64>,
    /// cut order priority: lower numbers burn first (every layer of priority 0, then 1 …); 0 = the default
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub priority: i32,
    /// drawn as a rectangle / polygon: what made it, so Properties can change its corners or sides later
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prim: Option<Prim>,
}

/// One corner of an outline rounded off: point `idx`, where two straight segments meet, becomes an arc of radius
/// `r` (smaller when the segments are too short for it). None when it isn't such a corner.
/// Which features Resize Slots changes: slots' widths, slots' depths, tabs' heights.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SlotFix {
    pub width: bool,
    pub depth: bool,
    pub tab_height: bool,
}

/// Resize Slots: in a closed outline of straight lines, every U (a side, a base square to it, a side back) is a
/// slot when the material lies beyond its base, else a tab. Slots whose base is `old` ± `tol` long get `new` wide
/// (both sides move apart evenly); slots whose sides are that long get that deep; tabs that tall get that tall.
/// Returns the outline and how many features changed.
pub fn resize_slots(c: &Contour, old: f64, new: f64, tol: f64, what: SlotFix) -> (Contour, usize) {
    let n = c.nodes.len();
    if !c.closed || n < 4 || c.nodes.iter().any(|k| k.has_in || k.has_out) {
        return (c.clone(), 0);
    }
    let p: Vec<Pt> = c.nodes.iter().map(|k| k.p).collect();
    let unit = |a: Pt, b: Pt| {
        let l = geom::dist(a, b).max(1e-12);
        pt((b.x - a.x) / l, (b.y - a.y) / l)
    };
    let dot = |a: Pt, b: Pt| a.x * b.x + a.y * b.y;
    let inside = |q: Pt| geom::point_in_poly(q, n, |k| (p[k].x, p[k].y));
    let mut moves = vec![pt(0.0, 0.0); n];
    let mut count = 0;
    let delta = new - old;
    for i in 0..n {
        let (a, b, cc, d) = ((i + n - 1) % n, i, (i + 1) % n, (i + 2) % n);
        let (into, base, out) = (unit(p[a], p[b]), unit(p[b], p[cc]), unit(p[cc], p[d]));
        // a U: in, across, back out the way it came
        if dot(into, base).abs() > 0.02 || dot(out, base).abs() > 0.02 || dot(into, out) > -0.98 {
            continue;
        }
        let (side1, side2, width) = (geom::dist(p[a], p[b]), geom::dist(p[cc], p[d]), geom::dist(p[b], p[cc]));
        let mid = pt((p[b].x + p[cc].x) / 2.0, (p[b].y + p[cc].y) / 2.0);
        let slot = inside(pt(mid.x + into.x * 1e-3, mid.y + into.y * 1e-3));
        let near = |v: f64| (v - old).abs() <= tol;
        if slot && what.width && near(width) {
            // the sides move apart along the base, half each
            for (k, s) in [(a, -1.0), (b, -1.0), (cc, 1.0), (d, 1.0)] {
                moves[k] = pt(moves[k].x + base.x * s * delta / 2.0, moves[k].y + base.y * s * delta / 2.0);
            }
            count += 1;
        }
        if near(side1) && near(side2) && (slot && what.depth || !slot && what.tab_height) {
            // the base moves on along the sides
            for k in [b, cc] {
                moves[k] = pt(moves[k].x + into.x * delta, moves[k].y + into.y * delta);
            }
            count += 1;
        }
    }
    let mut out = c.clone();
    for (k, m) in moves.iter().enumerate() {
        out.nodes[k].p = pt(out.nodes[k].p.x + m.x, out.nodes[k].p.y + m.y);
    }
    (out, count)
}

/// An open path with each straight end carried on along its direction to the nearest crossing with `others`
/// (flattened outlines, `true` = closed). None = no end moved.
pub fn extend_ends(c: &Contour, others: &[(Vec<Pt>, bool)]) -> Option<Contour> {
    let n = c.nodes.len();
    if c.closed || n < 2 {
        return None;
    }
    let hit = |p: Pt, from: Pt| -> Option<Pt> {
        let l = geom::dist(p, from);
        if l < 1e-9 {
            return None;
        }
        let u = pt((p.x - from.x) / l, (p.y - from.y) / l);
        let mut best: Option<f64> = None;
        for (pts, closed) in others {
            let m = pts.len();
            for i in 0..if *closed { m } else { m.saturating_sub(1) } {
                let (a, b) = (pts[i], pts[(i + 1) % m]);
                let (ex, ey) = (b.x - a.x, b.y - a.y);
                let den = u.x * ey - u.y * ex;
                if den.abs() < 1e-12 {
                    continue;
                }
                let (wx, wy) = (a.x - p.x, a.y - p.y);
                let t = (wx * ey - wy * ex) / den;
                let s = (wx * u.y - wy * u.x) / den;
                if t > 1e-6 && (-1e-9..=1.0 + 1e-9).contains(&s) && best.is_none_or(|b| t < b) {
                    best = Some(t);
                }
            }
        }
        best.map(|t| pt(p.x + u.x * t, p.y + u.y * t))
    };
    let mut out = c.clone();
    let mut moved = false;
    if !c.nodes[0].has_out && !c.nodes[1].has_in {
        if let Some(q) = hit(c.nodes[0].p, c.nodes[1].p) {
            out.nodes[0].p = q;
            moved = true;
        }
    }
    if !c.nodes[n - 1].has_in && !c.nodes[n - 2].has_out {
        if let Some(q) = hit(c.nodes[n - 1].p, c.nodes[n - 2].p) {
            out.nodes[n - 1].p = q;
            moved = true;
        }
    }
    moved.then_some(out)
}

/// LightBurn's Optimize › Smooth: the outline resampled every `r` / 8 and run through a [1 2 1] / 4 average 20 times
/// (a Gaussian of σ ≈ 0.4 r: wobbles shorter than about `r` fade, corners round off); open paths keep their ends.
pub fn smooth_contour(c: &Contour, r: f64) -> Contour {
    let pts = flatten(c, (r / 20.0).max(0.005));
    let h = (r / 8.0).max(0.005);
    let mut ring: Vec<Pt> = pts.clone();
    if c.closed && ring.len() > 2 && ring.first() != ring.last() {
        ring.push(ring[0]);
    }
    // resample at an even step along the outline
    let mut out = vec![ring[0]];
    let mut carry = 0.0;
    for w in ring.windows(2) {
        let l = geom::dist(w[0], w[1]);
        let mut t = h - carry;
        while t < l {
            out.push(pt(w[0].x + (w[1].x - w[0].x) * t / l, w[0].y + (w[1].y - w[0].y) * t / l));
            t += h;
        }
        carry = (carry + l) % h;
    }
    if !c.closed {
        out.push(*ring.last().unwrap());
    } else if out.len() > 1 && geom::dist(out[0], *out.last().unwrap()) < h * 0.5 {
        out.pop();
    }
    let n = out.len();
    if n < 3 {
        return c.clone();
    }
    for _ in 0..20 {
        let prev = out.clone();
        for i in 0..n {
            if !c.closed && (i == 0 || i == n - 1) {
                continue;
            }
            let (a, b) = (prev[(i + n - 1) % n], prev[(i + 1) % n]);
            out[i] = pt((a.x + 2.0 * prev[i].x + b.x) / 4.0, (a.y + 2.0 * prev[i].y + b.y) / 4.0);
        }
    }
    geom::make_polyline(&out, c.closed)
}

pub fn round_corner(c: &Contour, idx: usize, r: f64) -> Option<Contour> {
    let n = c.nodes.len();
    if r <= 0.0 || idx >= n || n < 3 || !c.closed && (idx == 0 || idx + 1 == n) {
        return None;
    }
    let (ip, inx) = ((idx + n - 1) % n, (idx + 1) % n);
    let (prev, node, next) = (c.nodes[ip], c.nodes[idx], c.nodes[inx]);
    if prev.has_out || node.has_in || node.has_out || next.has_in {
        return None; // a curve on either side
    }
    let (p, la, lb) = (node.p, geom::dist(prev.p, node.p), geom::dist(next.p, node.p));
    if la < 1e-9 || lb < 1e-9 {
        return None;
    }
    let u1 = pt((prev.p.x - p.x) / la, (prev.p.y - p.y) / la);
    let u2 = pt((next.p.x - p.x) / lb, (next.p.y - p.y) / lb);
    let th = (u1.x * u2.x + u1.y * u2.y).clamp(-1.0, 1.0).acos(); // the angle inside the corner
    if th < 1e-3 || th > std::f64::consts::PI - 1e-3 {
        return None;
    }
    let half_tan = (th / 2.0).tan();
    let d = (r / half_tan).min(0.5 * la.min(lb));
    let r = d * half_tan;
    let turn = std::f64::consts::PI - th;
    let k = 4.0 / 3.0 * (turn / 4.0).tan() * r; // a cubic's handle for this much arc
    let (a, b) = (pt(p.x + u1.x * d, p.y + u1.y * d), pt(p.x + u2.x * d, p.y + u2.y * d));
    let na = geom::Node { p: a, out: pt(a.x - u1.x * k, a.y - u1.y * k), has_out: true, ..Default::default() };
    let nb = geom::Node { p: b, inh: pt(b.x - u2.x * k, b.y - u2.y * k), has_in: true, ..Default::default() };
    let mut out = c.clone();
    out.nodes.splice(idx..=idx, [na, nb]);
    Some(out)
}

/// LightBurn's Remove Fillet: the curve at point `idx` (its start or end) between two straight sides goes, and the
/// sides run on to meet in a sharp corner. None when the point isn't on such a curve or the sides are parallel.
pub fn sharp_corner(c: &Contour, idx: usize) -> Option<Contour> {
    let n = c.nodes.len();
    if idx >= n || n < 3 {
        return None;
    }
    let at = |i: isize| -> Option<usize> {
        if c.closed {
            Some(i.rem_euclid(n as isize) as usize)
        } else {
            (0..n as isize).contains(&i).then_some(i as usize)
        }
    };
    let i = idx as isize;
    let curved = |a: usize, b: usize| c.nodes[a].has_out || c.nodes[b].has_in;
    // the curve's two ends: this point and the next, or the one before and this
    let (a, b) = match (at(i + 1), at(i - 1)) {
        (Some(nx), _) if curved(idx, nx) => (idx, nx),
        (_, Some(pv)) if curved(pv, idx) => (pv, idx),
        _ => return None,
    };
    let (pa, nb) = (at(a as isize - 1)?, at(b as isize + 1)?);
    if pa == b || curved(pa, a) || curved(b, nb) {
        return None; // the sides must be straight
    }
    let (p1, p2, p3, p4) = (c.nodes[pa].p, c.nodes[a].p, c.nodes[b].p, c.nodes[nb].p);
    let (d1, d2) = (pt(p2.x - p1.x, p2.y - p1.y), pt(p4.x - p3.x, p4.y - p3.y));
    let den = d1.x * d2.y - d1.y * d2.x;
    if den.abs() < 1e-12 {
        return None;
    }
    let t = ((p3.x - p1.x) * d2.y - (p3.y - p1.y) * d2.x) / den;
    let x = pt(p1.x + d1.x * t, p1.y + d1.y * t);
    let mut out = c.clone();
    out.nodes[a] = geom::Node { p: x, ..Default::default() };
    out.nodes.remove(b);
    Some(out)
}

/// What a drawn rectangle or polygon was made from (LightBurn keeps these editable). It goes stale when the outline
/// is changed some other way (points moved, turned, welded): `fits` says whether it still describes the outline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Prim {
    Rect {
        radius: f64,
    },
    /// `inner` = a star's inner corners, % of `r` (0 = a plain polygon); `dual` = LightBurn's Dual Star: a second
    /// star of `dual` % of `r`, turned half a point, welded on (0 = none)
    Polygon {
        c: [f64; 2],
        r: f64,
        rot: f64,
        sides: i32,
        inner: f64,
        #[serde(default, skip_serializing_if = "is_zero_f")]
        dual: f64,
    },
}

impl Prim {
    /// the outline it makes; a rectangle fills `b`
    pub fn contour(&self, b: &Rect) -> Contour {
        use gcarve_core::vecops;
        match *self {
            Prim::Rect { radius } => {
                let r = radius.min(b.w().min(b.h()) / 2.0 - 1e-6);
                if r <= 0.0 {
                    return geom::make_rect(b.x0, b.y0, b.x1, b.y1);
                }
                // four quarter circles as curves: 8 points, not a flattened fillet
                use std::f64::consts::{FRAC_PI_2, PI};
                let mut g = geom::Builder::default();
                g.start(pt(b.x0 + r, b.y0));
                g.line(pt(b.x1 - r, b.y0));
                g.arc(b.x1 - r, b.y0 + r, r, r, 0.0, -FRAC_PI_2, FRAC_PI_2);
                g.line(pt(b.x1, b.y1 - r));
                g.arc(b.x1 - r, b.y1 - r, r, r, 0.0, 0.0, FRAC_PI_2);
                g.line(pt(b.x0 + r, b.y1));
                g.arc(b.x0 + r, b.y1 - r, r, r, 0.0, FRAC_PI_2, FRAC_PI_2);
                g.line(pt(b.x0, b.y0 + r));
                g.arc(b.x0 + r, b.y0 + r, r, r, 0.0, PI, FRAC_PI_2);
                g.close();
                g.c
            }
            Prim::Polygon { c, r, rot, sides, inner, dual } => {
                let (c, sides) = (pt(c[0], c[1]), sides.clamp(3, 64));
                let star = |r: f64, rot: f64| if inner > 0.0 { vecops::make_star(c, r, r * inner / 100.0, sides, rot) } else { vecops::make_polygon(c, r, sides, rot) };
                let one = star(r, rot);
                if dual <= 0.0 {
                    return one;
                }
                let two = star(r * dual / 100.0, rot + std::f64::consts::PI / sides as f64);
                vecops::boolean_op(vecops::BoolOp::Weld, &[one.clone(), two], &[]).into_iter().max_by(|a, b| Document::measure(a).1.abs().total_cmp(&Document::measure(b).1.abs())).unwrap_or(one)
            }
        }
    }
    /// still the outline it made? (same box, point count and area)
    pub fn fits(&self, c: &Contour) -> bool {
        let b = bounds(c);
        let m = self.contour(&b);
        let bm = bounds(&m);
        let (am, ac) = (Document::measure(&m).1.abs(), Document::measure(c).1.abs());
        let tol = 1e-6 * (1.0 + b.w() + b.h());
        m.nodes.len() == c.nodes.len() && [bm.x0 - b.x0, bm.y0 - b.y0, bm.x1 - b.x1, bm.y1 - b.y1].iter().all(|d| d.abs() < tol) && (am - ac).abs() <= 1e-6 * am.max(1.0)
    }
    /// moved with its shape: a polygon's centre, size and turn follow `m` (a shear or uneven scale leaves it stale)
    pub fn transform(&mut self, m: &Affine) {
        if let Prim::Polygon { c, r, rot, .. } = self {
            let q = m.apply(pt(c[0], c[1]));
            *c = [q.x, q.y];
            *r *= (m.a * m.d - m.b * m.c).abs().sqrt();
            *rot += m.b.atan2(m.a);
        }
    }
}

fn is_zero_f(v: &f64) -> bool {
    *v == 0.0
}

fn is_zero(v: &i32) -> bool {
    *v == 0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub name: String,
    pub shapes: Vec<Shape>,
    pub layers: Vec<Layer>,
    /// machine bed, mm
    pub bed: [f64; 2],
    /// show sizes in inches (saved with the design, like LightBurn)
    #[serde(default)]
    pub inches: bool,
    /// File › Notes: settings that worked, the material, anything (saved with the design)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// named sizes (name, what was typed: "3mm", "gap * 2"), usable by name in the app's size boxes
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vars: Vec<(String, String)>,
    /// the laser this design is for (a saved machine's name, or one picked from the machine list)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub laser: String,
    /// LightBurn's virtual arrays: live copies of some shapes (they follow the originals; burnt as real shapes)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arrays: Vec<VirtualArray>,
    /// several sheets in one design (LightBurn's Pages): the open page's shapes are `shapes`, the others wait in
    /// their `Page`; cut settings are shared. Empty = one page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<Page>,
    #[serde(default)]
    pub page: usize,
    next_id: i32,
}

/// A sheet of the design: its name and, while another page is open, its shapes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapes: Vec<Shape>,
}

/// A grid of copies of `ids`, `cols` × `rows`, each `dx` / `dy` mm on from the one before (the originals are the
/// first cell).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualArray {
    pub ids: Vec<i32>,
    pub cols: u32,
    pub rows: u32,
    pub dx: f64,
    pub dy: f64,
    /// a circle instead of a grid: this many copies (the original counted) turned evenly round `centre`
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub turns: u32,
    #[serde(default)]
    pub centre: [f64; 2],
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

impl VirtualArray {
    /// where each copy goes (the original's cell left out)
    pub fn moves(&self) -> Vec<Affine> {
        if self.turns > 1 {
            let (cx, cy) = (self.centre[0], self.centre[1]);
            (1..self.turns).map(|k| Affine::translate(cx, cy).then(&Affine::rotate(std::f64::consts::TAU * k as f64 / self.turns as f64)).then(&Affine::translate(-cx, -cy))).collect()
        } else {
            (0..self.rows.max(1)).flat_map(|r| (0..self.cols.max(1)).map(move |c| (c, r))).filter(|&(c, r)| c + r > 0).map(|(c, r)| Affine::translate(c as f64 * self.dx, r as f64 * self.dy)).collect()
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Document {
            name: "Untitled".into(),
            shapes: Vec::new(),
            layers: vec![default_layer(0)],
            bed: [600.0, 400.0],
            inches: false,
            notes: String::new(),
            vars: Vec::new(),
            laser: String::new(),
            arrays: Vec::new(),
            pages: Vec::new(),
            page: 0,
            next_id: 1,
        }
    }
}

/// A fresh layer for palette colour `i`: the black one engraves, the rest cut.
pub fn default_layer(i: usize) -> Layer {
    let c = PALETTE[i % PALETTE.len()];
    match i % PALETTE.len() {
        0 => Layer::new("Engrave", c, Mode::Fill, 300.0, 40.0),
        1 => Layer::new("Score", c, Mode::Line, 100.0, 20.0),
        _ => Layer::new("Cut", c, Mode::Line, 15.0, 90.0),
    }
}

impl Document {
    /// how many pages (1 when the design has never had a second)
    pub fn page_count(&self) -> usize {
        self.pages.len().max(1)
    }
    /// open page `i`: the open page's shapes go back to it, page i's come out
    pub fn switch_page(&mut self, i: usize) {
        if i >= self.pages.len() || i == self.page {
            return;
        }
        std::mem::swap(&mut self.shapes, &mut self.pages[self.page].shapes);
        self.page = i;
        std::mem::swap(&mut self.shapes, &mut self.pages[i].shapes);
    }
    /// a new page after the open one (empty, or a copy of the open one with new ids), opened
    pub fn add_page(&mut self, copy: bool) -> usize {
        if self.pages.is_empty() {
            self.pages.push(Page { name: "Page 1".into(), shapes: Vec::new() });
            self.page = 0;
        }
        let mut shapes = Vec::new();
        if copy {
            // fresh ids; groups stay groups (group numbers are per page in practice, so they can be kept)
            for s in &self.shapes {
                let mut c = s.clone();
                c.id = self.next_id;
                self.next_id += 1;
                shapes.push(c);
            }
        }
        let at = self.page + 1;
        let name = format!("Page {}", self.pages.len() + 1);
        self.pages.insert(at, Page { name, shapes });
        // the open page's index moves nothing (inserted after it); open the new one
        self.switch_page(at);
        at
    }
    /// remove page `i` and its shapes (never the last one)
    pub fn remove_page(&mut self, i: usize) {
        if self.pages.len() < 2 || i >= self.pages.len() {
            return;
        }
        if i == self.page {
            self.switch_page(if i > 0 { i - 1 } else { 1 });
        }
        self.pages.remove(i);
        if self.page > i {
            self.page -= 1;
        }
        if self.pages.len() == 1 {
            self.pages.clear(); // one page again: no page list
            self.page = 0;
        }
    }

    pub fn add(&mut self, contour: Contour, layer: usize) -> i32 {
        let id = self.next_id;
        self.next_id += 1;
        while self.layers.len() <= layer {
            self.layers.push(default_layer(self.layers.len()));
        }
        self.shapes.push(Shape {
            id,
            contour,
            layer,
            name: String::new(),
            raster: None,
            group: 0,
            locked: false,
            text: None,
            code: None,
            start: None,
            power: None,
            tabs: Vec::new(),
            prim: None,
            priority: 0,
        });
        id
    }
    /// A picture as a shape `width` mm wide at `at` (its bottom-left), on `layer` (made an Image layer when it is
    /// a fresh one). PNG, JPEG, BMP, GIF or WebP bytes; scaled to 2000 px on the long side at most.
    pub fn add_image(&mut self, bytes: &[u8], at: Pt, width: f64, layer: usize) -> Result<i32, String> {
        let r = raster::Raster::decode(bytes, 2000)?;
        let h = width / r.aspect();
        Ok(self.add_raster(r, geom::make_rect(at.x, at.y, at.x + width, at.y + h), layer))
    }
    /// a decoded picture filling the rectangle `box_` on `layer` (a fresh layer becomes an Image layer)
    fn add_raster(&mut self, r: raster::Raster, box_: Contour, layer: usize) -> i32 {
        let id = self.add(box_, layer);
        self.find_mut(id).unwrap().raster = Some(r);
        if self.shapes.iter().filter(|s| s.layer == layer).count() == 1 && self.layers[layer].mode != Mode::Image {
            self.layers[layer].mode = Mode::Image;
            self.layers[layer].speed = 200.0;
            self.layers[layer].power = 30.0;
        }
        id
    }
    pub fn find(&self, id: i32) -> Option<&Shape> {
        self.shapes.iter().find(|s| s.id == id)
    }
    pub fn find_mut(&mut self, id: i32) -> Option<&mut Shape> {
        self.shapes.iter_mut().find(|s| s.id == id)
    }
    pub fn remove(&mut self, ids: &[i32]) {
        self.shapes.retain(|s| !ids.contains(&s.id));
    }
    /// `ids` plus every shape grouped with one of them
    /// The design as SVG / DXF text (gcarve-core's writers, inches), one layer name per shape.
    /// A one-page vector PDF of the outlines at true size (print it 100 %: templates, Print & Cut sheets), in the
    /// layers' colours, the page the design's box plus 10 mm. Pictures are left out.
    pub fn export_pdf(&self) -> Vec<u8> {
        self.export_pdf_in(true)
    }
    /// The same PDF in the layers' colours or all black (printing on a mono printer, Print & Cut marks).
    pub fn export_pdf_in(&self, colour: bool) -> Vec<u8> {
        let k = 72.0 / MM_PER_IN; // points per mm
        let b = self.bounds();
        let (x0, y0) = if b.is_empty() { (0.0, 0.0) } else { (b.x0 - 10.0, b.y0 - 10.0) };
        let (w, h) = if b.is_empty() { (210.0, 297.0) } else { (b.w() + 20.0, b.h() + 20.0) };
        let p = |q: Pt| format!("{:.3} {:.3}", (q.x - x0) * k, (q.y - y0) * k);
        let mut c = String::from("0.5 w 1 J 1 j\n");
        for s in self.shapes.iter().filter(|s| s.raster.is_none() && s.contour.nodes.len() >= 2) {
            let col = if colour { self.layers.get(s.layer).map_or([0, 0, 0], |l| l.color) } else { [0, 0, 0] };
            c += &format!("{:.3} {:.3} {:.3} RG\n{} m\n", col[0] as f64 / 255.0, col[1] as f64 / 255.0, col[2] as f64 / 255.0, p(s.contour.nodes[0].p));
            for i in 0..geom::seg_count(&s.contour) {
                let g = geom::segment(&s.contour, i);
                c += &if g.curve { format!("{} {} {} c\n", p(g.c1), p(g.c2), p(g.p1)) } else { format!("{} l\n", p(g.p1)) };
            }
            c += if s.contour.closed { "h S\n" } else { "S\n" };
        }
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.3} {:.3}] /Contents 4 0 R >>", w * k, h * k),
            format!("<< /Length {} >>\nstream\n{c}endstream", c.len()),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
        out
    }

    pub fn export_vectors(&self, kind: &str) -> String {
        let m = Affine::scale(1.0 / MM_PER_IN, 1.0 / MM_PER_IN);
        let cs: Vec<(Contour, String)> = self.shapes.iter().filter(|s| s.raster.is_none()).map(|s| (transformed(&s.contour, &m), self.layers[s.layer].name.clone())).collect();
        let items: Vec<(&Contour, &str)> = cs.iter().map(|(c, l)| (c, l.as_str())).collect();
        if kind == "dxf" {
            gcarve_core::dxf::export_dxf(&items)
        } else {
            let b = self.bounds();
            gcarve_core::svg::export_svg(&items, (b.x1.max(0.0) + 5.0) / MM_PER_IN, (b.y1.max(0.0) + 5.0) / MM_PER_IN)
        }
    }
    /// The design as the laser sees it: each sub-layer becomes an ordinary layer right after its parent, holding
    /// copies of the parent's shapes. Planner, previews and encoders all work on this.
    /// Shapes with a power scale go to a copy of their layer (right after it) at that share of its power, so every
    /// encoder handles them without per-shape power.
    /// The design with its virtual arrays made into real copies (new ids; a copied group gets a group of its own).
    pub fn with_arrays(&self) -> Document {
        let mut d = self.clone();
        d.arrays.clear();
        for a in &self.arrays {
            let src: Vec<Shape> = self.shapes.iter().filter(|s| a.ids.contains(&s.id)).cloned().collect();
            for m in a.moves() {
                d.copy_shapes(&src, &m);
            }
        }
        d
    }

    /// Whole copies of `src` (text, codes, pictures, settings) with new ids, each group in it a new group, moved by
    /// `m` (pictures resampled when turned); returns the new ids in `src`'s order
    pub fn copy_shapes(&mut self, src: &[Shape], m: &Affine) -> Vec<i32> {
        let mut next_g = self.shapes.iter().map(|s| s.group).max().unwrap_or(0) + 1;
        let mut groups: Vec<(i32, i32)> = Vec::new();
        let mut ids = Vec::new();
        for s in src {
            let mut n = s.clone();
            n.id = self.next_id;
            self.next_id += 1;
            if n.group != 0 {
                n.group = match groups.iter().find(|(old, _)| *old == s.group) {
                    Some((_, g)) => *g,
                    None => {
                        groups.push((s.group, next_g));
                        next_g += 1;
                        next_g - 1
                    }
                };
            }
            ids.push(n.id);
            self.shapes.push(n);
        }
        self.transform(&ids, m);
        ids
    }

    pub fn for_job(&self) -> Document {
        if !self.arrays.is_empty() {
            return self.with_arrays().for_job();
        }
        let scale = |s: &Shape| s.power.filter(|p| (p - 100.0).abs() > 1e-6);
        if self.layers.iter().all(|l| l.subs.is_empty() && l.global_passes <= 1) && self.shapes.iter().all(|s| scale(s).is_none()) {
            return self.clone();
        }
        let mut d = Document { layers: Vec::new(), shapes: Vec::new(), ..self.clone() };
        let mut map: Vec<Vec<(Option<f64>, usize)>> = Vec::new(); // design layer -> its job layers (main first) by power scale
        for (i, l) in self.layers.iter().enumerate() {
            let mut scales: Vec<f64> = self.shapes.iter().filter(|s| s.layer == i).filter_map(scale).collect();
            scales.sort_by(|a, b| b.partial_cmp(a).unwrap());
            scales.dedup();
            let mut idx = Vec::new();
            let ops: Vec<Layer> = std::iter::once(Layer { subs: Vec::new(), ..l.clone() })
                .chain(l.subs.iter().enumerate().map(|(k, sub)| {
                    let name = if sub.name.is_empty() || sub.name == l.name { format!("{} · {}", l.name, k + 2) } else { sub.name.clone() };
                    Layer { subs: Vec::new(), name, color: l.color, visible: l.visible, ..sub.clone() }
                }))
                .collect();
            // global passes: the whole run (layer and sub-layers) again, in order
            let reps = l.global_passes.clamp(1, plan::MAX_PASSES) as usize;
            for op in ops.iter().cycle().take(ops.len() * reps).cloned() {
                idx.push((None, d.layers.len()));
                d.layers.push(op.clone());
                for sc in &scales {
                    idx.push((Some(*sc), d.layers.len()));
                    d.layers.push(Layer { name: format!("{} · {}%", op.name, sc), power: op.power * sc / 100.0, power_min: op.power_min * sc / 100.0, ..op.clone() });
                }
            }
            map.push(idx);
        }
        let mut next = self.next_id;
        for s in &self.shapes {
            let sc = scale(s);
            for (k, li) in map[s.layer].iter().filter(|(x, _)| *x == sc).map(|(_, li)| li).enumerate() {
                let mut c = s.clone();
                c.layer = *li;
                c.power = None;
                if k > 0 {
                    c.id = next;
                    next += 1;
                }
                d.shapes.push(c);
            }
        }
        d.next_id = next;
        d
    }
    /// A LightBurn project (.lbrn2, mm): a CutSetting per layer that has vectors, a Path per shape (curves kept as
    /// beziers). Pictures are left out (LightBurn wants the image bytes in its own form).
    pub fn export_lbrn2(&self) -> String {
        use std::fmt::Write;
        let f = |v: f64| {
            let s = format!("{v:.4}");
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        };
        let mut out =
            String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<LightBurnProject AppVersion=\"1.7.00\" FormatVersion=\"1\" MaterialHeight=\"0\" MirrorX=\"False\" MirrorY=\"False\">\n");
        for (i, l) in self.layers.iter().enumerate() {
            if !self.shapes.iter().any(|s| s.layer == i) {
                continue;
            }
            let kind = match l.mode {
                Mode::Line => "Cut",
                Mode::Fill => "Scan",
                Mode::FillLine => "Scan+Cut",
                Mode::Offset => "Offset",
                Mode::Image => "Image",
            };
            let name = l.name.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
            let _ = writeln!(
                out,
                "<CutSetting type=\"{kind}\"><index Value=\"{i}\"/><name Value=\"{name}\"/><minPower Value=\"{}\"/><maxPower Value=\"{}\"/><speed Value=\"{}\"/><numPasses Value=\"{}\"/><interval Value=\"{}\"/><angle Value=\"{}\"/><priority Value=\"{i}\"/><doOutput Value=\"{}\"/><hide Value=\"{}\"/><runBlower Value=\"{}\"/><crossHatch Value=\"{}\"/><ditherMode Value=\"{}\"/></CutSetting>",
                f(l.power_min.min(l.power)),
                f(l.power),
                f(l.speed),
                l.passes.max(1),
                f(l.interval),
                f(l.angle),
                l.output as u8,
                !l.visible as u8,
                l.air as u8,
                l.cross as u8,
                LB_DITHER.iter().find(|d| d.1 == l.dither).map_or("dither", |d| d.0)
            );
        }
        for s in self.shapes.iter().filter(|s| s.raster.is_none() && s.contour.nodes.len() >= 2) {
            let c = &s.contour;
            let n = c.nodes.len();
            let mut c0: Vec<Option<Pt>> = vec![None; n];
            let mut c1: Vec<Option<Pt>> = vec![None; n];
            let mut prims = String::new();
            for i in 0..geom::seg_count(c) {
                let g = geom::segment(c, i);
                let (a, b) = (i, (i + 1) % n);
                if g.curve {
                    c0[a] = Some(g.c1);
                    c1[b] = Some(g.c2);
                    let _ = write!(prims, "B{a} {b}");
                } else {
                    let _ = write!(prims, "L{a} {b}");
                }
            }
            let mut verts = String::new();
            for (i, nd) in c.nodes.iter().enumerate() {
                let _ = write!(verts, "V{} {}", f(nd.p.x), f(nd.p.y));
                match c0[i] {
                    Some(q) => {
                        let _ = write!(verts, "c0x{}c0y{}", f(q.x), f(q.y));
                    }
                    None => verts.push_str("c0x1"),
                }
                match c1[i] {
                    Some(q) => {
                        let _ = write!(verts, "c1x{}c1y{}", f(q.x), f(q.y));
                    }
                    None => verts.push_str("c1x1"),
                }
            }
            let _ = writeln!(out, "<Shape Type=\"Path\" CutIndex=\"{}\"><XForm>1 0 0 1 0 0</XForm><VertList>{verts}</VertList><PrimList>{prims}</PrimList></Shape>", s.layer);
        }
        // pictures: LightBurn's Bitmap (W × H centred on the XForm's move), the grey pixels as a PNG
        for s in self.shapes.iter().filter(|s| s.raster.is_some()) {
            let (b, Some(png)) = (bounds(&s.contour), s.raster.as_ref().and_then(|r| r.png())) else { continue };
            let _ = writeln!(
                out,
                "<Shape Type=\"Bitmap\" CutIndex=\"{}\" W=\"{}\" H=\"{}\" Gamma=\"1\" Contrast=\"0\" Brightness=\"0\" File=\"\" Data=\"{}\"><XForm>1 0 0 1 {} {}</XForm></Shape>",
                s.layer,
                f(b.w()),
                f(b.h()),
                gcarve_core::photo::b64_encode(&png),
                f((b.x0 + b.x1) / 2.0),
                f((b.y0 + b.y1) / 2.0)
            );
        }
        out.push_str("</LightBurnProject>\n");
        out
    }
    /// Outline length (mm) and enclosed area (mm², 0 for open paths) of a contour.
    pub fn measure(c: &Contour) -> (f64, f64) {
        let pts = flatten(c, 0.02);
        if pts.len() < 2 {
            return (0.0, 0.0);
        }
        let mut len: f64 = pts.windows(2).map(|w| geom::dist(w[0], w[1])).sum();
        if c.closed && pts.first() != pts.last() {
            len += geom::dist(pts[0], *pts.last().unwrap());
        }
        let area = if c.closed {
            let n = pts.len();
            (0..n)
                .map(|i| {
                    let (a, b) = (pts[i], pts[(i + 1) % n]);
                    a.x * b.y - b.x * a.y
                })
                .sum::<f64>()
                .abs()
                / 2.0
        } else {
            0.0
        };
        (len, area)
    }
    pub fn with_groups(&self, ids: &[i32]) -> Vec<i32> {
        let groups: Vec<i32> = self.shapes.iter().filter(|s| s.group != 0 && ids.contains(&s.id)).map(|s| s.group).collect();
        let mut out: Vec<i32> = ids.to_vec();
        for s in &self.shapes {
            if s.group != 0 && groups.contains(&s.group) && !out.contains(&s.id) {
                out.push(s.id);
            }
        }
        out
    }
    /// one group out of `ids` (and whatever they were grouped with); returns the group number
    pub fn group(&mut self, ids: &[i32]) -> i32 {
        let all = self.with_groups(ids);
        let g = self.shapes.iter().map(|s| s.group).max().unwrap_or(0) + 1;
        for s in self.shapes.iter_mut().filter(|s| all.contains(&s.id)) {
            s.group = g;
        }
        g
    }
    /// LightBurn's Auto-Group: each outermost closed shape is grouped with what lies inside it (holes, detail).
    /// Pictures stay out. Returns the number of groups made.
    /// LightBurn's import clean-up: open paths among `ids` (per layer and group) whose ends meet within `tol` are
    /// joined into one, and a path whose two ends meet is closed. Returns the ids now standing for them.
    pub fn join_loose_ends(&mut self, ids: &[i32], tol: f64) -> Vec<i32> {
        let mut keys: Vec<(usize, i32)> = Vec::new();
        for s in self.shapes.iter().filter(|s| ids.contains(&s.id) && !s.contour.closed && s.raster.is_none() && s.text.is_none() && s.code.is_none()) {
            if !keys.contains(&(s.layer, s.group)) {
                keys.push((s.layer, s.group));
            }
        }
        let mut out: Vec<i32> = ids.iter().copied().filter(|id| self.find(*id).is_some_and(|s| s.contour.closed || s.raster.is_some() || s.text.is_some() || s.code.is_some())).collect();
        for (layer, group) in keys {
            let (mine, open): (Vec<i32>, Vec<Contour>) = self
                .shapes
                .iter()
                .filter(|s| ids.contains(&s.id) && !s.contour.closed && s.layer == layer && s.group == group && s.raster.is_none() && s.text.is_none() && s.code.is_none())
                .map(|s| (s.id, s.contour.clone()))
                .unzip();
            let joined: Vec<Contour> = gcarve_core::vecops::join_open(&open, tol)
                .into_iter()
                .map(|c| if !c.closed && c.nodes.len() >= 3 && geom::dist(c.nodes[0].p, c.nodes[c.nodes.len() - 1].p) <= tol { gcarve_core::vecops::close_contour(c) } else { c })
                .collect();
            self.remove(&mine);
            for c in joined {
                let id = self.add(c, layer);
                if let Some(s) = self.find_mut(id) {
                    s.group = group;
                }
                out.push(id);
            }
        }
        out
    }
    /// Shapes (not in `ids`) lying wholly inside one of the closed shapes in `ids`.
    pub fn inside_of(&self, ids: &[i32]) -> Vec<i32> {
        let outers: Vec<(Rect, Vec<Pt>)> =
            self.shapes.iter().filter(|s| ids.contains(&s.id) && s.contour.closed && s.raster.is_none()).map(|s| (bounds(&s.contour), flatten(&s.contour, 0.1))).collect();
        self.shapes
            .iter()
            .filter(|s| !ids.contains(&s.id))
            .filter(|s| {
                let (b, pts) = (bounds(&s.contour), flatten(&s.contour, 0.1));
                outers.iter().any(|(ob, o)| ob.contains(&b) && pts.iter().all(|p| geom::point_in_poly(*p, o.len(), |k| (o[k].x, o[k].y))))
            })
            .map(|s| s.id)
            .collect()
    }
    pub fn auto_group(&mut self, ids: &[i32]) -> usize {
        let cand: Vec<(i32, Rect, Vec<Pt>, f64, bool)> = self
            .shapes
            .iter()
            .filter(|s| ids.contains(&s.id) && s.raster.is_none())
            .map(|s| (s.id, bounds(&s.contour), flatten(&s.contour, 0.1), Document::measure(&s.contour).1, s.contour.closed))
            .collect();
        // each shape's root: the largest closed shape around it (itself when none)
        let root = |i: usize| {
            let (_, b, pts, _, _) = &cand[i];
            let Some(p) = pts.first() else { return i };
            (0..cand.len())
                .filter(|&j| j != i && cand[j].4 && cand[j].1.contains(b) && geom::point_in_poly(*p, cand[j].2.len(), |k| (cand[j].2[k].x, cand[j].2[k].y)))
                .max_by(|&a, &c| cand[a].3.total_cmp(&cand[c].3))
                .unwrap_or(i)
        };
        let roots: Vec<usize> = (0..cand.len()).map(root).collect();
        let mut made = 0;
        let mut g = self.shapes.iter().map(|s| s.group).max().unwrap_or(0);
        for r in 0..cand.len() {
            let members: Vec<i32> = (0..cand.len()).filter(|&i| roots[i] == r).map(|i| cand[i].0).collect();
            if members.len() > 1 && members.contains(&cand[r].0) {
                g += 1;
                made += 1;
                for s in self.shapes.iter_mut().filter(|s| members.contains(&s.id)) {
                    s.group = g;
                }
            }
        }
        made
    }
    pub fn ungroup(&mut self, ids: &[i32]) {
        let all = self.with_groups(ids);
        for s in self.shapes.iter_mut().filter(|s| all.contains(&s.id)) {
            s.group = 0;
        }
    }
    /// Swap layer `i` with its neighbour (cut order = list order); shapes follow their layer.
    pub fn move_layer(&mut self, i: usize, up: bool) -> bool {
        let j = if up { i.checked_sub(1) } else { Some(i + 1) };
        let Some(j) = j.filter(|j| *j < self.layers.len()) else { return false };
        self.layers.swap(i, j);
        for s in self.shapes.iter_mut().chain(self.pages.iter_mut().flat_map(|p| p.shapes.iter_mut())) {
            if s.layer == i {
                s.layer = j;
            } else if s.layer == j {
                s.layer = i;
            }
        }
        true
    }
    /// Drop an empty layer (never the last one); shapes above it move down one index.
    pub fn remove_layer(&mut self, i: usize) -> bool {
        if self.layers.len() < 2 || i >= self.layers.len() || self.shapes.iter().chain(self.pages.iter().flat_map(|p| p.shapes.iter())).any(|s| s.layer == i) {
            return false;
        }
        self.layers.remove(i);
        for s in self.shapes.iter_mut().chain(self.pages.iter_mut().flat_map(|p| p.shapes.iter_mut())) {
            if s.layer > i {
                s.layer -= 1;
            }
        }
        true
    }
    /// The picture in shape `id` traced to closed outlines (gcarve-core's tracer), placed over the picture, on
    /// `layer`, grouped. `threshold` None = automatic; `smooth` and `min_area` in pixels.
    pub fn trace_picture(&mut self, id: i32, layer: usize, threshold: Option<u8>, invert: bool, smooth: f64, min_area: f64) -> Vec<i32> {
        let Some((r, b)) = self.find(id).and_then(|s| s.raster.as_ref().map(|r| (r.clone(), bounds(&s.contour)))) else { return Vec::new() };
        let Some(px) = r.pixels() else { return Vec::new() };
        let o = gcarve_core::trace::TraceOptions { threshold, invert, smooth_px: smooth, min_area_px: min_area, width_in: b.w() / MM_PER_IN, centreline: false };
        let cs = gcarve_core::trace::trace(&px, r.w as usize, r.h as usize, &o);
        let m = Affine::translate(b.x0, b.y0).then(&Affine::scale(MM_PER_IN, MM_PER_IN));
        let out: Vec<i32> = cs.into_iter().map(|c| self.add(transformed(&c, &m), layer)).collect();
        if out.len() > 1 {
            self.group(&out);
        }
        out
    }
    /// Centerline trace of a picture: its dark strokes as single lines (`raster::centerline`), grouped, on `layer`.
    pub fn trace_centerline(&mut self, id: i32, layer: usize, threshold: Option<u8>, invert: bool, smooth_px: f64, min_len_px: f64) -> Vec<i32> {
        let Some((r, b)) = self.find(id).and_then(|s| s.raster.as_ref().map(|r| (r.clone(), bounds(&s.contour)))) else { return Vec::new() };
        let Some(px) = r.pixels() else { return Vec::new() };
        let (w, h) = (r.w as usize, r.h as usize);
        let to_mm = |p: Pt| pt(b.x0 + p.x / w as f64 * b.w(), b.y1 - p.y / h as f64 * b.h());
        let lines = raster::centerline(&px, w, h, threshold, invert, smooth_px.max(0.3), min_len_px);
        let out: Vec<i32> = lines
            .into_iter()
            .map(|(l, closed)| {
                let mut pts: Vec<Pt> = l.into_iter().map(to_mm).collect();
                if closed && pts.len() > 2 {
                    pts.pop();
                }
                self.add(geom::make_polyline(&pts, closed), layer)
            })
            .collect();
        if out.len() > 1 {
            self.group(&out);
        }
        out
    }
    /// a layer for `name` (importers' layer names), made when missing
    pub fn layer_named(&mut self, name: &str) -> usize {
        if name.is_empty() {
            return 0;
        }
        if let Some(i) = self.layers.iter().position(|l| l.name == name) {
            return i;
        }
        let mut l = default_layer(self.layers.len());
        l.name = name.to_string();
        self.layers.push(l);
        self.layers.len() - 1
    }
    pub fn bounds(&self) -> Rect {
        let mut r = Rect::default();
        for s in &self.shapes {
            r.add_rect(&bounds(&s.contour));
        }
        r
    }
    pub fn bounds_of(&self, ids: &[i32]) -> Rect {
        let mut r = Rect::default();
        for s in self.shapes.iter().filter(|s| ids.contains(&s.id)) {
            r.add_rect(&bounds(&s.contour));
        }
        r
    }
    pub fn transform(&mut self, ids: &[i32], m: &Affine) {
        for s in self.shapes.iter_mut().filter(|s| ids.contains(&s.id)) {
            // a turned / mirrored picture: its pixels turn with it onto a new upright box
            if let Some((r, nb)) = s.raster.as_ref().and_then(|r| r.transformed(&bounds(&s.contour), m)) {
                s.raster = Some(r);
                s.contour = geom::make_rect(nb.x0, nb.y0, nb.x1, nb.y1);
                continue;
            }
            s.contour = transformed(&s.contour, m);
            if let Some(p) = &mut s.prim {
                p.transform(m);
                // a rounded rectangle stretched straight keeps round corners (as drawn, not squashed)
                if matches!(p, Prim::Rect { radius } if *radius > 0.0) && m.b.abs() < 1e-12 && m.c.abs() < 1e-12 {
                    s.contour = p.contour(&bounds(&s.contour));
                }
            }
            // the points that belong to the outline go with it
            for p in s.start.iter_mut().chain(s.tabs.iter_mut()) {
                let q = m.apply(pt(p[0], p[1]));
                *p = [q.x, q.y];
            }
        }
    }

    /// Bend (° of arc across the width: + arches up, − sags) and taper (top width % of the bottom) the shapes as one
    /// piece, inside their box. Curves are flattened (0.05 mm); texts stop being editable text. Pictures are left.
    pub fn warp(&mut self, ids: &[i32], bend_deg: f64, taper_pct: f64) {
        let b = self.bounds_of(ids);
        if b.is_empty() {
            return;
        }
        let (cx, w, h) = ((b.x0 + b.x1) / 2.0, b.w().max(1e-9), b.h().max(1e-9));
        let th = bend_deg.to_radians();
        self.map_points(ids, |p: Pt| {
            let k = 1.0 + (taper_pct / 100.0 - 1.0) * (p.y - b.y0) / h;
            let x = cx + (p.x - cx) * k;
            if th.abs() < 1e-6 {
                return pt(x, p.y);
            }
            // the box's bottom edge becomes an arc of radius w/θ round a centre below it (above for a sag)
            let r0 = w / th;
            let (phi, r) = ((x - cx) / w * th, r0 + (p.y - b.y0));
            pt(cx + r * phi.sin(), b.y0 - r0 + r * phi.cos())
        });
    }

    /// Warp: the selection's box corners (bottom-left, bottom-right, top-right, top-left) moved to `to`, as a
    /// perspective (the four-point homography); outlines get a point every 0.5 mm first.
    pub fn warp_quad(&mut self, ids: &[i32], to: [Pt; 4]) {
        let b = self.bounds_of(ids);
        if b.is_empty() {
            return;
        }
        let from = [pt(b.x0, b.y0), pt(b.x1, b.y0), pt(b.x1, b.y1), pt(b.x0, b.y1)];
        if let Some(h) = raster::homography(from, to) {
            self.map_points(ids, |p| raster::apply_h(&h, p));
        }
    }

    /// Deform: a 4 × 4 grid of handles (row by row from the bottom, left to right; at rest they sit at thirds
    /// of the box) as a bicubic Bézier patch over the selection's box.
    pub fn deform(&mut self, ids: &[i32], grid: &[Pt; 16]) {
        let b = self.bounds_of(ids);
        if b.is_empty() {
            return;
        }
        let bern = |t: f64| [(1.0 - t).powi(3), 3.0 * t * (1.0 - t).powi(2), 3.0 * t * t * (1.0 - t), t.powi(3)];
        let (w, h) = (b.w().max(1e-9), b.h().max(1e-9));
        self.map_points(ids, |p| {
            let (bu, bv) = (bern((p.x - b.x0) / w), bern((p.y - b.y0) / h));
            let mut q = pt(0.0, 0.0);
            for j in 0..4 {
                for i in 0..4 {
                    let k = bu[i] * bv[j];
                    q.x += k * grid[j * 4 + i].x;
                    q.y += k * grid[j * 4 + i].y;
                }
            }
            q
        });
    }

    /// Every outline point of the shapes `ids` through `map` (straight edges get a point every 0.5 mm so they
    /// bend too; pictures are left; texts become plain outlines; start points and tabs move along).
    pub fn map_points(&mut self, ids: &[i32], map: impl Fn(Pt) -> Pt) {
        for s in self.shapes.iter_mut().filter(|s| ids.contains(&s.id) && s.raster.is_none()) {
            // straight edges get a point every 0.5 mm so they bend too
            let f = flatten(&s.contour, 0.05);
            let n = f.len();
            let segs = if s.contour.closed { n } else { n.saturating_sub(1) };
            let mut pts: Vec<Pt> = Vec::new();
            for i in 0..segs {
                let (a, c) = (f[i], f[(i + 1) % n]);
                let k = ((a.x - c.x).hypot(a.y - c.y) / 0.5).ceil().max(1.0) as usize;
                pts.extend((0..k).map(|j| map(pt(a.x + (c.x - a.x) * j as f64 / k as f64, a.y + (c.y - a.y) * j as f64 / k as f64))));
            }
            if !s.contour.closed {
                pts.extend(f.last().map(|p| map(*p)));
            }
            s.contour = geom::make_polyline(&pts, s.contour.closed);
            s.text = None;
            s.prim = None;
            for p in s.start.iter_mut().chain(s.tabs.iter_mut()) {
                let q = map(pt(p[0], p[1]));
                *p = [q.x, q.y];
            }
        }
    }

    /// The design as JSON (.gburn).
    pub fn to_json(&self) -> String {
        serde_json::to_string(&Versioned { version: 1, app: "GBurn", doc: self }).unwrap_or_default()
    }
    /// A .gburn back; damaged files are refused with a reason, never a panic.
    pub fn from_json(text: &str) -> Result<Document, String> {
        let v: VersionedOwned = serde_json::from_str(text).map_err(|e| format!("not a GBurn file: {e}"))?;
        let mut d = v.doc;
        if d.layers.is_empty() {
            d.layers.push(default_layer(0));
        }
        let top_layer = d.layers.len() - 1;
        for shapes in std::iter::once(&mut d.shapes).chain(d.pages.iter_mut().map(|p| &mut p.shapes)) {
            shapes.retain(|s| s.contour.nodes.iter().all(|n| n.p.x.is_finite() && n.p.y.is_finite()));
            for s in shapes {
                s.layer = s.layer.min(top_layer);
            }
        }
        if !(d.bed[0] > 0.0 && d.bed[1] > 0.0 && d.bed[0] < 1e5 && d.bed[1] < 1e5) {
            d.bed = [600.0, 400.0];
        }
        if d.page >= d.pages.len() {
            d.page = 0;
        }
        let top = d.shapes.iter().chain(d.pages.iter().flat_map(|p| p.shapes.iter())).map(|s| s.id).max().unwrap_or(0);
        d.next_id = d.next_id.max(top + 1); // ids never reused
        Ok(d)
    }

    /// Vectors from an SVG / DXF / LightBurn file (gcarve-core's importers: inches, Y up → mm), one layer per file
    /// layer; returns how many came in and the importer's note.
    pub fn import(&mut self, ext: &str, text: &str) -> Result<(usize, String), String> {
        self.import_bytes(ext, text.as_bytes())
    }
    /// Vectors from SVG / DXF / LightBurn / PDF / AI / EPS / HPGL / Gerber / Excellon bytes.
    pub fn import_bytes(&mut self, ext: &str, bytes: &[u8]) -> Result<(usize, String), String> {
        let text = String::from_utf8_lossy(bytes);
        let r = match ext.to_ascii_lowercase().as_str() {
            "svg" => gcarve_core::svg::import_svg(&text),
            "dxf" => gcarve_core::dxf::import_dxf(&text),
            "lbrn" | "lbrn2" => gcarve_core::lbrn::import_lbrn(&text),
            "plt" | "hpgl" | "hgl" => gcarve_core::hpgl::import_hpgl(&text),
            "pdf" | "ai" => gcarve_core::pdf::import_pdf(bytes),
            "eps" => gcarve_core::pdf::import_eps(bytes),
            e if GERBER_EXTS.contains(&e) => gcarve_core::gerber::import_gerber(&text),
            e if DRILL_EXTS.contains(&e) => gcarve_core::gerber::import_excellon(&text),
            e if GCODE_EXTS.contains(&e) => import_gcode(&text),
            e => return Err(format!("can't open .{e} files")),
        };
        if r.contours.is_empty() && r.images.is_empty() {
            return Err(if r.note.is_empty() { "no vectors in the file".into() } else { r.note });
        }
        let m = Affine::scale(MM_PER_IN, MM_PER_IN);
        let n = r.contours.len() + r.images.len();
        let before = self.layers.len();
        // groups in the file (LightBurn's) come in as groups: shapes sharing a number
        let mut by_group: std::collections::BTreeMap<u32, Vec<i32>> = Default::default();
        for (i, (c, layer)) in r.contours.into_iter().zip(r.layers).enumerate() {
            let l = self.layer_named(&layer);
            let id = self.add(transformed(&c, &m), l);
            if let Some(g) = r.groups.get(i).filter(|g| **g != 0) {
                by_group.entry(*g).or_default().push(id);
            }
        }
        for ids in by_group.into_values().filter(|v| v.len() > 1) {
            self.group(&ids);
        }
        // embedded pictures (LightBurn): an upright w × h box moved onto its corners (a turned one is resampled)
        for im in r.images {
            let [bl, br, _, tl] = im.corners.map(|p| m.apply(p));
            let (w, h) = (geom::dist(bl, br), geom::dist(bl, tl));
            let Ok(pic) = raster::Raster::decode(&gcarve_core::photo::b64_decode(&im.data), 2000) else { continue };
            if w < 1e-6 || h < 1e-6 {
                continue;
            }
            let l = self.layer_named(&im.layer);
            let id = self.add_raster(pic, geom::make_rect(0.0, 0.0, w, h), l);
            let to = Affine { a: (br.x - bl.x) / w, b: (br.y - bl.y) / w, c: (tl.x - bl.x) / h, d: (tl.y - bl.y) / h, e: bl.x, f: bl.y };
            self.transform(&[id], &to);
        }
        if ext.eq_ignore_ascii_case("lbrn2") || ext.eq_ignore_ascii_case("lbrn") {
            self.lbrn_cut_settings(&text, before);
        }
        // copper is burnt as a fill (an etch resist); drill holes stay outlines
        for l in &mut self.layers[before..] {
            if l.name == "Gerber" {
                let fill = default_layer(0);
                (l.mode, l.speed, l.power) = (Mode::Fill, fill.speed, fill.power);
            }
            // a G-code file is toolpaths: its moves are lines to follow, not areas to fill
            if l.name == "G-code" {
                l.mode = Mode::Line;
            }
        }
        Ok((n, r.note))
    }

    /// A LightBurn project's cut settings onto the layers its import just made (index `from` on): mode, speed,
    /// power, passes, interval, angle, output, shown, air, cross-hatch, LightBurn's colour, and the cut order
    /// (LightBurn's priority).
    fn lbrn_cut_settings(&mut self, text: &str, from: usize) {
        // LightBurn's first colours by cut index
        const COLOURS: [[u8; 3]; 10] = [[0, 0, 0], [0, 0, 255], [255, 0, 0], [0, 224, 0], [208, 208, 0], [255, 128, 0], [0, 224, 224], [255, 0, 255], [180, 180, 180], [0, 0, 160]];
        let mut prio: Vec<(usize, f64)> = Vec::new();
        for (i, _) in text.match_indices("<CutSetting") {
            let body = &text[i..text[i..].find("</CutSetting").map_or(text.len(), |e| i + e)];
            let v = |k: &str| body.find(&format!("<{k} Value=\"")).map(|j| &body[j + k.len() + 9..]).and_then(|s| s.find('"').map(|e| &s[..e]));
            let f = |k: &str| v(k).and_then(|x| x.parse::<f64>().ok());
            let Some(ix) = v("index").and_then(|x| x.parse::<usize>().ok()) else { continue };
            let name = v("name").filter(|n| !n.is_empty()).map_or_else(|| format!("C{ix:02}"), str::to_string);
            let Some(li) = (from..self.layers.len()).find(|&l| self.layers[l].name == name) else { continue };
            let kind = body.find("type=\"").map(|j| &body[j + 6..]).and_then(|s| s.find('"').map(|e| &s[..e])).unwrap_or("Cut");
            let l = &mut self.layers[li];
            l.mode = match kind {
                "Scan" => Mode::Fill,
                "Scan+Cut" => Mode::FillLine,
                "Offset" | "Offset Fill" => Mode::Offset,
                "Image" => Mode::Image,
                _ => Mode::Line,
            };
            if let Some(c) = COLOURS.get(ix) {
                l.color = *c;
            }
            l.speed = f("speed").filter(|x| *x > 0.0).unwrap_or(l.speed);
            l.power = f("maxPower").map_or(l.power, |p| p.clamp(0.0, 100.0));
            l.power_min = f("minPower").map_or(l.power_min, |p| p.clamp(0.0, l.power));
            l.passes = f("numPasses").map_or(l.passes, |p| (p as u32).clamp(1, 50));
            l.interval = f("interval").filter(|x| *x > 0.0).unwrap_or(l.interval);
            l.angle = f("angle").unwrap_or(l.angle);
            l.output = v("doOutput") != Some("0");
            l.visible = v("hide") != Some("1");
            l.air = v("runBlower").map_or(l.air, |b| b != "0");
            l.cross = v("crossHatch") == Some("1");
            l.dither = v("ditherMode").and_then(|m| LB_DITHER.iter().find(|d| d.0 == m)).map_or(l.dither, |d| d.1);
            prio.push((li, f("priority").unwrap_or(ix as f64)));
        }
        // cut order = priority: bubble the new layers into place (move_layer keeps the shapes with their layer)
        let mut order: Vec<f64> = (from..self.layers.len()).map(|l| prio.iter().find(|p| p.0 == l).map_or(f64::MAX, |p| p.1)).collect();
        for _ in 0..order.len() {
            for k in 1..order.len() {
                if order[k] < order[k - 1] {
                    order.swap(k, k - 1);
                    self.move_layer(from + k, true);
                }
            }
        }
    }
}

/// LightBurn's `ditherMode` names (checked against a LightBurn 2.x photo file: "stucki")
const LB_DITHER: [(&str, raster::Dither); 11] = [
    ("dither", raster::Dither::FloydSteinberg),
    ("threshold", raster::Dither::Threshold),
    ("jarvis", raster::Dither::Jarvis),
    ("stucki", raster::Dither::Stucki),
    ("atkinson", raster::Dither::Atkinson),
    ("ordered", raster::Dither::Ordered),
    ("grayscale", raster::Dither::Grayscale),
    ("halftone", raster::Dither::Halftone),
    ("newsprint", raster::Dither::Newsprint),
    ("sketch", raster::Dither::Sketch),
    ("3dslice", raster::Dither::Slices),
];

/// A Contour in the file: one row per node `[x, y, in_x, in_y, out_x, out_y, flags]` (flags 1 = has_in, 2 = has_out),
/// plus `closed`. gcarve-core's Contour has no serde of its own.
mod contour_json {
    use gcarve_core::geom::{pt, Contour, Node};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct C {
        nodes: Vec<[f64; 7]>,
        closed: bool,
    }

    pub fn serialize<S: Serializer>(c: &Contour, s: S) -> Result<S::Ok, S::Error> {
        C { nodes: c.nodes.iter().map(|n| [n.p.x, n.p.y, n.inh.x, n.inh.y, n.out.x, n.out.y, n.has_in as u8 as f64 + 2.0 * n.has_out as u8 as f64]).collect(), closed: c.closed }.serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Contour, D::Error> {
        let c = C::deserialize(d)?;
        Ok(Contour {
            nodes: c.nodes.iter().map(|r| Node { p: pt(r[0], r[1]), inh: pt(r[2], r[3]), out: pt(r[4], r[5]), has_in: r[6] as u8 & 1 != 0, has_out: r[6] as u8 & 2 != 0 }).collect(),
            closed: c.closed,
        })
    }
}

#[derive(Serialize)]
struct Versioned<'a> {
    version: u32,
    /// the account server checks this (server/api.go validBurn)
    app: &'static str,
    doc: &'a Document,
}
#[derive(Deserialize)]
struct VersionedOwned {
    #[allow(dead_code)]
    version: u32,
    doc: Document,
}

/// The first thing a visitor sees: a 90 mm coaster, cut outline, scored ring, engraved leaf pattern.
pub fn sample() -> Document {
    let mut d = Document { name: "Coaster".into(), ..Default::default() };
    d.layers = vec![default_layer(0), default_layer(1), default_layer(2)];
    let (cx, cy, r) = (150.0, 120.0, 45.0);
    d.add(geom::make_ellipse(cx, cy, r, r), 2);
    d.add(geom::make_ellipse(cx, cy, r - 4.0, r - 4.0), 1);
    // eight leaves round the centre, each a closed two-arc loop
    for k in 0..8 {
        let a = k as f64 * std::f64::consts::TAU / 8.0;
        let mut b = geom::Builder::default();
        let tip = pt(cx + 34.0 * a.cos(), cy + 34.0 * a.sin());
        let base = pt(cx + 8.0 * a.cos(), cy + 8.0 * a.sin());
        let side = pt(-a.sin(), a.cos());
        let mid = geom::lerp(base, tip, 0.5);
        b.start(base);
        b.cubic(base + side * 9.0, mid + side * 10.0, tip);
        b.cubic(mid - side * 10.0, base - side * 9.0, base);
        b.close();
        d.add(b.c, 0);
    }
    // a small centre dot and "GBURN" stands in for text until the app adds its own
    d.add(geom::make_ellipse(cx, cy, 3.0, 3.0), 0);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loose_ends_join_and_close_on_import() {
        let mut d = Document::default();
        // a triangle drawn as three separate lines (the last end 0.02 mm off) and one line far away
        let a = d.add(geom::make_polyline(&[pt(0.0, 0.0), pt(10.0, 0.0)], false), 0);
        let b = d.add(geom::make_polyline(&[pt(10.0, 0.0), pt(5.0, 8.0)], false), 0);
        let c = d.add(geom::make_polyline(&[pt(5.0, 8.0), pt(0.02, 0.0)], false), 0);
        let e = d.add(geom::make_polyline(&[pt(50.0, 50.0), pt(60.0, 50.0)], false), 0);
        let ids = d.join_loose_ends(&[a, b, c, e], 0.05);
        assert_eq!(ids.len(), 2);
        let closed: Vec<&Shape> = ids.iter().filter_map(|i| d.find(*i)).filter(|s| s.contour.closed).collect();
        assert_eq!(closed.len(), 1, "the triangle closed");
        assert!((Document::measure(&closed[0].contour).1.abs() - 40.0).abs() < 0.2);
        assert_eq!(d.shapes.len(), 2);
    }

    #[test]
    fn pages_keep_their_own_shapes() {
        let mut d = Document::default();
        let a = d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        let p2 = d.add_page(true);
        assert_eq!((p2, d.page_count(), d.shapes.len()), (1, 2, 1));
        assert_ne!(d.shapes[0].id, a, "a copy has its own id");
        d.add(geom::make_rect(20.0, 0.0, 30.0, 10.0), 0);
        d.switch_page(0);
        assert_eq!(d.shapes.len(), 1);
        assert!(d.find(a).is_some());
        // saved and loaded with page 1 open: page 2 still has its two shapes
        let back = Document::from_json(&d.to_json()).unwrap();
        assert_eq!((back.page, back.pages[1].shapes.len()), (0, 2));
        // layers renumber on every page: page 2's shapes sit on layer 1, empty on page 1
        d.layers.push(default_layer(1));
        d.pages[1].shapes.iter_mut().for_each(|s| s.layer = 1);
        assert!(!d.remove_layer(1), "used on another page");
        assert!(d.move_layer(1, true));
        assert!(d.pages[1].shapes.iter().all(|s| s.layer == 0) && d.shapes.iter().all(|s| s.layer == 1));
        // a bad layer on another page is clamped on load
        d.pages[1].shapes[0].layer = 99;
        let back = Document::from_json(&d.to_json()).unwrap();
        assert!(back.pages[1].shapes.iter().all(|s| s.layer < back.layers.len()));
        d.move_layer(0, false);
        d.remove_page(1);
        assert_eq!((d.page_count(), d.shapes.len()), (1, 1));
        assert!(d.pages.is_empty());
    }

    #[test]
    fn warp_and_deform_reshape_the_box() {
        let mut d = Document::default();
        let id = d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        // at rest both leave the square alone
        d.warp_quad(&[id], [pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0), pt(0.0, 10.0)]);
        let grid: [Pt; 16] = std::array::from_fn(|k| pt((k % 4) as f64 * 10.0 / 3.0, (k / 4) as f64 * 10.0 / 3.0));
        d.deform(&[id], &grid);
        assert!((Document::measure(&d.find(id).unwrap().contour).1.abs() - 100.0).abs() < 1e-6);
        // the top pulled in to 4 mm wide: a trapezoid of area (10 + 4) / 2 × 10
        d.warp_quad(&[id], [pt(0.0, 0.0), pt(10.0, 0.0), pt(7.0, 10.0), pt(3.0, 10.0)]);
        assert!((Document::measure(&d.find(id).unwrap().contour).1.abs() - 70.0).abs() < 0.01);
        // a deform handle in the top row moved up bulges the top edge
        let mut g = grid;
        g[13].y += 6.0;
        g[14].y += 6.0;
        let id2 = d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        d.deform(&[id2], &g);
        let b = bounds(&d.find(id2).unwrap().contour);
        assert!((b.y1 - 14.5).abs() < 0.05, "{}", b.y1); // the middle of the top edge rises 6 × ¾
    }

    #[test]
    fn shapes_inside_a_closed_shape_are_found() {
        let mut d = Document::default();
        let outer = d.add(geom::make_rect(0.0, 0.0, 50.0, 50.0), 0);
        let inner = d.add(geom::make_rect(10.0, 10.0, 15.0, 15.0), 0);
        let line = d.add(geom::make_polyline(&[pt(20.0, 20.0), pt(30.0, 30.0)], false), 0);
        d.add(geom::make_rect(40.0, 40.0, 60.0, 60.0), 0); // half out
        d.add(geom::make_rect(100.0, 0.0, 105.0, 5.0), 0);
        assert_eq!(d.inside_of(&[outer]), vec![inner, line]);
        assert!(d.inside_of(&[line]).is_empty(), "an open path holds nothing");
    }

    #[test]
    fn slots_and_tabs_resize_for_new_stock() {
        // a 40 × 20 plate with a 3 mm wide, 10 mm deep slot from the top edge and a 3 mm tall, 8 mm wide tab on the bottom
        let plate = geom::make_polyline(
            &[pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, -3.0), pt(18.0, -3.0), pt(18.0, 0.0), pt(40.0, 0.0), pt(40.0, 20.0), pt(21.5, 20.0), pt(21.5, 10.0), pt(18.5, 10.0), pt(18.5, 20.0), pt(0.0, 20.0)],
            true,
        );
        let (w, n) = resize_slots(&plate, 3.0, 4.0, 0.05, SlotFix { width: true, ..Default::default() });
        assert_eq!(n, 1);
        assert_eq!((w.nodes[8].p, w.nodes[9].p), (pt(22.0, 10.0), pt(18.0, 10.0)), "the slot 4 wide about its middle");
        let (t, n) = resize_slots(&plate, 3.0, 4.0, 0.05, SlotFix { tab_height: true, ..Default::default() });
        assert_eq!(n, 1);
        assert_eq!((t.nodes[2].p, t.nodes[3].p), (pt(10.0, -4.0), pt(18.0, -4.0)), "the tab 4 tall");
        let (_, n) = resize_slots(&plate, 3.0, 4.0, 0.05, SlotFix { depth: true, ..Default::default() });
        assert_eq!(n, 0, "the slot is 10 deep, not 3");
    }

    #[test]
    fn line_ends_extend_to_the_nearest_shape() {
        let line = geom::make_polyline(&[pt(10.0, 5.0), pt(20.0, 5.0)], false);
        let walls = vec![(vec![pt(30.0, 0.0), pt(30.0, 10.0)], false), (vec![pt(50.0, 0.0), pt(50.0, 10.0)], false), (vec![pt(0.0, 0.0), pt(4.0, 0.0), pt(4.0, 10.0), pt(0.0, 10.0)], true)];
        let e = extend_ends(&line, &walls).unwrap();
        assert_eq!((e.nodes[0].p, e.nodes[1].p), (pt(4.0, 5.0), pt(30.0, 5.0)));
        assert!(extend_ends(&line, &[(vec![pt(0.0, 20.0), pt(40.0, 20.0)], false)]).is_none(), "nothing ahead");
    }

    #[test]
    fn smoothing_flattens_a_zigzag_and_keeps_open_ends() {
        // a line along x with ±0.2 mm teeth every 0.5 mm: smoothed at 2 mm the teeth fade, the ends stay put
        let pts: Vec<Pt> = (0..=40).map(|i| pt(i as f64 * 0.5, if i % 2 == 0 { 0.2 } else { -0.2 })).collect();
        let c = geom::make_polyline(&pts, false);
        let sm = smooth_contour(&c, 2.0);
        let ys: Vec<f64> = sm.nodes.iter().filter(|n| n.p.x > 3.0 && n.p.x < 17.0).map(|n| n.p.y).collect();
        assert!(ys.iter().all(|y| y.abs() < 0.08), "{ys:?}");
        assert_eq!((sm.nodes[0].p, sm.nodes.last().unwrap().p), (pts[0], pts[40]));
        // a closed square stays closed and about its size
        let sq = smooth_contour(&geom::make_rect(0.0, 0.0, 20.0, 20.0), 1.0);
        let b = bounds(&sq);
        assert!(sq.closed && b.w() > 18.5 && b.w() < 20.01, "{}", b.w());
    }

    #[test]
    fn one_corner_rounds_to_an_arc() {
        let sq = geom::make_rect(0.0, 0.0, 20.0, 20.0);
        let i = sq.nodes.iter().position(|n| n.p == pt(20.0, 20.0)).unwrap();
        let c = round_corner(&sq, i, 5.0).unwrap();
        assert_eq!(c.nodes.len(), 5);
        // the area lost = r² (1 − π/4)
        let a = Document::measure(&c).1.abs();
        assert!((a - (400.0 - 25.0 * (1.0 - std::f64::consts::PI / 4.0))).abs() < 0.2, "{a}");
        // the arc's middle is r from the arc centre (15, 15)
        let mid = flatten(&c, 0.01).into_iter().map(|q| (geom::dist(q, pt(20.0, 20.0)), q)).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap().1;
        assert!((geom::dist(mid, pt(15.0, 15.0)) - 5.0).abs() < 0.01);
        // too big a radius shrinks to fit the sides; a curve or a straight point refuses
        assert!(round_corner(&sq, i, 50.0).is_some());
        assert!(round_corner(&c, i, 2.0).is_none());
        let line = geom::make_polyline(&[pt(0.0, 0.0), pt(5.0, 0.0), pt(10.0, 0.0)], false);
        assert!(round_corner(&line, 1, 2.0).is_none() && round_corner(&line, 0, 2.0).is_none());
        // and back: either end of the arc gives the square again
        for k in [i, i + 1] {
            let back = sharp_corner(&c, k).unwrap();
            assert_eq!(back.nodes.len(), 4);
            assert!(back.nodes.iter().any(|q| geom::dist(q.p, pt(20.0, 20.0)) < 1e-9) && back.nodes.iter().all(|q| !q.has_in && !q.has_out));
        }
        assert!(sharp_corner(&sq, i).is_none() && sharp_corner(&line, 1).is_none());
    }

    #[test]
    fn drawn_rectangles_and_polygons_stay_editable_until_reshaped() {
        let mut d = Document::default();
        let b = Rect { x0: 10.0, y0: 10.0, x1: 50.0, y1: 30.0 };
        let r = Prim::Rect { radius: 4.0 };
        let id = d.add(r.contour(&b), 0);
        d.find_mut(id).unwrap().prim = Some(r.clone());
        assert!(r.fits(&d.find(id).unwrap().contour));
        assert_eq!(d.find(id).unwrap().contour.nodes.len(), 8, "four lines and four quarter circles");
        let area = Document::measure(&d.find(id).unwrap().contour).1.abs();
        assert!((area - (800.0 - (4.0 - std::f64::consts::PI) * 16.0)).abs() < 0.5, "{area}");
        // moved and stretched: still a rounded rectangle
        d.transform(&[id], &Affine::translate(5.0, 0.0).then(&Affine::scale(2.0, 1.0)));
        let s = d.find(id).unwrap();
        assert!(s.prim.as_ref().unwrap().fits(&s.contour));
        // turned 30°: not any more
        d.transform(&[id], &Affine::rotate(0.5));
        let s = d.find(id).unwrap();
        assert!(!s.prim.as_ref().unwrap().fits(&s.contour));
        // a hexagon moved, grown and turned keeps its centre / size / turn
        // a dual star: two 5-point stars welded = 10 outer points, more area than either
        let ds = Prim::Polygon { c: [0.0, 0.0], r: 10.0, rot: 0.0, sides: 5, inner: 40.0, dual: 80.0 };
        let one = Prim::Polygon { c: [0.0, 0.0], r: 10.0, rot: 0.0, sides: 5, inner: 40.0, dual: 0.0 };
        let (dc, oc) = (ds.contour(&Rect::default()), one.contour(&Rect::default()));
        assert!(Document::measure(&dc).1.abs() > Document::measure(&oc).1.abs() * 1.1 && ds.fits(&dc) && dc.closed && dc.nodes.len() == 20);
        let p = Prim::Polygon { c: [0.0, 0.0], r: 10.0, rot: 0.0, sides: 6, inner: 0.0, dual: 0.0 };
        let id = d.add(p.contour(&Rect::default()), 0);
        d.find_mut(id).unwrap().prim = Some(p);
        d.transform(&[id], &Affine::translate(20.0, 5.0).then(&Affine::rotate(0.3)).then(&Affine::scale(1.5, 1.5)));
        let s = d.find(id).unwrap();
        let Some(Prim::Polygon { c, r, .. }) = s.prim.clone() else { panic!() };
        assert!(s.prim.as_ref().unwrap().fits(&s.contour), "the hexagon still fits");
        assert!((r - 15.0).abs() < 1e-9 && (c[0] - 20.0).abs() < 1e-9 && (c[1] - 5.0).abs() < 1e-9);
        // its node dragged: stale
        let mut c2 = s.contour.clone();
        c2.nodes[0].p.x += 1.0;
        assert!(!s.prim.as_ref().unwrap().fits(&c2));
    }

    #[test]
    fn sub_layers_run_right_after_their_layer_with_their_own_settings() {
        let mut d = Document { layers: vec![Layer::new("A", [0, 0, 0], Mode::Fill, 100.0, 30.0), Layer::new("B", [255, 0, 0], Mode::Line, 10.0, 90.0)], ..Default::default() };
        d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        d.add(geom::make_rect(20.0, 0.0, 30.0, 10.0), 1);
        assert_eq!(d.for_job(), d, "no subs: the same document");
        d.layers[0].subs.push(Layer { name: String::new(), ..Layer::new("A", [0, 0, 0], Mode::Line, 20.0, 60.0) });
        let j = d.for_job();
        assert_eq!(j.layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["A", "A · 2", "B"]);
        assert_eq!((j.layers[1].mode, j.layers[1].power), (Mode::Line, 60.0));
        assert_eq!(j.shapes.len(), 3);
        let mut ids: Vec<i32> = j.shapes.iter().map(|s| s.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), 3, "copies get their own ids");
        let p = plan::plan(&j, plan::Machine::default());
        assert!(p.strokes.iter().any(|s| s.layer == 1), "the sub-layer burns");
        let last_a = p.strokes.iter().rposition(|s| s.layer <= 1).unwrap();
        let first_b = p.strokes.iter().position(|s| s.layer == 2).unwrap();
        assert!(last_a < first_b, "sub-layer before the next layer");
        // the file keeps the sub-layer
        let back = Document::from_json(&d.to_json()).unwrap();
        assert_eq!(back.layers[0].subs.len(), 1);
    }

    #[test]
    fn a_power_scale_burns_its_shape_on_a_scaled_copy_of_the_layer() {
        let mut d = Document { layers: vec![Layer::new("A", [0, 0, 0], Mode::Line, 20.0, 80.0)], ..Default::default() };
        d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        d.add(geom::make_rect(20.0, 0.0, 30.0, 10.0), 0);
        d.add(geom::make_rect(40.0, 0.0, 50.0, 10.0), 0);
        d.layers[0].subs.push(Layer::new("A", [0, 0, 0], Mode::Fill, 100.0, 40.0));
        d.shapes[1].power = Some(50.0);
        d.shapes[2].power = Some(100.0); // 100 % = as the layer
        let j = d.for_job();
        assert_eq!(j.layers.iter().map(|l| (l.name.as_str(), l.power)).collect::<Vec<_>>(), [("A", 80.0), ("A · 50%", 40.0), ("A · 2", 40.0), ("A · 2 · 50%", 20.0)]);
        let on = |li: usize| j.shapes.iter().filter(|s| s.layer == li).count();
        assert_eq!([on(0), on(1), on(2), on(3)], [2, 1, 2, 1]);
        assert!(j.shapes.iter().all(|s| s.power.is_none()));
        let back = Document::from_json(&d.to_json()).unwrap();
        assert_eq!(back.shapes[1].power, Some(50.0));
    }

    #[test]
    fn a_virtual_array_follows_its_original_and_burns_as_copies() {
        let mut d = Document::default();
        let id = d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        d.arrays.push(VirtualArray { ids: vec![id], cols: 3, rows: 2, dx: 15.0, dy: 12.0, turns: 0, centre: [0.0, 0.0] });
        let job = d.for_job();
        assert_eq!(job.shapes.len(), 6);
        let b = job.bounds();
        assert!((b.w() - 40.0).abs() < 1e-9 && (b.h() - 22.0).abs() < 1e-9, "{b:?}");
        // move the original: the copies come along
        d.transform(&[id], &Affine::translate(100.0, 0.0));
        assert!((d.for_job().bounds().x0 - 100.0).abs() < 1e-9);
        // saved and read back
        assert_eq!(Document::from_json(&d.to_json()).unwrap().arrays, d.arrays);
        // a circle of 4 round (0, 0): the copies sit a quarter turn apart
        d.arrays = vec![VirtualArray { ids: vec![id], cols: 1, rows: 1, dx: 0.0, dy: 0.0, turns: 4, centre: [0.0, 0.0] }];
        let b = d.for_job().bounds();
        assert!((b.x0 + 110.0).abs() < 1e-9 && (b.x1 - 110.0).abs() < 1e-9, "{b:?}");
        // the original gone: nothing left to copy
        d.remove(&[id]);
        assert!(d.for_job().shapes.is_empty());
    }

    #[test]
    fn a_pdf_reads_back_at_true_size() {
        let d = sample();
        let black = String::from_utf8_lossy(&d.export_pdf_in(false)).into_owned();
        assert!(black.contains("0.000 0.000 0.000 RG") && !black.contains("1.000 0.000 0.000 RG"), "all black");
        let pdf = d.export_pdf();
        let mut e = Document { layers: Vec::new(), shapes: Vec::new(), ..Default::default() };
        let (n, note) = e.import_bytes("pdf", &pdf).unwrap();
        assert_eq!(n, d.shapes.iter().filter(|s| s.raster.is_none()).count(), "{note}");
        let (a, b) = (d.bounds(), e.bounds());
        assert!((a.w() - b.w()).abs() < 0.05 && (a.h() - b.h()).abs() < 0.05, "{a:?} {b:?}");
    }

    #[test]
    fn lightburn_groups_come_in_as_groups() {
        let lb = r#"<LightBurnProject FormatVersion="1"><Shape Type="Group" CutIndex="0"><XForm>1 0 0 1 0 0</XForm><Children>
<Shape Type="Rect" CutIndex="0" W="10" H="10" Cr="0"><XForm>1 0 0 1 5 5</XForm></Shape>
<Shape Type="Rect" CutIndex="0" W="10" H="10" Cr="0"><XForm>1 0 0 1 30 5</XForm></Shape></Children></Shape>
<Shape Type="Rect" CutIndex="0" W="10" H="10" Cr="0"><XForm>1 0 0 1 60 5</XForm></Shape></LightBurnProject>"#;
        let mut d = Document::default();
        d.import("lbrn2", lb).unwrap();
        let g: Vec<i32> = d.shapes.iter().map(|s| s.group).collect();
        assert!(g[0] != 0 && g[0] == g[1] && g[2] == 0, "{g:?}");
    }

    #[test]
    fn a_lightburn_project_reads_back_with_its_layers_and_curves() {
        let d = sample();
        let text = d.export_lbrn2();
        let mut e = Document { layers: Vec::new(), shapes: Vec::new(), ..Default::default() };
        let (n, _) = e.import("lbrn2", &text).unwrap();
        assert_eq!(n, d.shapes.len());
        let names: Vec<&str> = e.layers.iter().map(|l| l.name.as_str()).collect();
        for l in d.layers.iter().filter(|l| names.contains(&l.name.as_str())) {
            assert!(text.contains(&format!("<name Value=\"{}\"/>", l.name)));
        }
        let (a, b) = (d.bounds(), e.bounds());
        // the importer moves a design to 0,0: compare the size
        assert!((a.w() - b.w()).abs() < 0.01 && (a.h() - b.h()).abs() < 0.01, "{a:?} {b:?}");
        // curves stay curves
        assert!(text.contains('B') && e.shapes.iter().any(|s| s.contour.nodes.iter().any(|n| n.has_out)));
        // a picture goes out as a Bitmap and comes back the same size, pixels and place (relative to a box)
        let mut d = Document::default();
        let bx = d.add(geom::make_rect(0.0, 0.0, 50.0, 30.0), 0);
        let pic = d.add(geom::make_rect(10.0, 5.0, 30.0, 15.0), 1);
        d.find_mut(pic).unwrap().raster = Some(raster::Raster::from_gray(4, 2, &[0, 64, 128, 255, 255, 128, 64, 0]));
        let mut e = Document { layers: Vec::new(), shapes: Vec::new(), ..Default::default() };
        assert_eq!(e.import("lbrn2", &d.export_lbrn2()).unwrap().0, 2);
        let back = e.shapes.iter().find(|s| s.raster.is_some()).expect("the picture came back");
        let (pb, eb) = (bounds(&back.contour), e.bounds());
        assert!((pb.x0 - eb.x0 - 10.0).abs() < 1e-3 && (pb.y0 - eb.y0 - 5.0).abs() < 1e-3 && (pb.w() - 20.0).abs() < 1e-3 && (pb.h() - 10.0).abs() < 1e-3, "{pb:?}");
        assert_eq!(back.raster.as_ref().unwrap().pixels(), d.find(pic).unwrap().raster.as_ref().unwrap().pixels());
        let _ = bx;
        // the cut settings come back too, in LightBurn's priority order
        let mut d = sample();
        d.layers[0].mode = Mode::Offset;
        d.layers[1].output = false;
        d.layers[1].visible = false;
        d.layers[2].passes = 3;
        d.layers[2].power_min = 12.0;
        d.layers[2].air = true;
        d.layers[0].cross = true;
        d.layers[2].dither = raster::Dither::Stucki;
        let mut e = Document { layers: Vec::new(), shapes: Vec::new(), ..Default::default() };
        e.import("lbrn2", &d.export_lbrn2()).unwrap();
        for (a, b) in d.layers.iter().zip(&e.layers) {
            assert_eq!((&a.name, a.mode, a.speed, a.power, a.passes, a.output, a.visible, a.air, a.cross, a.dither), (&b.name, b.mode, b.speed, b.power, b.passes, b.output, b.visible, b.air, b.cross, b.dither));
            assert!((a.interval - b.interval).abs() < 1e-6 && (a.power_min - b.power_min).abs() < 1e-6);
        }
        // a project whose priorities run the other way round comes in in that order, shapes with their layers
        let t = d.export_lbrn2().replace("<priority Value=\"0\"/>", "<priority Value=\"9\"/>");
        let mut e = Document { layers: Vec::new(), shapes: Vec::new(), ..Default::default() };
        e.import("lbrn2", &t).unwrap();
        assert_eq!(e.layers.last().unwrap().name, d.layers[0].name);
        let leaves = d.shapes.iter().filter(|s| s.layer == 0).count();
        assert_eq!(e.shapes.iter().filter(|s| s.layer == e.layers.len() - 1).count(), leaves);
    }

    #[test]
    fn the_sample_saves_and_loads_back_the_same() {
        let d = sample();
        let back = Document::from_json(&d.to_json()).unwrap();
        assert_eq!(d, back);
        assert_eq!(d.shapes.len(), 11);
        assert!(d.to_json().starts_with("{\"version\":1,\"app\":\"GBurn\","));
        assert!(Document::from_json("{\"version\":1").is_err());
        assert!(Document::from_json("[]").is_err());
    }

    #[test]
    fn a_gcode_file_opens_as_its_burn_moves() {
        let g = "G21 G90\nG0 X10 Y10\nM4 S500\nG1 X70 Y10 F1000\nG1 X70 Y70\nG1 X10 Y70\nG1 X10 Y10\nM5\nG0 X0 Y0\n";
        let mut d = Document::default();
        let (n, _) = d.import_bytes("gcode", g.as_bytes()).unwrap();
        assert_eq!(n, 1);
        let c = &d.shapes.last().unwrap().contour;
        assert!(c.closed);
        let b = bounds(c);
        assert!((b.w() - 60.0).abs() < 1e-6 && (b.h() - 60.0).abs() < 1e-6, "{b:?}");
        assert_eq!(d.layers[d.shapes.last().unwrap().layer].mode, Mode::Line);
        assert!(d.import_bytes("nc", b"G0 X5\n").is_err());
    }

    #[test]
    fn pcb_copper_comes_in_as_a_fill_and_drills_as_circles() {
        let mut d = Document { layers: Vec::new(), shapes: Vec::new(), ..Default::default() };
        let g = "%FSLAX24Y24*%\n%MOMM*%\n%ADD10R,2.0X1.0*%\nD10*\nX100000Y50000D03*\nM02*\n";
        let (n, _) = d.import("GTL", g).unwrap();
        assert_eq!(n, 1);
        let l = d.layers.iter().position(|l| l.name == "Gerber").unwrap();
        assert_eq!(d.layers[l].mode, Mode::Fill);
        let b = d.bounds_of(&[d.shapes[0].id]);
        assert!((b.w() - 2.0).abs() < 1e-3 && (b.h() - 1.0).abs() < 1e-3 && (b.x0 - 9.0).abs() < 1e-3, "{b:?}");
        let (n, _) = d.import("drl", "M48\nMETRIC\nT1C1.0\n%\nT1\nX5.0Y5.0\nM30\n").unwrap();
        assert_eq!(n, 1);
        assert_eq!(d.layers.iter().find(|l| l.name == "Drill").unwrap().mode, Mode::Line);
    }

    #[test]
    fn a_plot_file_comes_in_as_millimetres_with_a_layer_per_pen() {
        let mut d = Document { layers: Vec::new(), shapes: Vec::new(), ..Default::default() };
        let (n, _) = d.import("plt", "IN;SP1;PU0,0;PD400,0,400,200,0,200,0,0;SP3;PU0,400;PD400,400;").unwrap();
        assert_eq!(n, 2);
        assert_eq!(d.layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Pen 1", "Pen 3"]);
        let b = d.bounds();
        assert!((b.w() - 10.0).abs() < 1e-9 && (b.h() - 10.0).abs() < 1e-9, "{b:?}");
    }

    #[test]
    fn an_svg_comes_in_as_millimetres_on_its_layers() {
        let mut d = Document::default();
        let (n, _) = d
            .import(
                "svg",
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 100 50"><rect x="10" y="10" width="30" height="20"/><circle cx="70" cy="25" r="10"/></svg>"#,
            )
            .unwrap();
        assert_eq!(n, 2);
        let b = d.bounds();
        assert!((b.w() - 70.0).abs() < 0.05 && (b.h() - 25.0).abs() < 0.05, "{b:?}");
        assert!(d.import("txt", "x").is_err());
        assert!(d.import("svg", "<svg></svg>").is_err());
    }

    #[test]
    fn groups_move_together_layers_reorder_and_pictures_trace() {
        let mut d = Document::default();
        let a = d.add(geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        let b = d.add(geom::make_rect(20.0, 0.0, 30.0, 10.0), 0);
        let c = d.add(geom::make_rect(40.0, 0.0, 50.0, 10.0), 0);
        d.group(&[a, b]);
        assert_eq!(d.with_groups(&[a]), vec![a, b]);
        assert_eq!(d.with_groups(&[c]), vec![c]);
        d.ungroup(&[b]);
        assert_eq!(d.with_groups(&[a]), vec![a]);
        d.layers.push(default_layer(1));
        d.layers.push(default_layer(2));
        d.find_mut(c).unwrap().layer = 2;
        assert!(d.move_layer(2, true));
        assert_eq!(d.find(c).unwrap().layer, 1);
        assert_eq!(d.layers[1].name, "Cut");
        assert!(!d.move_layer(0, true));
        assert!(!d.remove_layer(1), "not while it has shapes");
        assert!(d.remove_layer(2));
        assert_eq!(d.layers.len(), 2);
        // a 20 × 20 picture with a black square in the middle traces to one outline the size of the square
        let mut px = vec![255u8; 400];
        for y in 5..15 {
            for x in 5..15 {
                px[y * 20 + x] = 0;
            }
        }
        let mut png = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png, 20, 20);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&px).unwrap();
        }
        let id = d.add_image(&png, pt(100.0, 100.0), 40.0, 0).unwrap();
        let ids = d.trace_picture(id, 0, None, false, 0.5, 4.0);
        assert_eq!(ids.len(), 1, "{ids:?}");
        let tb = bounds(&d.find(ids[0]).unwrap().contour);
        assert!((tb.w() - 20.0).abs() < 2.5 && (tb.x0 - 110.0).abs() < 2.5, "{tb:?}");
        let back = Document::from_json(&d.to_json()).unwrap();
        assert_eq!(back, d);
    }

    /// damaged files never panic
    #[test]
    fn damaged_files_are_refused_not_crashed() {
        let good = sample().to_json();
        let n: usize = std::env::var("FUZZ_N").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
        let mut seed = 12345u64;
        for _ in 0..n {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut b = good.clone().into_bytes();
            let at = (seed as usize) % b.len();
            match seed % 3 {
                0 => b.truncate(at),
                1 => b[at] = (seed >> 8) as u8,
                _ => b.insert(at, b'9'),
            }
            let _ = Document::from_json(&String::from_utf8_lossy(&b));
        }
    }
}
