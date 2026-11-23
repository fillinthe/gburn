//! Dev tool: the galvo list a .gburn would send (packets, commands, opcodes). `cargo run -p gburn-core --example galvo_list_dump -- file.gburn`
use gburn_core::galvo::{job_list, GalvoOptions, LayerGalvo};
use gburn_core::plan::{plan, Machine};
use gburn_core::Document;

fn main() {
    let path = std::env::args().nth(1).expect("a .gburn file");
    let doc = Document::from_json(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    let job = doc.for_job();
    let p = plan(&job, Machine::default());
    let o = GalvoOptions { field_mm: doc.bed[0], ..Default::default() };
    let (l, skipped) = job_list(&job, &p, &o, &|i| LayerGalvo { freq_khz: job.layers[i].freq_khz, pulse_width: job.layers[i].pulse_ns as u16 });
    let packets = l.packets();
    println!("{} strokes, {} commands, {} packets, {skipped} skipped", p.strokes.len(), l.len(), packets.len());
    for (k, c) in l.packets().concat().chunks(12).enumerate().take(70) {
        println!("{k:3} {:04x} {:?}", u16::from_le_bytes([c[0], c[1]]), (1..6).map(|i| u16::from_le_bytes([c[2 * i], c[2 * i + 1]])).collect::<Vec<_>>());
    }
}
