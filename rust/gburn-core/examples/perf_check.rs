//! Times import, plan and G-code on real projects (a check that big designs stay quick).
fn main() {
    for p in std::env::args().skip(1) {
        let text = std::fs::read_to_string(&p).unwrap();
        let mut d = gburn_core::Document::default();
        let t0 = std::time::Instant::now();
        let _ = d.import("lbrn2", &text);
        let t1 = t0.elapsed();
        let plan = gburn_core::plan::plan(&d.for_job(), gburn_core::plan::Machine::default());
        let t2 = t0.elapsed();
        let g = gburn_core::gcode::gcode(&d, &plan, Default::default());
        let t3 = t0.elapsed();
        println!(
            "{}: {} shapes, import {:?}, plan {:?} ({} strokes), gcode {:?} ({} lines)",
            p.rsplit('/').next().unwrap(),
            d.shapes.len(),
            t1,
            t2 - t1,
            plan.strokes.len(),
            t3 - t2,
            g.lines().count()
        );
    }
}
