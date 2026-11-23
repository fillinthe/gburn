//! K40 lasers: the Lihuiyu M2 / M3 Nano board. It reads LHYMICRO-GL ("egv") text over a CH341 USB chip: direction
//! letters, step counts in mils (1/1000 in), D / U = laser down / up, a speed code per cut. Ported from MeerK40t's
//! `lihuiyu` driver and `laserspeed` (MIT, © 2021 meerk40t; THIRD_PARTY.md). Bytes only: the USB side is in the app.
//!
//! The board has no power control on the M2: the panel's knob sets the current. The beam only fires on a `D` inside
//! a cut (program mode); rapid moves (`I … S1P`) never carry one, so Frame and jogs are beam-off.

use crate::plan::Plan;
use crate::{Document, Pt};

pub const USB_VID: u16 = 0x1A86; // QinHeng (CH341A)
pub const USB_PID: u16 = 0x5512;
/// the board's answers (second byte of a status read)
pub const STATUS_OK: u8 = 206;
pub const STATUS_ERROR: u8 = 207;
pub const STATUS_FINISH: u8 = 236;
pub const STATUS_BUSY: u8 = 238;
pub const STATUS_POWER: u8 = 239;
pub const MILS_PER_MM: f64 = 1000.0 / 25.4;

const RIGHT: char = 'B';
const LEFT: char = 'T';
const TOP: char = 'L';
const BOTTOM: char = 'R';

/// A step count: `z` per 255, then a letter for 1–25, `|` + a letter for 26–51, or three digits for 52–254.
pub fn distance(v: u32) -> String {
    let mut s = "z".repeat((v / 255) as usize);
    let v = v % 255;
    if v >= 52 {
        s += &format!("{v:03}");
    } else if v >= 26 {
        s.push('|');
        s.push((b'a' + (v - 26) as u8) as char);
    } else if v > 0 {
        s.push((b'a' + (v - 1) as u8) as char);
    }
    s
}

fn encode16(v: i64) -> String {
    format!("{:03}{:03}", (v >> 8) & 0xFF_FFFF, v & 255)
}

fn accel_for(mm_s: f64) -> u8 {
    match mm_s {
        s if s <= 25.4 => 1,
        s if s <= 60.0 => 2,
        s if s < 127.0 => 3,
        _ => 4,
    }
}

/// MeerK40t's `get_code_from_speed` for the M2 board, vector cuts (no raster step). `suffix_c` None = as the board's
/// own software would (C below 7 mm/s); `limit` = its 240 mm/s cap (the stock software's 19.05 fallback).
pub fn speed_code_with(mm_s: f64, suffix_c: Option<bool>, limit: bool) -> String {
    let mm_s = if limit && mm_s > 240.0 { 19.05 } else { mm_s.max(0.01) };
    let accel = accel_for(mm_s);
    let suffix_c = suffix_c.unwrap_or(mm_s < 7.0);
    let (b, m) = match (suffix_c, accel) {
        (true, _) => (8.0, 12120.0 / 12.0),
        (false, 3) => (5632.0, 12120.0),
        (false, 4) => (6144.0, 12120.0),
        _ => (5120.0, 12120.0),
    };
    let period_ms = 25.4 / mm_s;
    let value = (65536.0 - (m * period_ms + b)).max(0.0) as i64;
    let step = ((mm_s.floor() as i64) + 1).min(128);
    let diag = (0.261_199_033_289 * m * period_ms / step as f64).clamp(0.0, 65535.0) as i64;
    let tail = if suffix_c { "C" } else { "" };
    let accel = if suffix_c { 1 } else { accel };
    format!("CV{}{accel}{step:03}{}{tail}", encode16(value), encode16(diag))
}

/// The speed code GBurn cuts with: MeerK40t's default (no "twitches": suffix C, no speed cap).
pub fn speed_code(mm_s: f64) -> String {
    speed_code_with(mm_s, Some(true), false)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Rapid,
    Program,
    Finish,
}

/// The encoder: MeerK40t's driver states (rapid / program / finished) and its direction bookkeeping, so a letter
/// goes out only when the board's direction changes.
pub struct Egv {
    pub out: String,
    state: State,
    /// where the head is, mils from the home corner (top left), Y down
    pub x: i64,
    pub y: i64,
    leftward: bool,
    topward: bool,
    x_on: bool,
    y_on: bool,
    horizontal: bool,
    laser: bool,
    speed: f64,
}

impl Default for Egv {
    fn default() -> Self {
        Egv { out: String::new(), state: State::Rapid, x: 0, y: 0, leftward: false, topward: false, x_on: false, y_on: false, horizontal: false, laser: false, speed: 0.0 }
    }
}

impl Egv {
    /// An encoder for a head standing at x, y (mils from home): every K40 move is relative to where it is.
    pub fn at(x: i64, y: i64) -> Egv {
        Egv { x, y, ..Default::default() }
    }
    fn is_left(&self) -> bool {
        self.x_on && !self.y_on && self.leftward
    }
    fn is_right(&self) -> bool {
        self.x_on && !self.y_on && !self.leftward
    }
    fn is_top(&self) -> bool {
        !self.x_on && self.y_on && self.topward
    }
    fn is_bottom(&self) -> bool {
        !self.x_on && self.y_on && !self.topward
    }

    pub fn rapid_mode(&mut self) {
        match self.state {
            State::Rapid => return,
            State::Finish => self.out += "S1P\n",
            State::Program => {
                self.out += "FNSE-\n"; // "-": the sender waits for the board to finish before going on
                self.laser = false;
            }
        }
        self.state = State::Rapid;
    }
    fn finished_mode(&mut self) {
        match self.state {
            State::Finish => return,
            State::Program => {
                self.out += "@NSE";
                self.laser = false;
            }
            State::Rapid => self.out += "I",
        }
        self.state = State::Finish;
    }
    /// Into a cut at `speed`, the board's directions declared for a first move of dx, dy.
    fn program_mode(&mut self, speed: f64, dx: i64, dy: i64) {
        if self.state == State::Program && speed == self.speed {
            return;
        }
        self.rapid_mode(); // a new speed only takes from outside a cut
        self.finished_mode();
        self.leftward = dx < 0;
        self.topward = dy <= 0;
        self.horizontal = dx.abs() >= dy.abs();
        self.speed = speed;
        self.out += &speed_code(speed);
        self.out.push('N');
        let xd = if self.leftward { LEFT } else { RIGHT };
        let yd = if self.topward { TOP } else { BOTTOM };
        if self.horizontal {
            (self.x_on, self.y_on) = (true, false);
            self.out.push(yd);
            self.out.push(xd);
        } else {
            (self.x_on, self.y_on) = (false, true);
            self.out.push(xd);
            self.out.push(yd);
        }
        self.out += "S1E";
        self.state = State::Program;
    }
    fn laser(&mut self, on: bool) {
        if self.laser != on {
            self.out.push(if on { 'D' } else { 'U' });
            self.laser = on;
        }
    }
    fn goto_xy(&mut self, dx: i64, dy: i64, on: Option<bool>) {
        let rapid = self.state != State::Program;
        if dx != 0 {
            self.x += dx;
            if dx > 0 {
                if !self.is_right() || rapid {
                    self.out.push(RIGHT);
                    self.leftward = false;
                }
            } else if !self.is_left() || rapid {
                self.out.push(LEFT);
                self.leftward = true;
            }
            (self.x_on, self.y_on) = (true, false);
            if let Some(on) = on {
                self.laser(on);
            }
            self.out += &distance(dx.unsigned_abs() as u32);
        }
        if dy != 0 {
            self.y += dy;
            if dy > 0 {
                if !self.is_bottom() || rapid {
                    self.out.push(BOTTOM);
                    self.topward = false;
                }
            } else if !self.is_top() || rapid {
                self.out.push(TOP);
                self.topward = true;
            }
            (self.x_on, self.y_on) = (false, true);
            if let Some(on) = on {
                self.laser(on);
            }
            self.out += &distance(dy.unsigned_abs() as u32);
        }
    }
    /// One straight, square or 45° run inside a cut.
    fn goto_octant(&mut self, dx: i64, dy: i64, on: bool) {
        if dx == 0 && dy == 0 {
            return;
        }
        if dx.abs() != dy.abs() {
            return self.goto_xy(dx, dy, Some(on));
        }
        (self.x_on, self.y_on) = (true, true);
        if dx > 0 && self.leftward {
            self.out.push(RIGHT);
            self.leftward = false;
        } else if dx < 0 && !self.leftward {
            self.out.push(LEFT);
            self.leftward = true;
        }
        if dy > 0 && self.topward {
            self.out.push(BOTTOM);
            self.topward = false;
        } else if dy < 0 && !self.topward {
            self.out.push(TOP);
            self.topward = true;
        }
        self.x += dx;
        self.y += dy;
        self.out.push('M');
        self.laser(on);
        self.out += &distance(dy.unsigned_abs() as u32);
    }

    /// A rapid move (beam off for sure) to an absolute point in mils.
    pub fn rapid_to(&mut self, x: i64, y: i64) {
        self.rapid_mode();
        let (dx, dy) = (x - self.x, y - self.y);
        if dx == 0 && dy == 0 {
            return;
        }
        self.out.push('I');
        self.goto_xy(dx, dy, None);
        self.out += "S1P\n";
    }
    /// A straight cut (or a laser-up move inside a cut) to an absolute point in mils: Bresenham steps grouped into
    /// runs the board can do in one command (square or 45°).
    pub fn line_to(&mut self, x: i64, y: i64, speed: f64, on: bool) {
        let (dx, dy) = (x - self.x, y - self.y);
        if dx == 0 && dy == 0 {
            return;
        }
        self.program_mode(speed, dx, dy);
        let (ax, ay) = (dx.abs(), dy.abs());
        let (sx, sy) = (dx.signum(), dy.signum());
        let mut err = ax - ay;
        let (mut cx, mut cy) = (0i64, 0i64);
        let mut run = (0i64, 0i64, 0i64); // step x, step y, count
        while cx != dx || cy != dy {
            let e2 = 2 * err;
            let (mut stx, mut sty) = (0, 0);
            if e2 > -ay && cx != dx {
                err -= ay;
                stx = sx;
            }
            if e2 < ax && cy != dy {
                err += ax;
                sty = sy;
            }
            if stx == 0 && sty == 0 {
                // the other axis is done: finish along the one left
                if cx != dx {
                    stx = sx;
                } else {
                    sty = sy;
                }
            }
            cx += stx;
            cy += sty;
            if (stx, sty) == (run.0, run.1) {
                run.2 += 1;
            } else {
                if run.2 > 0 {
                    self.goto_octant(run.0 * run.2, run.1 * run.2, on);
                }
                run = (stx, sty, 1);
            }
        }
        if run.2 > 0 {
            self.goto_octant(run.0 * run.2, run.1 * run.2, on);
        }
    }
}

pub struct K40Options {
    /// the bed in mm (the board's home is its top-left corner)
    pub bed: [f64; 2],
    /// back to the home corner after the job
    pub home_after: bool,
}

/// Design mm (Y up) → mils from the top-left home corner (Y down).
pub fn to_mils(p: Pt, bed_h: f64) -> (i64, i64) {
    ((p.x * MILS_PER_MM).round() as i64, ((bed_h - p.y) * MILS_PER_MM).round() as i64)
}

/// Jumps shorter than this stay inside the cut with the laser up (no stop and start of the board's program).
const SHORT_JUMP_MILS: i64 = 400;

/// The whole job as egv text, from a head at `from` (mils), and where it ends. Power comes from the machine's knob,
/// so strokes on layers at 0 % are left out and the rest burn wherever the plan's power share is over 30 %
/// (MeerK40t's raster rule).
pub fn job_egv(doc: &Document, plan: &Plan, o: &K40Options, from: (i64, i64)) -> (String, (i64, i64)) {
    let mut e = Egv::at(from.0, from.1);
    for st in &plan.strokes {
        let l = &doc.layers[st.layer];
        if l.power <= 0.0 || st.pts.len() < 2 {
            continue;
        }
        let speed = l.speed.max(0.1);
        let (x0, y0) = to_mils(st.pts[0], o.bed[1]);
        let near = (x0 - e.x).abs().max((y0 - e.y).abs()) < SHORT_JUMP_MILS;
        if e.state == State::Program && e.speed == speed && near {
            e.line_to(x0, y0, speed, false);
        } else {
            e.rapid_to(x0, y0);
        }
        if st.dwell_ms > 0.0 {
            // ponytail: a dot is a 2-mil burn (the board's dwell needs host-timed waits)
            e.line_to(x0 + 2, y0, speed, true);
            continue;
        }
        for (k, p) in st.pts[1..].iter().enumerate() {
            let w = st.pw.as_ref().and_then(|w| w.get(k)).copied().unwrap_or(1.0);
            let (x, y) = to_mils(*p, o.bed[1]);
            e.line_to(x, y, speed, w > 0.3);
        }
    }
    if o.home_after {
        e.rapid_to(0, 0);
    }
    e.rapid_mode();
    (e.out, (e.x, e.y))
}

/// The same job as a .egv file (MeerK40t / K40 Whisperer open it).
pub fn egv_file(job: &str) -> String {
    format!("Document type : LHYMICRO-GL file\nFile version: 1.0.01\nCopyright: Unknown\nCreator-Software: GBurn\n\n%0%0%0%0%\n{job}")
}

const CRC_TABLE: [u8; 32] =
    [0x00, 0x5E, 0xBC, 0xE2, 0x61, 0x3F, 0xDD, 0x83, 0xC2, 0x9C, 0x7E, 0x20, 0xA3, 0xFD, 0x1F, 0x41, 0x00, 0x9D, 0x23, 0xBE, 0x46, 0xDB, 0x65, 0xF8, 0x8C, 0x11, 0xAF, 0x32, 0xCA, 0x57, 0xE9, 0x74];

/// The board's 8-bit one-wire CRC (MeerK40t's lookup, after lentz.com.au; BSD).
pub fn crc(line: &[u8]) -> u8 {
    line.iter().fold(0u8, |c, &b| {
        let c = c ^ b;
        CRC_TABLE[(c & 0x0F) as usize] ^ CRC_TABLE[16 + ((c >> 4) & 0x0F) as usize]
    })
}

/// One USB write: 30 command bytes padded with `F`, framed for the CH341 in EPP mode (A6 00 · 30 bytes · A6 crc).
#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    pub bytes: Vec<u8>,
    /// after it, wait until the board reports it finished (the `-` of `FNSE-`)
    pub wait_finish: bool,
}

pub fn frame(payload: &[u8]) -> Packet {
    let mut p = payload.to_vec();
    p.resize(30, b'F');
    let c = crc(&p);
    let mut bytes = vec![0xA6, 0x00];
    bytes.extend_from_slice(&p);
    bytes.extend_from_slice(&[0xA6, c]);
    Packet { bytes, wait_finish: false }
}

/// egv text → packets the way MeerK40t's controller cuts its queue: up to 30 bytes or to a line end.
pub fn packets(egv: &str) -> Vec<Packet> {
    let b = egv.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let rest = &b[i..];
        let mut len = rest.iter().take(30).position(|&c| c == b'\n').map_or(rest.len().min(30), |n| n + 1);
        if rest[len - 1] == b'-' && rest.get(len) == Some(&b'\n') {
            len += 1; // keep a wait mark with its line end
        }
        let mut p = rest[..len].to_vec();
        i += len;
        let mut wait = false;
        if p.last() == Some(&b'\n') {
            p.pop();
            if p.last() == Some(&b'-') {
                p.pop();
                wait = true;
            }
            if p.is_empty() && !wait {
                p.push(b'F');
            }
        }
        if p.is_empty() {
            if let Some(last) = out.last_mut() {
                let last: &mut Packet = last;
                last.wait_finish |= wait;
            }
            continue;
        }
        let mut pk = frame(&p);
        pk.wait_finish = wait;
        out.push(pk);
    }
    out
}

/// What a decoded job does: burnt segments (mils, from the home corner) and the head's end point.
#[derive(Default, Debug)]
pub struct Decoded {
    pub burns: Vec<[(i64, i64); 2]>,
    pub end: (i64, i64),
    pub speeds: Vec<String>,
}

/// Reads egv text back (the read-back check): direction letters, step counts, M diagonals, D / U.
pub fn decode(egv: &str) -> Decoded {
    let c: Vec<char> = egv.chars().collect();
    let mut d = Decoded::default();
    let (mut x, mut y) = (0i64, 0i64);
    let (mut xs, mut ys) = (1i64, 1i64);
    let mut axis = 'x';
    let mut on = false;
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        let mut steps: Option<i64> = None;
        match ch {
            'B' | 'T' => {
                xs = if ch == 'B' { 1 } else { -1 };
                axis = 'x';
            }
            'R' | 'L' => {
                ys = if ch == 'R' { 1 } else { -1 };
                axis = 'y';
            }
            'M' => axis = 'm',
            'D' => on = true,
            'U' | 'I' | 'F' | '@' => on = false,
            'S' => i += 1, // S1P / S1E / S2P: skip its digit
            'C' if c.get(i + 1) == Some(&'V') => {
                let start = i;
                i += 2;
                while i < c.len() && c[i].is_ascii_digit() {
                    i += 1;
                }
                if c.get(i) == Some(&'C') {
                    i += 1;
                }
                d.speeds.push(c[start..i].iter().collect());
                continue;
            }
            'z' => steps = Some(255),
            'a'..='y' => steps = Some(ch as i64 - 'a' as i64 + 1),
            '|' if i + 1 < c.len() => {
                i += 1;
                steps = Some(c[i] as i64 - 'a' as i64 + 26);
            }
            '0'..='9' if i + 2 < c.len() => {
                steps = c[i..i + 3].iter().collect::<String>().parse().ok();
                i += 2;
            }
            _ => {}
        }
        if let Some(n) = steps {
            let (nx, ny) = match axis {
                'x' => (x + xs * n, y),
                'y' => (x, y + ys * n),
                _ => (x + xs * n, y + ys * n),
            };
            if on {
                d.burns.push([(x, y), (nx, ny)]);
            }
            (x, y) = (nx, ny);
        }
        i += 1;
    }
    d.end = (x, y);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_codes_match_the_boards_own_software() {
        // from MeerK40t's test_laser_speeds (the stock Chinese software's codes, M2 board): value ± 1, gear, step,
        // diagonal ± 1
        for (want, mm_s) in [
            ("CV1081541001038126C", 0.68),
            ("CV1531831001026182C", 0.98),
            ("CV0640541008005155", 7.0),
            ("CV2142322058000024", 57.0),
            ("CV2162113071000016", 70.0),
            ("CV2222223109000006", 108.0),
            ("CV2240783125000005", 124.0),
            ("CV2231374128000004", 142.0),
            ("CV2242384128000003", 170.0),
            ("CV1722241020000211", 249.0),
        ] {
            let got = speed_code_with(mm_s, None, true);
            let v = |s: &str, a: usize| s[a..a + 3].parse::<i64>().unwrap() * 256 + s[a + 3..a + 6].parse::<i64>().unwrap();
            assert!((v(&got, 2) - v(want, 2)).abs() <= 1, "{mm_s}: {got} vs {want}");
            assert_eq!(&got[8..12], &want[8..12], "{mm_s}: gear + step {got} vs {want}");
            assert!((v(&got, 12) - v(want, 12)).abs() <= 1, "{mm_s}: diagonal {got} vs {want}");
            assert_eq!(got.ends_with('C'), want.ends_with('C'));
        }
    }

    #[test]
    fn a_square_encodes_like_meerk40t() {
        // MeerK40t's test_drivers_lihuiyu egv_rect: a 1 cm square at 2 cm, 2 cm (from the top left), 15 mm/s
        let mut e = Egv::default();
        let mm = |v: f64| (v * MILS_PER_MM).round() as i64;
        e.rapid_to(mm(20.0), mm(20.0));
        for (x, y) in [(30.0, 20.0), (30.0, 30.0), (20.0, 30.0), (20.0, 20.0)] {
            e.line_to(mm(x), mm(y), 15.0, true);
        }
        e.rapid_mode();
        assert_eq!(e.out, "IBzzzvRzzzvS1P\nICV2490731016000027CNLBS1EDz139Rz139Tz139Lz139FNSE-\n");
        let pk = packets(&e.out);
        assert_eq!(pk[0].bytes.len(), 34);
        assert_eq!(&pk[0].bytes[..2], &[0xA6, 0x00]);
        assert_eq!(&pk[0].bytes[2..16], b"IBzzzvRzzzvS1P");
        assert!(pk[0].bytes[16..32].iter().all(|&b| b == b'F'));
        assert_eq!(pk[0].bytes[32], 0xA6);
        assert_eq!(pk[0].bytes[33], crc(&pk[0].bytes[2..32]));
        assert!(!pk[0].wait_finish && pk.last().unwrap().wait_finish, "the cut ends with a wait for the board");
        let text: String = pk.iter().map(|p| String::from_utf8_lossy(&p.bytes[2..32]).trim_end_matches('F').to_string()).collect();
        assert_eq!(text, "IBzzzvRzzzvS1PICV2490731016000027CNLBS1EDz139Rz139Tz139Lz139FNSE", "packets keep every command in order");
        let burnt: i64 = decode(&e.out).burns.iter().map(|[a, b]| (b.0 - a.0).abs() + (b.1 - a.1).abs()).sum();
        assert_eq!(burnt, 4 * 394, "four 1 cm sides burnt");
        assert_eq!(distance(0), "");
        assert_eq!(distance(25), "y");
        assert_eq!(distance(26), "|a");
        assert_eq!(distance(51), "|z");
        assert_eq!(distance(52), "052");
        assert_eq!(distance(510), "zz");
    }

    #[test]
    fn a_job_reads_back_to_the_plan() {
        let d = crate::sample();
        let job = d.for_job();
        let plan = crate::plan::plan(&job, crate::plan::Machine::default());
        let o = K40Options { bed: d.bed, home_after: true };
        let (egv, end) = job_egv(&job, &plan, &o, (0, 0));
        assert_eq!(end, (0, 0));
        let back = decode(&egv);
        // every burnt point lies on the bed, the head ends at home, and the burnt extents match the plan's
        let (mut lo, mut hi) = ((i64::MAX, i64::MAX), (i64::MIN, i64::MIN));
        for [a, b] in &back.burns {
            for p in [a, b] {
                (lo.0, lo.1, hi.0, hi.1) = (lo.0.min(p.0), lo.1.min(p.1), hi.0.max(p.0), hi.1.max(p.1));
            }
        }
        let (mut plo, mut phi) = ((i64::MAX, i64::MAX), (i64::MIN, i64::MIN));
        for st in plan.strokes.iter().filter(|s| job.layers[s.layer].power > 0.0) {
            for (k, w) in st.pts.windows(2).enumerate() {
                if st.pw.as_ref().and_then(|v| v.get(k)).copied().unwrap_or(1.0) > 0.3 {
                    for p in w {
                        let m = to_mils(*p, d.bed[1]);
                        (plo.0, plo.1, phi.0, phi.1) = (plo.0.min(m.0), plo.1.min(m.1), phi.0.max(m.0), phi.1.max(m.1));
                    }
                }
            }
        }
        assert!((lo.0 - plo.0).abs() <= 1 && (lo.1 - plo.1).abs() <= 1 && (hi.0 - phi.0).abs() <= 1 && (hi.1 - phi.1).abs() <= 1, "{lo:?} {hi:?} vs {plo:?} {phi:?}");
        assert_eq!(back.end, (0, 0));
        let burnt_mm: f64 = back.burns.iter().map(|[a, b]| (((b.0 - a.0) as f64).hypot((b.1 - a.1) as f64)) / MILS_PER_MM).sum();
        assert!((burnt_mm / plan.cut_mm - 1.0).abs() < 0.08, "{burnt_mm} vs {}", plan.cut_mm);
        // one speed code per layer speed met, every packet whole
        assert!(back.speeds.iter().all(|s| s.starts_with("CV") && s.ends_with('C')));
        assert!(packets(&egv).iter().all(|p| p.bytes.len() == 34 && p.bytes[33] == crc(&p.bytes[2..32])));
        assert!(egv_file(&egv).starts_with("Document type : LHYMICRO-GL file\n"));
    }
}
