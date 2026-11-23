//! Single-line fonts: AutoCAD compiled shape fonts (.shx, "shapes 1.0 / 1.1" and "unifont 1.0"), the kind
//! LightBurn reads. Each character is a little program of pen moves (vectors, offsets, octant / fractional / bulge
//! arcs, sub-shapes); run, it leaves open polylines in font units (Y up) and the pen where the next one starts.

use crate::{geom, pt, Contour, Pt};
use std::collections::HashMap;

pub struct ShxFont {
    pub name: String,
    /// capital height and descender, font units
    pub above: f64,
    pub below: f64,
    unifont: bool,
    shapes: HashMap<u32, Vec<u8>>,
}

/// the 16 vector directions, each a unit of travel in x / y (the diagonals at ½ steps, as AutoCAD draws them)
const DIRS: [(f64, f64); 16] = [
    (1.0, 0.0),
    (1.0, 0.5),
    (1.0, 1.0),
    (0.5, 1.0),
    (0.0, 1.0),
    (-0.5, 1.0),
    (-1.0, 1.0),
    (-1.0, 0.5),
    (-1.0, 0.0),
    (-1.0, -0.5),
    (-1.0, -1.0),
    (-0.5, -1.0),
    (0.0, -1.0),
    (0.5, -1.0),
    (1.0, -1.0),
    (1.0, -0.5),
];

impl ShxFont {
    /// Read a compiled .shx file. Big fonts (Asian) aren't read.
    pub fn parse(b: &[u8]) -> Result<ShxFont, String> {
        let end = b.iter().position(|c| *c == 0x1A).filter(|i| *i < 64).ok_or("not an SHX font")?;
        let head = String::from_utf8_lossy(&b[..end]);
        if !head.starts_with("AutoCAD-86") {
            return Err("not an SHX font".into());
        }
        let u16le = |i: usize| -> Result<u16, String> { b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| "the SHX file is cut short".to_string()) };
        let mut shapes = HashMap::new();
        let mut info: Vec<u8> = Vec::new();
        let unifont = head.contains("unifont");
        if unifont {
            let n = b.get(end + 1..end + 5).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or("the SHX file is cut short")?;
            let len = u16le(end + 5)? as usize;
            let mut at = end + 7;
            info = b.get(at..at + len).ok_or("the SHX file is cut short")?.to_vec();
            at += len;
            for _ in 1..n.min(70_000) {
                let (Ok(code), Ok(len)) = (u16le(at), u16le(at + 2)) else { break };
                let Some(def) = b.get(at + 4..at + 4 + len as usize) else { break };
                shapes.insert(code as u32, def.to_vec());
                at += 4 + len as usize;
            }
        } else if head.contains("shapes") {
            let count = u16le(end + 5)? as usize;
            let mut index = Vec::with_capacity(count);
            for k in 0..count {
                index.push((u16le(end + 7 + k * 4)?, u16le(end + 9 + k * 4)? as usize));
            }
            let mut at = end + 7 + count * 4;
            for (code, len) in index {
                let def = b.get(at..at + len).ok_or("the SHX file is cut short")?;
                if code == 0 {
                    info = def.to_vec();
                } else {
                    shapes.insert(code as u32, def.to_vec());
                }
                at += len;
            }
        } else {
            return Err("big fonts aren't supported".into());
        }
        // the font's own record: name, above, below
        let name_end = info.iter().position(|c| *c == 0).unwrap_or(info.len());
        let name = String::from_utf8_lossy(&info[..name_end]).trim().to_string();
        let above = info.get(name_end + 1).copied().filter(|v| *v > 0).unwrap_or(10) as f64;
        let below = info.get(name_end + 2).copied().unwrap_or(2) as f64;
        // each shape's own bytes start after its name
        for def in shapes.values_mut() {
            let skip = def.iter().position(|c| *c == 0).map_or(0, |i| i + 1);
            def.drain(..skip.min(def.len()));
        }
        Ok(ShxFont { name: if name.is_empty() { "SHX font".into() } else { name }, above, below, unifont, shapes })
    }

    /// One character: its strokes (open polylines, font units) and how far the pen moved on.
    pub fn glyph(&self, ch: char) -> (Vec<Contour>, f64) {
        let mut p = Pen { at: pt(0.0, 0.0), scale: 1.0, down: true, stack: Vec::new(), line: vec![pt(0.0, 0.0)], out: Vec::new() };
        match self.shapes.get(&(ch as u32)) {
            Some(def) => self.run(def, &mut p, 0),
            None => return (Vec::new(), self.above * 0.6),
        }
        p.flush();
        (p.out.into_iter().filter(|l| l.len() > 1).map(|l| geom::make_polyline(&l, false)).collect(), p.at.x)
    }

    fn run(&self, def: &[u8], p: &mut Pen, depth: u32) {
        let mut i = 0;
        while i < def.len() {
            i = self.step(def, i, p, depth, false);
        }
    }

    /// Carry out the command at `i` (or, `dry`, only measure it); returns where the next one starts.
    fn step(&self, d: &[u8], i: usize, p: &mut Pen, depth: u32, dry: bool) -> usize {
        let b = d[i];
        let s8 = |k: usize| d.get(k).map_or(0.0, |v| *v as i8 as f64);
        let u8_ = |k: usize| d.get(k).map_or(0.0, |v| *v as f64);
        if b >> 4 != 0 {
            let (dx, dy) = DIRS[(b & 15) as usize];
            let n = (b >> 4) as f64 * p.scale;
            if !dry {
                p.move_by(dx * n, dy * n);
            }
            return i + 1;
        }
        match b {
            0 => d.len(), // end of shape
            1 => {
                if !dry {
                    p.down = true;
                    p.line = vec![p.at];
                }
                i + 1
            }
            2 => {
                if !dry {
                    p.flush();
                    p.down = false;
                }
                i + 1
            }
            3 | 4 => {
                let k = u8_(i + 1).max(1.0);
                if !dry {
                    p.scale = if b == 3 { p.scale / k } else { p.scale * k };
                }
                i + 2
            }
            5 => {
                if !dry {
                    p.stack.push(p.at);
                }
                i + 1
            }
            6 => {
                if !dry {
                    if let Some(q) = p.stack.pop() {
                        p.flush();
                        p.at = q;
                        p.line = vec![q];
                    }
                }
                i + 1
            }
            7 => {
                let (code, next) = if self.unifont { ((u8_(i + 1) as u32) << 8 | u8_(i + 2) as u32, i + 3) } else { (u8_(i + 1) as u32, i + 2) };
                if !dry && depth < 8 {
                    if let Some(sub) = self.shapes.get(&code) {
                        self.run(sub, p, depth + 1);
                    }
                }
                next
            }
            8 => {
                if !dry {
                    p.move_by(s8(i + 1) * p.scale, s8(i + 2) * p.scale);
                }
                i + 3
            }
            9 => {
                let mut k = i + 1;
                while k + 1 < d.len() && !(d[k] == 0 && d[k + 1] == 0) {
                    if !dry {
                        p.move_by(s8(k) * p.scale, s8(k + 1) * p.scale);
                    }
                    k += 2;
                }
                k + 2
            }
            10 => {
                // octant arc: radius, then (clockwise?, start octant, octants spanned; 0 = a full circle)
                if !dry {
                    let r = u8_(i + 1) * p.scale;
                    let o = d.get(i + 2).copied().unwrap_or(0);
                    let cw = o & 0x80 != 0;
                    let (s, n) = (((o >> 4) & 7) as f64, if o & 7 == 0 { 8.0 } else { (o & 7) as f64 });
                    let a0 = s * std::f64::consts::FRAC_PI_4;
                    let span = n * std::f64::consts::FRAC_PI_4 * if cw { -1.0 } else { 1.0 };
                    p.arc(r, a0, span);
                }
                i + 3
            }
            11 => {
                // fractional arc: start / end offsets (1/256 of an octant), radius (2 bytes), octant byte as above
                if !dry {
                    let (so, eo) = (u8_(i + 1), u8_(i + 2));
                    let r = (u8_(i + 3) * 256.0 + u8_(i + 4)) * p.scale;
                    let o = d.get(i + 5).copied().unwrap_or(0);
                    let cw = o & 0x80 != 0;
                    let (s, n) = (((o >> 4) & 7) as f64, if o & 7 == 0 { 8.0 } else { (o & 7) as f64 });
                    let oct = std::f64::consts::FRAC_PI_4;
                    let sign = if cw { -1.0 } else { 1.0 };
                    let a0 = s * oct + sign * so / 256.0 * oct;
                    let mut span = n * oct - so / 256.0 * oct;
                    if eo > 0.0 {
                        span -= oct - eo / 256.0 * oct;
                    }
                    p.arc(r, a0, span * sign);
                }
                i + 6
            }
            12 => {
                if !dry {
                    p.bulge(s8(i + 1) * p.scale, s8(i + 2) * p.scale, s8(i + 3) / 127.0);
                }
                i + 4
            }
            13 => {
                let mut k = i + 1;
                while k + 1 < d.len() && !(d[k] == 0 && d[k + 1] == 0) {
                    if !dry {
                        p.bulge(s8(k) * p.scale, s8(k + 1) * p.scale, s8(k + 2) / 127.0);
                    }
                    k += 3;
                }
                k + 2
            }
            // vertical text only: the next command is skipped
            14 => {
                if i + 1 < d.len() {
                    self.step(d, i + 1, p, depth, true)
                } else {
                    i + 1
                }
            }
            _ => i + 1,
        }
    }
}

struct Pen {
    at: Pt,
    scale: f64,
    down: bool,
    stack: Vec<Pt>,
    line: Vec<Pt>,
    out: Vec<Vec<Pt>>,
}

impl Pen {
    fn move_by(&mut self, dx: f64, dy: f64) {
        self.to(pt(self.at.x + dx, self.at.y + dy));
    }
    fn to(&mut self, q: Pt) {
        if self.down {
            if self.line.is_empty() {
                self.line.push(self.at);
            }
            self.line.push(q);
        } else {
            self.line = vec![q];
        }
        self.at = q;
    }
    fn flush(&mut self) {
        if self.down && self.line.len() > 1 {
            self.out.push(std::mem::take(&mut self.line));
        }
        self.line = vec![self.at];
    }
    /// round the centre the pen is `r` from at angle `a0`, through `span` radians (+ anticlockwise)
    fn arc(&mut self, r: f64, a0: f64, span: f64) {
        let c = pt(self.at.x - r * a0.cos(), self.at.y - r * a0.sin());
        let n = ((span.abs() / std::f64::consts::FRAC_PI_4) * 4.0).ceil().max(1.0) as usize;
        for k in 1..=n {
            let a = a0 + span * k as f64 / n as f64;
            self.to(pt(c.x + r * a.cos(), c.y + r * a.sin()));
        }
    }
    /// to (dx, dy) along an arc of bulge `b` (2 × height ÷ chord; + = anticlockwise), straight when 0
    fn bulge(&mut self, dx: f64, dy: f64, b: f64) {
        let end = pt(self.at.x + dx, self.at.y + dy);
        if b.abs() < 1e-6 {
            return self.to(end);
        }
        let theta = 4.0 * b.atan(); // the included angle
        let chord = dx.hypot(dy);
        let r = chord / (2.0 * (theta / 2.0).sin());
        // the centre: off the chord's middle, square to it
        let (mx, my) = ((self.at.x + end.x) / 2.0, (self.at.y + end.y) / 2.0);
        let h = r * (theta / 2.0).cos();
        let (nx, ny) = (-dy / chord, dx / chord);
        let c = pt(mx + nx * h, my + ny * h);
        let a0 = (self.at.y - c.y).atan2(self.at.x - c.x);
        let n = (theta.abs() / 0.2).ceil().max(2.0) as usize;
        for k in 1..n {
            let a = a0 + theta * k as f64 / n as f64;
            self.to(pt(c.x + r.abs() * a.cos(), c.y + r.abs() * a.sin()));
        }
        self.to(end);
    }
}

/// A two-letter "shapes 1.0" font (TINY: 'L' and 'O') for checks.
pub fn sample() -> Vec<u8> {
    let mut b = b"AutoCAD-86 shapes 1.0\r\n\x1a".to_vec();
    let info = b"TINY\0\x0a\x02\x00\x00".to_vec();
    let l = {
        // pen up, up 10 (A4 = len 10 dir 4), pen down, down 10 (AC), right 6 (60), pen up, right 2 (20), end
        let mut v = b"L\0".to_vec();
        v.extend([0x02, 0xA4, 0x01, 0xAC, 0x60, 0x02, 0x20, 0x00]);
        v
    };
    let o = {
        // pen up, right 5, pen down, a full circle radius 5 starting at octant 0 (centre to the left), pen up, right 7
        let mut v = b"O\0".to_vec();
        v.extend([0x02, 0x50, 0x01, 0x0A, 0x05, 0x00, 0x02, 0x70, 0x00]);
        v
    };
    let shapes: [(u16, &Vec<u8>); 3] = [(0, &info), (b'L' as u16, &l), (b'O' as u16, &o)];
    b.extend(0u16.to_le_bytes());
    b.extend((b'O' as u16).to_le_bytes());
    b.extend(3u16.to_le_bytes());
    for (code, def) in &shapes {
        b.extend(code.to_le_bytes());
        b.extend((def.len() as u16).to_le_bytes());
    }
    for (_, def) in &shapes {
        b.extend(def.iter());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_draw_lines_and_arcs() {
        let f = ShxFont::parse(&sample()).unwrap();
        assert_eq!((f.name.as_str(), f.above, f.below), ("TINY", 10.0, 2.0));
        let (l, adv) = f.glyph('L');
        assert_eq!(adv, 8.0);
        assert_eq!(l.len(), 1, "one stroke: down then across");
        let pts: Vec<Pt> = l[0].nodes.iter().map(|n| n.p).collect();
        assert_eq!(pts, vec![pt(0.0, 10.0), pt(0.0, 0.0), pt(6.0, 0.0)]);
        let (o, adv) = f.glyph('O');
        assert_eq!(adv, 12.0);
        let ring = geom::flatten(&o[0], 0.01);
        // every point 5 from the centre (0, 0)
        assert!(ring.iter().all(|q| (q.x.hypot(q.y) - 5.0).abs() < 1e-9));
        assert!(f.glyph('Z').0.is_empty(), "a missing character is a space");
        assert!(ShxFont::parse(b"hello").is_err());
    }
}
