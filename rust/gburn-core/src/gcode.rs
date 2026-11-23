//! G-code for GRBL-family lasers (GRBL 1.1, grblHAL, FluidNC, Smoothie-ish): mm, absolute, M4 dynamic power so the
//! beam follows the real speed through corners (laser mode $32=1). `S` runs 0..`s_max` ($30, usually 1000).

use crate::plan::Plan;
use crate::{Document, Pt};

#[derive(Clone, Debug, PartialEq)]
pub struct GcodeOptions {
    /// the stock's thickness, mm: machines that focus by Z from the job (xTool M1: Z = 17 − thickness, Z down)
    pub thickness: f64,
    /// $30: the S value for 100 % power
    pub s_max: f64,
    /// M4 (power follows speed) or M3 (constant)
    pub dynamic: bool,
    /// G0 X0 Y0 at the end
    pub home_after: bool,
    /// where `home_after` goes when the job ran in bed coordinates (mm)
    pub finish: Pt,
    /// "Start from current position": the head stands at this design point when the job starts (G92 makes it so);
    /// None = absolute coordinates
    pub origin: Option<Pt>,
    /// rotary: design Y × this = the Y the controller gets (mm per rotation ÷ the object's circumference); 1 = off
    pub y_scale: f64,
    /// user origin: every coordinate moved by this (the job's origin point lands on the saved machine position)
    pub shift: Pt,
    /// scanning offset: (speed mm/s, mm) pairs = how far a return scan line lands from an outgoing one at that
    /// speed (belt stretch, laser lag); each scan line is moved back half of it along its direction. Empty = off.
    pub scan_offsets: Vec<(f64, f64)>,
    /// the machine's own lines before the job (after G21 G90 G94) and after it (Machine Settings)
    pub start: String,
    pub end: String,
    /// S on every G1 (controllers that forget it between moves)
    pub s_every: bool,
    /// air assist on M7 (mist) instead of M8 (flood), as some machines are wired
    pub air_m7: bool,
    /// the machine's 0,0 on another corner: X becomes width − X, Y height − Y (Some(bed width / height))
    pub mirror: [Option<f64>; 2],
    /// the letter design Y goes out as: 'Y', or 'A' for a rotary chuck on a fourth axis
    pub y_axis: char,
    /// the machine's X and Y wired the other way round: X goes out as Y and Y as X (not with a rotary on A)
    pub swap_xy: bool,
    /// blank moves inside fills and pictures (overscan, white space) as G0: faster, the beam off for sure
    pub blank_g0: bool,
    /// M73 P<percent> R<minutes left> as the job goes (Marlin / Prusa-style progress on the machine's screen)
    pub progress: bool,
    /// which firmware reads the file: the on / off commands, the dwell unit and whether S must be on every G1
    pub flavor: Flavor,
}

/// The table's offset at `speed`: linear between the rows, the end rows beyond them.
pub fn scan_offset_at(table: &[(f64, f64)], speed: f64) -> f64 {
    let mut t: Vec<(f64, f64)> = table.to_vec();
    t.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    match t.len() {
        0 => 0.0,
        _ if speed <= t[0].0 => t[0].1,
        _ if speed >= t[t.len() - 1].0 => t[t.len() - 1].1,
        _ => {
            let i = t.windows(2).position(|w| speed <= w[1].0).unwrap_or(0);
            let (a, b) = (t[i], t[i + 1]);
            a.1 + (b.1 - a.1) * (speed - a.0) / (b.0 - a.0).max(1e-9)
        }
    }
}

impl Default for GcodeOptions {
    fn default() -> Self {
        GcodeOptions {
            thickness: 0.0,
            s_max: 1000.0,
            dynamic: true,
            home_after: true,
            finish: Pt { x: 0.0, y: 0.0 },
            origin: None,
            y_scale: 1.0,
            shift: Pt { x: 0.0, y: 0.0 },
            scan_offsets: Vec::new(),
            start: String::new(),
            end: String::new(),
            s_every: false,
            y_axis: 'Y',
            air_m7: false,
            mirror: [None, None],
            swap_xy: false,
            blank_g0: false,
            progress: false,
            flavor: Flavor::Grbl,
        }
    }
}

/// The G-code firmwares GBurn writes for (samples and docs compared 2026-11-03).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Flavor {
    /// GRBL, grblHAL, FluidNC: M3 / M4 + S, `G4 P` in seconds, bare `S0` between M3 strokes
    #[default]
    Grbl,
    /// Smoothieware: G1 fires, G0 doesn't; S 0–1; `G4 P` in ms
    Smoothie,
    /// Marlin with the laser feature: `M3 I` inline (every G0 drops the power, so S on every G1); M4 hangs stock
    /// Creality builds; `G4 P` in ms
    Marlin,
    /// RepRapFirmware (Duet, `M452` in the start G-code): S on every G1 (non-sticky), `G4 P` in ms
    Rrf,
    /// Snapmaker: `M3` before each burning stroke, `M5` after it, S on the G1s (0–255)
    Snapmaker,
    /// Marlin with the laser on the part-cooling fan pin: power is `M106 S0–255` before each burning move
    MarlinFan,
    /// xTool S1 over USB: LightBurn 2.2's xTool job (header `M110 / M109 / M7 S1 / M96 / G92 U0`, `M4` + `M15 S1`
    /// round the burning, `M6` + `M7 S0 N0 D10` at the end), S 0–1000; travel as `G1 … S0` (UNVERIFIED on a
    /// machine: from LightBurn console pastes, docs/research/xtool-s1.md)
    XTool,
    /// xTool F1 (and F1 Lite / Ultra) on firmware before 40.51 / 40.52 / 40.53 / 40.55: GRBL-like lines on TCP 8780,
    /// BeamInk's session (`$L`, `M114 S1` blue / `S2` infrared, … `M6 P1`, `$P`), S 0–1000 on every G1, M4 so the beam
    /// stops with the mirrors (docs/research/xtool-network.md §3; BeamInk ran it on a real F1, GBurn hasn't)
    XToolF1 { ir: bool },
    /// xTool M1 on old firmware (an uploaded `gcodes.txt`, started by its button): only G0 / G1 / G4 / G90 / G91 in
    /// the body (M3 / M4 / M5 crash it, no G92), whole S 0–1000, never F0, Z points down with focus at Z 17 for
    /// 0 mm stock; header / footer as xtm1_toolkit's (MIT, ran on the author's M1; docs/research/xtool-network.md §6)
    XToolM1,
}

/// The xTool M1's upload: a stored (uncompressed) zip holding one file, `gcodes.txt` (xtm1_toolkit's package)
pub fn m1_zip(gcode: &str) -> Vec<u8> {
    let (data, name) = (gcode.as_bytes(), b"gcodes.txt");
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    let (crc, len) = (!crc, data.len() as u32);
    let mut z = Vec::with_capacity(data.len() + 128);
    let w16 = |z: &mut Vec<u8>, v: u16| z.extend_from_slice(&v.to_le_bytes());
    let w32 = |z: &mut Vec<u8>, v: u32| z.extend_from_slice(&v.to_le_bytes());
    // local header: version 1.0, no flags, stored, 1980-01-01 00:00
    w32(&mut z, 0x0403_4b50);
    for v in [10, 0, 0, 0, 0x21] {
        w16(&mut z, v);
    }
    for v in [crc, len, len] {
        w32(&mut z, v);
    }
    w16(&mut z, name.len() as u16);
    w16(&mut z, 0);
    z.extend_from_slice(name);
    z.extend_from_slice(data);
    // the central directory's one entry, then its end record
    let cd = z.len() as u32;
    w32(&mut z, 0x0201_4b50);
    for v in [20, 10, 0, 0, 0, 0x21] {
        w16(&mut z, v);
    }
    for v in [crc, len, len] {
        w32(&mut z, v);
    }
    for v in [name.len() as u16, 0, 0, 0, 0] {
        w16(&mut z, v);
    }
    w32(&mut z, 0);
    w32(&mut z, 0);
    z.extend_from_slice(name);
    let cd_len = z.len() as u32 - cd;
    w32(&mut z, 0x0605_4b50);
    for v in [0, 0, 1, 1] {
        w16(&mut z, v);
    }
    w32(&mut z, cd_len);
    w32(&mut z, cd);
    w16(&mut z, 0);
    z
}

fn n(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// The whole job. Every stroke: travel there with the laser off, then feed through its points at the layer's speed
/// and power. Feed is mm/min.
pub fn gcode(doc: &Document, plan: &Plan, o: GcodeOptions) -> String {
    let mut g = String::with_capacity(plan.strokes.len() * 64);
    g += &format!("; GBurn {} — {}\n; {} strokes, {:.0} mm cut, {:.0} mm travel, ~{:.0} s\n", env!("CARGO_PKG_VERSION"), doc.name, plan.strokes.len(), plan.cut_mm, plan.travel_mm, plan.seconds);
    let fl = o.flavor;
    let grbl = fl == Flavor::Grbl;
    let xtool = fl == Flavor::XTool;
    let f1 = matches!(fl, Flavor::XToolF1 { .. });
    let m1 = fl == Flavor::XToolM1;
    g += &match fl {
        Flavor::Grbl => "G21\nG90\nG94\n".to_string(),
        Flavor::XTool => "G21\nG90\nG0 F3000\nM110 X1 Y1 Z1\nM109 S1\nM7 S1\nM96 S0\nG92 U0\n".to_string(),
        Flavor::XToolM1 => format!("G90\nG1 F9600\nG0 F9600\nM19 S1\nM18 S0\nG4 P0.1\nM4 S0\nM104 X0\nG0 Z{}\nG0 Y30\n", n((17.0 - o.thickness).clamp(0.0, 35.0))),
        Flavor::XToolF1 { ir } => format!("$L\nG0 F180000\nM4 S0\nG1 F180000\nM114 S{}\nG21\nG90\n", if ir { 2 } else { 1 }),
        _ => "G21\nG90\n".to_string(),
    };
    // the laser back to its ready state after a dot / pierce turned it on and off (M4 / M3 S0 on GRBL)
    let ready = |dynamic: bool| -> &'static str {
        match fl {
            Flavor::Grbl if dynamic => "M4 S0\n",
            Flavor::Grbl | Flavor::Smoothie | Flavor::Rrf => "M3 S0\n",
            Flavor::Marlin => "M3 I S0\n",
            Flavor::XTool => "M4\nM15 S1\n",
            Flavor::XToolF1 { .. } => "M4 S0\n",
            Flavor::Snapmaker | Flavor::MarlinFan | Flavor::XToolM1 => "",
        }
    };
    let (fire, off) = if fl == Flavor::MarlinFan { ("M106 S", "M106 S0\n") } else { ("M3 S", "M5\n") };
    // GRBL's G4 P is seconds; the others read P as milliseconds
    let dwell = |ms: f64| if grbl || xtool || f1 || m1 { format!("G4 P{}\n", n(ms / 1000.0)) } else { format!("G4 P{}\n", n(ms)) };
    let s_every = o.s_every || matches!(fl, Flavor::Marlin | Flavor::Rrf | Flavor::XToolF1 { .. } | Flavor::XToolM1);
    // the job's length in the machine's own lines
    let timed = |t: &str| t.replace("{seconds}", &format!("{:.0}", plan.seconds)).replace("{minutes}", &format!("{:.1}", plan.seconds / 60.0));
    let (start, end) = (timed(&o.start), timed(&o.end));
    for l in start.lines().map(str::trim).filter(|l| !l.is_empty()) {
        g += l;
        g.push('\n');
    }
    let ys = if o.y_scale > 0.0 { o.y_scale } else { 1.0 };
    let ya = o.y_axis; // 'Y', or 'A' for a rotary on its own axis (then ys turns mm into degrees)
    let (sx, sy) = (o.shift.x, o.shift.y);
    // a machine whose 0,0 isn't the front-left corner: X / Y mirrored across the bed
    let fx = |x: f64| o.mirror[0].map_or(x, |w| w - x);
    let fy = |y: f64| o.mirror[1].map_or(y, |h| h - y);
    // "X… Y…" for a design point (mirrored, rotary-scaled, X / Y swapped when the machine needs it)
    let xy = |x: f64, y: f64| {
        let (a, b) = (n(fx(x)), n(fy(y) * ys));
        if o.swap_xy && ya == 'Y' {
            format!("X{b} Y{a}")
        } else {
            format!("X{a} {ya}{b}")
        }
    };
    if let Some(p) = o.origin.filter(|_| !m1) {
        g += &format!("G92 {}\n", xy(p.x, p.y)); // here = this design point
    }
    let mut dynamic = o.dynamic && grbl; // the M3 / M4 in force (M4 is GRBL's only)
    g += if fl == Flavor::MarlinFan { off } else { ready(dynamic) };
    let (mut feed, mut s_now, mut air, mut z) = (-1.0, -1.0, false, 0.0);
    // whole S values for GRBL's 0–1000; a 0–1 scale (Smoothieware) keeps 3 decimals
    let sv = |v: f64| if o.s_max >= 100.0 { v.round() } else { (v * 1000.0).round() / 1000.0 };
    let mut last_layer: Option<usize> = None;
    let total = plan.times.last().map_or(0.0, |t| t[2]);
    let mut pct_now = -1.0;
    for (i, st) in plan.strokes.iter().enumerate() {
        if o.progress && total > 0.0 {
            let done = plan.times.get(i).map_or(0.0, |t| t[0]) / total;
            let pct = (done * 100.0).floor();
            if pct > pct_now {
                g += &format!("M73 P{pct} R{}\n", ((1.0 - done) * plan.seconds / 60.0).ceil());
                pct_now = pct;
            }
        }
        // a layer's pauses: after the one before it, before its own first stroke
        if last_layer != Some(st.layer) {
            let after = last_layer.map_or(0.0, |p| doc.layers[p].pause_after);
            for ms in [after, doc.layers[st.layer].pause_before] {
                if ms > 0.0 {
                    g += &dwell(ms);
                }
            }
            last_layer = Some(st.layer);
        }
        // Z moves are relative to where the job started (G91 for the one move), so no Z homing is needed
        let zi = plan.z.get(i).copied().unwrap_or(0.0);
        if (zi - z).abs() > 1e-9 {
            g += &format!("G91\nG0 Z{}\nG90\n", n(if m1 { z - zi } else { zi - z })); // the M1's Z points down
            z = zi;
        }
        let l = &doc.layers[st.layer];
        // a scan line (fill / picture rows: an open straight run, however many power steps it holds; not an outline)
        // moves back half the scanning offset along its own direction
        let scan = matches!(l.mode, crate::Mode::Fill | crate::Mode::FillLine | crate::Mode::Image) && st.pts.len() >= 2 && st.pts.first() != st.pts.last() && {
            let (a, b) = (st.pts[0], st.pts[st.pts.len() - 1]);
            let d = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt().max(1e-9);
            st.pts.iter().all(|p| ((p.x - a.x) * (b.y - a.y) - (p.y - a.y) * (b.x - a.x)).abs() / d < 1e-3)
        };
        let (sx, sy) = if scan && !o.scan_offsets.is_empty() {
            let (a, b) = (st.pts[0], st.pts[st.pts.len() - 1]);
            let d = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt().max(1e-9);
            let half = scan_offset_at(&o.scan_offsets, l.speed) / 2.0;
            (sx - half * (b.x - a.x) / d, sy - half * (b.y - a.y) / d)
        } else {
            (sx, sy)
        };
        // a constant-power layer switches to M3 for its strokes (S0 between them), the rest go back to M4
        let want = o.dynamic && grbl && !l.constant_power;
        if want != dynamic {
            dynamic = want;
            g += ready(dynamic);
            s_now = 0.0;
        }
        if l.air != air && !xtool && !f1 && !m1 {
            air = l.air;
            g += if !air {
                "M9\n"
            } else if o.air_m7 {
                "M7\n"
            } else {
                "M8\n"
            };
        }
        let p0 = st.pts[0];
        if xtool {
            // ponytail: xTool travel at F6000 with S0 (LightBurn frames that way); G0's beam-off isn't confirmed there
            g += &format!("G1 {} S0 F6000\n", xy(p0.x + sx, p0.y + sy));
            (feed, s_now) = (6000.0, 0.0);
        } else {
            g += &format!("G0 {}\n", xy(p0.x + sx, p0.y + sy));
        }
        let f = if m1 { (l.speed * 60.0).max(60.0) } else { l.speed * 60.0 }; // F0 hangs the M1
        let full = l.power / 100.0 * o.s_max;
        if st.dwell_ms > 0.0 && m1 {
            // no M3 on the M1: a dot is a 0.02 mm burn (like the Ruida's)
            g += &format!("G1 {} F{} S{}\n", xy(p0.x + sx + 0.02, p0.y + sy), n(l.speed.max(1.0) * 60.0), n(sv(full)));
            (feed, s_now) = (l.speed.max(1.0) * 60.0, sv(full));
            continue;
        }
        if st.dwell_ms > 0.0 {
            // a dot: constant power while standing still (M4 fires only in motion), then off
            g += &format!("{fire}{}\n{}{off}{}", n(sv(full)), dwell(st.dwell_ms as f64), ready(dynamic));
            s_now = 0.0;
            continue;
        }
        // Line pauses: G4 standing still, the beam on at the pierce power (M3 fires without motion) when set
        let pause = |ms: f64| -> String {
            match (ms > 0.0, l.pierce > 0.0 && !m1) {
                (false, _) => String::new(),
                (true, false) => dwell(ms),
                (true, true) => format!("{fire}{}\n{}{off}{}", n(sv(l.pierce.min(100.0) / 100.0 * o.s_max)), dwell(ms), ready(dynamic)),
            }
        };
        if fl == Flavor::Snapmaker {
            g += "M3\n";
            s_now = -1.0; // S again on the first G1 after the switch-on
        }
        let line = l.mode == crate::Mode::Line;
        if line && l.pause_start > 0.0 {
            g += &pause(l.pause_start);
            s_now = if l.pierce > 0.0 { 0.0 } else { s_now };
        }
        for (k, p) in st.pts[1..].iter().enumerate() {
            // a greyscale picture runs from Min power (white) to Power (black), like LightBurn; other strokes: × pw
            let w = st.pw.as_ref().and_then(|w| w.get(k)).copied().unwrap_or(1.0) as f64;
            let grey = l.mode == crate::Mode::Image && l.dither == crate::raster::Dither::Grayscale && l.power_min > 0.0 && w > 0.0;
            let s = sv(if grey { (l.power_min.min(l.power) + (l.power - l.power_min.min(l.power)) * w) / 100.0 * o.s_max } else { full * w });
            if o.blank_g0 && !xtool && w <= 0.0 && st.pw.is_some() && l.mode != crate::Mode::Line {
                g += &format!("G0 {}\n", xy(p.x + sx, p.y + sy));
                continue;
            }
            if fl == Flavor::MarlinFan && (s - s_now).abs() > 1e-9 {
                g += &format!("M106 S{}\n", n(s)); // the fan-pin laser's power, set before the move
                s_now = s;
            }
            g += &format!("G1 {}", xy(p.x + sx, p.y + sy));
            if (f - feed).abs() > 1e-9 {
                g += &format!(" F{}", n(f));
                feed = f;
            }
            if fl != Flavor::MarlinFan && ((s - s_now).abs() > 1e-9 || s_every) {
                g += &format!(" S{}", n(s));
                s_now = s;
            }
            g.push('\n');
        }
        if line && l.pause_end > 0.0 {
            g += &pause(l.pause_end);
            s_now = if l.pierce > 0.0 { 0.0 } else { s_now };
        }
        // off between strokes: M4 only fires in G1, but GRBL's M3 would keep burning through the next G0 (the other
        // firmwares never fire on G0)
        match fl {
            Flavor::Grbl if !dynamic => {
                g += "S0\n";
                s_now = 0.0;
            }
            Flavor::Snapmaker => g += "M5\n",
            Flavor::XToolF1 { .. } => {
                g += "G0 S0\n"; // BeamInk's beam-off after every burn
                s_now = 0.0;
            }
            Flavor::MarlinFan if s_now != 0.0 => {
                g += off;
                s_now = 0.0;
            }
            _ => {}
        }
    }
    if let Some(p) = last_layer.filter(|p| doc.layers[*p].pause_after > 0.0) {
        g += &dwell(doc.layers[p].pause_after);
    }
    if air {
        g += "M9\n";
    }
    g += match fl {
        Flavor::MarlinFan => "M107\n",
        Flavor::XTool => "M15 S0\nM5\n",
        Flavor::XToolM1 => "G0 Z0 F3000\nG0 X0 Y0 F9600\nG4 P0.1\nM05\nM6 P1\n",
        Flavor::XToolF1 { .. } => "G90\nG0 S0\nG0 F180000\nG1 F180000\nM6 P1\n$P\n",
        _ => "M5\n",
    };
    if o.progress {
        g += "M73 P100 R0\n";
    }
    if z != 0.0 && !m1 {
        g += &format!("G91\nG0 Z{}\nG90\n", n(-z)); // back to the starting height
    }
    if o.home_after && !xtool && !f1 && !m1 {
        // with G92: back to where the head stood at the start (the origin point), then the machine's own coordinates
        g += &match o.origin {
            Some(p) => format!("G0 {}\nG92.1\n", xy(p.x, p.y)),
            None => format!("G0 {}\n", xy(o.finish.x, o.finish.y)),
        };
    } else if o.origin.is_some() && !m1 {
        g += "G92.1\n";
    }
    for l in end.lines().map(str::trim).filter(|l| !l.is_empty()) {
        g += l;
        g.push('\n');
    }
    if xtool {
        g += "M6\nM7 S0 N0 D10\n"; // the job's end, the exhaust fan off
    }
    g
}

#[cfg(test)]
mod tests {
    #[test]
    fn each_firmware_gets_its_own_switch_on_dwell_and_power() {
        let mut d = crate::sample();
        d.layers[0].pause_before = 250.0;
        let p = crate::plan::plan(&d.for_job(), crate::plan::Machine::default());
        let out = |flavor, s_max| gcode(&d, &p, GcodeOptions { flavor, s_max, ..Default::default() });
        let lines = |g: &str| g.lines().map(str::to_string).collect::<Vec<_>>();
        // every G1 right after a G0 carries its S (Marlin's inline mode drops the power on G0; RRF's S isn't sticky)
        let s_after_travel = |g: &str| lines(g).windows(2).filter(|w| w[0].starts_with("G0 ") && w[1].starts_with("G1 ")).all(|w| w[1].contains(" S"));
        let grbl = out(Flavor::Grbl, 1000.0);
        assert!(grbl.contains("G94\n") && grbl.contains("\nM4 S0\n") && grbl.contains("G4 P0.25\n"));
        let marlin = out(Flavor::Marlin, 255.0);
        assert!(marlin.contains("\nM3 I S0\n") && !marlin.contains("M4") && !marlin.contains("G94") && marlin.contains("G4 P250\n"), "{}", &marlin[..400]);
        assert!(s_after_travel(&marlin) && !lines(&marlin).iter().any(|l| l == "S0"));
        assert!(marlin.lines().filter_map(|l| l.split_whitespace().find_map(|w| w.strip_prefix('S')?.parse::<f64>().ok())).all(|s| s <= 255.0));
        let rrf = out(Flavor::Rrf, 255.0);
        assert!(lines(&rrf).iter().filter(|l| l.starts_with("G1 ")).all(|l| l.contains(" S")) && rrf.contains("G4 P250\n"));
        let smoothie = out(Flavor::Smoothie, 1.0);
        assert!(!smoothie.contains("M4") && !lines(&smoothie).iter().any(|l| l == "S0") && smoothie.contains("G4 P250\n"));
        // Snapmaker: the laser switched on before each burning stroke and off after it, S on its first move
        let snap = out(Flavor::Snapmaker, 255.0);
        let l = lines(&snap);
        assert_eq!(l.iter().filter(|x| *x == "M3").count() + 1, l.iter().filter(|x| *x == "M5").count(), "one M5 per M3 plus the end");
        assert!(l.windows(2).filter(|w| w[0] == "M3" && w[1].starts_with("G1 ")).all(|w| w[1].contains(" S")));
        // xTool S1: LightBurn 2.2's header, M4 + M15 S1, travel as G1 … S0, M6 at the end; no air M8 / G94
        let xt = out(Flavor::XTool, 1000.0);
        assert!(xt.starts_with("; GBurn") && xt.contains("\nG21\nG90\nG0 F3000\nM110 X1 Y1 Z1\nM109 S1\nM7 S1\nM96 S0\nG92 U0\n"));
        assert!(xt.contains("M4\nM15 S1\n") && xt.trim_end().ends_with("M15 S0\nM5\nM6\nM7 S0 N0 D10") && !xt.contains("G94") && !xt.contains("\nG0 X"), "{}", &xt[..400]);
        assert!(lines(&xt).windows(2).filter(|w| w[0].ends_with("S0 F6000")).all(|w| !w[1].starts_with("G1 ") || w[1].contains(" S")), "the first cut after a travel names its power");
        assert!(xt.contains("G4 P0.25\n"), "seconds, like GRBL");
        // xTool F1 (old firmware): BeamInk's session, blue or infrared, S on every G1, G0 S0 after each burn, no air
        let f1 = out(Flavor::XToolF1 { ir: false }, 1000.0);
        assert!(f1.contains("\n$L\nG0 F180000\nM4 S0\nG1 F180000\nM114 S1\nG21\nG90\n") && f1.trim_end().ends_with("G90\nG0 S0\nG0 F180000\nG1 F180000\nM6 P1\n$P"), "{}", &f1[..400]);
        assert!(lines(&f1).iter().filter(|l| l.starts_with("G1 X")).all(|l| l.contains(" S")) && !f1.contains("M8") && !f1.contains("M3 S0") && f1.contains("G4 P0.25\n"));
        assert!(lines(&f1).windows(2).filter(|w| w[1].starts_with("G0 X")).skip(1).all(|w| w[0] == "G0 S0"), "every travel after a burn follows a G0 S0");
        assert!(out(Flavor::XToolF1 { ir: true }, 1000.0).contains("\nM114 S2\n"));
        // xTool M1: only G0 / G1 / G4 / G90 / G91 between its header and footer, whole S, never F0, focus Z from the stock
        let mut m1o = GcodeOptions { flavor: Flavor::XToolM1, thickness: 3.0, origin: Some(crate::pt(1.0, 1.0)), ..Default::default() };
        let m1 = gcode(&d, &p, m1o.clone());
        let body: Vec<&str> = m1.lines().filter(|l| !l.starts_with(';')).collect();
        let foot = body.iter().position(|l| *l == "G0 Z0 F3000").unwrap();
        assert!(m1.contains("\nM104 X0\nG0 Z14\nG0 Y30\n") && m1.trim_end().ends_with("G0 Z0 F3000\nG0 X0 Y0 F9600\nG4 P0.1\nM05\nM6 P1"), "{}", &m1[..400]);
        assert!(body[10..foot].iter().all(|l| ["G0 ", "G1 ", "G4 ", "G90", "G91"].iter().any(|c| l.starts_with(c))), "{:?}", body.iter().find(|l| !l.starts_with('G')));
        assert!(!m1.contains("G92") && !m1.contains(" F0") && body.iter().filter(|l| l.starts_with("G1 X")).all(|l| l.contains(" S") && !l.split(" S").nth(1).unwrap().contains('.')));
        m1o.thickness = 40.0;
        assert!(gcode(&d, &p, m1o).contains("\nG0 Z0\n"), "focus Z kept inside 0–35");
        // its upload: a stored zip of gcodes.txt (CRC-32 check value, sizes, one entry)
        let z = m1_zip("123456789");
        assert_eq!(&z[14..18], &0xCBF4_3926u32.to_le_bytes());
        assert_eq!(z.len(), 30 + 10 + 9 + 46 + 10 + 22);
        assert_eq!(&z[30..40], b"gcodes.txt");
        // fan-pin laser: no M3 / M4 / M5 at all, M106 power before the moves, M106 S0 between strokes, M107 at the end
        let fan = out(Flavor::MarlinFan, 255.0);
        assert!(!fan.contains("M3") && !fan.contains("M4") && !fan.contains("M5") && fan.contains("\nM107\n"), "{}", &fan[..400]);
        assert!(lines(&fan).iter().filter(|l| l.starts_with("G1 ")).all(|l| !l.contains(" S")));
        assert!(lines(&fan).windows(2).filter(|w| w[1].starts_with("G0 ")).all(|w| !w[0].starts_with("M106 S") || w[0] == "M106 S0"), "travel never with the fan laser on");
    }

    use super::*;
    use crate::plan::{plan, Machine};
    use crate::{bounds, sample, MM_PER_IN};

    #[test]
    fn global_passes_repeat_the_run_and_layers_pause() {
        let mut d = Document::default();
        d.add(crate::geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        let mut sub = d.layers[0].clone();
        sub.speed = 7.0;
        (d.layers[0].subs, d.layers[0].global_passes, d.layers[0].pause_before) = (vec![sub], 3, 250.0);
        let job = d.for_job();
        assert_eq!(job.layers.len(), 6, "main, sub × 3");
        assert_eq!(job.layers.iter().map(|l| l.speed == 7.0).collect::<Vec<_>>(), [false, true, false, true, false, true]);
        let p = plan(&job, Machine::default());
        let g = gcode(&job, &p, GcodeOptions::default());
        assert_eq!(g.matches("G4 P0.25\n").count(), 3, "{g}"); // the sub-layer was copied before the pause was set
    }

    #[test]
    fn swapped_axes_and_blank_runs_as_rapids() {
        let mut d = Document::default();
        d.add(crate::geom::make_rect(0.0, 0.0, 10.0, 4.0), 0);
        (d.layers[0].mode, d.layers[0].overscan) = (crate::Mode::Fill, 3.0);
        let p = plan(&d, Machine::default());
        let g = gcode(&d, &p, GcodeOptions { swap_xy: true, home_after: false, ..Default::default() });
        // a 10 wide × 4 tall box: swapped, every X is ≤ 4 (design Y) and Y reaches 10 (design X)
        let val = |l: &str, c: char| l.split_whitespace().find(|w| w.starts_with(c)).and_then(|w| w[1..].parse::<f64>().ok());
        let g1: Vec<&str> = g.lines().filter(|l| l.starts_with("G1")).collect();
        assert!(g1.iter().all(|l| val(l, 'X').is_some_and(|x| x <= 4.0 + 1e-6)), "{g}");
        assert!(g1.iter().any(|l| val(l, 'Y').is_some_and(|y| y >= 10.0 - 1e-6)), "{g}");
        let plain = gcode(&d, &p, GcodeOptions::default());
        let fast = gcode(&d, &p, GcodeOptions { blank_g0: true, ..Default::default() });
        let g0 = |g: &str| g.lines().filter(|l| l.starts_with("G0")).count();
        assert!(g0(&fast) > g0(&plain), "overscan runs go as G0");
        assert!(!fast.lines().any(|l| l.starts_with("G1") && l.ends_with(" S0")), "no G1 at S0 left");
    }

    #[test]
    fn progress_counts_up_to_a_hundred() {
        let mut d = Document::default();
        for k in 0..20 {
            d.add(crate::geom::make_rect(k as f64 * 12.0, 0.0, k as f64 * 12.0 + 10.0, 10.0), 0);
        }
        d.layers[0].mode = crate::Mode::Line;
        let p = plan(&d, Machine::default());
        let g = gcode(&d, &p, GcodeOptions { progress: true, ..Default::default() });
        let pcts: Vec<f64> = g.lines().filter_map(|l| l.strip_prefix("M73 P")).map(|l| l.split(' ').next().unwrap().parse().unwrap()).collect();
        assert_eq!(pcts.first(), Some(&0.0));
        assert_eq!(pcts.last(), Some(&100.0));
        assert!(pcts.windows(2).all(|w| w[1] > w[0]) && pcts.len() >= 15, "{pcts:?}");
        assert!(!gcode(&d, &p, GcodeOptions::default()).contains("M73"));
    }

    #[test]
    fn line_pauses_wait_and_cut_through_fires_while_waiting() {
        let mut d = Document::default();
        d.add(crate::geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        let l = &mut d.layers[0];
        (l.mode, l.pause_start, l.pause_end, l.pierce) = (crate::Mode::Line, 300.0, 100.0, 0.0);
        let p = plan(&d, Machine::default());
        let g = gcode(&d, &p, GcodeOptions::default());
        assert!(g.contains("G4 P0.3\n") && g.contains("G4 P0.1\n"), "{g}");
        d.layers[0].pierce = 50.0;
        let g = gcode(&d, &p, GcodeOptions::default());
        assert!(g.contains("M3 S500\nG4 P0.3\nM5\nM4 S0\n"), "{g}");
    }

    /// generated output must equal a read-back: gcarve-core's G-code reader parses what we wrote, the extents of
    /// the laser-on moves match the design, the count of strokes matches, the power is right
    #[test]
    fn the_gcode_reads_back_to_the_design() {
        let d = sample();
        let p = plan(&d, Machine::default());
        let g = gcode(&d, &p, GcodeOptions::default());
        let prog = gcarve_core::parse_program(&g, 1000.0);
        assert!(prog.errors.is_empty(), "{:?}", prog.errors);
        let mut r = crate::Rect::default();
        let mut cuts = 0;
        for m in prog.motions.iter().filter(|m| m.kind != gcarve_core::MoveType::Rapid) {
            r.add(crate::pt(m.to.x * MM_PER_IN, m.to.y * MM_PER_IN));
            cuts += 1;
        }
        let db = d.bounds();
        for (a, b) in [(r.x0, db.x0), (r.y0, db.y0), (r.x1, db.x1), (r.y1, db.y1)] {
            assert!((a - b).abs() < 0.05, "read back {a} vs design {b}");
        }
        let expect: usize = p.strokes.iter().map(|s| s.pts.len() - 1).sum();
        assert_eq!(cuts, expect);
        // 90 % power on the cut layer = S900, and the job ends with the laser off
        assert!(g.contains(" S900\n"), "{g}");
        assert!(g.trim_end().ends_with("M5\nG0 X0 Y0"));
        // the circle's extents through the stroke points too (flattening is fine enough)
        let circle = bounds(&d.shapes[0].contour);
        assert!((circle.w() - 90.0).abs() < 0.01);
        // M3 flavour switches S off between strokes
        let g3 = gcode(&d, &p, GcodeOptions { dynamic: false, ..Default::default() });
        assert!(g3.starts_with("; GBurn") && g3.contains("M3 S0\n") && g3.contains("\nS0\n"));
    }

    #[test]
    fn greyscale_power_and_a_current_position_origin_come_out_right() {
        let mut d = Document { layers: vec![crate::Layer::new("Img", [0, 0, 0], crate::Mode::Image, 100.0, 50.0)], ..Default::default() };
        d.layers[0].dither = crate::raster::Dither::Grayscale;
        let mut png = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png, 2, 1);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&[0, 128]).unwrap();
        }
        d.add_image(&png, crate::pt(10.0, 10.0), 2.0, 0).unwrap();
        d.layers[0].interval = 1.0;
        let p = plan(&d, Machine::default());
        let g = gcode(&d, &p, GcodeOptions { origin: Some(crate::pt(10.0, 10.0)), ..Default::default() });
        assert!(g.contains("G92 X10 Y10\n"), "{g}");
        assert!(g.contains(" S500\n") && g.contains(" S249\n"), "{g}"); // black = 50 %, mid grey ≈ half of that
        assert!(g.trim_end().ends_with("G0 X10 Y10\nG92.1"), "back where the head started: {g}");
        let prog = gcarve_core::parse_program(&g, 1000.0);
        assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    }

    #[test]
    fn z_steps_down_each_pass_and_comes_back_up() {
        let mut d = Document { layers: vec![crate::Layer::new("Cut", [255, 0, 0], crate::Mode::Line, 10.0, 90.0)], ..Default::default() };
        d.layers[0].passes = 3;
        d.layers[0].z_offset = -1.0;
        d.layers[0].z_step = -0.5;
        d.add(crate::geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        let p = plan(&d, Machine::default());
        assert_eq!(p.z, vec![-1.0, -1.5, -2.0]);
        let g = gcode(&d, &p, GcodeOptions::default());
        let zs: Vec<&str> = g.lines().filter(|l| l.starts_with("G0 Z")).collect();
        assert_eq!(zs, ["G0 Z-1", "G0 Z-0.5", "G0 Z-0.5", "G0 Z2"], "{g}");
        let prog = gcarve_core::parse_program(&g, 1000.0);
        assert!(prog.errors.is_empty(), "{:?}", prog.errors);
        // a constant-power layer runs in M3 with S0 between its strokes, then M4 again
        let mut d2 = d.clone();
        d2.layers.push(crate::Layer::new("M3", [0, 0, 255], crate::Mode::Line, 10.0, 50.0));
        d2.layers[1].constant_power = true;
        d2.add(crate::geom::make_rect(20.0, 0.0, 30.0, 10.0), 1);
        d2.add(crate::geom::make_rect(40.0, 0.0, 50.0, 10.0), 1);
        d2.layers[0].passes = 1;
        d2.layers[0].z_offset = 0.0;
        d2.layers[0].z_step = 0.0;
        let g = gcode(&d2, &plan(&d2, Machine { optimize: false, ..Machine::default() }), GcodeOptions::default());
        let m3 = g.find("M3 S0\n").expect("switches to M3");
        assert!(g[m3..].contains("\nS0\n"), "{g}");
        assert!(g.matches("M4 S0\n").count() == 1, "only the start is M4: {g}");
        // the head goes to the finish spot afterwards
        let g = gcode(&d, &p, GcodeOptions { finish: crate::pt(100.0, 50.0), ..Default::default() });
        assert!(g.ends_with("G0 X100 Y50\n"), "{g}");
        // greyscale between Min power and Power: the faintest grey still burns at Min power
        let mut dg = Document { layers: vec![crate::Layer::new("Img", [0, 0, 0], crate::Mode::Image, 100.0, 80.0)], ..Default::default() };
        (dg.layers[0].dither, dg.layers[0].power_min, dg.layers[0].interval) = (crate::raster::Dither::Grayscale, 20.0, 1.0);
        let id = dg.add(crate::geom::make_rect(0.0, 0.0, 2.0, 1.0), 0);
        dg.find_mut(id).unwrap().raster = Some(crate::raster::Raster::from_gray(2, 1, &[0, 250]));
        let g = gcode(&dg, &plan(&dg, Machine::default()), GcodeOptions::default());
        let ss: Vec<f64> = g.split(" S").skip(1).filter_map(|t| t.split_whitespace().next()?.parse().ok()).filter(|v| *v > 0.0).collect();
        assert!(ss.iter().any(|v| (*v - 800.0).abs() < 1.0) && ss.iter().all(|v| *v >= 200.0), "{ss:?}");
        // a rear-right origin: X and Y mirrored across a 100 × 50 bed
        let g = gcode(&d, &p, GcodeOptions { mirror: [Some(100.0), Some(50.0)], home_after: false, ..Default::default() });
        let first = g.lines().find(|l| l.starts_with("G0 X")).unwrap();
        let p0 = p.strokes[0].pts[0];
        assert_eq!(first, format!("G0 X{} Y{}", n(100.0 - p0.x), n(50.0 - p0.y)));
        // air assist on M7
        let mut da = d.clone();
        da.layers.iter_mut().for_each(|l| l.air = true);
        let g = gcode(&da, &plan(&da, Machine::default()), GcodeOptions { air_m7: true, ..Default::default() });
        assert!(g.contains("M7\n") && !g.contains("M8\n"), "air on M7");
        // a 0–1 power scale keeps its fractions; S can go on every move
        let g = gcode(&d, &p, GcodeOptions { s_max: 1.0, s_every: true, ..Default::default() });
        assert!(g.lines().filter(|l| l.starts_with("G1 ")).all(|l| l.contains(" S")), "S on every move");
        assert!(g.lines().any(|l| l.ends_with(" S0.9")), "90 % on a 0–1 scale");
        // a rotary on A: Y goes out as degrees on A
        let g = gcode(&d, &p, GcodeOptions { y_axis: 'A', y_scale: 2.0, home_after: false, ..Default::default() });
        assert!(g.contains(" A") && !g.lines().any(|l| l.starts_with('G') && l.contains(" Y")), "A instead of Y");
        // the machine's own start / end lines go round the job
        let g = gcode(&d, &p, GcodeOptions { start: "$H\n\n M7 ".into(), end: "M9\nG4 P1".into(), ..Default::default() });
        assert!(g.contains("G94\n$H\nM7\n") && g.ends_with("M9\nG4 P1\n"), "{g}");
        // {seconds} / {minutes}: the job's estimate
        let g = gcode(&d, &p, GcodeOptions { start: "; takes {seconds} s = {minutes} min".into(), ..Default::default() });
        assert!(g.contains(&format!("; takes {:.0} s = {:.1} min\n", p.seconds, p.seconds / 60.0)), "{g}");
        // no Z settings: no Z moves at all
        d.layers[0].z_offset = 0.0;
        d.layers[0].z_step = 0.0;
        let p = plan(&d, Machine::default());
        assert!(p.z.is_empty() && !gcode(&d, &p, GcodeOptions::default()).contains('Z'));
    }

    #[test]
    fn a_rotary_scales_y_only() {
        let d = sample();
        let p = plan(&d, Machine::default());
        let g = gcode(&d, &p, GcodeOptions { y_scale: 2.0, home_after: false, ..Default::default() });
        let prog = gcarve_core::parse_program(&g, 1000.0);
        let b = d.bounds();
        assert!(((prog.max.y - prog.min.y) * MM_PER_IN - 2.0 * b.h()).abs() < 0.1, "{}", (prog.max.y - prog.min.y) * MM_PER_IN);
        assert!(((prog.max.x - prog.min.x) * MM_PER_IN - b.w()).abs() < 0.1);
    }

    #[test]
    fn dots_dwell_with_constant_power_and_a_shift_moves_everything() {
        let mut d = Document { layers: vec![crate::Layer::new("Dot", [255, 0, 0], crate::Mode::Line, 10.0, 50.0)], ..Default::default() };
        d.layers[0].dot_ms = 20.0;
        d.layers[0].dot_mm = 5.0;
        d.add(crate::geom::make_polyline(&[crate::pt(0.0, 0.0), crate::pt(10.0, 0.0)], false), 0);
        let p = plan(&d, Machine::default());
        let g = gcode(&d, &p, GcodeOptions { shift: crate::pt(100.0, 50.0), home_after: false, ..Default::default() });
        assert_eq!(g.matches("G4 P0.02").count(), 3, "{g}");
        assert!(g.contains("G0 X100 Y50\nM3 S500\n"), "{g}");
        assert!(g.contains("G0 X110 Y50\n"));
        let prog = gcarve_core::parse_program(&g, 1000.0);
        assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    }

    #[test]
    fn scan_lines_move_back_half_the_offset_along_their_direction() {
        assert_eq!(scan_offset_at(&[], 100.0), 0.0);
        let t = [(300.0, 0.2), (100.0, 0.1)];
        assert_eq!(scan_offset_at(&t, 50.0), 0.1);
        assert!((scan_offset_at(&t, 200.0) - 0.15).abs() < 1e-12);
        assert_eq!(scan_offset_at(&t, 900.0), 0.2);
        let mut d = Document { layers: vec![crate::Layer::new("F", [0, 0, 0], crate::Mode::Fill, 100.0, 50.0)], ..Default::default() };
        d.layers[0].interval = 5.0;
        d.add(crate::geom::make_rect(0.0, 0.0, 10.0, 10.0), 0);
        // plain hatch lines, then ramped ones (many power steps a line: still scan lines)
        for ramp in [0.0, 2.0] {
            d.layers[0].ramp = ramp;
            let p = plan(&d, Machine::default());
            assert!(ramp == 0.0 || p.strokes.iter().any(|s| s.pts.len() > 4));
            let g = gcode(&d, &p, GcodeOptions { scan_offsets: vec![(100.0, 0.4)], home_after: false, ..Default::default() });
            let plain = gcode(&d, &p, GcodeOptions { home_after: false, ..Default::default() });
            let xs = |g: &str| -> Vec<f64> { g.lines().filter(|l| l.starts_with("G1")).filter_map(|l| l.split_whitespace().find(|w| w.starts_with('X')).and_then(|w| w[1..].parse().ok())).collect() };
            let (a, b) = (xs(&g), xs(&plain));
            assert_eq!(a.len(), b.len());
            // left-to-right lines end 0.2 short, right-to-left ones 0.2 further right
            assert!(a.iter().zip(&b).all(|(x, y)| ((x - y).abs() - 0.2).abs() < 1e-6), "{a:?} {b:?}");
            assert!(a.iter().zip(&b).any(|(x, y)| x < y) && a.iter().zip(&b).any(|(x, y)| x > y));
        }
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(n(1.0), "1");
        assert_eq!(n(1.2345), "1.234");
        assert_eq!(n(-0.0001), "0");
    }
}
