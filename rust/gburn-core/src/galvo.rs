//! BJJCZ / EZCAD2 galvo boards (LMC, "balor": the Thunder Aurora 8 and most fibre markers). The board takes 12-byte
//! single commands (6 little-endian u16: opcode + 5 values; answered with 8 bytes, 4 u16, the fourth a status word)
//! and 3072-byte lists of 256 such commands run by `ExecuteList`. Coordinates are 16-bit galvo units, 0x8000 at the
//! centre of the lens field; a lens correction table (the machine's `.cor` file) is uploaded at start. Protocol from
//! galvoplotter / MeerK40t balormk (MIT, see THIRD_PARTY.md) after Bryce Schroeder's balor reverse engineering.
//!
//! This module only builds bytes: the init sequence, the job lists, and reads a `.cor`. The USB conversation
//! (write 12 or 3072 bytes, read 8) is the app's.

use crate::plan::Plan;
use crate::{Document, Pt};
use serde::{Deserialize, Serialize};

// list opcodes (in a list packet)
pub const LIST_JUMP_TO: u16 = 0x8001;
pub const LIST_END_OF_LIST: u16 = 0x8002;
pub const LIST_LASER_ON_POINT: u16 = 0x8003;
pub const LIST_DELAY_TIME: u16 = 0x8004;
pub const LIST_MARK_TO: u16 = 0x8005;
pub const LIST_JUMP_SPEED: u16 = 0x8006;
pub const LIST_LASER_ON_DELAY: u16 = 0x8007;
pub const LIST_LASER_OFF_DELAY: u16 = 0x8008;
pub const LIST_MARK_FREQ: u16 = 0x800A;
pub const LIST_MARK_POWER_RATIO: u16 = 0x800B;
pub const LIST_MARK_SPEED: u16 = 0x800C;
pub const LIST_JUMP_DELAY: u16 = 0x800D;
pub const LIST_POLYGON_DELAY: u16 = 0x800F;
pub const LIST_WRITE_PORT: u16 = 0x8011;
pub const LIST_MARK_CURRENT: u16 = 0x8012;

/// Fibre current for a power %: 0–100 % → 0–0xFFF (past 100 % would overflow the board's 12 bits)
pub fn fibre_current(power: f64) -> u16 {
    (power.clamp(0.0, 100.0) * 0xFFF as f64 / 100.0).round() as u16
}
pub const LIST_QSWITCH_PERIOD: u16 = 0x801B;
pub const LIST_SET_CO2_FPK: u16 = 0x801E;
pub const LIST_FIBER_OPEN_MO: u16 = 0x8021;
pub const LIST_FIBER_YLPM_PULSE_WIDTH: u16 = 0x8026;
pub const LIST_READY: u16 = 0x8051;
// single commands
pub const DISABLE_LASER: u16 = 0x0002;
pub const ENABLE_LASER: u16 = 0x0004;
pub const EXECUTE_LIST: u16 = 0x0005;
pub const SET_PWM_PULSE_WIDTH: u16 = 0x0006;
pub const GET_VERSION: u16 = 0x0007;
pub const GET_SERIAL_NO: u16 = 0x0009;
pub const GET_LIST_STATUS: u16 = 0x000A;
pub const GET_POSITION_XY: u16 = 0x000C;
pub const GOTO_XY: u16 = 0x000D;
pub const WRITE_COR_LINE: u16 = 0x0010;
pub const RESET_LIST: u16 = 0x0012;
pub const RESTART_LIST: u16 = 0x0013;
pub const WRITE_COR_TABLE: u16 = 0x0015;
pub const SET_CONTROL_MODE: u16 = 0x0016;
pub const SET_DELAY_MODE: u16 = 0x0017;
pub const SET_END_OF_LIST: u16 = 0x0019;
pub const SET_FIRST_PULSE_KILLER: u16 = 0x001A;
pub const SET_LASER_MODE: u16 = 0x001B;
pub const SET_TIMING: u16 = 0x001C;
pub const SET_STANDBY: u16 = 0x001D;
pub const SET_PWM_HALF_PERIOD: u16 = 0x001E;
pub const STOP_EXECUTE: u16 = 0x001F;
pub const STOP_LIST: u16 = 0x0020;
pub const WRITE_PORT: u16 = 0x0021;
pub const WRITE_ANALOG_PORT_1: u16 = 0x0022;
pub const READ_PORT: u16 = 0x0025;
pub const SET_FPK_PARAM_2: u16 = 0x002E;
pub const SET_FLY_RES: u16 = 0x0032;
pub const FIBER_SET_MO: u16 = 0x0033;
pub const ENABLE_Z: u16 = 0x003A;
pub const RESET: u16 = 0x0040;
pub const GET_MARK_TIME: u16 = 0x0041;
/// status word bits (the fourth u16 of a reply)
pub const STATUS_BUSY: u16 = 0x04;
pub const STATUS_READY: u16 = 0x20;
pub const STATUS_AXIS: u16 = 0x40;
pub const LASER_PIN: u16 = 0;
pub const LIGHT_PIN: u16 = 8;
pub const USB_VID: u16 = 0x9588;
pub const USB_PID: u16 = 0x9899;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Source {
    #[default]
    Fiber,
    Co2,
    /// a 355 nm UV source: the CO2 commands (frequency, first pulse killer, power ratio as the trigger pulse; no fibre
    /// current, Q-switch or main oscillator), after MeerK40t's balor "uv" source (MIT)
    Uv,
}

/// The lens correction table: 65 × 65 (dx, dy) in the board's encoding (sign in bit 15).
#[derive(Clone, Debug, PartialEq)]
pub struct CorTable {
    pub entries: Vec<[u16; 2]>,
    /// the field size the file was made for, mm (0 when the file doesn't say)
    pub field_mm: f64,
}

impl CorTable {
    /// A `.cor` file: "LMC1COR_1.0" (UTF-16 label, 0x1FA header with the scale at double 43, then 65 × 65 × 2 doubles)
    /// or the older int format (0x16 label, 0xE header, int32 pairs).
    pub fn parse(bytes: &[u8]) -> Result<CorTable, String> {
        if bytes.len() < 0x16 + 0xE + 65 * 65 * 8 {
            return Err("too short for a lens correction file".into());
        }
        let label: String = bytes[..0x16].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).map(|u| char::from_u32(u as u32).unwrap_or('?')).collect();
        let fix = |v: i64| -> u16 { (if v >= 0 { v } else { -v + 0x8000 }) as u16 };
        // the header holds the scale in board units per mm: field = 65536 / scale (MeerK40t's lens_size)
        let field = |scale: f64| -> f64 {
            let mm = 65536.0 / scale;
            if mm.is_finite() && mm > 1.0 && mm < 2000.0 { mm } else { 0.0 }
        };
        if label.starts_with("LMC1COR_1.0") {
            let head = &bytes[0x16..];
            if head.len() < 0x1FA + 65 * 65 * 16 {
                return Err("truncated LMC1COR_1.0 file".into());
            }
            let d = |i: usize| f64::from_le_bytes(head[2 + i * 8..2 + i * 8 + 8].try_into().unwrap());
            let field_mm = field(d(43));
            let body = &head[0x1FA..];
            let entries = (0..65 * 65)
                .map(|k| {
                    [fix(f64::from_le_bytes(body[k * 16..k * 16 + 8].try_into().unwrap()).round() as i64), fix(f64::from_le_bytes(body[k * 16 + 8..k * 16 + 16].try_into().unwrap()).round() as i64)]
                })
                .collect();
            Ok(CorTable { entries, field_mm })
        } else {
            let head = &bytes[0x16..];
            let field_mm = field(f64::from_le_bytes(head[6..14].try_into().unwrap()));
            let body = &head[0xE..];
            if body.len() < 65 * 65 * 8 {
                return Err("truncated correction file".into());
            }
            let entries = (0..65 * 65)
                .map(|k| [fix(i32::from_le_bytes(body[k * 8..k * 8 + 4].try_into().unwrap()) as i64), fix(i32::from_le_bytes(body[k * 8 + 4..k * 8 + 8].try_into().unwrap()) as i64)])
                .collect();
            Ok(CorTable { entries, field_mm })
        }
    }
}

/// A lens correction made of numbers, for a machine without its `.cor` file: mark a square, measure it, tune.
/// Applied to the design before it goes out (the board gets a blank table). u, v = −1..1 across the field.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LensFix {
    /// size, × (1 = as designed)
    pub scale: [f64; 2],
    /// x × (1 + bulge[0]·v²), y × (1 + bulge[1]·u²): + pushes the middle of the sides out (fixes a pincushion)
    pub bulge: [f64; 2],
    /// x × (1 + trapezoid[0]·v): + widens the top; y × (1 + trapezoid[1]·u): + heightens the right
    pub trapezoid: [f64; 2],
    /// x += skew·y (a parallelogram)
    pub skew: f64,
    /// marking round a cylinder without a rotary (applied first)
    #[serde(default)]
    pub cyl: Cylinder,
}

/// Cylinder correction: the design is the unrolled surface (arc length from the top line); each point goes where the
/// beam from the mirrors crosses the flat focal plane on its way to that point of the surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cylinder {
    pub on: bool,
    /// the object's diameter, mm
    pub diameter: f64,
    /// from the mirrors to the focal plane (focus distance + lens to mirror centre), mm
    pub mirror_mm: f64,
    /// the surface curves across X (the cylinder lies front to back); else across Y
    pub curve_x: bool,
    /// the inside of a curve (a bowl) rather than the outside
    pub concave: bool,
}

impl Cylinder {
    /// unrolled mm from the top line → flat-field mm (the half turn's edge is as far as it goes)
    fn map(&self, s: f64) -> f64 {
        let r = self.diameter / 2.0;
        if !self.on || r <= 0.0 || self.mirror_mm <= 0.0 {
            return s;
        }
        let phi = (s / r).clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
        let drop = r * (1.0 - phi.cos()) * if self.concave { -1.0 } else { 1.0 };
        r * phi.sin() * self.mirror_mm / (self.mirror_mm + drop).max(1e-6)
    }
}

impl Default for LensFix {
    fn default() -> Self {
        LensFix { scale: [1.0, 1.0], bulge: [0.0, 0.0], trapezoid: [0.0, 0.0], skew: 0.0, cyl: Cylinder::default() }
    }
}

impl LensFix {
    /// field-centred mm → corrected field-centred mm (`half` = half the field)
    pub fn apply(&self, x: f64, y: f64, half: f64) -> (f64, f64) {
        let (x, y) = if self.cyl.curve_x { (self.cyl.map(x), y) } else { (x, self.cyl.map(y)) };
        let (u, v) = (x / half, y / half);
        (x * self.scale[0] * (1.0 + self.bulge[0] * v * v) * (1.0 + self.trapezoid[0] * v) + self.skew * y, y * self.scale[1] * (1.0 + self.bulge[1] * u * u) * (1.0 + self.trapezoid[1] * u))
    }
    /// the design point a corrected one came from (the mirrors' position back to the design), by iteration
    pub fn unapply(&self, x: f64, y: f64, half: f64) -> (f64, f64) {
        let (mut ux, mut uy) = (x, y);
        for _ in 0..40 {
            let (ax, ay) = self.apply(ux, uy, half);
            (ux, uy) = (ux + (x - ax) * 0.9, uy + (y - ay) * 0.9);
        }
        (ux, uy)
    }
    /// straight lines bend under it (bulge / trapezoid), so they go out in short pieces
    pub fn bends(&self) -> bool {
        self.bulge != [0.0, 0.0] || self.trapezoid != [0.0, 0.0] || self.cyl.on
    }
}

/// Everything the board needs beyond the design: lens field, source, speeds, delays (galvoplotter's defaults).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GalvoOptions {
    /// lens field, mm (the whole 0..0xFFFF range)
    pub field_mm: f64,
    pub source: Source,
    /// mm/s with the beam off
    pub travel_speed: f64,
    /// µs
    pub delay_on: f64,
    pub delay_off: f64,
    pub delay_polygon: f64,
    pub delay_jump: f64,
    /// ms at the end of a list before the beam is trusted off
    pub delay_end: f64,
    pub delay_open_mo_ms: f64,
    /// the design's X / Y run the other way on this machine
    pub flip_x: bool,
    pub flip_y: bool,
    pub first_pulse_killer: u16,
    pub pwm_pulse_width: u16,
    pub pwm_half_period: u16,
    pub standby: (u16, u16),
    pub timing_mode: u16,
    pub delay_mode: u16,
    pub laser_mode: u16,
    pub control_mode: u16,
    /// the numbers lens correction (identity by default)
    #[serde(default)]
    pub fix: LensFix,
    /// the board output that switches the red pointer (EZCAD / MeerK40t default 8; Thunder's Aurora wiring differs)
    #[serde(default = "default_light_pin")]
    pub light_pin: u16,
    /// the galvo's X is the design's Y (a head mounted a quarter turn round: the owner's Aurora marked the +X arrow
    /// pointing to the back, 2026-10-24); applied before the flips
    #[serde(default)]
    pub swap_xy: bool,
    /// UV sources that mark harder at lower values (MeerK40t's Invert Power: the ratio goes out for 100 − %)
    #[serde(default)]
    pub uv_invert: bool,
}

impl GalvoOptions {
    /// the CO2 / UV power ratio for `power` % at `freq_khz`
    fn ratio(&self, power: f64, freq_khz: f64) -> u16 {
        let p = power.clamp(0.0, 100.0);
        let p = if self.source == Source::Uv && self.uv_invert { 100.0 - p } else { p };
        (200.0 * p / freq_khz.max(0.1)).round() as u16
    }
}

fn default_light_pin() -> u16 {
    LIGHT_PIN
}

impl Default for GalvoOptions {
    fn default() -> Self {
        GalvoOptions {
            field_mm: 110.0,
            source: Source::Fiber,
            uv_invert: false,
            travel_speed: 2000.0,
            delay_on: 100.0,
            delay_off: 100.0,
            delay_polygon: 100.0,
            delay_jump: 8.0,
            delay_end: 300.0,
            delay_open_mo_ms: 8.0,
            flip_x: false,
            flip_y: false,
            first_pulse_killer: 200,
            pwm_pulse_width: 125,
            pwm_half_period: 125,
            standby: (2000, 20),
            timing_mode: 1,
            delay_mode: 1,
            laser_mode: 1,
            control_mode: 0,
            fix: LensFix::default(),
            light_pin: LIGHT_PIN,
            swap_xy: false,
        }
    }
}

impl GalvoOptions {
    pub fn galvos_per_mm(&self) -> f64 {
        65535.0 / self.field_mm.max(1.0)
    }
    /// design mm (the field centred on the bed's centre) → galvo units (0x8000 = centre); None when outside the field
    pub fn to_galvo(&self, p: Pt, bed: [f64; 2]) -> Option<(u16, u16)> {
        let g = self.galvos_per_mm();
        let (cx, cy) = self.fix.apply(p.x - bed[0] / 2.0, p.y - bed[1] / 2.0, self.field_mm.max(1.0) / 2.0);
        let (mut x, mut y) = if self.swap_xy { (cy * g, cx * g) } else { (cx * g, cy * g) };
        if self.flip_x {
            x = -x;
        }
        if !self.flip_y {
            y = -y; // galvo Y grows downward on an unflipped machine (EZCAD's view)
        }
        let (gx, gy) = (32768.0 + x, 32768.0 + y);
        if !(0.0..=65535.0).contains(&gx) || !(0.0..=65535.0).contains(&gy) {
            return None;
        }
        Some((gx.round() as u16, gy.round() as u16))
    }
    /// speed mm/s → galvo units per ms
    pub fn speed(&self, mm_s: f64) -> u16 {
        (mm_s * self.galvos_per_mm() / 1000.0).clamp(1.0, 65535.0) as u16
    }
}

/// One 12-byte command (single or list entry).
pub fn cmd(op: u16, v: [u16; 5]) -> [u8; 12] {
    let mut b = [0u8; 12];
    for (i, w) in [op, v[0], v[1], v[2], v[3], v[4]].iter().enumerate() {
        b[i * 2..i * 2 + 2].copy_from_slice(&w.to_le_bytes());
    }
    b
}
fn delay_words(us: f64) -> [u16; 5] {
    [us.abs().min(65535.0) as u16, if us > 0.0 { 0 } else { 0x8000 }, 0, 0, 0]
}

/// The singles sent once after connecting (galvoplotter `init_laser`): reset, the correction table (or a blank one),
/// enable, modes, timings. Each is answered with 8 bytes except the correction lines.
pub fn init_sequence(o: &GalvoOptions, cor: Option<&CorTable>) -> Vec<[u8; 12]> {
    let mut v = vec![cmd(GET_SERIAL_NO, [0; 5]), cmd(GET_VERSION, [0; 5]), cmd(RESET, [0; 5])];
    match cor {
        Some(t) if t.entries.len() == 65 * 65 => {
            v.push(cmd(WRITE_COR_TABLE, [1, 0, 0, 0, 0]));
            for (k, e) in t.entries.iter().enumerate() {
                v.push(cmd(WRITE_COR_LINE, [e[0], e[1], (k != 0) as u16, 0, 0]));
            }
        }
        _ => v.push(cmd(WRITE_COR_TABLE, [0; 5])),
    }
    v.push(cmd(ENABLE_LASER, [0; 5]));
    v.push(cmd(SET_CONTROL_MODE, [o.control_mode, 0, 0, 0, 0]));
    v.push(cmd(SET_LASER_MODE, [o.laser_mode, 0, 0, 0, 0]));
    v.push(cmd(SET_DELAY_MODE, [o.delay_mode, 0, 0, 0, 0]));
    v.push(cmd(SET_TIMING, [o.timing_mode, 0, 0, 0, 0]));
    v.push(cmd(SET_STANDBY, [o.standby.0, o.standby.1, 0, 0, 0]));
    v.push(cmd(SET_FIRST_PULSE_KILLER, [o.first_pulse_killer, 0, 0, 0, 0]));
    v.push(cmd(SET_PWM_HALF_PERIOD, [o.pwm_half_period, 0, 0, 0, 0]));
    v.push(cmd(SET_PWM_PULSE_WIDTH, [o.pwm_pulse_width, 0, 0, 0, 0]));
    if o.source == Source::Fiber {
        v.push(cmd(FIBER_SET_MO, [0; 5]));
    }
    v.push(cmd(SET_FPK_PARAM_2, [0xFFB, 1, 409, 100, 0]));
    v.push(cmd(SET_FLY_RES, [0, 99, 1000, 25, 0]));
    v.push(cmd(ENABLE_Z, [0; 5]));
    v.push(cmd(WRITE_ANALOG_PORT_1, [0x7FF, 0, 0, 0, 0]));
    v.push(cmd(ENABLE_Z, [0; 5]));
    v
}

/// A list under construction: 12-byte entries, packed into 3072-byte packets of 256 (the rest EndOfList).
#[derive(Default)]
pub struct ListBuilder {
    entries: Vec<[u8; 12]>,
    at: Option<(u16, u16)>,
}

impl ListBuilder {
    pub fn push(&mut self, op: u16, v: [u16; 5]) {
        self.entries.push(cmd(op, v));
    }
    fn dist(&self, x: u16, y: u16) -> u16 {
        self.at.map_or(0, |(ax, ay)| (((x as f64 - ax as f64).powi(2) + (y as f64 - ay as f64).powi(2)).sqrt()).min(65535.0) as u16)
    }
    pub fn jump(&mut self, x: u16, y: u16) {
        if self.at == Some((x, y)) {
            return;
        }
        let d = self.dist(x, y);
        self.push(LIST_JUMP_TO, [x, y, 0, d, 0]);
        self.at = Some((x, y));
    }
    pub fn mark(&mut self, x: u16, y: u16) {
        if self.at == Some((x, y)) {
            return;
        }
        let d = self.dist(x, y);
        self.push(LIST_MARK_TO, [x, y, 0, d, 0]);
        self.at = Some((x, y));
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// The 3072-byte packets: 256 commands each, only the last one padded with EndOfList (the board stops at the first
    /// EndOfList it meets, so a padded packet in the middle ended the owner's Aurora frame after one packet, 2026-10-24;
    /// galvoplotter pads the same way).
    pub fn packets(&self) -> Vec<Vec<u8>> {
        let eol = cmd(LIST_END_OF_LIST, [0; 5]);
        let mut out = Vec::new();
        for chunk in self.entries.chunks(256) {
            let mut p = Vec::with_capacity(3072);
            for e in chunk {
                p.extend_from_slice(e);
            }
            while p.len() < 3072 {
                p.extend_from_slice(&eol);
            }
            out.push(p);
        }
        if out.is_empty() {
            out.push(eol.repeat(256));
        }
        out
    }
}

/// Per-layer settings the galvo needs beyond speed / power.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerGalvo {
    /// kHz (fibre Q-switch / CO2 modulation)
    pub freq_khz: f64,
    /// fibre (MOPA) pulse width, ns; 0 = the source's default
    pub pulse_width: u16,
}

/// The job: one list builder with every layer's strokes, the beam settings written whenever they change.
/// Strokes outside the field are skipped. Fill / Image layers are vector scan lines here as well.
pub fn job_list(doc: &Document, plan: &Plan, o: &GalvoOptions, per_layer: &dyn Fn(usize) -> LayerGalvo) -> (ListBuilder, usize) {
    let mut l = ListBuilder::default();
    let mut skipped = 0;
    let mut port: u16 = 1 << LASER_PIN;
    l.push(LIST_READY, [0; 5]);
    if o.source == Source::Fiber {
        l.push(LIST_DELAY_TIME, [(o.delay_open_mo_ms * 100.0) as u16, 0, 0, 0, 0]);
    }
    l.push(LIST_WRITE_PORT, [port, 0, 0, 0, 0]);
    l.push(LIST_JUMP_SPEED, [o.speed(o.travel_speed), 0, 0, 0, 0]);
    l.push(LIST_JUMP_DELAY, delay_words(o.delay_jump));
    let mut last: Option<usize> = None;
    // the share of the layer's power in force (greyscale pictures, ramps, dot width): sent again only when it changes
    let mut share = 1.0f64;
    let power_cmd = |l: &mut ListBuilder, power: f64, freq_khz: f64| match o.source {
        Source::Fiber => l.push(LIST_MARK_CURRENT, [fibre_current(power), 0, 0, 0, 0]),
        Source::Co2 | Source::Uv => l.push(LIST_MARK_POWER_RATIO, [o.ratio(power, freq_khz), 0, 0, 0, 0]),
    };
    // a pause on the list: delay commands of at most 600 ms each (10 µs units)
    let pause = |l: &mut ListBuilder, ms: f64| {
        let mut left = ms.max(0.0);
        while left > 0.0 {
            let now = left.min(600.0);
            l.push(LIST_DELAY_TIME, [(now * 100.0).round() as u16, 0, 0, 0, 0]);
            left -= now;
        }
    };
    for s in &plan.strokes {
        if last != Some(s.layer) {
            if let Some(p) = last {
                pause(&mut l, doc.layers[p].pause_after);
            }
            share = 1.0;
            let layer = &doc.layers[s.layer];
            pause(&mut l, layer.pause_before);
            let g = per_layer(s.layer);
            match o.source {
                Source::Fiber => {
                    if g.pulse_width > 0 {
                        l.push(LIST_FIBER_YLPM_PULSE_WIDTH, [g.pulse_width, 0, 0, 0, 0]);
                    }
                    l.push(LIST_MARK_CURRENT, [fibre_current(layer.power), 0, 0, 0, 0]);
                    l.push(LIST_QSWITCH_PERIOD, [(20000.0 / g.freq_khz.max(0.1)).round() as u16, 0, 0, 0, 0]);
                }
                Source::Co2 | Source::Uv => {
                    l.push(LIST_MARK_FREQ, [(10000.0 / g.freq_khz.max(0.1)).round() as u16, 0, 0, 0, 0]);
                    l.push(LIST_SET_CO2_FPK, [(2000.0 / g.freq_khz.max(0.1)).round() as u16, (2000.0 / g.freq_khz.max(0.1)).round() as u16, 0, 0, 0]);
                    l.push(LIST_MARK_POWER_RATIO, [o.ratio(layer.power, g.freq_khz), 0, 0, 0, 0]);
                }
            }
            l.push(LIST_MARK_SPEED, [o.speed(layer.speed), 0, 0, 0, 0]);
            // the layer's own timings, else Machine Settings'
            let [on, off, poly, jump] = layer.timings.unwrap_or([o.delay_on, o.delay_off, o.delay_polygon, o.delay_jump]);
            l.push(LIST_LASER_ON_DELAY, delay_words(on));
            l.push(LIST_LASER_OFF_DELAY, delay_words(off));
            l.push(LIST_POLYGON_DELAY, delay_words(poly));
            l.push(LIST_JUMP_DELAY, delay_words(jump));
            last = Some(s.layer);
        }
        let pts: Vec<Option<(u16, u16)>> = s.pts.iter().map(|p| o.to_galvo(*p, doc.bed)).collect();
        if pts.iter().any(|p| p.is_none()) {
            skipped += 1;
            continue;
        }
        let (x0, y0) = pts[0].unwrap();
        l.jump(x0, y0);
        if s.dwell_ms > 0.0 {
            l.push(LIST_LASER_ON_POINT, [((s.dwell_ms as f64) * 100.0).min(60000.0) as u16, 0, 0, 0, 0]); // 10 µs units
            continue;
        }
        let layer = &doc.layers[s.layer];
        let grey = layer.mode == crate::Mode::Image && layer.dither == crate::raster::Dither::Grayscale && layer.power_min > 0.0;
        for (k, w) in s.pts.windows(2).enumerate() {
            let pw = s.pw.as_ref().and_then(|w| w.get(k)).copied().unwrap_or(1.0) as f64;
            let dead = pw <= 0.0;
            if !dead {
                // greyscale runs from Min power (white) to Power (black), as on G-code; anything else: × pw
                let want = if grey && layer.power > 0.0 { (layer.power_min.min(layer.power) + (layer.power - layer.power_min.min(layer.power)) * pw) / layer.power } else { pw };
                let want = (want * 256.0).round() / 256.0;
                if (want - share).abs() > 1e-9 {
                    power_cmd(&mut l, layer.power * want, per_layer(s.layer).freq_khz);
                    share = want;
                }
            }
            // under a bending correction a straight segment goes out in ≤ 0.5 mm pieces so it bends with the field
            let n = if o.fix.bends() && !dead { (crate::geom::dist(w[0], w[1]) / 0.5).ceil().max(1.0) as usize } else { 1 };
            for j in 1..=n {
                let t = j as f64 / n as f64;
                let q = crate::pt(w[0].x + (w[1].x - w[0].x) * t, w[0].y + (w[1].y - w[0].y) * t);
                let Some((x, y)) = (if j == n { pts[k + 1] } else { o.to_galvo(q, doc.bed) }) else { continue };
                if dead {
                    l.jump(x, y);
                } else {
                    l.mark(x, y);
                }
            }
        }
    }
    if let Some(p) = last {
        pause(&mut l, doc.layers[p].pause_after);
    }
    // the beam off at the end, the lens parked, a settle time
    port &= !(1 << LASER_PIN);
    l.push(LIST_WRITE_PORT, [port, 0, 0, 0, 0]);
    l.push(LIST_DELAY_TIME, [(o.delay_end * 100.0) as u16, 0, 0, 0, 0]);
    l.push(LIST_END_OF_LIST, [0; 5]);
    (l, skipped)
}

/// The Fire button on a galvo: the beam on where the mirrors point (`at`, galvo units) for `ms` at `power` %, then off.
pub fn fire_list(at: (u16, u16), o: &GalvoOptions, power: f64, freq_khz: f64, ms: f64) -> ListBuilder {
    let mut l = ListBuilder::default();
    l.push(LIST_READY, [0; 5]);
    if o.source == Source::Fiber {
        l.push(LIST_DELAY_TIME, [(o.delay_open_mo_ms * 100.0) as u16, 0, 0, 0, 0]);
    }
    l.push(LIST_WRITE_PORT, [1 << LASER_PIN, 0, 0, 0, 0]);
    match o.source {
        Source::Fiber => {
            l.push(LIST_MARK_CURRENT, [fibre_current(power), 0, 0, 0, 0]);
            l.push(LIST_QSWITCH_PERIOD, [(20000.0 / freq_khz.max(0.1)).round() as u16, 0, 0, 0, 0]);
        }
        Source::Co2 | Source::Uv => {
            l.push(LIST_MARK_FREQ, [(10000.0 / freq_khz.max(0.1)).round() as u16, 0, 0, 0, 0]);
            l.push(LIST_MARK_POWER_RATIO, [o.ratio(power, freq_khz), 0, 0, 0, 0]);
        }
    }
    l.jump(at.0, at.1);
    l.push(LIST_LASER_ON_POINT, [(ms * 100.0).clamp(1.0, 60000.0) as u16, 0, 0, 0, 0]);
    l.push(LIST_WRITE_PORT, [0; 5]);
    l.push(LIST_END_OF_LIST, [0; 5]);
    l
}

/// A framing list: the red pointer round the box, beam pin off, `loops` times.
pub fn frame_list(r: &crate::Rect, bed: [f64; 2], o: &GalvoOptions, loops: usize) -> ListBuilder {
    frame_path_list(&box_path(r), bed, o, loops)
}

/// the box's corners as a closed path
pub fn box_path(r: &crate::Rect) -> Vec<Pt> {
    [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1), (r.x0, r.y0)].iter().map(|c| crate::pt(c.0, c.1)).collect()
}

/// The red pointer round `pts` (the box, or the design's outline hull), beam pin off, `loops` times.
pub fn frame_path_list(pts: &[Pt], bed: [f64; 2], o: &GalvoOptions, loops: usize) -> ListBuilder {
    let mut l = ListBuilder::default();
    l.push(LIST_READY, [0; 5]);
    l.push(LIST_WRITE_PORT, [1 << o.light_pin.min(15), 0, 0, 0, 0]);
    l.push(LIST_JUMP_SPEED, [o.speed(o.travel_speed), 0, 0, 0, 0]);
    l.push(LIST_JUMP_DELAY, delay_words(o.delay_jump));
    // under a bending correction the sides go in 1 mm steps so the light shows them bent as the marks will be
    let mut path: Vec<Pt> = pts.first().copied().into_iter().collect();
    for w in pts.windows(2) {
        let n = if o.fix.bends() { crate::geom::dist(w[0], w[1]).ceil().max(1.0) as usize } else { 1 };
        path.extend((1..=n).map(|j| crate::pt(w[0].x + (w[1].x - w[0].x) * j as f64 / n as f64, w[0].y + (w[1].y - w[0].y) * j as f64 / n as f64)));
    }
    for _ in 0..loops.max(1) {
        for p in &path {
            if let Some((x, y)) = o.to_galvo(*p, bed) {
                l.jump(x, y);
            }
        }
    }
    l.push(LIST_WRITE_PORT, [0; 5]);
    l.push(LIST_END_OF_LIST, [0; 5]);
    l
}

/// The 4 u16 of a reply; the status word is the last.
pub fn reply_words(r: &[u8]) -> Option<[u16; 4]> {
    (r.len() >= 8).then(|| [u16::from_le_bytes([r[0], r[1]]), u16::from_le_bytes([r[2], r[3]]), u16::from_le_bytes([r[4], r[5]]), u16::from_le_bytes([r[6], r[7]])])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{plan, Machine};
    use crate::{geom, sample, Layer, Mode};

    fn cor_lmc1(field: f64) -> Vec<u8> {
        let mut b: Vec<u8> = "LMC1COR_1.0".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        b.extend([0u8; 2]);
        for i in 0..63 {
            b.extend((if i == 43 { field } else { 0.0 }).to_le_bytes());
        }
        assert_eq!(b.len(), 0x16 + 0x1FA);
        for k in 0..65 * 65 {
            b.extend((k as f64 % 7.0 - 3.0).to_le_bytes());
            b.extend(((k / 65) as f64).to_le_bytes());
        }
        b
    }

    #[test]
    fn correction_files_parse_in_both_formats() {
        let t = CorTable::parse(&cor_lmc1(65536.0 / 110.0)).unwrap();
        assert_eq!(t.entries.len(), 65 * 65);
        assert!((t.field_mm - 110.0).abs() < 1e-9);
        assert_eq!(t.entries[0], [0x8003, 0]); // -3 → sign bit, 0
        assert_eq!(t.entries[4], [1, 0]);
        let mut old: Vec<u8> = vec![0u8; 0x16];
        old.extend([0u8; 6]);
        old.extend(496.5798611111111f64.to_le_bytes()); // Thunder's Aurora 110X110.cor ("JCZ_COR_2_1")
        for k in 0..65 * 65i32 {
            old.extend((k % 5 - 2).to_le_bytes());
            old.extend(1i32.to_le_bytes());
        }
        let t2 = CorTable::parse(&old).unwrap();
        assert!((t2.field_mm - 131.97).abs() < 0.01);
        assert_eq!(t2.entries[0], [0x8002, 1]);
        assert!(CorTable::parse(&[0; 100]).is_err());
        // the init sequence carries every line
        let init = init_sequence(&GalvoOptions::default(), Some(&t));
        assert_eq!(init.iter().filter(|c| u16::from_le_bytes([c[0], c[1]]) == WRITE_COR_LINE).count(), 65 * 65);
        assert_eq!(init_sequence(&GalvoOptions::default(), None).iter().filter(|c| u16::from_le_bytes([c[0], c[1]]) == WRITE_COR_LINE).count(), 0);
    }

    #[test]
    fn cylinder_correction_pulls_the_sides_in() {
        let mut f = LensFix { cyl: Cylinder { on: true, diameter: 40.0, mirror_mm: 200.0, curve_x: true, concave: false }, ..Default::default() };
        let (x, y) = f.apply(10.0, 3.0, 55.0);
        // 10 mm round a 20 mm radius = 0.5 rad: 20 sin 0.5 × 200 / (200 + 20 (1 − cos 0.5))
        assert!((x - 9.4725).abs() < 1e-3 && (y - 3.0).abs() < 1e-12, "{x} {y}");
        let (bx, by) = f.unapply(x, y, 55.0);
        assert!((bx - 10.0).abs() < 1e-6 && (by - 3.0).abs() < 1e-6);
        assert!(f.bends());
        f.cyl.concave = true;
        assert!(f.apply(10.0, 0.0, 55.0).0 > 9.4725, "a bowl's sides come closer to the lens");
    }

    #[test]
    fn the_numbers_correction_moves_the_field_and_bends_long_lines() {
        let o = GalvoOptions { field_mm: 100.0, ..Default::default() };
        let bed = [100.0, 100.0];
        assert_eq!(o.to_galvo(crate::pt(50.0, 50.0), bed), Some((32768, 32768)), "the centre stays");
        // 2 % bigger in X: a point 40 mm right of the centre lands 0.8 mm further out
        let big = GalvoOptions { fix: LensFix { scale: [1.02, 1.0], ..Default::default() }, ..o.clone() };
        let (gx, _) = big.to_galvo(crate::pt(90.0, 50.0), bed).unwrap();
        assert!((gx as f64 - 32768.0 - 40.8 * big.galvos_per_mm()).abs() < 1.0);
        // a bulge leaves the centre line alone and moves the middle of the top edge sideways not at all, its ends out
        let bulge = GalvoOptions { fix: LensFix { bulge: [0.1, 0.0], ..Default::default() }, ..o.clone() };
        assert_eq!(bulge.to_galvo(crate::pt(50.0, 90.0), bed), o.to_galvo(crate::pt(50.0, 90.0), bed));
        assert!(bulge.to_galvo(crate::pt(90.0, 90.0), bed).unwrap().0 > o.to_galvo(crate::pt(90.0, 90.0), bed).unwrap().0);
        // a 20 mm line marks in pieces under a bulge, in one move without
        let mut d = Document { layers: vec![Layer::new("Mark", [0, 0, 0], Mode::Line, 500.0, 60.0)], ..Default::default() };
        d.bed = bed;
        d.add(geom::make_polyline(&[crate::pt(40.0, 80.0), crate::pt(60.0, 80.0)], false), 0);
        let p = plan(&d, Machine::default());
        let marks = |o: &GalvoOptions| {
            job_list(&d, &p, o, &|_| LayerGalvo { freq_khz: 30.0, pulse_width: 0 }).0.packets().concat().chunks(12).filter(|c| u16::from_le_bytes([c[0], c[1]]) == LIST_MARK_TO).count()
        };
        assert_eq!(marks(&o), 1);
        // the correction undone: back where it was
        let f = LensFix { scale: [1.02, 0.98], bulge: [0.05, -0.03], trapezoid: [0.02, 0.0], skew: 0.01, ..LensFix::default() };
        let (cx, cy) = f.apply(30.0, -20.0, 50.0);
        let (bx, by) = f.unapply(cx, cy, 50.0);
        assert!((bx - 30.0).abs() < 1e-6 && (by + 20.0).abs() < 1e-6, "{bx} {by}");
        assert_eq!(marks(&bulge), 40);
    }

    #[test]
    fn greyscale_and_ramps_change_the_power_along_a_stroke() {
        let o = GalvoOptions { field_mm: 110.0, ..Default::default() };
        let mut d = Document { layers: vec![Layer::new("Photo", [0, 0, 0], Mode::Image, 500.0, 80.0)], ..Default::default() };
        d.bed = [110.0, 110.0];
        (d.layers[0].dither, d.layers[0].interval) = (crate::raster::Dither::Grayscale, 1.0);
        let id = d.add(geom::make_rect(50.0, 50.0, 54.0, 51.0), 0);
        d.find_mut(id).unwrap().raster = Some(crate::raster::Raster::from_gray(4, 1, &[0, 80, 160, 220]));
        let p = plan(&d, Machine::default());
        let (l, _) = job_list(&d, &p, &o, &|_| LayerGalvo { freq_khz: 30.0, pulse_width: 0 });
        let currents: Vec<u16> = l.packets().concat().chunks(12).filter(|c| u16::from_le_bytes([c[0], c[1]]) == LIST_MARK_CURRENT).map(|c| u16::from_le_bytes([c[2], c[3]])).collect();
        assert!(currents.iter().all(|c| *c <= 0xFFF));
        assert_eq!((fibre_current(250.0), fibre_current(-5.0)), (0xFFF, 0), "power clamped to 0–100 %");
        let mut distinct = currents.clone();
        distinct.sort();
        distinct.dedup();
        assert!(distinct.len() >= 3, "power steps through the greys: {currents:?}");
        assert!(currents.iter().all(|c| *c <= (0.8 * 4095.0) as u16 + 1), "never over the layer's power");
    }

    #[test]
    fn a_job_becomes_lists_with_the_settings_and_moves_in_field_units() {
        let o = GalvoOptions { field_mm: 110.0, ..Default::default() };
        let mut d = Document { layers: vec![Layer::new("Mark", [0, 0, 0], Mode::Line, 500.0, 60.0)], ..Default::default() };
        d.bed = [110.0, 110.0];
        d.add(geom::make_rect(45.0, 45.0, 65.0, 65.0), 0); // 20 mm square round the centre
        let p = plan(&d, Machine::default());
        let (l, skipped) = job_list(&d, &p, &o, &|_| LayerGalvo { freq_khz: 30.0, pulse_width: 0 });
        assert_eq!(skipped, 0);
        let pk = l.packets();
        assert_eq!(pk.len(), 1);
        assert_eq!(pk[0].len(), 3072);
        let ops: Vec<u16> = pk[0].chunks(12).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(ops[0], LIST_READY);
        assert!(ops.contains(&LIST_MARK_CURRENT) && ops.contains(&LIST_QSWITCH_PERIOD) && ops.contains(&LIST_MARK_SPEED));
        assert_eq!(ops.iter().filter(|o| **o == LIST_MARK_TO).count(), 4);
        assert_eq!(ops.iter().filter(|o| **o == LIST_JUMP_TO).count(), 1);
        assert_eq!(*ops.last().unwrap(), LIST_END_OF_LIST);
        // the square's corners: ±10 mm from the centre = ±5958 galvo units
        let g = o.galvos_per_mm();
        let jump = pk[0].chunks(12).find(|c| u16::from_le_bytes([c[0], c[1]]) == LIST_JUMP_TO).unwrap();
        let (x, y) = (u16::from_le_bytes([jump[2], jump[3]]), u16::from_le_bytes([jump[4], jump[5]]));
        assert!(((x as f64 - 0x8000 as f64).abs() - 10.0 * g).abs() < 2.0, "{x}");
        assert!(((y as f64 - 0x8000 as f64).abs() - 10.0 * g).abs() < 2.0, "{y}");
        // power 60 % → current 0x999; speed 500 mm/s → 298 units/ms
        let cur = pk[0].chunks(12).find(|c| u16::from_le_bytes([c[0], c[1]]) == LIST_MARK_CURRENT).unwrap();
        assert_eq!(u16::from_le_bytes([cur[2], cur[3]]), (0.6 * 4095.0f64).round() as u16);
        assert_eq!(o.speed(500.0), 297);
        // a shape outside the field is skipped, not clipped
        d.add(geom::make_rect(1.0, 1.0, 5.0, 5.0), 0); // the field's corner: inside
        d.add(geom::make_rect(100.0, 100.0, 130.0, 130.0), 0); // past the field: skipped
        let p = plan(&d, Machine::default());
        let (_, skipped) = job_list(&d, &p, &o, &|_| LayerGalvo { freq_khz: 30.0, pulse_width: 0 });
        assert_eq!(skipped, 1);
        // the sample (90 mm coaster) fits a 110 mm field: many packets, all 3072 bytes
        let s = sample();
        let p = plan(&s, Machine::default());
        let (l, sk) = job_list(&s, &p, &GalvoOptions { field_mm: 400.0, ..Default::default() }, &|_| LayerGalvo { freq_khz: 30.0, pulse_width: 0 });
        assert_eq!(sk, 0);
        assert!(l.packets().len() > 5 && l.packets().iter().all(|p| p.len() == 3072));
        // only the last packet carries EndOfList: the board stops at the first one it meets
        let eol = cmd(LIST_END_OF_LIST, [0; 5]);
        let ps = l.packets();
        assert!(ps[..ps.len() - 1].iter().all(|p| !p.chunks(12).any(|c| c == eol)), "an EndOfList inside a middle packet");
        assert!(ps.last().unwrap().chunks(12).any(|c| c == eol));
        // CO2 source writes frequency / power ratio instead
        let (l2, _) = job_list(&s, &p, &GalvoOptions { source: Source::Co2, ..o.clone() }, &|_| LayerGalvo { freq_khz: 20.0, pulse_width: 0 });
        let ops2: Vec<u16> = l2.packets()[0].chunks(12).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert!(ops2.contains(&LIST_MARK_FREQ) && ops2.contains(&LIST_MARK_POWER_RATIO) && !ops2.contains(&LIST_MARK_CURRENT));
        // UV: CO2's commands; the one capture of EZCAD2 + UV (MeerK40t, from a Discord log) at 30 kHz, 36 %: frequency
        // 333, first pulse killer 67 / 67, power ratio 240; Invert power sends 100 − %
        let mut d1 = crate::Document { bed: [110.0, 110.0], ..Default::default() };
        d1.add(crate::geom::make_rect(50.0, 50.0, 10.0, 10.0), 0);
        d1.layers[0].power = 36.0;
        let p1 = plan(&d1, Machine::default());
        let words = |o: &GalvoOptions| -> Vec<(u16, u16, u16)> {
            let (l, _) = job_list(&d1, &p1, o, &|_| LayerGalvo { freq_khz: 30.0, pulse_width: 0 });
            l.packets()[0].chunks(12).map(|c| (u16::from_le_bytes([c[0], c[1]]), u16::from_le_bytes([c[2], c[3]]), u16::from_le_bytes([c[4], c[5]]))).collect()
        };
        let uv = GalvoOptions { source: Source::Uv, field_mm: 110.0, ..Default::default() };
        let w = words(&uv);
        assert!(w.contains(&(LIST_MARK_FREQ, 333, 0)) && w.contains(&(LIST_SET_CO2_FPK, 67, 67)) && w.contains(&(LIST_MARK_POWER_RATIO, 240, 0)), "{w:?}");
        assert!(!w.iter().any(|x| x.0 == LIST_MARK_CURRENT || x.0 == LIST_QSWITCH_PERIOD), "no fibre commands for UV");
        assert!(words(&GalvoOptions { uv_invert: true, ..uv.clone() }).contains(&(LIST_MARK_POWER_RATIO, 427, 0)), "64 % at 30 kHz");
        // fire: the beam pin on, one point for the time asked, then every pin off
        let fl = fire_list((32768, 32768), &o, 10.0, 30.0, 100.0);
        let fw: Vec<(u16, u16)> = fl.packets()[0].chunks(12).map(|c| (u16::from_le_bytes([c[0], c[1]]), u16::from_le_bytes([c[2], c[3]]))).collect();
        assert!(fw.contains(&(LIST_LASER_ON_POINT, 10000)) && fw.contains(&(LIST_MARK_CURRENT, 410)));
        let on = fw.iter().position(|w| w.0 == LIST_LASER_ON_POINT).unwrap();
        assert!(fw[on..].contains(&(LIST_WRITE_PORT, 0)), "the beam pin off after");
        // frame: light pin on, 5 corners per loop
        let f = frame_list(&crate::Rect { x0: 45.0, y0: 45.0, x1: 65.0, y1: 65.0 }, d.bed, &o, 3);
        let fo: Vec<u16> = f.packets()[0].chunks(12).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(fo.iter().filter(|o| **o == LIST_JUMP_TO).count(), 1 + 4 * 3);
        // an outline frame: a triangle's 3 sides per loop
        let tri = [crate::pt(50.0, 50.0), crate::pt(60.0, 50.0), crate::pt(55.0, 58.0), crate::pt(50.0, 50.0)];
        let t = frame_path_list(&tri, d.bed, &o, 2);
        let to: Vec<u16> = t.packets()[0].chunks(12).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(to.iter().filter(|o| **o == LIST_JUMP_TO).count(), 1 + 3 * 2);
        assert_eq!(reply_words(&[1, 0, 2, 0, 3, 0, 0x24, 0]), Some([1, 2, 3, 0x24]));
    }
}
