//! Writes the Aurora orientation test: a 40 mm square, an arrow pointing +X (right) along its middle, a triangle
//! pointing +Y (up / away) on its top edge, and a small square in the top-right corner, so a mark shows at once
//! whether the field is mirrored or turned and whether 40 mm comes out 40 mm.
//! `cargo run -p gburn-core --example galvo_orient_file -- out.gburn`
use gburn_core::{default_layer, geom, pt, Document, Mode};

fn poly(d: &mut Document, pts: &[(f64, f64)], close: bool) {
    let mut b = geom::Builder::default();
    b.start(pt(pts[0].0, pts[0].1));
    for p in &pts[1..] {
        b.line(pt(p.0, p.1));
    }
    if close {
        b.close();
    }
    d.add(b.c, 0);
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "Aurora orientation test.gburn".into());
    let mut d = Document::default();
    d.name = "Aurora orientation test".into();
    d.bed = [110.0, 110.0];
    let mut l = default_layer(0);
    l.name = "Test mark".into();
    l.mode = Mode::Line;
    l.speed = 300.0;
    l.power = 50.0;
    l.freq_khz = 30.0;
    l.passes = 2;
    d.layers = vec![l];
    let (cx, cy, h) = (55.0, 55.0, 20.0);
    poly(&mut d, &[(cx - h, cy - h), (cx + h, cy - h), (cx + h, cy + h), (cx - h, cy + h)], true);
    // arrow → along the middle, pointing +X
    poly(&mut d, &[(cx - 15.0, cy), (cx + 15.0, cy)], false);
    poly(&mut d, &[(cx + 9.0, cy + 5.0), (cx + 15.0, cy), (cx + 9.0, cy - 5.0)], false);
    // triangle ▲ on the top edge, pointing +Y
    poly(&mut d, &[(cx - 5.0, cy + h - 8.0), (cx + 5.0, cy + h - 8.0), (cx, cy + h - 1.0)], true);
    // a 5 mm square in the top-right corner
    poly(&mut d, &[(cx + h - 7.0, cy + h - 7.0), (cx + h - 2.0, cy + h - 7.0), (cx + h - 2.0, cy + h - 2.0), (cx + h - 7.0, cy + h - 2.0)], true);
    d.notes = "Aurora orientation: the arrow should point to the RIGHT, the triangle to the BACK (away from you), the small square sits top-right. The big square is 40 mm.".into();
    std::fs::write(&out, d.to_json()).expect("write");
    println!("wrote {out}");
}
