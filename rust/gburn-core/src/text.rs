//! Text as data: what a placed text was made from (so it can be edited again), variable text, QR codes, and
//! LightBurn material libraries. Font outlines themselves come from gcarve-ui's bundled face in the app.

use crate::{geom, pt, Contour, Pt};
use serde::{Deserialize, Serialize};

/// What a text group was made from; every glyph shape of the group carries a copy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextSpec {
    pub text: String,
    /// capital height, mm
    pub height: f64,
    pub bold: bool,
    /// extra space between letters, % of the height
    #[serde(default)]
    pub spacing: f64,
    /// line pitch as a factor of the font's own (1 = normal)
    #[serde(default = "one")]
    pub line_gap: f64,
    /// 0 left, 1 centre, 2 right
    #[serde(default)]
    pub align: u8,
    /// laid along this shape's outline (0 = none)
    #[serde(default)]
    pub path_id: i32,
    /// the serial number this copy was made with (for `{n}`)
    #[serde(default)]
    pub serial: u32,
    /// added to the serial number and the CSV row (LightBurn's Offset): copies in an array count up
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub offset: u32,
    /// "Family|Style" of a font on the computer or a loaded file; "" = the built-in Liberation Sans (`bold` picks)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub font: String,
    /// letters welded into one outline (script fonts cut as one piece, overlaps not burnt twice)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub weld: bool,
    /// letter width, % (narrow < 100 < wide)
    #[serde(default = "hundred", skip_serializing_if = "is_hundred")]
    pub width: f64,
    /// slant, degrees (italic look; negative leans back)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub slant: f64,
    /// shown in capitals whatever is typed (and whatever a CSV column holds)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub upper: bool,
    /// text on a path: moved along it by this % of its length (round a circle: 25 = a quarter turn)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub path_offset: f64,
    /// round a closed shape: inside it from the bottom (else outside it from the top)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub path_inside: bool,
    /// squeeze a line narrower when it's wider than this, mm (0 = off; LightBurn's Squeeze for variable names)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fit: f64,
    /// with `fit`: shrink the whole line (height too) instead of only squeezing it narrower
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fit_keep: bool,
    /// on a path: the letters bend with it (LightBurn's Distort) instead of each standing straight on it
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub distort: bool,
    /// what stays put when the words change: 0 the top, 1 the middle, 2 the bottom
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub valign: u8,
    /// right-to-left (Hebrew, Arabic letters unjoined): each line laid out from the right, numbers and Latin words kept
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rtl: bool,
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

fn one() -> f64 {
    1.0
}
fn hundred() -> f64 {
    100.0
}
fn is_hundred(v: &f64) -> bool {
    *v == 100.0
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl TextSpec {
    /// any `{tag}`: a serial, a date / time, or a CSV column
    pub fn has_vars(&self) -> bool {
        self.text.find('{').is_some_and(|i| self.text[i..].contains('}'))
    }
}

/// A CSV file for merging into texts: the first row names the columns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merge {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl Merge {
    /// Commas, semicolons or tabs (whichever the header row has most of), "quoted, fields" with "" for a quote,
    /// CRLF or LF; blank lines skipped.
    pub fn parse(text: &str) -> Merge {
        let first = text.lines().next().unwrap_or("");
        let sep = [',', ';', '\t'].into_iter().max_by_key(|c| first.matches(*c).count()).unwrap_or(',');
        let mut rows: Vec<Vec<String>> = Vec::new();
        let (mut row, mut field, mut quoted) = (Vec::new(), String::new(), false);
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' if quoted && chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' if quoted => quoted = false,
                '"' if field.is_empty() => quoted = true,
                c if c == sep && !quoted => row.push(std::mem::take(&mut field)),
                '\r' if !quoted => {}
                '\n' if !quoted => {
                    row.push(std::mem::take(&mut field));
                    if row.iter().any(|f: &String| !f.trim().is_empty()) {
                        rows.push(std::mem::take(&mut row));
                    }
                    row.clear();
                }
                c => field.push(c),
            }
        }
        row.push(field);
        if row.iter().any(|f| !f.trim().is_empty()) {
            rows.push(row);
        }
        let headers = if rows.is_empty() { Vec::new() } else { rows.remove(0).into_iter().map(|h| h.trim().to_string()).collect() };
        Merge { headers, rows }
    }
    /// `{column}` (any case) → that column of `row`; other tags stay for `expand_vars`.
    pub fn fill(&self, text: &str, row: usize) -> String {
        let Some(r) = self.rows.get(row) else { return text.to_string() };
        let mut out = text.to_string();
        for (k, h) in self.headers.iter().enumerate() {
            if h.is_empty() {
                continue;
            }
            let v = r.get(k).map_or("", |v| v.as_str());
            // case-insensitive tag match
            let tag = format!("{{{}}}", h.to_lowercase());
            while let Some(i) = out.to_lowercase().find(&tag) {
                out.replace_range(i..i + tag.len(), v);
            }
        }
        out
    }
}

/// `{n}` → the serial, `{n:4}` zero-padded to 4, `{date}` → `date`, `{time}` → `time`.
pub fn expand_vars(text: &str, serial: u32, date: &str, time: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let tag = &after[..j];
        match tag {
            "n" => out.push_str(&serial.to_string()),
            "date" => out.push_str(date),
            "time" => out.push_str(time),
            t if t.starts_with("n:") => {
                // {n:4} = 0007, {n:x} / {n:X4} = hexadecimal (lower / upper case, padded)
                let f = &t[2..];
                let (hex, upper, w) = match f.chars().next() {
                    Some('x') => (true, false, &f[1..]),
                    Some('X') => (true, true, &f[1..]),
                    _ => (false, false, f),
                };
                match (w.is_empty(), w.parse::<usize>().map(|w| w.min(32))) {
                    (true, _) if hex => out.push_str(&if upper { format!("{serial:X}") } else { format!("{serial:x}") }),
                    (false, Ok(w)) if hex && upper => out.push_str(&format!("{serial:0w$X}")),
                    (false, Ok(w)) if hex => out.push_str(&format!("{serial:0w$x}")),
                    (false, Ok(w)) => out.push_str(&format!("{serial:0w$}")),
                    _ => out.push_str(&format!("{{{t}}}")),
                }
            }
            t => out.push_str(&format!("{{{t}}}")),
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out
}

/// A layer's cut settings in the words: {speed:2} {power:2} {minpower:2} {passes:2} {interval:2} {dpi:2}
/// {freq:2} {layer:2} (layer 2's; 1 = the first). Unknown layers or tags are left as typed.
pub fn layer_tags(text: &str, layers: &[crate::Layer]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let tag = &after[..j];
        let num = |v: f64, d: usize| {
            let s = format!("{v:.d$}");
            if s.contains('.') {
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            } else {
                s
            }
        };
        let v = tag.split_once(':').and_then(|(k, n)| Some((k, layers.get(n.trim().parse::<usize>().ok()?.checked_sub(1)?)?))).and_then(|(k, l)| {
            Some(match k {
                "speed" => num(l.speed, 1),
                "power" => num(l.power, 1),
                "minpower" => num(l.power_min, 1),
                "passes" => l.passes.to_string(),
                "interval" => num(l.interval, 3),
                "dpi" => num(crate::MM_PER_IN / l.interval.max(0.001), 0),
                "freq" => num(l.freq_khz, 1),
                "layer" => l.name.clone(),
                _ => return None,
            })
        });
        match v {
            Some(v) => out.push_str(&v),
            None => out.push_str(&format!("{{{tag}}}")),
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out
}

/// LightBurn's date / time codes (Qt's: yyyy-MM-dd, d MMM yy, HH:mm, h:mm AP; text in 'quotes' as is) as the
/// strftime codes GBurn fills with; a format that already has % codes is left alone.
pub fn qt_date_format(f: &str) -> String {
    if f.contains('%') {
        return f.to_string();
    }
    let c: Vec<char> = f.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch == '\'' {
            // quoted text; '' is a quote
            if c.get(i + 1) == Some(&'\'') {
                out.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < c.len() && c[i] != '\'' {
                if c[i] == '%' {
                    out.push('%');
                }
                out.push(c[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        let mut n = 1;
        while i + n < c.len() && c[i + n] == ch {
            n += 1;
        }
        let code = match (ch, n) {
            ('y', 4..) => "%Y",
            ('y', _) => "%y",
            ('M', 4..) => "%B",
            ('M', 3) => "%b",
            ('M', 2) => "%m",
            ('M', _) => "%-m",
            ('d', 4..) => "%A",
            ('d', 3) => "%a",
            ('d', 2) => "%d",
            ('d', _) => "%-d",
            ('H', 2..) => "%H",
            ('H', _) => "%-H",
            ('h', 2..) => "%I",
            ('h', _) => "%-I",
            ('m', 2..) => "%M",
            ('m', _) => "%-M",
            ('s', 2..) => "%S",
            ('s', _) => "%-S",
            ('A', _) | ('P', _) if f[..].contains("AP") => "%p",
            ('a', _) | ('p', _) if f[..].contains("ap") => "%P",
            ('w', _) => "%V",
            ('t', _) => "%Z",
            _ => "",
        };
        if code.is_empty() {
            out.extend(std::iter::repeat_n(ch, n));
            i += n;
        } else if matches!(ch, 'A' | 'a') && c.get(i + 1).is_some_and(|x| *x == 'P' || *x == 'p') {
            out.push_str(code);
            i += 2; // AP / ap as one code
        } else {
            out.push_str(code);
            i += n;
        }
    }
    out
}

/// Lines that held a {tag} and came out blank (an empty CSV field) are dropped, so addresses close up.
/// A right-to-left line in the order its glyphs go down left to right: the line reversed, then runs of digits and
/// Latin letters (with . , : - / inside them) turned back so "שלום 2026" keeps 2026.
/// ponytail: no Unicode bidi or Arabic joining; add the unicode-bidi crate + shaping if Arabic users need it
pub fn rtl_line(line: &str) -> String {
    let mut cs: Vec<char> = line.chars().rev().collect();
    let ltr = |c: char| c.is_ascii_alphanumeric();
    let mut i = 0;
    while i < cs.len() {
        if !ltr(cs[i]) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j + 1 < cs.len() && (ltr(cs[j + 1]) || (".,:-/".contains(cs[j + 1]) && j + 2 < cs.len() && ltr(cs[j + 2]))) {
            j += 1;
        }
        cs[i..=j].reverse();
        i = j + 1;
    }
    cs.into_iter().collect()
}

pub fn drop_empty_lines(template: &str, filled: &str) -> String {
    let (t, f): (Vec<&str>, Vec<&str>) = (template.lines().collect(), filled.lines().collect());
    if t.len() != f.len() {
        return filled.to_string();
    }
    t.iter().zip(&f).filter(|(a, b)| !(a.contains('{') && b.trim().is_empty())).map(|(_, b)| *b).collect::<Vec<_>>().join("\n")
}

/// Where along an open or closed outline a glyph sits: the point at distance `d` and the direction there.
pub fn along_path(path: &[Pt], d: f64) -> (Pt, f64) {
    let total: f64 = path.windows(2).map(|w| geom::dist(w[0], w[1])).sum();
    let mut left = d.clamp(0.0, total);
    for w in path.windows(2) {
        let l = geom::dist(w[0], w[1]);
        if left <= l || std::ptr::eq(w, path.windows(2).last().unwrap()) {
            let u = if l > 0.0 { (left / l).clamp(0.0, 1.0) } else { 0.0 };
            return (pt(w[0].x + (w[1].x - w[0].x) * u, w[0].y + (w[1].y - w[0].y) * u), (w[1].y - w[0].y).atan2(w[1].x - w[0].x));
        }
        left -= l;
    }
    (path[0], 0.0)
}

/// A QR code as filled squares (one closed contour per dark run), `size` mm across, bottom-left at `at`.
pub fn qr_shapes(text: &str, size: f64, at: Pt) -> Result<Vec<Contour>, String> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| format!("QR: {e}"))?;
    let n = code.width();
    Ok(module_shapes(n, n, |x, y| code[(x, y)] == qrcode::Color::Dark, size, at))
}

/// A Data Matrix (ECC 200, what fibre lasers mark on parts) as filled squares, `size` mm wide, bottom-left at `at`.
pub fn datamatrix_shapes(text: &str, size: f64, at: Pt) -> Result<Vec<Contour>, String> {
    let code = datamatrix::DataMatrix::encode(text.as_bytes(), datamatrix::SymbolList::default()).map_err(|e| format!("Data Matrix: {e:?}"))?;
    let b = code.bitmap();
    let (w, bits) = (b.width(), b.bits());
    Ok(module_shapes(w, b.height(), |x, y| bits[y * w + x], size, at))
}

/// A `w` × `h` module code (row 0 at the top) as rectangles: dark modules side by side in a row are one rectangle
/// (fewer shapes, the same engraving). `size` = the width with one module of quiet zone each side.
fn module_shapes(w: usize, h: usize, dark: impl Fn(usize, usize) -> bool, size: f64, at: Pt) -> Vec<Contour> {
    let cell = size / (w as f64 + 2.0);
    let top = at.y + (h as f64 + 1.0) * cell;
    let mut out = Vec::new();
    for y in 0..h {
        let mut x = 0;
        while x < w {
            if dark(x, y) {
                let x0 = x;
                while x < w && dark(x, y) {
                    x += 1;
                }
                let (px0, px1) = (at.x + (x0 as f64 + 1.0) * cell, at.x + (x as f64 + 1.0) * cell);
                out.push(geom::make_rect(px0, top - (y as f64 + 1.0) * cell, px1, top - y as f64 * cell));
            } else {
                x += 1;
            }
        }
    }
    out
}

/// What a placed code was made from: "qr", "datamatrix" or a `BARCODES` kind; `text` may hold tags ({n}, {date},
/// CSV columns); `size` = width (QR, Data Matrix) or bar height (barcodes), `module` = narrowest bar.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeSpec {
    pub kind: String,
    pub text: String,
    pub size: f64,
    #[serde(default)]
    pub module: f64,
    /// added to the serial number and the CSV row, as `TextSpec.offset`
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub offset: u32,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

impl CodeSpec {
    pub fn has_vars(&self) -> bool {
        self.text.find('{').is_some_and(|i| self.text[i..].contains('}'))
    }
    /// The code for `text` (the tags already filled in), bottom-left at 0, 0.
    pub fn shapes(&self, text: &str) -> Result<Vec<Contour>, String> {
        match self.kind.as_str() {
            "qr" => qr_shapes(text, self.size, pt(0.0, 0.0)),
            "datamatrix" => datamatrix_shapes(text, self.size, pt(0.0, 0.0)),
            k => barcode_shapes(k, text, self.size, if self.module > 0.01 { self.module } else { 0.3 }, pt(0.0, 0.0)).map(|(bars, _)| bars),
        }
    }
}

/// Barcode kinds the Barcode dialog offers.
pub const BARCODES: [(&str, &str); 8] = [
    ("code128", "Code 128 (any text)"),
    ("ean13", "EAN-13 (12 or 13 digits)"),
    ("upca", "UPC-A (11 or 12 digits)"),
    ("ean8", "EAN-8 (7 or 8 digits)"),
    ("code39", "Code 39 (A–Z, 0–9, - . $ / + % space)"),
    ("code93", "Code 93 (A–Z, 0–9, - . $ / + % space)"),
    ("codabar", "Codabar (A…D, digits, - $ : / . +)"),
    ("itf", "Interleaved 2 of 5 (digits)"),
];

/// A barcode as filled bars (one closed contour per bar), `height` mm tall, `module` mm per narrowest bar, with a
/// 10-module quiet zone each side; bottom-left at `at`. Returns the bars and the total width.
pub fn barcode_shapes(kind: &str, text: &str, height: f64, module: f64, at: Pt) -> Result<(Vec<Contour>, f64), String> {
    use barcoders::sym::{codabar::Codabar, code128::Code128, code39::Code39, code93::Code93, ean13::EAN13, ean8::EAN8, tf::TF};
    let bits: Vec<u8> = match kind {
        // character set B (U+0181 is barcoders' marker for it): letters, digits and punctuation
        "code128" => Code128::new(format!("\u{0181}{text}")).map_err(|e| format!("Code 128: {e}"))?.encode(),
        "ean13" => EAN13::new(text.trim()).map_err(|e| format!("EAN-13: {e}"))?.encode(),
        // UPC-A is EAN-13 with a leading 0
        "upca" => EAN13::new(format!("0{}", text.trim())).map_err(|e| format!("UPC-A: {e}"))?.encode(),
        "ean8" => EAN8::new(text.trim()).map_err(|e| format!("EAN-8: {e}"))?.encode(),
        "code93" => Code93::new(text.to_ascii_uppercase()).map_err(|e| format!("Code 93: {e}"))?.encode(),
        "codabar" => Codabar::new(text.trim().to_ascii_uppercase()).map_err(|e| format!("Codabar: {e}"))?.encode(),
        "itf" => TF::interleaved(text.trim()).map_err(|e| format!("Interleaved 2 of 5: {e}"))?.encode(),
        _ => Code39::new(text.to_ascii_uppercase()).map_err(|e| format!("Code 39: {e}"))?.encode(),
    };
    let quiet = 10.0;
    let mut out = Vec::new();
    let mut i = 0;
    while i < bits.len() {
        if bits[i] == 1 {
            let j = (i..bits.len()).find(|&k| bits[k] == 0).unwrap_or(bits.len());
            let x0 = at.x + (quiet + i as f64) * module;
            out.push(geom::make_rect(x0, at.y, at.x + (quiet + j as f64) * module, at.y + height));
            i = j;
        } else {
            i += 1;
        }
    }
    Ok((out, (bits.len() as f64 + 2.0 * quiet) * module))
}

/// A LightBurn material library (.clb) of `entries`: a `<Material>` per material, an `<Entry>` per thickness and
/// description, the cut setting inside (what `parse_clb` reads back).
pub fn write_clb(entries: &[ClbEntry]) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;");
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<LightBurnLibrary DisplayUnits=\"0\">\n");
    let mut materials: Vec<&str> = Vec::new();
    for e in entries {
        if !materials.contains(&e.material.as_str()) {
            materials.push(&e.material);
        }
    }
    for m in materials {
        x += &format!("    <Material name=\"{}\">\n", esc(m));
        for e in entries.iter().filter(|e| e.material == m) {
            let t = if e.thickness.trim().is_empty() { "-1" } else { e.thickness.trim() };
            x += &format!("        <Entry Thickness=\"{}\" Desc=\"{}\" NoThickTitle=\"\">\n", esc(t), esc(&e.desc));
            x += &format!("            <CutSetting type=\"{}\">\n", esc(&e.kind));
            x += &format!("                <maxPower Value=\"{}\"/>\n                <speed Value=\"{}\"/>\n", e.power, e.speed);
            if e.passes > 1 {
                x += &format!("                <numPasses Value=\"{}\"/>\n", e.passes);
            }
            x += &format!("                <interval Value=\"{}\"/>\n            </CutSetting>\n        </Entry>\n", e.interval);
        }
        x += "    </Material>\n";
    }
    x + "</LightBurnLibrary>\n"
}

/// One cut setting read from a LightBurn material library (.clb).
#[derive(Clone, Debug, PartialEq)]
pub struct ClbEntry {
    pub name: String,
    /// the library's grouping: material › thickness (mm, "-1" = any) › description
    pub material: String,
    pub thickness: String,
    pub desc: String,
    /// "Cut" / "Scan" / "Image" as LightBurn writes it
    pub kind: String,
    pub speed: f64,
    pub power: f64,
    pub passes: u32,
    pub interval: f64,
}

/// `.clb` = XML: `<Material name>` › `<Entry Thickness Desc>` › `<CutSetting type>` with `<speed>`, `<maxPower>`,
/// `<numPasses>`, `<interval>` children (`Value` attributes). ponytail: a tag scan, like the LBRN2 reader.
pub fn parse_clb(xml: &str) -> Vec<ClbEntry> {
    fn attr<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
        let i = tag.find(&format!(" {key}=\""))? + key.len() + 3;
        tag[i..].find('"').map(|e| &tag[i..i + e])
    }
    let mut out = Vec::new();
    let (mut material, mut thickness, mut desc) = (String::new(), String::new(), String::new());
    let mut cur: Option<ClbEntry> = None;
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        let Some(j) = rest.find('>') else { break };
        let tag = &rest[1..j];
        rest = &rest[j + 1..];
        if tag.starts_with("Material ") {
            material = attr(tag, "name").unwrap_or("").to_string();
        } else if tag.starts_with("Entry ") {
            thickness = attr(tag, "Thickness").unwrap_or("").to_string();
            desc = attr(tag, "Desc").unwrap_or("").to_string();
        } else if tag.starts_with("CutSetting") && !tag.starts_with("CutSetting_") && !tag.starts_with('/') {
            let kind = attr(tag, "type").unwrap_or("Cut").to_string();
            let name = [material.as_str(), thickness.as_str(), desc.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
            cur = Some(ClbEntry { name, material: material.clone(), thickness: thickness.clone(), desc: desc.clone(), kind, speed: 0.0, power: 0.0, passes: 1, interval: 0.1 });
        } else if tag.starts_with("/CutSetting") {
            if let Some(e) = cur.take() {
                if e.speed > 0.0 {
                    out.push(e);
                }
            }
        } else if let Some(e) = &mut cur {
            let v = attr(tag, "Value").and_then(|v| v.parse::<f64>().ok());
            if let Some(v) = v {
                if tag.starts_with("speed") {
                    e.speed = v;
                } else if tag.starts_with("maxPower") {
                    e.power = v;
                } else if tag.starts_with("numPasses") {
                    e.passes = v.max(1.0) as u32;
                } else if tag.starts_with("interval") {
                    e.interval = v;
                } else if tag.starts_with("name") {
                    // a named layer inside the entry: LightBurn keeps the layer name here
                }
            } else if tag.starts_with("name ") {
                if let Some(n) = attr(tag, "Value") {
                    if !n.is_empty() && !e.name.contains(n) {
                        e.name = format!("{} · {n}", e.name);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_expand_and_the_rest_stays() {
        assert_eq!(expand_vars("Part {n} of {n:4} on {date} at {time} {x}", 7, "2026-10-20", "14:05"), "Part 7 of 0007 on 2026-10-20 at 14:05 {x}");
        assert_eq!(expand_vars("open {brace", 1, "", ""), "open {brace");
        assert_eq!(expand_vars("{n:99999999999}", 7, "", "").len(), 32, "a huge width is capped");
        assert!(TextSpec {
            text: "A{n}".into(),
            height: 10.0,
            bold: false,
            spacing: 0.0,
            line_gap: 1.0,
            align: 0,
            path_id: 0,
            serial: 0,
            offset: 0,
            fit_keep: false,
            font: String::new(),
            weld: false,
            width: 100.0,
            slant: 0.0,
            path_offset: 0.0,
            path_inside: false,
            fit: 0.0,
            upper: false,
            valign: 0,
            rtl: false,
            distort: false
        }
        .has_vars());
    }

    #[test]
    fn lightburn_date_codes_become_strftime() {
        assert_eq!(qt_date_format("yyyy-MM-dd"), "%Y-%m-%d");
        assert_eq!(qt_date_format("d MMM yy, h:mm AP"), "%-d %b %y, %-I:%M %p");
        assert_eq!(qt_date_format("dddd 'at' HH:mm"), "%A at %H:%M");
        assert_eq!(qt_date_format("%d/%m"), "%d/%m");
    }

    #[test]
    fn right_to_left_lines_keep_numbers_and_latin_words() {
        assert_eq!(rtl_line("אבג"), "גבא");
        assert_eq!(rtl_line("שלום 2026"), "2026 םולש");
        assert_eq!(rtl_line("א 12.5 ב GBurn"), "GBurn ב 12.5 א");
    }

    #[test]
    fn hex_serials_layer_codes_and_blank_lines() {
        assert_eq!(expand_vars("{n:x} {n:X4} {n:5}", 255, "", ""), "ff 00FF 00255");
        let mut l = crate::Layer::new("Cut", [255, 0, 0], crate::Mode::Line, 12.5, 80.0);
        l.passes = 3;
        assert_eq!(layer_tags("{layer:1} {speed:1} mm/s {power:1}% ×{passes:1} {speed:9} {x:1}", &[l]), "Cut 12.5 mm/s 80% ×3 {speed:9} {x:1}");
        assert_eq!(drop_empty_lines("{name}\n{company}\n{town}", "Ann\n \nLeeds"), "Ann\nLeeds");
    }

    #[test]
    fn a_csv_fills_its_columns_into_the_text() {
        let m = Merge::parse("Name,City,Note\r\nAlice,Oslo,\"likes \"\"oak\"\", maple\"\n\nBob;x,Rome,\n");
        assert_eq!(m.headers, ["Name", "City", "Note"]);
        assert_eq!(m.rows.len(), 2, "{:?}", m.rows);
        assert_eq!(m.rows[0][2], "likes \"oak\", maple");
        assert_eq!(m.fill("Hi {name} from {CITY} #{n}", 1), "Hi Bob;x from Rome #{n}");
        assert_eq!(m.fill("{name}", 9), "{name}", "no such row: unchanged");
        let semi = Merge::parse("a;b\n1;2\n");
        assert_eq!((semi.headers.len(), semi.rows[0][1].as_str()), (2, "2"));
        assert!(TextSpec {
            text: "To {name}".into(),
            height: 5.0,
            bold: false,
            spacing: 0.0,
            line_gap: 1.0,
            align: 0,
            path_id: 0,
            serial: 0,
            offset: 0,
            fit_keep: false,
            font: String::new(),
            weld: false,
            width: 100.0,
            slant: 0.0,
            path_offset: 0.0,
            path_inside: false,
            fit: 0.0,
            upper: false,
            valign: 0,
            rtl: false,
            distort: false
        }
        .has_vars());
    }

    #[test]
    fn glyphs_find_their_spot_along_a_path() {
        let path = [pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0)];
        let (p, a) = along_path(&path, 5.0);
        assert!((p.x - 5.0).abs() < 1e-9 && a.abs() < 1e-9);
        let (p, a) = along_path(&path, 15.0);
        assert!((p.x - 10.0).abs() < 1e-9 && (p.y - 5.0).abs() < 1e-9 && (a - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        let (p, _) = along_path(&path, 99.0);
        assert_eq!(p, pt(10.0, 10.0));
    }

    #[test]
    fn a_data_matrix_reads_back_from_its_squares() {
        let (size, at) = (12.0, pt(5.0, 5.0));
        let d = datamatrix_shapes("GB-0042", size, at).unwrap();
        let code = datamatrix::DataMatrix::encode(b"GB-0042", datamatrix::SymbolList::default()).unwrap().bitmap();
        let (w, h) = (code.width(), code.height());
        let cell = size / (w as f64 + 2.0);
        // light every module whose centre lies in one of our rectangles, then decode that grid
        let mut px = vec![false; w * h];
        for (i, p) in px.iter_mut().enumerate() {
            let (x, y) = (at.x + (i % w) as f64 * cell + 1.5 * cell, at.y + (h - 1 - i / w) as f64 * cell + 1.5 * cell);
            *p = d.iter().any(|c| {
                let b = crate::bounds(c);
                x > b.x0 && x < b.x1 && y > b.y0 && y < b.y1
            });
        }
        assert_eq!(datamatrix::DataMatrix::decode(&px, w).unwrap(), b"GB-0042");
    }

    #[test]
    fn a_qr_code_is_dark_rectangles_inside_its_square() {
        let q = qr_shapes("https://gburn.app", 30.0, pt(10.0, 10.0)).unwrap();
        assert!(q.len() > 50, "{}", q.len());
        let mut r = crate::Rect::default();
        for c in &q {
            r.add_rect(&crate::bounds(c));
        }
        assert!(r.x0 >= 10.0 && r.y0 >= 10.0 && r.x1 <= 40.0 + 1e-9 && r.y1 <= 40.0 + 1e-9, "{r:?}");
        // the finder pattern's outer ring: a 7-module-wide bar in the top row
        let cell = 30.0 / (q.len() as f64).max(1.0); // not the cell size; just make sure a wide bar exists
        let _ = cell;
        let widest = q.iter().map(|c| crate::bounds(c).w()).fold(0.0, f64::max);
        assert!(widest > 5.0, "{widest}");
    }

    #[test]
    fn barcodes_are_bars_of_whole_modules() {
        // Code 128: start 11 + 5 characters × 11 + check 11 + stop 13 = 90 modules
        let (bars, w) = barcode_shapes("code128", "GBurn", 15.0, 0.3, pt(0.0, 0.0)).unwrap();
        assert!((w - (90.0 + 20.0) * 0.3).abs() < 1e-9, "{w}");
        for b in &bars {
            let r = crate::bounds(b);
            let modules = r.w() / 0.3;
            assert!((modules - modules.round()).abs() < 1e-9 && (1.0..=4.0).contains(&modules.round()), "{modules}");
            assert!((r.h() - 15.0).abs() < 1e-9 && r.x0 >= 3.0 - 1e-9);
        }
        // EAN-13: 95 modules, 30 bars; the check digit is worked out from 12 digits
        let (bars, w) = barcode_shapes("ean13", "750103131130", 20.0, 0.33, pt(0.0, 0.0)).unwrap();
        assert!((w - 115.0 * 0.33).abs() < 1e-9 && bars.len() == 30, "{} bars", bars.len());
        assert!(barcode_shapes("ean13", "12AB", 20.0, 0.33, pt(0.0, 0.0)).is_err());
        assert!(barcode_shapes("code39", "gburn-1", 10.0, 0.3, pt(0.0, 0.0)).is_ok(), "lower case goes up");
        for (kind, text) in [("upca", "03600029145"), ("ean8", "5512345"), ("code93", "GB-42"), ("codabar", "A40156B"), ("itf", "123456")] {
            let (bars, _) = barcode_shapes(kind, text, 10.0, 0.3, pt(0.0, 0.0)).unwrap_or_else(|e| panic!("{kind}: {e}"));
            assert!(bars.len() > 10, "{kind}: {} bars", bars.len());
        }
    }

    #[test]
    fn a_lightburn_library_reads_its_cut_settings() {
        let clb = r#"<?xml version="1.0"?><LightBurnLibrary>
<Material name="Birch ply"><Entry Thickness="3" Desc="through cut"><CutSetting type="Cut"><index Value="0"/><name Value="C00"/><maxPower Value="80"/><speed Value="12"/><numPasses Value="2"/></CutSetting></Entry>
<Entry Thickness="3" Desc="engrave"><CutSetting type="Scan"><index Value="1"/><maxPower Value="35"/><speed Value="300"/><interval Value="0.08"/></CutSetting></Entry></Material>
<Material name="Acrylic"><Entry Thickness="-1" Desc="cast"><CutSetting type="Cut"><maxPower Value="90"/><speed Value="8"/></CutSetting></Entry></Material></LightBurnLibrary>"#;
        let v = parse_clb(clb);
        assert_eq!(v.len(), 3, "{v:?}");
        assert_eq!(v[0].name, "Birch ply · 3 · through cut · C00");
        assert_eq!((v[0].kind.as_str(), v[0].speed, v[0].power, v[0].passes), ("Cut", 12.0, 80.0, 2));
        assert_eq!((v[1].kind.as_str(), v[1].interval), ("Scan", 0.08));
        assert_eq!(v[2].speed, 8.0);
        assert!(parse_clb("nonsense").is_empty());
        assert_eq!((v[1].material.as_str(), v[1].thickness.as_str(), v[1].desc.as_str()), ("Birch ply", "3", "engrave"));
        // our own library reads back the same (names, kinds, numbers)
        let back = parse_clb(&write_clb(&v));
        assert_eq!(back.len(), 3);
        for (a, b) in v.iter().zip(&back) {
            assert_eq!((&a.material, &a.thickness, &a.desc, &a.kind, a.speed, a.power, a.passes), (&b.material, &b.thickness, &b.desc, &b.kind, b.speed, b.power, b.passes));
        }
    }
}
