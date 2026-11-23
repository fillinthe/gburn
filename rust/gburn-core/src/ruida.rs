//! Ruida (RDC644x: Thunder Nova and most CO2 lasers). The command stream a controller runs: as a `.rd` file (USB
//! stick) or sent over UDP in acknowledged packets. Protocol from the public notes (EduTech wiki) and MeerK40t's
//! `rdjob.py` (MIT, see THIRD_PARTY.md), checked against a LightBurn `.rd` from the owner's archive
//! (`rust/testdata/sample.rd`): same magic (0x88), same header, layer and tail sequence, same file-sum rule.
//!
//! Units: the controller works in µm; (0, 0) is the machine's home corner, Y growing away from it (toward the
//! front on a rear-homing machine). `RdOptions::home` says which corner of the bed that is, and the design's
//! bottom-left Y-up millimetres are mapped onto it.

use crate::plan::Plan;
use crate::{Document, Pt};

// ---- byte level -------------------------------------------------------------------------------------------------

/// MeerK40t's swizzle (`rdjob.py`), magic 0x88 for 644XG / 644XS / 320 / 633X / 654XG; 0x11 for 634XG; 0x38 for RDL9635.
pub fn swizzle_byte(b: u8, magic: u8) -> u8 {
    let mut b = b;
    b ^= b >> 7;
    b ^= b << 7;
    b ^= b >> 7;
    b ^= magic;
    b.wrapping_add(1)
}

pub fn unswizzle_byte(b: u8, magic: u8) -> u8 {
    let mut b = b.wrapping_sub(1);
    b ^= magic;
    b ^= b >> 7;
    b ^= b << 7;
    b ^= b >> 7;
    b
}

pub fn swizzle(data: &[u8], magic: u8) -> Vec<u8> {
    data.iter().map(|&b| swizzle_byte(b, magic)).collect()
}
pub fn unswizzle(data: &[u8], magic: u8) -> Vec<u8> {
    data.iter().map(|&b| unswizzle_byte(b, magic)).collect()
}

/// The magic a swizzled stream was made with: the one that turns its first byte into a command (≥ 0x80) and most
/// of the rest into data bytes (< 0x80). A `.rd` starts with D8 (ref point); UDP replies with DA or CC.
pub fn guess_magic(data: &[u8]) -> Option<u8> {
    [0x88u8, 0x11, 0x38].into_iter().find(|m| {
        let u = unswizzle(&data[..data.len().min(64)], *m);
        !u.is_empty() && u[0] >= 0x80 && u.iter().filter(|b| **b < 0x80).count() * 2 > u.len()
    })
}

/// 16-bit big-endian sum of the bytes as sent (swizzled), in front of each UDP packet.
pub fn checksum(data: &[u8]) -> [u8; 2] {
    let s: u32 = data.iter().map(|&b| b as u32).sum();
    [((s >> 8) & 0xFF) as u8, (s & 0xFF) as u8]
}

/// A coordinate in µm as five 7-bit bytes (35 bits, two's complement), most significant first.
pub fn encode35(v: i64) -> [u8; 5] {
    let v = v as u64 & 0x7_FFFF_FFFF;
    [((v >> 28) & 0x7F) as u8, ((v >> 21) & 0x7F) as u8, ((v >> 14) & 0x7F) as u8, ((v >> 7) & 0x7F) as u8, (v & 0x7F) as u8]
}
pub fn decode35(b: &[u8]) -> i64 {
    let mut v: u64 = 0;
    for x in &b[..5] {
        v = (v << 7) | (*x as u64 & 0x7F);
    }
    if v & (1 << 34) != 0 {
        v as i64 - (1 << 35)
    } else {
        v as i64
    }
}
/// A 14-bit value as two 7-bit bytes (power: 0..=0x3FFF = 0..100 %; relative moves: signed µm, ±8191).
pub fn encode14(v: i32) -> [u8; 2] {
    let v = v as u32 & 0x3FFF;
    [((v >> 7) & 0x7F) as u8, (v & 0x7F) as u8]
}
pub fn decode14(b: &[u8]) -> i32 {
    let v = (((b[0] & 0x7F) as i32) << 7) | (b[1] & 0x7F) as i32;
    if v > 0x1FFF {
        v - 0x4000
    } else {
        v
    }
}
fn power14(percent: f64) -> [u8; 2] {
    encode14((percent.clamp(0.0, 100.0) * 16383.0 / 100.0) as i32)
}

// ---- commands -----------------------------------------------------------------------------------------------------

pub const ACK: u8 = 0xCC;
pub const ERR: u8 = 0xCD;
pub const ENQ: u8 = 0xCE;
pub const NAK: u8 = 0xCF;

pub const MEM_CARD_ID: u16 = 0x057E;
pub const MEM_MACHINE_STATUS: u16 = 0x0400;
pub const MEM_CURRENT_X: u16 = 0x0421;
pub const MEM_CURRENT_Y: u16 = 0x0431;
pub const MEM_BED_X: u16 = 0x0026;
pub const MEM_BED_Y: u16 = 0x0036;
/// LightBurn's Get Controller Info: usage counters (addresses and names from MeerK40t's Ruida emulator, MIT;
/// units as it names them, unverified on a real controller)
pub const MEM_INFO: [(u16, &str); 7] = [
    (0x0201, "Total time on (s)"),
    (0x0202, "Total job time (s)"),
    (0x0203, "Jobs run"),
    (0x0208, "Previous job time"),
    (0x0211, "Total laser-on time"),
    (0x0223, "X total travel (m)"),
    (0x0233, "Y total travel (m)"),
];
pub const STATUS_JOB_RUNNING: i64 = 0x0000_0001;
pub const STATUS_MOVING: i64 = 0x0100_0000;

/// Which bed corner the controller calls (0, 0): where the machine homes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum Home {
    #[default]
    RearLeft,
    RearRight,
    FrontLeft,
    FrontRight,
}

impl Home {
    pub const ALL: [(Home, &'static str); 4] = [(Home::RearLeft, "rear left (Thunder, most CO2)"), (Home::RearRight, "rear right"), (Home::FrontLeft, "front left"), (Home::FrontRight, "front right")];
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RdOptions {
    pub magic: u8,
    pub home: Home,
    /// bed size, mm
    pub bed: [f64; 2],
    /// None = absolute machine coordinates; Some(p) = "start from the current position": the head stands at
    /// design point `p` when the job starts (LightBurn's file in testdata does this: D8 12 + coordinates relative to it)
    pub current: Option<Pt>,
    /// rotary: design Y × this before the bed mapping (1 = off)
    pub y_scale: f64,
    /// with `current`: D8 11 (the controller's own anchor / user origin, set on its panel) instead of D8 12 (where the head is)
    pub anchor: bool,
}

impl Default for RdOptions {
    fn default() -> Self {
        RdOptions { magic: 0x88, home: Home::RearLeft, bed: [600.0, 400.0], current: None, y_scale: 1.0, anchor: false }
    }
}

impl RdOptions {
    /// design mm (Y up, bottom-left origin) → machine µm
    pub fn to_machine(&self, p: Pt) -> (i64, i64) {
        let p = crate::pt(p.x, p.y * if self.y_scale > 0.0 { self.y_scale } else { 1.0 });
        let x = match self.home {
            Home::RearLeft | Home::FrontLeft => p.x,
            _ => self.bed[0] - p.x,
        };
        let y = match self.home {
            Home::RearLeft | Home::RearRight => self.bed[1] - p.y,
            _ => p.y,
        };
        ((x * 1000.0).round() as i64, (y * 1000.0).round() as i64)
    }
    pub fn to_design(&self, x_um: i64, y_um: i64) -> Pt {
        let (x, y) = (x_um as f64 / 1000.0, y_um as f64 / 1000.0);
        crate::pt(
            match self.home {
                Home::RearLeft | Home::FrontLeft => x,
                _ => self.bed[0] - x,
            },
            match self.home {
                Home::RearLeft | Home::RearRight => self.bed[1] - y,
                _ => y,
            } / if self.y_scale > 0.0 { self.y_scale } else { 1.0 },
        )
    }
}

/// The unswizzled command stream, built command by command.
#[derive(Default)]
pub struct RdJob {
    pub bytes: Vec<u8>,
    at: Option<(i64, i64)>,
}

impl RdJob {
    fn cmd(&mut self, parts: &[&[u8]]) {
        for p in parts {
            self.bytes.extend_from_slice(p);
        }
    }
    fn coord(&mut self, op: &[u8], x: i64, y: i64) {
        self.cmd(&[op, &encode35(x), &encode35(y)]);
    }
    fn part_coord(&mut self, op: &[u8], part: u8, x: i64, y: i64) {
        self.cmd(&[op, &[part], &encode35(x), &encode35(y)]);
    }
    /// travel with the beam off; absolute the first time and for long hops, relative otherwise (rdjob.py `jump`)
    pub fn jump(&mut self, x: i64, y: i64) {
        match self.at {
            Some((ax, ay)) if (x - ax).abs() <= 8191 && (y - ay).abs() <= 8191 => {
                let (dx, dy) = ((x - ax) as i32, (y - ay) as i32);
                if dx == 0 && dy == 0 {
                    return;
                } else if dy == 0 {
                    self.cmd(&[&[0x8A], &encode14(dx)]);
                } else if dx == 0 {
                    self.cmd(&[&[0x8B], &encode14(dy)]);
                } else {
                    self.cmd(&[&[0x89], &encode14(dx), &encode14(dy)]);
                }
            }
            _ => self.coord(&[0x88], x, y),
        }
        self.at = Some((x, y));
    }
    /// cut to a point (beam on at the layer's power)
    pub fn mark(&mut self, x: i64, y: i64) {
        match self.at {
            Some((ax, ay)) if (x - ax).abs() <= 8191 && (y - ay).abs() <= 8191 => {
                let (dx, dy) = ((x - ax) as i32, (y - ay) as i32);
                if dx == 0 && dy == 0 {
                    return;
                } else if dy == 0 {
                    self.cmd(&[&[0xAA], &encode14(dx)]);
                } else if dx == 0 {
                    self.cmd(&[&[0xAB], &encode14(dy)]);
                } else {
                    self.cmd(&[&[0xA9], &encode14(dx), &encode14(dy)]);
                }
            }
            _ => self.coord(&[0xA8], x, y),
        }
        self.at = Some((x, y));
    }
    /// sum of every byte so far (the file sum the controller checks)
    pub fn sum(&self) -> i64 {
        self.bytes.iter().map(|b| *b as i64).sum()
    }
}

/// Bounds in machine µm of every stroke of layer `li` (None when it has none).
fn layer_bounds(plan: &Plan, o: &RdOptions, li: usize) -> Option<[i64; 4]> {
    let mut r: Option<[i64; 4]> = None;
    for s in plan.strokes.iter().filter(|s| s.layer == li) {
        for p in &s.pts {
            let (x, y) = o.to_machine(*p);
            r = Some(match r {
                None => [x, y, x, y],
                Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
            });
        }
    }
    r
}

/// The job as the controller runs it (unswizzled). Layers in plan order, one Ruida "part" each; every stroke a jump
/// then marks. Fill and Image layers go as vector scan lines (the controller's raster mode is not used).
/// [min 1, max 1, min 2, max 2] power % for a layer: both heads alike (a single-head machine ignores laser 2), or one
/// head with the other at 0 (`Layer.head`)
fn head_powers(l: &crate::Layer) -> [f64; 4] {
    let pmin = if l.power_min > 0.0 { l.power_min.min(l.power) } else { l.power };
    match l.head {
        1 => [pmin, l.power, 0.0, 0.0],
        2 => [0.0, 0.0, pmin, l.power],
        _ => [pmin, l.power, pmin, l.power],
    }
}

pub fn rd_commands(doc: &Document, plan: &Plan, o: &RdOptions) -> Vec<u8> {
    let mut j = RdJob::default();
    // coordinates: machine µm, or relative to the head's start point (negative-going, like LightBurn's file)
    let origin = o.current.map(|p| o.to_machine(p)).unwrap_or((0, 0));
    let m = |p: Pt| {
        let (x, y) = o.to_machine(p);
        (x - origin.0, y - origin.1)
    };
    let layers: Vec<usize> = {
        let mut v: Vec<usize> = Vec::new();
        for s in &plan.strokes {
            if !v.contains(&s.layer) {
                v.push(s.layer);
            }
        }
        v
    };
    // whole-job bounds
    let mut all: Option<[i64; 4]> = None;
    for li in &layers {
        if let Some([x0, y0, x1, y1]) = layer_bounds(plan, o, *li) {
            let b = [x0 - origin.0, y0 - origin.1, x1 - origin.0, y1 - origin.1];
            all = Some(match all {
                None => b,
                Some(a) => [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])],
            });
        }
    }
    let [x0, y0, x1, y1] = all.unwrap_or([0; 4]);
    // ---- header (LightBurn's order, rdjob.py write_header)
    j.cmd(&[if o.current.is_some() {
        if o.anchor {
            &[0xD8, 0x11]
        } else {
            &[0xD8, 0x12]
        }
    } else {
        &[0xD8, 0x10]
    }]); // ref point: anchor / current position / machine zero
    j.cmd(&[&[0xF0]]); // ref point set
    j.cmd(&[&[0xF1, 0x02, 0x00]]); // block cutting off
    j.cmd(&[&[0xD8, 0x00]]); // start process
    j.cmd(&[&[0xE7, 0x06], &[0; 10]]); // feed repeat 0 0
    j.cmd(&[&[0xE7, 0x38, 0x00]]); // feed auto pause off
    j.coord(&[0xE7, 0x03], x0, y0); // process top-left
    j.coord(&[0xE7, 0x07], x1, y1); // process bottom-right
    j.coord(&[0xE7, 0x50], x0, y0); // document min
    j.coord(&[0xE7, 0x51], x1, y1); // document max
    j.cmd(&[&[0xE7, 0x04, 0x00, 0x01, 0x00, 0x01], &[0; 10]]); // process repeat 1 × 1
    j.cmd(&[&[0xE7, 0x05, 0x00]]); // array direction
    let part_no = |i: usize| i.min(0x7F) as u8; // 7-bit bytes (0x80+ = a command): past 128 layers they share part 127
    for (part, li) in layers.iter().enumerate() {
        let l = &doc.layers[*li];
        let part = part_no(part);
        j.cmd(&[&[0xC9, 0x04, part], &encode35((l.speed * 1000.0) as i64)]); // speed, µm/s
        let [a1, b1, a2, b2] = head_powers(l);
        j.cmd(&[&[0xC6, 0x31, part], &power14(a1)]); // min power 1
        j.cmd(&[&[0xC6, 0x32, part], &power14(b1)]); // max power 1
        j.cmd(&[&[0xC6, 0x41, part], &power14(a2)]); // min power 2
        j.cmd(&[&[0xC6, 0x42, part], &power14(b2)]); // max power 2
        let color = (l.color[2] as i64) << 16 | (l.color[1] as i64) << 8 | l.color[0] as i64; // BGR
        j.cmd(&[&[0xCA, 0x06, part], &encode35(color)]);
        j.cmd(&[&[0xCA, 0x41, part, 0x02]]); // work mode (as LightBurn writes it)
        if let Some([bx0, by0, bx1, by1]) = layer_bounds(plan, o, *li) {
            let (bx0, by0, bx1, by1) = (bx0 - origin.0, by0 - origin.1, bx1 - origin.0, by1 - origin.1);
            j.part_coord(&[0xE7, 0x52], part, bx0, by0);
            j.part_coord(&[0xE7, 0x53], part, bx1, by1);
            j.part_coord(&[0xE7, 0x61], part, bx0, by0);
            j.part_coord(&[0xE7, 0x62], part, bx1, by1);
        }
    }
    j.cmd(&[&[0xCA, 0x22, part_no(layers.len().saturating_sub(1))]]); // max layer part
    j.cmd(&[&[0xE7, 0x54, 0x00], &[0; 5]]); // pen offsets
    j.cmd(&[&[0xE7, 0x54, 0x01], &[0; 5]]);
    j.cmd(&[&[0xE7, 0x55, 0x00], &[0; 5]]); // layer offsets
    j.cmd(&[&[0xE7, 0x55, 0x01], &[0; 5]]);
    j.cmd(&[&[0xF1, 0x03], &[0; 10]]); // display offset
    j.cmd(&[&[0xF1, 0x00, 0x00], &[0xF1, 0x01, 0x00], &[0xF2, 0x00, 0x00], &[0xF2, 0x01, 0x00]]); // element indices
    j.cmd(&[&[0xF2, 0x02], b"GBurn\0\0\0\0\0"]); // element name (10 bytes)
    j.coord(&[0xF2, 0x03], x0, y0);
    j.coord(&[0xF2, 0x04], x1, y1);
    j.cmd(&[&[0xF2, 0x05, 0x00, 0x01, 0x00, 0x01], &[0; 10]]); // element array 1 × 1
    j.cmd(&[&[0xF2, 0x06], &[0; 10]]);
    j.cmd(&[&[0xF2, 0x07, 0x00]]);
    j.cmd(&[&[0xEA, 0x00]]); // array start
    j.cmd(&[&[0xE7, 0x60, 0x00]]); // current element
    j.coord(&[0xE7, 0x13], x0, y0); // array min / max
    j.coord(&[0xE7, 0x17], x1, y1);
    j.coord(&[0xE7, 0x23], 0, 0); // array add
    j.cmd(&[&[0xE7, 0x24, 0x00]]); // array mirror
    j.coord(&[0xE7, 0x37], x1, y1); // array even distance
    j.cmd(&[&[0xE7, 0x08, 0x00, 0x01, 0x00, 0x01], &[0; 10]]); // array repeat 1 × 1
                                                               // ---- layers
                                                               // a block per run of strokes on one layer: one per layer, or several when the plan goes group by group
    for run in plan.strokes.chunk_by(|a, b| a.layer == b.layer) {
        let li = &run[0].layer;
        let part = layers.iter().position(|l| l == li).unwrap_or(0);
        let l = &doc.layers[*li];
        j.cmd(&[&[0xCA, 0x01, 0x01]]); // work mode 1 (vector)
        j.cmd(&[&[0xCA, 0x02, part_no(part)]]); // layer number
        j.cmd(&[&[0xCA, 0x01, 0x30]]); // laser 2 offset off
        j.cmd(&[&[0xCA, 0x01, 0x10]]); // laser device 0
        j.cmd(&[if l.air { &[0xCA, 0x01, 0x13] } else { &[0xCA, 0x01, 0x12] }]); // air assist
        j.cmd(&[&[0xC9, 0x02], &encode35((l.speed * 1000.0) as i64)]);
        let [a1, b1, a2, b2] = head_powers(l);
        j.cmd(&[&[0xC6, 0x01], &power14(a1)]);
        j.cmd(&[&[0xC6, 0x02], &power14(b1)]);
        j.cmd(&[&[0xC6, 0x21], &power14(a2)]);
        j.cmd(&[&[0xC6, 0x22], &power14(b2)]);
        j.cmd(&[&[0xC6, 0x12], &[0; 5]]); // laser on delay 0
        j.cmd(&[&[0xC6, 0x13], &[0; 5]]); // laser off delay 0
        j.cmd(&[&[0xCA, 0x03, 0x01]]); // laser tube start
        for s in run {
            let (x, y) = m(s.pts[0]);
            j.jump(x, y);
            if s.dwell_ms > 0.0 {
                // ponytail: a dot is a 0.02 mm cut (the layer's speed sets how long it burns); no dwell command yet
                j.mark(x + 20, y);
                continue;
            }
            for (k, p) in s.pts[1..].iter().enumerate() {
                let (x, y) = m(*p);
                // a dead run (overscan, power 0) travels instead of cutting
                if s.pw.as_ref().and_then(|w| w.get(k)).is_some_and(|w| *w <= 0.0) {
                    j.jump(x, y);
                } else {
                    j.mark(x, y);
                }
            }
        }
        j.cmd(&[&[0xE7, 0x00]]); // block end
        j.cmd(&[&[0xCA, 0x01, 0x00]]); // layer end
        j.cmd(&[&[0xCA, 0x01, 0x30]]);
    }
    // ---- tail
    j.cmd(&[&[0xEB]]); // array end
    j.cmd(&[&[0xE7, 0x00]]);
    let sum = j.sum() + 0xD7; // the file sum counts the EOF byte too (rdjob.py write_tail)
    j.cmd(&[&[0xE5, 0x05], &encode35(sum)]);
    j.cmd(&[&[0xD7]]); // end of file
    j.bytes
}

/// The `.rd` file: the command stream swizzled.
pub fn rd_file(doc: &Document, plan: &Plan, o: &RdOptions) -> Vec<u8> {
    swizzle(&rd_commands(doc, plan, o), o.magic)
}

/// UDP packets for a command stream: whole commands, ≤ 1000 bytes each, swizzled, a checksum of the bytes as sent
/// in front (udp_connection.py `_package`). The controller answers each with ACK (0xCC) or NAK (0xCF, resend).
pub fn udp_packets(commands: &[u8], magic: u8) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut mark = 0;
    for (i, b) in commands.iter().enumerate() {
        if *b >= 0x80 && i > start && i - start > 1000 {
            out.push(packet(&commands[start..mark], magic));
            start = mark;
        }
        if *b >= 0x80 {
            mark = i;
        }
    }
    if start < commands.len() {
        out.push(packet(&commands[start..], magic));
    }
    out
}

/// One packet: checksum + swizzled bytes.
pub fn packet(commands: &[u8], magic: u8) -> Vec<u8> {
    let s = swizzle(commands, magic);
    let mut p = checksum(&s).to_vec();
    p.extend(s);
    p
}

/// DA 00 mem: ask for a setting / status word (unswizzled; wrap with `packet`).
pub fn get_setting(mem: u16) -> Vec<u8> {
    vec![0xDA, 0x00, (mem >> 8) as u8, (mem & 0xFF) as u8]
}

/// A reply to `get_setting` (unswizzled): DA 01 mem(2) value(5) → (mem, value).
pub fn parse_reply(reply: &[u8]) -> Option<(u16, i64)> {
    if reply.len() >= 9 && reply[0] == 0xDA && reply[1] == 0x01 {
        Some((((reply[2] as u16) << 8) | reply[3] as u16, decode35(&reply[4..9])))
    } else {
        None
    }
}

pub fn cmd_home_xy() -> Vec<u8> {
    vec![0xD8, 0x2A]
}
pub fn cmd_stop() -> Vec<u8> {
    vec![0xD8, 0x01]
}
pub fn cmd_pause() -> Vec<u8> {
    vec![0xD8, 0x02]
}
pub fn cmd_resume() -> Vec<u8> {
    vec![0xD8, 0x03]
}
/// C9 02: the speed for the moves that follow, mm/s (sent before jogs and frames: else the last job's speed holds)
pub fn cmd_move_speed(mm_s: f64) -> Vec<u8> {
    let mut v = vec![0xC9, 0x02];
    v.extend(encode35((mm_s.clamp(1.0, 1000.0) * 1000.0) as i64));
    v
}
/// D9 10 00: move the head to machine µm, absolute (from the machine's 0,0), beam off.
///
/// Option 0x00 only. 0x02 / 0x03 move *by* x / y from where the head is (seen on the owner's Nova 63, 2026-10-24: a
/// jog to X 270.6 went to 260.6 + 270.6). **0x01 ("light") FIRED THE BEAM on the Nova during Frame (2026-10-24)**:
/// never send it; there is deliberately no way to ask for it here.
pub fn cmd_rapid_xy(x: i64, y: i64) -> Vec<u8> {
    let mut v = vec![0xD9, 0x10, 0x00];
    v.extend(encode35(x));
    v.extend(encode35(y));
    v
}

// ---- the controller's own settings (LightBurn's Machine Settings for a DSP) ------------------------------------------
// Addresses and units from MeerK40t's Ruida emulator (MIT); **unverified on a real controller**. Lengths are µm, speeds
// µm/s, accelerations µm/s²; the rest are shown as the controller stores them.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingUnit {
    /// µm, shown in mm
    Mm,
    /// µm/s, shown in mm/s
    MmS,
    /// µm/s², shown in mm/s²
    MmS2,
    /// as stored
    Raw,
}

impl SettingUnit {
    /// (controller value → shown value factor, unit label)
    pub fn scale(self) -> (f64, &'static str) {
        match self {
            SettingUnit::Mm => (0.001, "mm"),
            SettingUnit::MmS => (0.001, "mm/s"),
            SettingUnit::MmS2 => (0.001, "mm/s²"),
            SettingUnit::Raw => (1.0, ""),
        }
    }
}

pub const MACHINE_SETTINGS: [(u16, &str, SettingUnit); 22] = [
    (0x0026, "X range (bed width)", SettingUnit::Mm),
    (0x0036, "Y range (bed height)", SettingUnit::Mm),
    (0x0023, "X max speed", SettingUnit::MmS),
    (0x0033, "Y max speed", SettingUnit::MmS),
    (0x0024, "X start speed", SettingUnit::MmS),
    (0x0034, "Y start speed", SettingUnit::MmS),
    (0x0025, "X max acceleration", SettingUnit::MmS2),
    (0x0035, "Y max acceleration", SettingUnit::MmS2),
    (0x002B, "X backlash", SettingUnit::Mm),
    (0x003B, "Y backlash", SettingUnit::Mm),
    (0x002A, "X home offset", SettingUnit::Mm),
    (0x003A, "Y home offset", SettingUnit::Mm),
    (0x0005, "Travel speed (G0)", SettingUnit::MmS),
    (0x000C, "Homing speed", SettingUnit::MmS),
    (0x000E, "Engrave line-step speed", SettingUnit::MmS),
    (0x0101, "Corner (turn) speed", SettingUnit::MmS),
    (0x0102, "Cutting acceleration", SettingUnit::MmS2),
    (0x0103, "Travel delay (ms)", SettingUnit::Raw),
    (0x0011, "Laser 1 PWM frequency (Hz)", SettingUnit::Raw),
    (0x0012, "Laser 1 min power", SettingUnit::Raw),
    (0x0013, "Laser 1 max power", SettingUnit::Raw),
    (0x004B, "Z backlash", SettingUnit::Mm),
];

/// what connect reads for the time estimate: X / Y max speed, X / Y max acceleration, cutting acceleration
pub const MOTION_SETTINGS: [u16; 5] = [0x0023, 0x0033, 0x0025, 0x0035, 0x0102];

/// DA 01 mem v v: write a setting (MeerK40t sends the value twice)
pub fn cmd_set_setting(mem: u16, v: i64) -> Vec<u8> {
    let mut c = vec![0xDA, 0x01, (mem >> 8) as u8, (mem & 0xFF) as u8];
    c.extend(encode35(v));
    c.extend(encode35(v));
    c
}

// ---- the controller's memory (files kept on the controller, run from its panel) ------------------------------------
// Commands from MeerK40t's Ruida emulator (MIT, decoded from RDWorks / LightBurn traffic); unverified on a real controller.

/// How many files the controller holds (ask with `get_setting`).
pub const MEM_DOC_COUNT: u16 = 0x0205;

/// A file name as the controller keeps it: up to 8 upper-case letters or digits (anything else becomes _).
pub fn doc_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_whitespace()).map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).take(8).collect();
    if n.is_empty() {
        "GBURN".into()
    } else {
        n
    }
}
/// E8 02, E7 01 name 00, then the job: the controller stores it as `name` instead of running it.
pub fn cmd_upload(name: &str, commands: &[u8]) -> Vec<u8> {
    let mut v = vec![0xE8, 0x02, 0xE7, 0x01];
    v.extend(doc_name(name).bytes());
    v.push(0);
    v.extend_from_slice(commands);
    v
}
/// E8 01 n: the name of file n (from 1); answered by `parse_doc_name`
pub fn cmd_doc_name(n: u16) -> Vec<u8> {
    let mut v = vec![0xE8, 0x01];
    v.extend(encode14(n as i32));
    v
}
/// E8 03 n: run file n
pub fn cmd_run_doc(n: u16) -> Vec<u8> {
    let mut v = vec![0xE8, 0x03];
    v.extend(encode14(n as i32));
    v
}
/// E8 00 n n: delete file n (0 = every file)
pub fn cmd_delete_doc(n: u16) -> Vec<u8> {
    let mut v = vec![0xE8, 0x00];
    v.extend(encode14(n as i32));
    v.extend(encode14(n as i32));
    v
}
/// The answer to `cmd_doc_name` (unswizzled): E8 01 n(2) name 00 → (n, name); None if it isn't one or isn't whole yet.
pub fn parse_doc_name(reply: &[u8]) -> Option<(u16, String, usize)> {
    if reply.len() < 5 || reply[0] != 0xE8 || reply[1] != 0x01 {
        return None;
    }
    let end = reply[4..].iter().position(|b| *b == 0)? + 4;
    Some((decode14(&reply[2..4]) as u16, String::from_utf8_lossy(&reply[4..end]).into_owned(), end + 1))
}

// ---- decoder (read-back checks, and opening .rd files) --------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RdMove {
    /// machine µm (relative to the ref point for "current position" jobs)
    pub x: i64,
    pub y: i64,
    pub cut: bool,
    /// layer power % (max power 1) and speed mm/s in force
    pub power: f64,
    pub speed: f64,
}

/// Split an unswizzled stream into commands (a command byte is ≥ 0x80, its data bytes < 0x80).
pub fn split_commands(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut mark = 0;
    for (i, b) in data.iter().enumerate() {
        if *b >= 0x80 && mark != i {
            out.push(&data[mark..i]);
            mark = i;
        }
    }
    if mark < data.len() {
        out.push(&data[mark..]);
    }
    out
}

/// Every head move of an unswizzled job, with the beam state and the settings in force; plus the declared file
/// sum and whether it matches.
pub fn rd_decode(data: &[u8]) -> (Vec<RdMove>, bool) {
    let (mut x, mut y) = (0i64, 0i64);
    let (mut power, mut speed) = (0.0, 0.0);
    let mut moves = Vec::new();
    let mut sum_ok = false;
    let mut running: i64 = 0;
    for c in split_commands(data) {
        match c {
            [0x88, rest @ ..] if rest.len() >= 10 => {
                x = decode35(&rest[0..5]);
                y = decode35(&rest[5..10]);
                moves.push(RdMove { x, y, cut: false, power, speed });
            }
            [0x89, rest @ ..] if rest.len() >= 4 => {
                x += decode14(&rest[0..2]) as i64;
                y += decode14(&rest[2..4]) as i64;
                moves.push(RdMove { x, y, cut: false, power, speed });
            }
            [0x8A, rest @ ..] if rest.len() >= 2 => {
                x += decode14(rest) as i64;
                moves.push(RdMove { x, y, cut: false, power, speed });
            }
            [0x8B, rest @ ..] if rest.len() >= 2 => {
                y += decode14(rest) as i64;
                moves.push(RdMove { x, y, cut: false, power, speed });
            }
            [0xA8, rest @ ..] if rest.len() >= 10 => {
                x = decode35(&rest[0..5]);
                y = decode35(&rest[5..10]);
                moves.push(RdMove { x, y, cut: true, power, speed });
            }
            [0xA9, rest @ ..] if rest.len() >= 4 => {
                x += decode14(&rest[0..2]) as i64;
                y += decode14(&rest[2..4]) as i64;
                moves.push(RdMove { x, y, cut: true, power, speed });
            }
            [0xAA, rest @ ..] if rest.len() >= 2 => {
                x += decode14(rest) as i64;
                moves.push(RdMove { x, y, cut: true, power, speed });
            }
            [0xAB, rest @ ..] if rest.len() >= 2 => {
                y += decode14(rest) as i64;
                moves.push(RdMove { x, y, cut: true, power, speed });
            }
            [0xC6, 0x02, rest @ ..] if rest.len() >= 2 => power = ((((rest[0] & 0x7F) as i32) << 7) | (rest[1] & 0x7F) as i32) as f64 * 100.0 / 16383.0, // unsigned 14-bit
            [0xC9, 0x02, rest @ ..] if rest.len() >= 5 => speed = decode35(rest) as f64 / 1000.0,
            [0xE5, 0x05, rest @ ..] if rest.len() >= 5 => sum_ok = decode35(rest) == running + 0xD7,
            _ => {}
        }
        if c[0] != 0xE5 {
            running += c.iter().map(|b| *b as i64).sum::<i64>();
        }
    }
    (moves, sum_ok)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_two_head_layer_powers_only_its_head() {
        let mut d = crate::sample();
        d.layers[0].head = 2;
        let p = crate::plan::plan(&d, crate::plan::Machine::default());
        let c = rd_commands(&d, &p, &RdOptions::default());
        let cs = split_commands(&c);
        let pw = |op: u8| cs.iter().find(|c| c.len() >= 5 && c[0] == 0xC6 && c[1] == op && c[2] == 0).map(|c| (((c[3] & 0x7F) as i32) << 7) | (c[4] & 0x7F) as i32).unwrap();
        assert_eq!(pw(0x32), 0, "layer 1's laser 1 off");
        assert!(pw(0x42) > 0, "layer 1's laser 2 on");
        d.layers[0].head = 0;
        assert_eq!(head_powers(&d.layers[0])[1], head_powers(&d.layers[0])[3], "both heads alike by default");
    }

    #[test]
    fn a_setting_writes_its_value_twice() {
        let c = cmd_set_setting(0x0023, 300_000);
        assert_eq!(&c[..4], &[0xDA, 0x01, 0x00, 0x23]);
        assert_eq!((decode35(&c[4..9]), decode35(&c[9..14])), (300_000, 300_000));
        let mut seen = std::collections::HashSet::new();
        assert!(MACHINE_SETTINGS.iter().all(|m| seen.insert(m.0)), "no address twice");
    }

    #[test]
    fn controller_files_name_send_run_and_delete() {
        assert_eq!(doc_name("my sign v2.gburn"), "MYSIGNV2");
        assert_eq!(doc_name("Ünï"), "_N_");
        let up = cmd_upload("tag", &[0xD7]);
        assert_eq!(up, [0xE8, 0x02, 0xE7, 0x01, b'T', b'A', b'G', 0, 0xD7]);
        assert_eq!(cmd_run_doc(3), [0xE8, 0x03, 0, 3]);
        assert_eq!(cmd_delete_doc(200), [0xE8, 0x00, 1, 72, 1, 72]);
        let mut reply = cmd_doc_name(2);
        reply.extend(b"COASTER\0rest");
        assert_eq!(parse_doc_name(&reply), Some((2, "COASTER".to_string(), 12)));
        assert_eq!(parse_doc_name(&reply[..8]), None, "not whole yet");
    }

    use super::*;
    use crate::plan::{plan, Machine};
    use crate::{geom, sample, Layer, Mode};

    #[test]
    fn swizzle_is_a_bijection_and_matches_known_values() {
        for m in [0x88u8, 0x11, 0x38] {
            let mut seen = [false; 256];
            for b in 0..=255u8 {
                let s = swizzle_byte(b, m);
                assert!(!seen[s as usize]);
                seen[s as usize] = true;
                assert_eq!(unswizzle_byte(s, m), b);
            }
        }
        assert_eq!(swizzle_byte(0x00, 0x88), 0x89);
    }

    #[test]
    fn numbers_pack_into_seven_bit_groups() {
        for v in [0i64, 1, 127, 128, 16383, 100_000, 1_234_567, -1, -5000, -1_000_000] {
            assert_eq!(decode35(&encode35(v)), v, "{v}");
            assert!(encode35(v).iter().all(|b| *b < 0x80));
        }
        for v in [0i32, 1, 8191, -1, -8191] {
            assert_eq!(decode14(&encode14(v)), v, "{v}");
        }
        assert_eq!(power14(100.0), [0x7F, 0x7F]);
        assert_eq!(checksum(&[0xFF, 0x02]), [0x01, 0x01]);
    }

    /// LightBurn's own file for the owner's Nova: unswizzles with 0x88, splits into commands, its file sum checks,
    /// and the job is a sane size (a 45-crate logo sheet)
    #[test]
    fn lightburns_rd_file_reads_back() {
        // a LightBurn file kept out of the repository: the test runs where it is present
        let Ok(raw) = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata/sample.rd")) else { return };
        let raw = &raw[..];
        assert_eq!(guess_magic(raw), Some(0x88));
        let u = unswizzle(raw, 0x88);
        assert_eq!(&u[..2], &[0xD8, 0x12], "starts from the current position");
        let (moves, sum_ok) = rd_decode(&u);
        assert!(sum_ok, "file sum");
        assert!(moves.len() > 200_000, "{}", moves.len());
        let cuts = moves.iter().filter(|m| m.cut).count();
        assert!(cuts > 100_000);
        let (x0, x1) = moves.iter().fold((i64::MAX, i64::MIN), |(a, b), m| (a.min(m.x), b.max(m.x)));
        let (y0, y1) = moves.iter().fold((i64::MAX, i64::MIN), |(a, b), m| (a.min(m.y), b.max(m.y)));
        // relative to the start point: within a 1600 × 1000 mm bed
        assert!((x1 - x0) < 1_600_000 && (y1 - y0) < 1_000_000, "{} × {} µm", x1 - x0, y1 - y0);
        assert!(moves.iter().any(|m| m.speed > 0.0 && m.power > 0.0));
    }

    /// our file reads back to the plan: strokes, extents (through the bed mapping) and the per-layer settings
    #[test]
    fn more_than_128_layers_keep_their_part_bytes_7_bit() {
        let mut d = Document { layers: (0..130).map(|k| crate::Layer::new("L", [0, 0, 0], crate::Mode::Line, 10.0, 50.0 + k as f64 * 0.1)).collect(), ..Default::default() };
        for k in 0..130 {
            d.add(crate::geom::make_rect(k as f64, 0.0, k as f64 + 0.5, 0.5), k);
        }
        let p = plan(&d, Machine::default());
        let u = rd_commands(&d, &p, &RdOptions { bed: [600.0, 400.0], ..Default::default() });
        let parts: Vec<&[u8]> = split_commands(&u).into_iter().filter(|c| c.starts_with(&[0xCA, 0x02])).collect();
        assert!(parts.len() == 130 && parts.iter().all(|c| c.len() == 3), "a part byte ≥ 0x80 would split the command");
    }

    #[test]
    fn our_rd_file_reads_back_to_the_plan() {
        let d = sample();
        let p = plan(&d, Machine::default());
        let o = RdOptions { bed: [600.0, 400.0], ..Default::default() };
        let file = rd_file(&d, &p, &o);
        assert_eq!(guess_magic(&file), Some(0x88));
        let u = unswizzle(&file, 0x88);
        assert_eq!(&u[..2], &[0xD8, 0x10]);
        assert_eq!(*u.last().unwrap(), 0xD7);
        let (moves, sum_ok) = rd_decode(&u);
        assert!(sum_ok);
        let cuts = moves.iter().filter(|m| m.cut).count();
        let expect: usize = p.strokes.iter().map(|s| s.pts.len() - 1).sum();
        assert_eq!(cuts, expect);
        // extents back in design mm
        let mut r = crate::Rect::default();
        for m in &moves {
            r.add(o.to_design(m.x, m.y));
        }
        let b = d.bounds();
        for (a, b) in [(r.x0, b.x0), (r.y0, b.y0), (r.x1, b.x1), (r.y1, b.y1)] {
            assert!((a - b).abs() < 0.01, "{a} vs {b}");
        }
        // the cut layer's speed and power are in force on its cuts
        let cut_layer = d.layers.iter().position(|l| l.name == "Cut").unwrap();
        let first_cut_stroke = p.strokes.iter().position(|s| s.layer == cut_layer).unwrap();
        let cut_moves_before: usize = p.strokes[..first_cut_stroke].iter().map(|s| s.pts.len()).sum();
        let m = moves[cut_moves_before + 1];
        assert!((m.speed - 15.0).abs() < 0.01 && (m.power - 90.0).abs() < 0.1, "{m:?}");
        // "current position": coordinates relative to the anchor, header says so
        let o2 = RdOptions { current: Some(crate::pt(b.x0, b.y0)), ..o };
        let u2 = unswizzle(&rd_file(&d, &p, &o2), 0x88);
        assert_eq!(&u2[..2], &[0xD8, 0x12]);
        let (m2, ok2) = rd_decode(&u2);
        assert!(ok2);
        assert!(m2.iter().any(|m| m.y < 0) && m2.iter().all(|m| m.x >= -1), "relative to the bottom-left corner: x ≥ 0, y ≤ 0 on a rear-left machine");
        // packets: whole commands, ≤ ~1 KB, checksums right
        let cmds = rd_commands(&d, &p, &o);
        let pk = udp_packets(&cmds, 0x88);
        assert!(pk.len() > 10);
        let mut back = Vec::new();
        for p in &pk {
            assert!(p.len() <= 1024 + 2, "{}", p.len());
            assert_eq!(&p[..2], &checksum(&p[2..]));
            back.extend(unswizzle(&p[2..], 0x88));
        }
        assert_eq!(back, cmds);
        // a status reply
        let q = get_setting(MEM_CURRENT_X);
        assert_eq!(q, vec![0xDA, 0x00, 0x04, 0x21]);
        let mut reply = vec![0xDA, 0x01, 0x04, 0x21];
        reply.extend(encode35(123_456));
        assert_eq!(parse_reply(&reply), Some((MEM_CURRENT_X, 123_456)));
        assert_eq!(cmd_rapid_xy(1000, 2000)[..3], [0xD9, 0x10, 0x00], "absolute, beam off (0x01 fired the Nova's beam)");
        assert_eq!(cmd_move_speed(100.0)[..2], [0xC9, 0x02]);
        assert_eq!(decode35(&cmd_move_speed(100.0)[2..]), 100_000);
    }

    #[test]
    fn home_corners_map_both_ways() {
        for h in [Home::RearLeft, Home::RearRight, Home::FrontLeft, Home::FrontRight] {
            let o = RdOptions { home: h, bed: [600.0, 400.0], ..Default::default() };
            let p = crate::pt(100.0, 50.0);
            let (x, y) = o.to_machine(p);
            let back = o.to_design(x, y);
            assert!((back.x - p.x).abs() < 1e-6 && (back.y - p.y).abs() < 1e-6, "{h:?}");
        }
        // rear-left (Thunder): design y = 0 (front) is far from home
        let o = RdOptions::default();
        assert_eq!(o.to_machine(crate::pt(0.0, 0.0)), (0, 400_000));
        // a layer with air assist and a dead run: the dead run is a jump
        let mut d = Document { layers: vec![Layer::new("F", [0, 0, 0], Mode::Fill, 100.0, 50.0)], ..Default::default() };
        d.layers[0].air = true;
        d.layers[0].overscan = 2.0;
        d.layers[0].interval = 5.0;
        d.add(geom::make_rect(10.0, 10.0, 20.0, 20.0), 0);
        let p = plan(&d, Machine::default());
        let u = rd_commands(&d, &p, &o);
        assert!(split_commands(&u).iter().any(|c| *c == [0xCA, 0x01, 0x13]));
        let (moves, _) = rd_decode(&u);
        let cuts = moves.iter().filter(|m| m.cut).count();
        assert_eq!(cuts, p.strokes.len(), "one cut per scan line, the dead runs travel");
        // min power lands in C6 01 (and the max stays in C6 02)
        d.layers[0].power_min = 12.0;
        let u = rd_commands(&d, &p, &o);
        let cs = split_commands(&u);
        let min = cs.iter().find(|c| c.len() > 3 && c[0] == 0xC6 && c[1] == 0x01).unwrap();
        assert_eq!(&min[2..4], &power14(12.0));
        // a rotary scales y before the bed mapping
        let o2 = RdOptions { y_scale: 2.0, ..o };
        assert_eq!(o2.to_machine(crate::pt(0.0, 100.0)).1, 200_000);
        let back = o2.to_design(0, 200_000);
        assert!((back.y - 100.0).abs() < 1e-6);
    }
}
