//! Image engraving: a picture kept as 8-bit grey (base64 in the file), turned into scan lines by a dither or by
//! power modulation (greyscale). Rows run along X at the layer's interval, alternating direction; `overscan` adds
//! a dead run at both ends so the head is up to speed where the burn starts.

use crate::plan::Stroke;
use crate::{pt, Affine, Pt, Rect};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Dither {
    /// black where darker than mid grey
    Threshold,
    /// error diffusion (LightBurn's "Dither")
    #[default]
    FloydSteinberg,
    /// Jarvis-Judice-Ninke: smoother error diffusion
    Jarvis,
    /// 8×8 Bayer pattern
    Ordered,
    /// power follows darkness (fibre / CO2 on wood)
    Grayscale,
    /// Stucki: error diffusion over a wider area, the smoothest tones
    Stucki,
    /// clustered dots that grow from each cell's centre (newsprint look; holds up on wood and stone)
    Halftone,
    /// Atkinson: passes on only 3/4 of the error, so lights and darks stay clean (more contrast)
    Atkinson,
    /// only the edges (a line drawing of the picture)
    Sketch,
    /// 3D slices: each pass burns everything darker than its own level, so darker = more passes = deeper (relief on a
    /// fibre laser; set Passes to the number of levels)
    Slices,
    /// newsprint: round dots on a fixed 45° screen of 8 px cells (the halftone without settings)
    Newsprint,
}

impl Dither {
    pub const ALL: [(Dither, &'static str); 11] = [
        (Dither::Threshold, "Threshold"),
        (Dither::FloydSteinberg, "Dither (Floyd–Steinberg)"),
        (Dither::Jarvis, "Dither (Jarvis)"),
        (Dither::Stucki, "Dither (Stucki)"),
        (Dither::Atkinson, "Dither (Atkinson)"),
        (Dither::Sketch, "Sketch (edges)"),
        (Dither::Slices, "3D slices (a level per pass)"),
        (Dither::Ordered, "Ordered"),
        (Dither::Halftone, "Halftone"),
        (Dither::Newsprint, "Newsprint"),
        (Dither::Grayscale, "Grayscale (power)"),
    ];
    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|a| a.0 == self).map_or("", |a| a.1)
    }
}

/// A 3D model (STL or 3MF bytes) seen from above as a depth map: the top white, the lowest black, about `cells`
/// pixels, and the model's footprint (its X / Y size in its own units, taken as mm). Darker burns deeper with
/// Grayscale or 3D slices.
pub fn depth_map(bytes: &[u8], is_3mf: bool, cells: usize) -> Result<(Raster, f64, f64), String> {
    use gcarve_core::relief;
    let tris = if is_3mf { gcarve_core::threemf::load_3mf(bytes)? } else { relief::load_stl(bytes)? };
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for t in &tris {
        for p in [t.a, t.b, t.c] {
            (lo[0], lo[1], hi[0], hi[1]) = (lo[0].min(p.x), lo[1].min(p.y), hi[0].max(p.x), hi[1].max(p.y));
        }
    }
    let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
    if !(w > 1e-9 && h > 1e-9) {
        return Err("the model is flat from above".into());
    }
    let mut r = relief::Relief { w, h, ..Default::default() };
    relief::rasterize_relief(&tris, &mut r, cells.min(4_000_000));
    let (nx, ny) = (r.nx as usize, r.ny as usize);
    if nx == 0 || ny == 0 {
        return Err("no surface to map".into());
    }
    // the relief's rows run up from the bottom; a picture's row 0 is the top
    let px: Vec<u8> = (0..ny).rev().flat_map(|iy| r.height[iy * nx..(iy + 1) * nx].iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect::<Vec<_>>()).collect();
    Ok((Raster::from_gray(nx as u32, ny as u32, &px), w, h))
}

/// A picture in a shape: grey pixels, row 0 at the top.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Raster {
    pub w: u32,
    pub h: u32,
    /// base64 of w × h bytes, 0 = black
    pub gray: String,
    /// adjustments applied before the dither: −100..100, −100..100, 0.2..5 (1 = none)
    #[serde(default)]
    pub brightness: f64,
    #[serde(default)]
    pub contrast: f64,
    #[serde(default = "one")]
    pub gamma: f64,
    /// sharpen (unsharp mask), 0..100
    #[serde(default)]
    pub sharpen: f64,
    /// Halftone screen: cell size mm (0 = 6 scan lines) and angle °
    #[serde(default)]
    pub halftone: [f64; 2],
    /// denoise (3 × 3 median, blended), 0..100
    #[serde(default)]
    pub denoise: f64,
}

fn one() -> f64 {
    1.0
}

impl Raster {
    /// From PNG / JPEG / BMP / GIF / WebP bytes, scaled down to `max_px` on the long side (transparent = white).
    pub fn decode(bytes: &[u8], max_px: u32) -> Result<Raster, String> {
        let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
        let img = if img.width().max(img.height()) > max_px { img.resize(max_px, max_px, image::imageops::FilterType::Triangle) } else { img };
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        if w == 0 || h == 0 {
            return Err("empty picture".into());
        }
        let px: Vec<u8> = rgba
            .pixels()
            .map(|p| {
                let [r, g, b, a] = p.0;
                let l = (0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b)) * f64::from(a) / 255.0 + 255.0 * (1.0 - f64::from(a) / 255.0);
                l.round().clamp(0.0, 255.0) as u8
            })
            .collect();
        Ok(Raster::from_gray(w, h, &px))
    }
    /// The picture as it is (no adjustments) as a PNG.
    pub fn png(&self) -> Option<Vec<u8>> {
        let img = image::GrayImage::from_raw(self.w, self.h, self.pixels()?)?;
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).ok()?;
        Some(out.into_inner())
    }
    pub fn pixels(&self) -> Option<Vec<u8>> {
        let px = gcarve_core::photo::b64_decode(&self.gray);
        ((self.w as usize).checked_mul(self.h as usize) == Some(px.len()) && self.w > 0).then_some(px)
    }
    /// The box of the pixels that burn (darker than near-white; lighter than near-black when `negative`) as
    /// fractions from the top-left [x0, y0, x1, y1]; None when nothing burns. Frame uses it so a picture's blank
    /// margins (white or transparent) aren't framed.
    pub fn ink_box(&self, negative: bool) -> Option<[f64; 4]> {
        let px = self.pixels()?;
        let (w, h) = (self.w as usize, self.h as usize);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for (i, v) in px.iter().enumerate() {
            if if negative { *v > 5 } else { *v < 250 } {
                let (x, y) = (i % w, i / w);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
        (x1 > x0).then(|| [x0 as f64 / w as f64, y0 as f64 / h as f64, x1 as f64 / w as f64, y1 as f64 / h as f64])
    }
    /// the Halftone screen in pixels of a grid at `interval` mm: [cell px, angle °]
    pub fn screen_px(&self, interval: f64) -> [f64; 2] {
        [if self.halftone[0] > 0.0 { self.halftone[0] / interval.max(0.01) } else { 6.0 }, self.halftone[1]]
    }
    /// The pixels resized to `w` × `h` (Lanczos; averages when shrinking), adjustments kept
    pub fn resampled(&self, w: u32, h: u32) -> Option<Raster> {
        let img = image::GrayImage::from_raw(self.w, self.h, self.pixels()?)?;
        let out = image::imageops::resize(&img, w.max(1), h.max(1), image::imageops::FilterType::Lanczos3);
        Some(Raster { w: out.width(), h: out.height(), gray: gcarve_core::photo::b64_encode(out.as_raw()), ..self.clone() })
    }
    pub fn from_gray(w: u32, h: u32, px: &[u8]) -> Raster {
        Raster { w, h, gray: gcarve_core::photo::b64_encode(px), brightness: 0.0, contrast: 0.0, gamma: 1.0, sharpen: 0.0, halftone: [0.0, 0.0], denoise: 0.0 }
    }
    /// The picture placed over `r`, moved by `m` when that turns, mirrors or skews it: the pixels are resampled
    /// (nearest) onto a new upright grid round the moved box, white outside, same pixel density. None = nothing to
    /// bake (a plain move / positive scale: the box alone carries it) or no pixels.
    pub fn transformed(&self, r: &Rect, m: &Affine) -> Option<(Raster, Rect)> {
        if m.b.abs() < 1e-12 && m.c.abs() < 1e-12 && m.a > 0.0 && m.d > 0.0 {
            return None;
        }
        let px = self.pixels()?;
        let det = m.a * m.d - m.b * m.c;
        if det.abs() < 1e-12 || r.is_empty() {
            return None;
        }
        let corners = [pt(r.x0, r.y0), pt(r.x1, r.y0), pt(r.x1, r.y1), pt(r.x0, r.y1)].map(|p| m.apply(p));
        let mut nr = Rect::default();
        corners.iter().for_each(|p| nr.add(*p));
        // pixels per mm after the move: the old density times the area change, spread evenly
        let k = ((self.w * self.h) as f64 / (r.w() * r.h()) / det.abs()).sqrt();
        let (mut nw, mut nh) = ((nr.w() * k).round().max(1.0), (nr.h() * k).round().max(1.0));
        let cap = 2000.0 / nw.max(nh);
        if cap < 1.0 {
            (nw, nh) = ((nw * cap).round().max(1.0), (nh * cap).round().max(1.0));
        }
        let (nw, nh) = (nw as usize, nh as usize);
        // inverse of m
        let inv = |p: Pt| {
            let (x, y) = (p.x - m.e, p.y - m.f);
            pt((m.d * x - m.c * y) / det, (-m.b * x + m.a * y) / det)
        };
        let mut out = vec![255u8; nw * nh];
        for j in 0..nh {
            for i in 0..nw {
                let q = inv(pt(nr.x0 + (i as f64 + 0.5) * nr.w() / nw as f64, nr.y1 - (j as f64 + 0.5) * nr.h() / nh as f64));
                let (u, v) = ((q.x - r.x0) / r.w() * self.w as f64, (r.y1 - q.y) / r.h() * self.h as f64);
                if u >= 0.0 && v >= 0.0 && u < self.w as f64 && v < self.h as f64 {
                    out[j * nw + i] = px[v as usize * self.w as usize + u as usize];
                }
            }
        }
        Some((Raster { gray: gcarve_core::photo::b64_encode(&out), w: nw as u32, h: nh as u32, ..self.clone() }, nr))
    }
    /// The picture (placed over `r`) with every pixel outside the closed `polys` turned white: a picture cut to a shape.
    pub fn masked(&self, r: &Rect, polys: &[Vec<Pt>]) -> Option<Raster> {
        let mut px = self.pixels()?;
        let keep = inside_mask(polys, r, self.w as usize, self.h as usize);
        for (p, k) in px.iter_mut().zip(keep) {
            if !k {
                *p = 255;
            }
        }
        Some(Raster { gray: gcarve_core::photo::b64_encode(&px), ..self.clone() })
    }
    /// The part of the picture (placed over `r`) inside `keep`, snapped out to whole pixels, and where it sits.
    /// None = no overlap or no pixels.
    pub fn cropped(&self, r: &Rect, keep: &Rect) -> Option<(Raster, Rect)> {
        let px = self.pixels()?;
        let (w, h) = (self.w as usize, self.h as usize);
        let (pw, ph) = ((r.x1 - r.x0) / w as f64, (r.y1 - r.y0) / h as f64);
        let col = |x: f64| ((x - r.x0) / pw).clamp(0.0, w as f64);
        let row = |y: f64| ((r.y1 - y) / ph).clamp(0.0, h as f64); // row 0 = the top
        let (c0, c1) = (col(keep.x0).floor() as usize, col(keep.x1).ceil() as usize);
        let (r0, r1) = (row(keep.y1).floor() as usize, row(keep.y0).ceil() as usize);
        if c1 <= c0 || r1 <= r0 {
            return None;
        }
        let out: Vec<u8> = (r0..r1).flat_map(|y| px[y * w + c0..y * w + c1].to_vec()).collect();
        let at = Rect { x0: r.x0 + c0 as f64 * pw, x1: r.x0 + c1 as f64 * pw, y0: r.y1 - r1 as f64 * ph, y1: r.y1 - r0 as f64 * ph };
        Some((Raster { w: (c1 - c0) as u32, h: (r1 - r0) as u32, gray: gcarve_core::photo::b64_encode(&out), ..self.clone() }, at))
    }
    /// The pixels with brightness, contrast and gamma applied (a 256-entry table).
    pub fn adjusted(&self) -> Option<Vec<u8>> {
        let mut px = self.pixels()?;
        if self.denoise > 0.0 {
            // a 3 × 3 median takes out specks (scanner dust, JPEG noise), blended in by the amount
            let (w, h) = (self.w as usize, self.h as usize);
            let a = self.denoise.clamp(0.0, 100.0) / 100.0;
            let src = px.clone();
            for y in 0..h {
                for x in 0..w {
                    let mut n: Vec<u8> = Vec::with_capacity(9);
                    for yy in y.saturating_sub(1)..(y + 2).min(h) {
                        for xx in x.saturating_sub(1)..(x + 2).min(w) {
                            n.push(src[yy * w + xx]);
                        }
                    }
                    n.sort_unstable();
                    let med = n[n.len() / 2] as f64;
                    let v = src[y * w + x] as f64;
                    px[y * w + x] = (v + a * (med - v)).round() as u8;
                }
            }
        }
        if self.sharpen > 0.0 {
            // unsharp mask: push each pixel away from its 3 × 3 neighbourhood's mean
            let (w, h) = (self.w as usize, self.h as usize);
            let a = self.sharpen.clamp(0.0, 100.0) / 50.0;
            let src = px.clone();
            for y in 0..h {
                for x in 0..w {
                    let (mut sum, mut n) = (0.0, 0.0);
                    for yy in y.saturating_sub(1)..(y + 2).min(h) {
                        for xx in x.saturating_sub(1)..(x + 2).min(w) {
                            sum += src[yy * w + xx] as f64;
                            n += 1.0;
                        }
                    }
                    let v = src[y * w + x] as f64;
                    px[y * w + x] = (v + a * (v - sum / n)).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        if self.brightness == 0.0 && self.contrast == 0.0 && (self.gamma - 1.0).abs() < 1e-9 {
            return Some(px);
        }
        let c = (self.contrast.clamp(-100.0, 100.0) + 100.0) / 100.0;
        let c = if c > 1.0 { c * c } else { c }; // +100 = ×4 steepness
        let lut: Vec<u8> = (0..256)
            .map(|v| {
                let mut x = v as f64 / 255.0;
                x = ((x - 0.5) * c + 0.5).clamp(0.0, 1.0);
                x = (x + self.brightness.clamp(-100.0, 100.0) / 100.0).clamp(0.0, 1.0);
                x = x.powf(1.0 / self.gamma.clamp(0.2, 5.0));
                (x * 255.0).round() as u8
            })
            .collect();
        for p in &mut px {
            *p = lut[*p as usize];
        }
        Some(px)
    }
    pub fn aspect(&self) -> f64 {
        self.w as f64 / self.h.max(1) as f64
    }
}

const BAYER: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// The perspective map (3 × 3, row-major, h[8] = 1) taking the four `from` points onto the four `to` points;
/// None when three of them line up. (Direct linear transform: 8 equations, Gaussian elimination.)
pub fn homography(from: [Pt; 4], to: [Pt; 4]) -> Option<[f64; 9]> {
    let mut m = [[0.0f64; 9]; 8];
    for k in 0..4 {
        let (x, y, u, v) = (from[k].x, from[k].y, to[k].x, to[k].y);
        m[2 * k] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        m[2 * k + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    for c in 0..8 {
        let p = (c..8).max_by(|&a, &b| m[a][c].abs().total_cmp(&m[b][c].abs()))?;
        if m[p][c].abs() < 1e-12 {
            return None;
        }
        m.swap(c, p);
        let pivot = m[c];
        for (r, row) in m.iter_mut().enumerate() {
            if r != c {
                let f = row[c] / pivot[c];
                for (x, p) in row.iter_mut().zip(pivot).skip(c) {
                    *x -= f * p;
                }
            }
        }
    }
    let mut h = [0.0; 9];
    for (i, row) in m.iter().enumerate() {
        h[i] = row[8] / row[i];
    }
    h[8] = 1.0;
    Some(h)
}

/// `p` through the map `h`
pub fn apply_h(h: &[f64; 9], p: Pt) -> Pt {
    let w = h[6] * p.x + h[7] * p.y + h[8];
    pt((h[0] * p.x + h[1] * p.y + h[2]) / w, (h[3] * p.x + h[4] * p.y + h[5]) / w)
}

/// A wide-angle lens's bend: where a point of a straight-lens photo (`p`, pixels) shows in the real one. `k` < 0 is
/// barrel (edges pulled in, most USB cameras); 0 = none. About the centre, r in half-diagonals.
pub fn lens_bend(p: Pt, w: usize, h: usize, k: f64) -> Pt {
    let (cx, cy, r0) = (w as f64 / 2.0, h as f64 / 2.0, (w as f64).hypot(h as f64) / 2.0);
    let (dx, dy) = (p.x - cx, p.y - cy);
    let f = 1.0 + k * (dx * dx + dy * dy) / (r0 * r0);
    pt(cx + dx * f, cy + dy * f)
}

/// The inverse of `lens_bend`: a point clicked on the real photo → the straight-lens photo.
pub fn lens_unbend(p: Pt, w: usize, h: usize, k: f64) -> Pt {
    let mut q = p;
    for _ in 0..30 {
        let b = lens_bend(q, w, h, k);
        q = pt(q.x + p.x - b.x, q.y + p.y - b.y);
    }
    q
}

/// A camera photo (`w` × `h` grey pixels, row 0 at the top) straightened onto the bed: `bed_to_px` maps bed mm to
/// straight-lens photo pixels (`k` = `lens_bend`); the result covers `r` at `px_per_mm` (at most 2000 px), white
/// where the photo doesn't reach.
pub fn warp_photo(px: &[u8], w: usize, h: usize, bed_to_px: &[f64; 9], k_lens: f64, r: &Rect, px_per_mm: f64) -> Raster {
    let k = px_per_mm.min(2000.0 / r.w().max(r.h()).max(1e-9));
    let (nw, nh) = (((r.w() * k).round() as usize).max(1), ((r.h() * k).round() as usize).max(1));
    let mut out = vec![255u8; nw * nh];
    for j in 0..nh {
        for i in 0..nw {
            let q = lens_bend(apply_h(bed_to_px, pt(r.x0 + (i as f64 + 0.5) / k, r.y1 - (j as f64 + 0.5) / k)), w, h, k_lens);
            if q.x >= 0.0 && q.y >= 0.0 && (q.x as usize) < w && (q.y as usize) < h {
                out[j * nw + i] = px[q.y as usize * w + q.x as usize];
            }
        }
    }
    Raster::from_gray(nw as u32, nh as u32, &out)
}

/// Darkness 0..1 per cell of a `cols × rows` grid over the picture (box sampling), row 0 at the top.
fn sample(px: &[u8], w: usize, h: usize, cols: usize, rows: usize, negative: bool) -> Vec<f32> {
    let mut out = vec![0f32; cols * rows];
    for r in 0..rows {
        let y0 = r * h / rows;
        let y1 = ((r + 1) * h / rows).max(y0 + 1).min(h);
        for c in 0..cols {
            let x0 = c * w / cols;
            let x1 = ((c + 1) * w / cols).max(x0 + 1).min(w);
            let mut sum = 0u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += px[y * w + x] as u64;
                }
            }
            let mean = sum as f32 / ((y1 - y0) * (x1 - x0)) as f32 / 255.0;
            out[r * cols + c] = if negative { mean } else { 1.0 - mean };
        }
    }
    out
}

/// Which pixels of a `w` × `h` grid laid over `r` (row 0 at the top) have their centre inside the closed `polys`
/// (even-odd, so holes stay holes): a scanline per row.
pub fn inside_mask(polys: &[Vec<Pt>], r: &Rect, w: usize, h: usize) -> Vec<bool> {
    let mut out = vec![false; w * h];
    let (cw, rh) = (r.w() / w.max(1) as f64, r.h() / h.max(1) as f64);
    let mut xs: Vec<f64> = Vec::new();
    for row in 0..h {
        let y = r.y1 - (row as f64 + 0.5) * rh;
        xs.clear();
        for p in polys {
            let n = p.len();
            for i in 0..n {
                let (a, b) = (p[i], p[(i + 1) % n]);
                if (a.y <= y) != (b.y <= y) {
                    xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
                }
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for pair in xs.chunks(2) {
            if let [x0, x1] = pair {
                let c0 = (((x0 - r.x0) / cw) - 0.5).ceil().max(0.0) as usize;
                let c1 = (((x1 - r.x0) / cw) - 0.5).floor().min(w as f64 - 1.0);
                if c1 >= 0.0 {
                    for c in c0..=(c1 as usize) {
                        out[row * w + c] = true;
                    }
                }
            }
        }
    }
    out
}

/// Closed vectors as a picture over `r`: black inside (even-odd), white elsewhere, `px_per_mm` pixels per mm
/// (at most 2000 on the long side).
pub fn vectors_to_raster(polys: &[Vec<Pt>], r: &Rect, px_per_mm: f64) -> Raster {
    let k = px_per_mm.max(0.1).min(2000.0 / r.w().max(r.h()).max(1e-6));
    let (w, h) = (((r.w() * k).round() as usize).max(1), ((r.h() * k).round() as usize).max(1));
    let px: Vec<u8> = inside_mask(polys, r, w, h).into_iter().map(|i| if i { 0 } else { 255 }).collect();
    Raster::from_gray(w as u32, h as u32, &px)
}

/// Which cells burn (dithers) or how hard (greyscale): 0..1 per cell.
/// `screen`: the Halftone cell (px) and angle (°)
fn dither(dark: &mut [f32], cols: usize, rows: usize, mode: Dither, screen: [f64; 2]) {
    match mode {
        Dither::Threshold | Dither::Slices => dark.iter_mut().for_each(|d| *d = (*d >= 0.5) as u8 as f32),
        Dither::Ordered => {
            for r in 0..rows {
                for c in 0..cols {
                    let t = (BAYER[r % 8][c % 8] as f32 + 0.5) / 64.0;
                    dark[r * cols + c] = (dark[r * cols + c] > t) as u8 as f32;
                }
            }
        }
        Dither::Halftone | Dither::Newsprint => {
            // round dots on a screen of `cell` px squares turned `angle`°: each cell burns as many of its pixels as
            // its mean darkness asks for, nearest its centre first, so darker cells grow bigger dots
            let screen = if mode == Dither::Newsprint { [8.0, 45.0] } else { screen };
            let (cell, angle) = (screen[0].max(1.0), screen[1].to_radians());
            let (sin, cos) = (angle.sin(), angle.cos());
            let mut cells: std::collections::HashMap<_, (f32, Vec<_>)> = std::collections::HashMap::new();
            for r in 0..rows {
                for c in 0..cols {
                    let (x, y) = (c as f64 + 0.5, r as f64 + 0.5);
                    let (u, v) = ((x * cos + y * sin) / cell, (-x * sin + y * cos) / cell);
                    let (fu, fv) = (u.floor(), v.floor());
                    let d = ((u - fu - 0.5).powi(2) + (v - fv - 0.5).powi(2), r * cols + c);
                    let e = cells.entry((fu as i64, fv as i64)).or_default();
                    e.0 += dark[d.1].clamp(0.0, 1.0);
                    e.1.push(d);
                }
            }
            for (sum, mut px) in cells.into_values() {
                px.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                let burn = sum.round() as usize;
                for (k, (_, i)) in px.into_iter().enumerate() {
                    dark[i] = (k < burn) as u8 as f32;
                }
            }
        }
        Dither::Sketch => {
            // Sobel edge strength on the darkness; strong edges burn
            let src = dark.to_vec();
            let at = |c: usize, r: usize| src[r.min(rows - 1) * cols + c.min(cols - 1)].clamp(0.0, 1.0);
            for r in 0..rows {
                for c in 0..cols {
                    let (cl, cr, ru, rd) = (c.saturating_sub(1), c + 1, r.saturating_sub(1), r + 1);
                    let gx = at(cr, ru) + 2.0 * at(cr, r) + at(cr, rd) - at(cl, ru) - 2.0 * at(cl, r) - at(cl, rd);
                    let gy = at(cl, rd) + 2.0 * at(c, rd) + at(cr, rd) - at(cl, ru) - 2.0 * at(c, ru) - at(cr, ru);
                    dark[r * cols + c] = ((gx * gx + gy * gy).sqrt() > 0.5) as u8 as f32;
                }
            }
        }
        Dither::FloydSteinberg | Dither::Jarvis | Dither::Stucki | Dither::Atkinson => {
            // error diffusion, serpentine rows so the pattern has no drift
            let kernel: &[(i32, i32, f32)] = if mode == Dither::FloydSteinberg {
                &[(1, 0, 7.0 / 16.0), (-1, 1, 3.0 / 16.0), (0, 1, 5.0 / 16.0), (1, 1, 1.0 / 16.0)]
            } else if mode == Dither::Atkinson {
                &[(1, 0, 0.125), (2, 0, 0.125), (-1, 1, 0.125), (0, 1, 0.125), (1, 1, 0.125), (0, 2, 0.125)]
            } else if mode == Dither::Stucki {
                &[
                    (1, 0, 8.0 / 42.0),
                    (2, 0, 4.0 / 42.0),
                    (-2, 1, 2.0 / 42.0),
                    (-1, 1, 4.0 / 42.0),
                    (0, 1, 8.0 / 42.0),
                    (1, 1, 4.0 / 42.0),
                    (2, 1, 2.0 / 42.0),
                    (-2, 2, 1.0 / 42.0),
                    (-1, 2, 2.0 / 42.0),
                    (0, 2, 4.0 / 42.0),
                    (1, 2, 2.0 / 42.0),
                    (2, 2, 1.0 / 42.0),
                ]
            } else {
                &[
                    (1, 0, 7.0 / 48.0),
                    (2, 0, 5.0 / 48.0),
                    (-2, 1, 3.0 / 48.0),
                    (-1, 1, 5.0 / 48.0),
                    (0, 1, 7.0 / 48.0),
                    (1, 1, 5.0 / 48.0),
                    (2, 1, 3.0 / 48.0),
                    (-2, 2, 1.0 / 48.0),
                    (-1, 2, 3.0 / 48.0),
                    (0, 2, 5.0 / 48.0),
                    (1, 2, 3.0 / 48.0),
                    (2, 2, 1.0 / 48.0),
                ]
            };
            for r in 0..rows {
                let rev = r % 2 == 1;
                for k in 0..cols {
                    let c = if rev { cols - 1 - k } else { k };
                    let v = dark[r * cols + c];
                    let out = (v >= 0.5) as u8 as f32;
                    let err = v - out;
                    dark[r * cols + c] = out;
                    for &(dx, dy, wgt) in kernel {
                        let dx = if rev { -dx } else { dx };
                        let (nc, nr) = (c as i32 + dx, r as i32 + dy);
                        if nc >= 0 && (nc as usize) < cols && (nr as usize) < rows {
                            dark[nr as usize * cols + nc as usize] += err * wgt;
                        }
                    }
                }
            }
        }
        Dither::Grayscale => dark.iter_mut().for_each(|d| *d = d.clamp(0.0, 1.0)),
    }
}

/// Scan lines for a picture filling `r` (mm): rows `interval` apart, cells `interval` wide, alternating direction.
/// Dithers give on/off runs (a stroke per run, overscan on each row's ends); greyscale gives one stroke per row with
/// a power per cell (`pw`). Cells lighter than `min_dark` are skipped in greyscale.
/// The picture as the laser will burn it (one pixel per scan cell, black = burnt, greyscale = power) as PNG bytes:
/// what LightBurn calls Save Processed Bitmap.
/// The picture as it will burn (placed over `r`, a pixel per scan line and dot): (columns, rows, grey, 0 = burnt).
pub fn processed_gray(raster: &Raster, r: &Rect, interval: f64, mode: Dither, negative: bool) -> Option<(usize, usize, Vec<u8>)> {
    let px = raster.adjusted()?;
    // at most ~25 M cells (100 MB of f32): a big picture at a tiny interval scans coarser rather than run out of memory
    let interval = interval.max(0.01).max((r.w() * r.h() / 25e6).sqrt());
    let cols = ((r.w() / interval).round() as usize).clamp(1, 20_000);
    let rows = ((r.h() / interval).round() as usize).clamp(1, 20_000);
    let mut dark = sample(&px, raster.w as usize, raster.h as usize, cols, rows, negative);
    dither(&mut dark, cols, rows, mode, raster.screen_px(interval));
    Some((cols, rows, dark.iter().map(|d| (255.0 * (1.0 - d.clamp(0.0, 1.0))).round() as u8).collect()))
}

pub fn processed_png(raster: &Raster, r: &Rect, interval: f64, mode: Dither, negative: bool) -> Option<Vec<u8>> {
    let (cols, rows, gray) = processed_gray(raster, r, interval, mode, negative)?;
    let img = image::GrayImage::from_raw(cols as u32, rows as u32, gray)?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// `raster_strokes` with the scan lines turned `angle_deg` anticlockwise (LightBurn's Scan angle on an Image layer):
/// the picture, its adjustments and negative baked in, is turned the other way onto an upright grid (white outside),
/// scanned, and the lines turned back about the box's centre.
#[allow(clippy::too_many_arguments)]
pub fn raster_strokes_at(raster: &Raster, r: &Rect, interval: f64, mode: Dither, negative: bool, overscan: f64, one_way: bool, layer: usize, angle_deg: f64) -> Vec<Stroke> {
    let a = angle_deg.rem_euclid(180.0);
    if a < 1e-6 || r.is_empty() {
        return raster_strokes(raster, r, interval, mode, negative, overscan, one_way, layer);
    }
    let Some(mut px) = raster.adjusted() else { return Vec::new() };
    if negative {
        px.iter_mut().for_each(|p| *p = 255 - *p);
    }
    let base = Raster { halftone: raster.halftone, ..Raster::from_gray(raster.w, raster.h, &px) };
    let (cx, cy) = ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0);
    let about = |rad: f64| Affine::translate(cx, cy).then(&Affine::rotate(rad)).then(&Affine::translate(-cx, -cy));
    let Some((t, tr)) = base.transformed(r, &about(-a.to_radians())) else { return Vec::new() };
    let back = about(a.to_radians());
    let mut out = raster_strokes(&t, &tr, interval, mode, false, overscan, one_way, layer);
    out.iter_mut().for_each(|s| s.pts.iter_mut().for_each(|p| *p = back.apply(*p)));
    out
}

#[allow(clippy::too_many_arguments)]
pub fn raster_strokes(raster: &Raster, r: &Rect, interval: f64, mode: Dither, negative: bool, overscan: f64, one_way: bool, layer: usize) -> Vec<Stroke> {
    let Some(px) = raster.adjusted() else { return Vec::new() };
    // at most ~25 M cells (100 MB of f32): a big picture at a tiny interval scans coarser rather than run out of memory
    let interval = interval.max(0.01).max((r.w() * r.h() / 25e6).sqrt());
    let cols = ((r.w() / interval).round() as usize).clamp(1, 20_000);
    let rows = ((r.h() / interval).round() as usize).clamp(1, 20_000);
    let mut dark = sample(&px, raster.w as usize, raster.h as usize, cols, rows, negative);
    dither(&mut dark, cols, rows, mode, raster.screen_px(interval));
    let cw = r.w() / cols as f64;
    let rh = r.h() / rows as f64;
    let mut out = Vec::new();
    for row in 0..rows {
        let y = r.y1 - (row as f64 + 0.5) * rh; // row 0 at the top of the picture
        let rev = !one_way && row % 2 == 1;
        let line = &dark[row * cols..(row + 1) * cols];
        if mode == Dither::Grayscale {
            if line.iter().all(|d| *d < 0.02) {
                continue;
            }
            // one stroke across the row, a power per cell; dead runs at both ends
            let (x0, x1) = (r.x0 - overscan, r.x1 + overscan);
            let mut pts = Vec::with_capacity(cols + 3);
            let mut pw = Vec::with_capacity(cols + 2);
            let cells: Vec<f32> = if rev { line.iter().rev().copied().collect() } else { line.to_vec() };
            let (start, end) = if rev { (x1, x0) } else { (x0, x1) };
            pts.push(pt(start, y));
            if overscan > 0.0 {
                pts.push(pt(if rev { r.x1 } else { r.x0 }, y));
                pw.push(0.0);
            }
            for (c, d) in cells.iter().enumerate() {
                let x = if rev { r.x1 - (c as f64 + 1.0) * cw } else { r.x0 + (c as f64 + 1.0) * cw };
                pts.push(pt(x, y));
                pw.push(*d);
            }
            if overscan > 0.0 {
                pts.push(pt(end, y));
                pw.push(0.0);
            }
            out.push(Stroke { pts, layer, pw: Some(pw), dwell_ms: 0.0, keep_start: false });
            continue;
        }
        // runs of burning cells
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut c = 0;
        while c < cols {
            if line[c] >= 0.5 {
                let s = c;
                while c < cols && line[c] >= 0.5 {
                    c += 1;
                }
                runs.push((s, c));
            } else {
                c += 1;
            }
        }
        if runs.is_empty() {
            continue;
        }
        if rev {
            runs.reverse();
        }
        for (k, (a, b)) in runs.iter().enumerate() {
            let (xa, xb) = (r.x0 + *a as f64 * cw, r.x0 + *b as f64 * cw);
            let (mut s, mut e) = if rev { (xb, xa) } else { (xa, xb) };
            // overscan: the first and last run of a row get a dead approach / exit
            let mut pts = Vec::with_capacity(4);
            let mut pw = Vec::with_capacity(3);
            if overscan > 0.0 && k == 0 {
                let o = if rev { s + overscan } else { s - overscan };
                pts.push(pt(o, y));
                pw.push(0.0);
                s = s.max(f64::MIN);
            }
            pts.push(pt(s, y));
            pts.push(pt(e, y));
            pw.push(1.0);
            if overscan > 0.0 && k == runs.len() - 1 {
                let o = if rev { e - overscan } else { e + overscan };
                pts.push(pt(o, y));
                pw.push(0.0);
                e = e.min(f64::MAX);
            }
            let _ = e;
            out.push(Stroke { pts, layer, pw: if pw.iter().all(|p| *p == 1.0) { None } else { Some(pw) }, dwell_ms: 0.0, keep_start: false });
        }
    }
    out
}

/// Centerline trace: the dark strokes of a picture (`w` × `h` grey, row 0 at the top) as single lines down their
/// middles, in pixel coordinates (y down). Zhang–Suen thinning, then the skeleton followed from its ends and
/// junctions (loops on their own), each line simplified to `tol` px; lines shorter than `min_len` px are dropped.
/// `threshold`: dark below it (None = Otsu); `invert` traces the light strokes.
pub fn centerline(px: &[u8], w: usize, h: usize, threshold: Option<u8>, invert: bool, tol: f64, min_len: f64) -> Vec<(Vec<Pt>, bool)> {
    if px.len() != w * h || w < 3 || h < 3 {
        return Vec::new();
    }
    let t = threshold.unwrap_or_else(|| otsu(px));
    let mut on: Vec<bool> = px.iter().map(|&v| (v < t) != invert).collect();
    // a one-pixel frame stays off so neighbours never leave the picture
    for x in 0..w {
        on[x] = false;
        on[(h - 1) * w + x] = false;
    }
    for y in 0..h {
        on[y * w] = false;
        on[y * w + w - 1] = false;
    }
    // Zhang–Suen: peel the boundary in two sub-steps until nothing changes
    loop {
        let mut changed = false;
        for step in 0..2 {
            let mut gone = Vec::new();
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let i = y * w + x;
                    if !on[i] {
                        continue;
                    }
                    // p2..p9 clockwise from the top
                    let n = [on[i - w], on[i - w + 1], on[i + 1], on[i + w + 1], on[i + w], on[i + w - 1], on[i - 1], on[i - w - 1]];
                    let b = n.iter().filter(|v| **v).count();
                    let a = (0..8).filter(|&k| !n[k] && n[(k + 1) % 8]).count();
                    let (c1, c2) = if step == 0 { (n[0] && n[2] && n[4], n[2] && n[4] && n[6]) } else { (n[0] && n[2] && n[6], n[0] && n[4] && n[6]) };
                    if (2..=6).contains(&b) && a == 1 && !c1 && !c2 {
                        gone.push(i);
                    }
                }
            }
            changed |= !gone.is_empty();
            for i in gone {
                on[i] = false;
            }
        }
        if !changed {
            break;
        }
    }
    // neighbours: a diagonal only when no straight neighbour already joins them (no staircase triangles)
    let nbrs = |i: usize| -> Vec<usize> {
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        let at = |dx: i64, dy: i64| ((y + dy) as usize) * w + (x + dx) as usize;
        let mut v = Vec::new();
        for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            if on[at(dx, dy)] {
                v.push(at(dx, dy));
            }
        }
        for (dx, dy) in [(1, -1), (1, 1), (-1, 1), (-1, -1)] {
            if on[at(dx, dy)] && !on[at(dx, 0)] && !on[at(0, dy)] {
                v.push(at(dx, dy));
            }
        }
        v
    };
    let mut used: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let key = |a: usize, b: usize| (a.min(b), a.max(b));
    let p = |i: usize| pt((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
    let mut out = Vec::new();
    let walk = |start: usize, first: usize, used: &mut std::collections::HashSet<(usize, usize)>| -> (Vec<Pt>, bool) {
        let mut path = vec![p(start), p(first)];
        used.insert(key(start, first));
        let (mut prev, mut cur) = (start, first);
        loop {
            let n = nbrs(cur);
            if n.len() != 2 {
                break; // an end or a junction
            }
            let next = if n[0] == prev { n[1] } else { n[0] };
            if used.contains(&key(cur, next)) {
                break;
            }
            used.insert(key(cur, next));
            path.push(p(next));
            (prev, cur) = (cur, next);
            if cur == start {
                return (path, true);
            }
        }
        (path, false)
    };
    let pixels: Vec<usize> = (0..w * h).filter(|&i| on[i]).collect();
    // ends and junctions first, then whatever is left (loops)
    for pass in 0..2 {
        for &i in &pixels {
            let n = nbrs(i);
            if pass == 0 && n.len() == 2 {
                continue;
            }
            for nb in n {
                if !used.contains(&key(i, nb)) {
                    let (path, closed) = walk(i, nb, &mut used);
                    out.push((path, closed));
                }
            }
        }
    }
    out.into_iter().map(|(path, closed)| (simplify_px(&path, tol), closed)).filter(|(path, _)| path.windows(2).map(|q| crate::geom::dist(q[0], q[1])).sum::<f64>() >= min_len).collect()
}

/// Otsu's threshold: the grey level that best splits the picture in two
fn otsu(px: &[u8]) -> u8 {
    let mut hist = [0f64; 256];
    px.iter().for_each(|&v| hist[v as usize] += 1.0);
    let total = px.len() as f64;
    let sum: f64 = (0..256).map(|i| i as f64 * hist[i]).sum();
    let (mut wb, mut sb, mut best, mut t) = (0.0, 0.0, 0.0, 128u8);
    for (i, &c) in hist.iter().enumerate() {
        wb += c;
        if wb == 0.0 || wb == total {
            continue;
        }
        sb += i as f64 * c;
        let (mb, mf) = (sb / wb, (sum - sb) / (total - wb));
        let between = wb * (total - wb) * (mb - mf) * (mb - mf);
        if between > best {
            (best, t) = (between, i as u8 + 1);
        }
    }
    t
}

/// Douglas–Peucker on a pixel path
fn simplify_px(p: &[Pt], tol: f64) -> Vec<Pt> {
    if p.len() < 3 {
        return p.to_vec();
    }
    let (a, b) = (p[0], p[p.len() - 1]);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l = (dx * dx + dy * dy).sqrt();
    let d = |q: &Pt| if l < 1e-9 { crate::geom::dist(*q, a) } else { ((q.x - a.x) * dy - (q.y - a.y) * dx).abs() / l };
    let (k, far) = p[1..p.len() - 1].iter().enumerate().map(|(k, q)| (k + 1, d(q))).fold((0, 0.0), |m, x| if x.1 > m.1 { x } else { m });
    if far <= tol {
        return vec![a, b];
    }
    let mut left = simplify_px(&p[..=k], tol);
    left.pop();
    left.extend(simplify_px(&p[k..], tol));
    left
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_sizes_are_refused_or_scanned_coarser() {
        // w × h past u32 (a crafted file) reads as no pixels, not a panic
        assert!(Raster { w: 70_000, h: 70_000, ..Raster::from_gray(1, 1, &[0]) }.pixels().is_none());
        // 200 mm at 0.01 mm would be 400 M cells: capped near 25 M (5000 rows)
        let s = raster_strokes(&Raster::from_gray(1, 1, &[0]), &Rect { x0: 0.0, y0: 0.0, x1: 200.0, y1: 200.0 }, 0.01, Dither::Threshold, false, 0.0, false, 0);
        assert!(!s.is_empty() && s.len() <= 5001, "{}", s.len());
    }

    #[test]
    fn ink_box_skips_white_margins() {
        let mut g = vec![255u8; 10 * 4];
        g[10 + 3] = 0; // x 3, y 1
        g[2 * 10 + 6] = 100; // x 6, y 2
        let r = Raster::from_gray(10, 4, &g);
        assert_eq!(r.ink_box(false), Some([0.3, 0.25, 0.7, 0.75]));
        assert_eq!(Raster::from_gray(2, 2, &[255; 4]).ink_box(false), None);
        assert_eq!(Raster::from_gray(2, 1, &[0, 255]).ink_box(true), Some([0.5, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn a_thick_stroke_traces_to_one_line_down_its_middle() {
        // a 5 px tall bar across a 60 × 21 picture, and a ring
        let (w, h) = (60usize, 21usize);
        let mut px = vec![255u8; w * h];
        for y in 8..13 {
            for x in 5..55 {
                px[y * w + x] = 0;
            }
        }
        let lines = centerline(&px, w, h, None, false, 0.8, 3.0);
        assert_eq!(lines.len(), 1, "{lines:?}");
        let (l, closed) = &lines[0];
        assert!(!closed && l.len() <= 3, "{l:?}");
        assert!(l.iter().all(|p| (p.y - 10.5).abs() < 1.01), "down the middle: {l:?}");
        let span = (l[0].x - l[l.len() - 1].x).abs();
        assert!(span > 40.0, "{span}");
        // a ring traces to one closed loop
        let (w, h) = (40usize, 40usize);
        let ring: Vec<u8> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f64 - 19.5, (i / w) as f64 - 19.5);
                if (12.0..16.0).contains(&(x * x + y * y).sqrt()) {
                    0
                } else {
                    255
                }
            })
            .collect();
        let loops = centerline(&ring, w, h, Some(128), false, 0.8, 3.0);
        assert!(loops.iter().any(|(l, c)| *c && l.len() > 8), "{loops:?}");
    }

    #[test]
    fn a_tilted_photo_is_straightened_onto_the_bed() {
        // a 100 × 100 mm bed seen in perspective: its corners at these photo pixels (top narrower than the bottom)
        let bed = [pt(0.0, 0.0), pt(100.0, 0.0), pt(100.0, 100.0), pt(0.0, 100.0)];
        let seen = [pt(10.0, 190.0), pt(190.0, 190.0), pt(150.0, 20.0), pt(50.0, 20.0)];
        let h = homography(bed, seen).unwrap();
        for (b, s) in bed.iter().zip(&seen) {
            let q = apply_h(&h, *b);
            assert!((q.x - s.x).abs() < 1e-6 && (q.y - s.y).abs() < 1e-6, "{q:?} vs {s:?}");
        }
        // a photo: dark everywhere inside the seen bed's left half (x < the line from (10,190) to (50,20) mid…)
        let (w, hh) = (200usize, 200usize);
        let mut px = vec![255u8; w * hh];
        // paint the photo pixel of every bed point with x < 50 mm dark
        for j in 0..200 {
            for i in 0..100 {
                let q = apply_h(&h, pt(i as f64 * 0.5, j as f64 * 0.5));
                px[(q.y as usize).min(hh - 1) * w + (q.x as usize).min(w - 1)] = 0;
            }
        }
        let r = warp_photo(&px, w, hh, &h, 0.0, &Rect { x0: 0.0, y0: 0.0, x1: 100.0, y1: 100.0 }, 1.0);
        let g = r.pixels().unwrap();
        // straightened: the left half of the bed is (mostly) dark, the right half light
        let dark = |x0: usize, x1: usize| (0..100).flat_map(|j| (x0..x1).map(move |i| (i, j))).filter(|&(i, j)| g[j * 100 + i] < 128).count() as f64 / ((x1 - x0) * 100) as f64;
        assert!(dark(5, 45) > 0.8 && dark(55, 95) < 0.05, "{} {}", dark(5, 45), dark(55, 95));
        assert!(homography([pt(0.0, 0.0), pt(1.0, 1.0), pt(2.0, 2.0), pt(3.0, 3.0)], bed).is_none());
        // the lens bend and its inverse
        for p in [pt(5.0, 7.0), pt(100.0, 100.0), pt(190.0, 12.0)] {
            let b = lens_bend(p, 200, 200, -0.2);
            let back = lens_unbend(b, 200, 200, -0.2);
            assert!((back.x - p.x).abs() < 1e-6 && (back.y - p.y).abs() < 1e-6, "{p:?} {b:?} {back:?}");
        }
        assert!(lens_bend(pt(5.0, 5.0), 200, 200, -0.2).x > 5.0, "barrel pulls the corners in");
    }

    #[test]
    fn mirrored_and_turned_pictures_move_their_pixels() {
        // 2 × 1 px over a 2 × 1 mm box: black left, white right
        let p = Raster::from_gray(2, 1, &[0, 255]);
        let r = Rect { x0: 0.0, y0: 0.0, x1: 2.0, y1: 1.0 };
        assert!(p.transformed(&r, &Affine::scale(2.0, 3.0)).is_none(), "a plain stretch needs no resampling");
        let mirror = Affine::translate(2.0, 0.0).then(&Affine::scale(-1.0, 1.0));
        let (m, mb) = p.transformed(&r, &mirror).unwrap();
        assert_eq!((m.w, m.h, m.pixels().unwrap()), (2, 1, vec![255, 0]));
        assert!((mb.x0, mb.x1) == (0.0, 2.0));
        // a quarter turn anticlockwise: the left (black) end goes to the bottom, row 0 is the top
        let (t, tb) = p.transformed(&r, &Affine::rotate(std::f64::consts::FRAC_PI_2)).unwrap();
        assert_eq!((t.w, t.h, t.pixels().unwrap()), (1, 2, vec![255, 0]));
        assert!((tb.w() - 1.0).abs() < 1e-9 && (tb.h() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_picture_saves_as_png_unchanged() {
        let r = Raster::from_gray(3, 2, &[0, 50, 100, 150, 200, 255]);
        let back = image::load_from_memory(&r.png().unwrap()).unwrap().to_luma8();
        assert_eq!(back.into_raw(), vec![0, 50, 100, 150, 200, 255]);
    }

    #[test]
    fn a_model_becomes_a_depth_map_top_light_base_dark() {
        // a 10 × 10 ramp: z = x / 10, two triangles (ASCII STL)
        let stl = "solid r\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 10 0 1\nvertex 10 10 1\nendloop\nendfacet\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 10 10 1\nvertex 0 10 0\nendloop\nendfacet\nendsolid r\n";
        let (r, w, h) = depth_map(stl.as_bytes(), false, 400).unwrap();
        assert_eq!((w, h), (10.0, 10.0));
        let px = r.pixels().unwrap();
        let row = &px[(r.h as usize / 2) * r.w as usize..][..r.w as usize];
        assert!(row[0] < 30 && row[row.len() - 1] > 225 && row.windows(2).all(|p| p[1] >= p[0]), "{row:?}");
    }

    fn checker(w: u32, h: u32) -> Raster {
        let px: Vec<u8> = (0..w * h).map(|i| if (i % w < w / 2) ^ (i / w < h / 2) { 0 } else { 255 }).collect();
        Raster::from_gray(w, h, &px)
    }

    #[test]
    fn shapes_mask_pictures_and_become_pictures() {
        // a ring (square with a square hole) on a 10 × 10 mm area at 1 px/mm
        let r = Rect { x0: 0.0, y0: 0.0, x1: 10.0, y1: 10.0 };
        let sq = |a: f64, b: f64| vec![crate::pt(a, a), crate::pt(b, a), crate::pt(b, b), crate::pt(a, b)];
        let ring = [sq(1.0, 9.0), sq(3.0, 7.0)];
        let m = inside_mask(&ring, &r, 10, 10);
        assert!(m[5 * 10 + 1] && m[10 + 5], "the ring");
        assert!(!m[5 * 10 + 5], "the hole");
        assert!(!m[0], "outside");
        assert_eq!(m.iter().filter(|v| **v).count(), 64 - 16);
        let pic = vectors_to_raster(&ring, &r, 1.0);
        assert_eq!((pic.w, pic.h), (10, 10));
        assert_eq!(pic.pixels().unwrap().iter().filter(|p| **p == 0).count(), 48);
        // masking a grey picture to the inner square: only that stays grey
        let grey = Raster::from_gray(10, 10, &[100u8; 100]);
        let cut = grey.masked(&r, &[sq(3.0, 7.0)]).unwrap().pixels().unwrap();
        assert_eq!(cut.iter().filter(|p| **p == 100).count(), 16);
        assert_eq!(cut.iter().filter(|p| **p == 255).count(), 84);
        // cropping to 2.5..7.5 snaps out to pixels 2..8, rows counted from the top
        let mut px = vec![255u8; 100];
        px[2 * 10 + 3] = 0; // column 3, row 2 = x 3..4, y 7..8
        let (c, at) = Raster::from_gray(10, 10, &px).cropped(&r, &Rect { x0: 2.5, y0: 2.5, x1: 7.5, y1: 7.5 }).unwrap();
        assert_eq!((c.w, c.h), (6, 6));
        assert_eq!((at.x0, at.y0, at.x1, at.y1), (2.0, 2.0, 8.0, 8.0));
        assert_eq!(c.pixels().unwrap()[1], 0, "the dark pixel is at column 1 of row 0");
    }

    #[test]
    fn halftone_dots_grow_with_darkness_and_stucki_keeps_the_tone() {
        for mode in [Dither::Halftone, Dither::Stucki, Dither::Newsprint] {
            for tone in [0.2f32, 0.5, 0.8] {
                let (cols, rows) = (60, 60);
                let mut d = vec![tone; cols * rows];
                dither(&mut d, cols, rows, mode, [6.0, 0.0]);
                assert!(d.iter().all(|v| *v == 0.0 || *v == 1.0), "{mode:?}: on/off only");
                let share = d.iter().sum::<f32>() / d.len() as f32;
                assert!((share - tone).abs() < 0.06, "{mode:?} at {tone}: {share}");
            }
        }
        // halftone: a dot is a cluster round the cell centre, not scattered
        let mut d = vec![0.2f32; 36];
        dither(&mut d, 6, 6, Dither::Halftone, [6.0, 0.0]);
        assert_eq!(d[2 * 6 + 2] + d[3 * 6 + 3], 2.0, "centre burns");
        assert_eq!(d[0] + d[5] + d[30] + d[35], 0.0, "corners don't");
        // a turned, bigger screen keeps the tone too
        for tone in [0.25f32, 0.6] {
            let mut d = vec![tone; 120 * 120];
            dither(&mut d, 120, 120, Dither::Halftone, [10.0, 45.0]);
            let share = d.iter().sum::<f32>() / d.len() as f32;
            assert!((share - tone).abs() < 0.03, "45° at {tone}: {share}");
        }
    }

    #[test]
    fn atkinson_sketch_sharpen_and_one_way_scanning() {
        // Atkinson: on / off, lights stay clean (a 10 % grey burns less than error-faithful dithers would)
        let mut d = vec![0.5f32; 3600];
        dither(&mut d, 60, 60, Dither::Atkinson, [6.0, 0.0]);
        assert!(d.iter().all(|v| *v == 0.0 || *v == 1.0));
        assert!((d.iter().sum::<f32>() / 3600.0 - 0.5).abs() < 0.15);
        // sketch: only the edges between the checker's quarters burn
        let r = checker(20, 20);
        let mut px: Vec<f32> = r.pixels().unwrap().iter().map(|p| 1.0 - *p as f32 / 255.0).collect();
        dither(&mut px, 20, 20, Dither::Sketch, [6.0, 0.0]);
        assert_eq!(px[0] + px[19] + px[19 * 20] + px[399], 0.0, "flat corners stay");
        assert_eq!(px[2 * 20 + 9] + px[2 * 20 + 10], 2.0, "the vertical edge burns");
        // sharpen pushes an edge's two sides apart
        let mut e = Raster::from_gray(6, 1, &[100, 100, 100, 200, 200, 200]);
        e.sharpen = 100.0;
        let p = e.adjusted().unwrap();
        assert!(p[2] < 100 && p[3] > 200 && p[0] == 100, "{p:?}");
        // the processed picture: one pixel per cell, the checker's dark quarters black
        let b = Rect { x0: 0.0, y0: 0.0, x1: 10.0, y1: 10.0 };
        let png = processed_png(&r, &b, 1.0, Dither::Threshold, false).unwrap();
        let back = Raster::decode(&png, 2000).unwrap();
        assert_eq!((back.w, back.h), (10, 10));
        let bp = back.pixels().unwrap();
        assert_eq!((bp[0], bp[9]), (255, 0), "top left white, top right black");
        // one way: every row burns left to right
        let s = raster_strokes(&r, &b, 1.0, Dither::Threshold, false, 0.0, true, 0);
        assert!(s.iter().all(|s| s.pts[0].x < s.pts[1].x));
        assert!(raster_strokes(&r, &b, 1.0, Dither::Threshold, false, 0.0, false, 0).iter().any(|s| s.pts[0].x > s.pts[1].x));
        // a 90° scan angle: lines run up and down, still only over the dark quarters (negative too)
        for neg in [false, true] {
            let s = raster_strokes_at(&r, &b, 1.0, Dither::Threshold, neg, 0.0, false, 0, 90.0);
            assert!(!s.is_empty() && s.iter().all(|s| (s.pts[0].x - s.pts[1].x).abs() < 1e-6), "vertical");
            let burnt: f64 = s.iter().map(|s| (s.pts[0].y - s.pts[1].y).abs()).sum();
            assert!((burnt - 50.0).abs() < 3.0, "half of 10 × 10 at 1 mm lines: {burnt}");
            assert!(s.iter().all(|s| s.pts.iter().all(|p| p.x > -0.6 && p.x < 10.6 && p.y > -0.6 && p.y < 10.6)));
            // top-right quarter dark, top-left light (flipped when negative)
            let top_right = s.iter().any(|s| s.pts[0].x > 5.0 && s.pts.iter().any(|p| p.y > 9.0));
            assert_eq!(top_right, !neg);
        }
    }

    #[test]
    fn a_picture_resamples_to_a_new_size_keeping_its_tone() {
        let r = checker(20, 20);
        let small = r.resampled(4, 4).unwrap();
        assert_eq!((small.w, small.h), (4, 4));
        let p = small.pixels().unwrap();
        assert!(p[0] > 200 && p[3] < 55, "top left light, top right dark: {p:?}");
    }

    #[test]
    fn a_checkerboard_dithers_to_runs_on_the_dark_quarters() {
        let r = checker(20, 20);
        let b = Rect { x0: 0.0, y0: 0.0, x1: 10.0, y1: 10.0 };
        for mode in [Dither::Threshold, Dither::FloydSteinberg, Dither::Jarvis, Dither::Ordered] {
            let s = raster_strokes(&r, &b, 1.0, mode, false, 0.0, false, 0);
            assert_eq!(s.len(), 10, "{mode:?}: one run per row");
            // top rows (y near 10): dark on the right half (the top-left quarter of the checker is white)
            let top = s.iter().find(|s| s.pts[0].y > 9.0).unwrap();
            let xs = (top.pts[0].x.min(top.pts[1].x), top.pts[0].x.max(top.pts[1].x));
            assert!((xs.0 - 5.0).abs() < 1e-9 && (xs.1 - 10.0).abs() < 1e-9, "{mode:?}: {xs:?}");
            assert!(s.iter().all(|s| s.pw.is_none()));
        }
        // negative flips the quarters
        let s = raster_strokes(&r, &b, 1.0, Dither::Threshold, true, 0.0, false, 0);
        let top = s.iter().find(|s| s.pts[0].y > 9.0).unwrap();
        assert!(top.pts.iter().all(|p| p.x <= 5.0 + 1e-9), "{:?}", top.pts);
        // overscan adds dead runs with power 0 at the row ends
        let s = raster_strokes(&r, &b, 1.0, Dither::Threshold, false, 2.0, false, 0);
        let top = s.iter().find(|s| s.pts.iter().any(|p| p.y > 9.0)).unwrap();
        assert_eq!(top.pw.as_deref(), Some(&[0.0f32, 1.0, 0.0][..]));
        assert!(top.pts.iter().any(|p| p.x < 3.1) && top.pts.iter().any(|p| p.x > 11.9), "{:?}", top.pts);
        // greyscale: one stroke per row, a power per cell, mid grey = half power
        let flat = Raster { w: 4, h: 2, gray: gcarve_core::photo::b64_encode(&[128u8; 8]), brightness: 0.0, contrast: 0.0, gamma: 1.0, sharpen: 0.0, halftone: [0.0, 0.0], denoise: 0.0 };
        let s = raster_strokes(&flat, &Rect { x0: 0.0, y0: 0.0, x1: 4.0, y1: 2.0 }, 1.0, Dither::Grayscale, false, 0.0, false, 0);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].pw.as_ref().unwrap().len(), 4);
        assert!((s[0].pw.as_ref().unwrap()[0] - 0.498).abs() < 0.01);
        // the second row runs back the other way
        assert!(s[0].pts[0].x < s[0].pts[1].x && s[1].pts[0].x > s[1].pts[1].x);
    }

    #[test]
    fn adjustments_brighten_steepen_and_gamma_the_pixels() {
        let mut r = Raster { w: 3, h: 1, gray: gcarve_core::photo::b64_encode(&[0, 128, 255]), brightness: 0.0, contrast: 0.0, gamma: 1.0, sharpen: 0.0, halftone: [0.0, 0.0], denoise: 0.0 };
        assert_eq!(r.adjusted().unwrap(), vec![0, 128, 255]);
        r.brightness = 50.0;
        assert_eq!(r.adjusted().unwrap(), vec![128, 255, 255]);
        r.brightness = 0.0;
        r.contrast = 100.0;
        let a = r.adjusted().unwrap();
        assert!(a[0] == 0 && (a[1] as i32 - 128).abs() <= 2 && a[2] == 255, "{a:?}"); // the midpoint stays, the ends clamp
        r.contrast = -100.0;
        assert_eq!(r.adjusted().unwrap(), vec![128, 128, 128]);
        r.contrast = 0.0;
        r.gamma = 2.0;
        assert!(r.adjusted().unwrap()[1] > 170);
    }

    #[test]
    fn pictures_decode_from_png_bytes() {
        // a 2×1 PNG: black, white
        let mut png = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png, 2, 1);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&[0, 255]).unwrap();
        }
        let r = Raster::decode(&png, 1000).unwrap();
        assert_eq!((r.w, r.h), (2, 1));
        assert_eq!(r.pixels().unwrap(), vec![0, 255]);
        assert!(Raster::decode(b"not a picture", 100).is_err());
    }
}
