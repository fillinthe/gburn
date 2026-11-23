//! The simulation's "how it might look" view: a few common laser materials, each with its colour, a generated wood
//! grain (or speckle), and how a mark darkens (or lightens) with the energy put in. A rough guide only: real results
//! depend on the wood, the lens, the focus and the air.

use eframe::egui::{Color32, ColorImage};

pub struct Material {
    pub name: &'static str,
    pub base: [u8; 3],
    /// grain lines (woods); None = speckle only
    pub grain: Option<[u8; 3]>,
    /// the first, light marks …
    pub light: [u8; 3],
    /// … and the full burn (slate, anodised metal and acrylic go lighter, not darker)
    pub full: [u8; 3],
    /// how quickly marks reach `full`: darkness = 1 − e^(−sens · power % / speed mm/s)
    pub sens: f64,
}

pub const MATERIALS: [Material; 16] = [
    Material { name: "Birch plywood", base: [226, 200, 152], grain: Some([204, 170, 118]), light: [196, 150, 95], full: [58, 34, 18], sens: 4.0 },
    Material { name: "Maple", base: [238, 218, 182], grain: Some([222, 196, 150]), light: [205, 160, 105], full: [70, 40, 20], sens: 3.5 },
    Material { name: "Cherry", base: [196, 128, 86], grain: Some([172, 102, 64]), light: [150, 90, 55], full: [45, 22, 12], sens: 4.0 },
    Material { name: "Walnut", base: [108, 74, 52], grain: Some([84, 56, 38]), light: [72, 46, 31], full: [26, 15, 10], sens: 4.0 },
    Material { name: "Bamboo", base: [220, 190, 130], grain: Some([198, 162, 102]), light: [172, 122, 72], full: [60, 35, 18], sens: 4.5 },
    Material { name: "MDF", base: [180, 138, 96], grain: None, light: [130, 90, 58], full: [40, 25, 15], sens: 5.0 },
    Material { name: "Cork", base: [196, 150, 105], grain: None, light: [124, 82, 50], full: [40, 25, 15], sens: 6.0 },
    Material { name: "Cardboard", base: [200, 165, 120], grain: None, light: [150, 110, 70], full: [50, 30, 15], sens: 6.0 },
    Material { name: "Leather", base: [140, 90, 58], grain: None, light: [100, 60, 35], full: [35, 20, 12], sens: 5.0 },
    Material { name: "Slate", base: [62, 66, 72], grain: None, light: [150, 152, 155], full: [208, 208, 208], sens: 3.0 },
    Material { name: "Black anodised aluminium", base: [32, 32, 34], grain: None, light: [120, 120, 125], full: [226, 226, 230], sens: 2.0 },
    Material { name: "Black acrylic", base: [22, 22, 24], grain: None, light: [90, 90, 92], full: [172, 172, 174], sens: 3.0 },
    Material { name: "Clear acrylic", base: [196, 206, 212], grain: None, light: [222, 230, 234], full: [246, 248, 250], sens: 3.0 },
    // fibre marking: fast (1000–3000 mm/s), so they darken (or ablate) at much less energy per mm
    Material { name: "Stainless steel (fibre)", base: [178, 180, 184], grain: None, light: [128, 118, 108], full: [34, 30, 32], sens: 90.0 },
    Material { name: "Brass (fibre)", base: [200, 164, 84], grain: None, light: [150, 112, 52], full: [52, 40, 26], sens: 70.0 },
    Material { name: "Black-painted metal (fibre)", base: [28, 28, 30], grain: None, light: [130, 130, 135], full: [206, 206, 210], sens: 70.0 },
];

fn mix(a: [u8; 3], b: [u8; 3], t: f64) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| (a[i] as f64 + (b[i] as f64 - a[i] as f64) * t).round() as u8)
}

/// The colour a mark gets: `power` %, `speed` mm/s, `share` = the segment's part of the power (greyscale, 0..1).
pub fn mark(m: &Material, power: f64, speed: f64, share: f64) -> Color32 {
    let e = power * share / speed.max(0.1);
    let d = 1.0 - (-m.sens * e).exp();
    let c = if d < 0.3 { mix(m.base, m.light, d / 0.3) } else { mix(m.light, m.full, (d - 0.3) / 0.7) };
    Color32::from_rgb(c[0], c[1], c[2])
}

/// 0..1 value noise on a 16-cell lattice that repeats every `period` cells (so the texture tiles)
fn noise(x: f64, y: f64, period: i64) -> f64 {
    let h = |i: i64, j: i64| {
        let (i, j) = (i.rem_euclid(period) as u64, j.rem_euclid(period) as u64);
        let mut v = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ j.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
        v ^= v >> 29;
        v = v.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        (v >> 40) as f64 / (1u64 << 24) as f64
    };
    let (ix, iy) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let (a, b, c, d) = (h(ix, iy), h(ix + 1, iy), h(ix, iy + 1), h(ix + 1, iy + 1));
    let top = a + (b - a) * s(fx);
    let bottom = c + (d - c) * s(fx);
    top + (bottom - top) * s(fy)
}

pub const TILE: usize = 256;

/// A seamless 256 × 256 tile of the material (grain runs along X).
pub fn tile(m: &Material) -> ColorImage {
    let mut px = Vec::with_capacity(TILE * TILE * 4);
    let tau = std::f64::consts::TAU;
    for y in 0..TILE {
        for x in 0..TILE {
            let (fx, fy) = (x as f64, y as f64);
            let speck = noise(fx / 4.0, fy / 4.0, (TILE / 4) as i64) - 0.5;
            let c = match m.grain {
                Some(g) => {
                    // rings: stripes across Y, bent by slow noise and a long wave, so lines wander like grain
                    let bend = 9.0 * noise(fx / 64.0, fy / 32.0, (TILE / 64) as i64) + 3.0 * (tau * fx / TILE as f64).sin();
                    let ring = (tau * 22.0 * (fy + bend) / TILE as f64).sin() * 0.5 + 0.5;
                    // thin dark late-wood lines, a soft wash between them, fine streaks along the grain
                    let fine = noise(fx / 16.0, fy / 1.5, 16);
                    mix(m.base, g, ring.powi(14) * 0.9 + ring * 0.15 + fine * 0.22 + speck * 0.12)
                }
                None => mix(m.base, [0, 0, 0], speck * 0.12 + 0.03),
            };
            px.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    ColorImage::from_rgba_unmultiplied([TILE, TILE], &px)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_darken_with_energy_and_some_materials_go_light() {
        let ply = &MATERIALS[0];
        let lum = |c: Color32| c.r() as u32 + c.g() as u32 + c.b() as u32;
        let (none, engrave, cut) = (mark(ply, 0.0, 300.0, 1.0), mark(ply, 40.0, 300.0, 1.0), mark(ply, 90.0, 15.0, 1.0));
        assert_eq!(none, Color32::from_rgb(226, 200, 152), "no power: the bare wood");
        assert!(lum(none) > lum(engrave) && lum(engrave) > lum(cut));
        assert!(lum(cut) < 150, "a through cut is nearly black");
        let slate = MATERIALS.iter().find(|m| m.name == "Slate").unwrap();
        assert!(lum(mark(slate, 60.0, 100.0, 1.0)) > lum(mark(slate, 0.0, 100.0, 1.0)), "slate marks light");
        // greyscale: half the power, a lighter mark
        assert!(lum(mark(ply, 40.0, 300.0, 0.5)) > lum(engrave));
    }

    #[test]
    fn tiles_wrap_without_a_seam() {
        for m in &MATERIALS[..2] {
            let t = tile(m);
            assert_eq!(t.size, [TILE, TILE]);
            // the last column runs into the first: neighbours across the seam differ like any neighbours
            let px = |x: usize, y: usize| t.pixels[y * TILE + x];
            let step = |a: Color32, b: Color32| (a.r() as i32 - b.r() as i32).abs();
            let seam: i32 = (0..TILE).map(|y| step(px(TILE - 1, y), px(0, y))).sum();
            let inner: i32 = (0..TILE).map(|y| step(px(TILE / 2, y), px(TILE / 2 + 1, y))).sum();
            assert!(seam <= inner * 3 + TILE as i32, "seam {seam} vs inner {inner}");
        }
    }
}
