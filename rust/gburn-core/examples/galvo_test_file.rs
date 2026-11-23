//! Writes the Aurora first-test design: a 20 mm square and a small cross in the middle of a 110 mm galvo field, one
//! Line layer at low power. `cargo run -p gburn-core --example galvo_test_file -- out.gburn`
use gburn_core::{default_layer, geom, pt, Document, Mode};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "Aurora first test.gburn".into());
    // optional: power %, speed mm/s, passes
    let arg = |i: usize, d: f64| std::env::args().nth(i).and_then(|a| a.parse().ok()).unwrap_or(d);
    let (power, speed, passes) = (arg(2, 20.0), arg(3, 500.0), arg(4, 1.0) as u32);
    let mut d = Document::default();
    d.name = "Aurora first test".into();
    d.bed = [110.0, 110.0];
    let mut l = default_layer(0);
    l.name = "Test mark".into();
    l.mode = Mode::Line;
    l.speed = speed; // mm/s
    l.power = power; // % of the fibre: 20 = a light mark on anodised / painted metal
    l.freq_khz = 30.0;
    l.passes = passes;
    d.layers = vec![l];
    let (cx, cy, h) = (55.0, 55.0, 10.0);
    let mut b = geom::Builder::default();
    b.start(pt(cx - h, cy - h));
    b.line(pt(cx + h, cy - h));
    b.line(pt(cx + h, cy + h));
    b.line(pt(cx - h, cy + h));
    b.close();
    d.add(b.c, 0);
    for (a, z) in [(pt(cx - 3.0, cy), pt(cx + 3.0, cy)), (pt(cx, cy - 3.0), pt(cx, cy + 3.0))] {
        let mut b = geom::Builder::default();
        b.start(a);
        b.line(z);
        d.add(b.c, 0);
    }
    d.notes = "First test on the Aurora 8: Frame first (red pointer traces the square), then Start with a scrap tag under the lens and a hand on Stop.".into();
    std::fs::write(&out, d.to_json()).expect("write");
    println!("wrote {out}");
}
