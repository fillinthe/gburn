//! Imports every file given on the command line (.lbrn2, Gerber, drill… by extension) and prints what came in (a
//! check against real files).
fn main() {
    for p in std::env::args().skip(1) {
        let text = std::fs::read_to_string(&p).unwrap();
        let mut d = gburn_core::Document::default();
        let paths = text.matches("Shape Type=\"Path\"").count() + text.matches("Shape Type=\"Rect\"").count();
        let ext = p.rsplit('.').next().unwrap_or("lbrn2");
        match d.import(ext, &text) {
            Ok((n, note)) => {
                let b = d.bounds();
                let layers: Vec<String> = d.layers.iter().map(|l| format!("{}:{}", l.name, d.shapes.iter().filter(|s| d.layers[s.layer].name == l.name).count())).collect();
                println!("{}: {n} shapes from {paths} paths, {:.1} × {:.1} mm, layers {layers:?} {note}", p.rsplit('/').next().unwrap(), b.w(), b.h());
            }
            Err(e) => println!("{p}: ERROR {e}"),
        }
    }
}
