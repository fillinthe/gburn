//! Writes the sample design's G-code as each controller family gets it (GRBL, Smoothieware, Marlin, RepRapFirmware,
//! Snapmaker, a fan-pin laser), so the files can be compared with other programs' output for the same firmware.
//! `cargo run -p gburn-core --example gcode_dialects -- <out dir>`
use gburn_core::gcode::{gcode, Flavor, GcodeOptions};
fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let d = gburn_core::sample();
    let plan = gburn_core::plan::plan(&d.for_job(), gburn_core::plan::Machine::default());
    for (name, o) in [
        ("grbl", GcodeOptions::default()),
        ("grbl-m3", GcodeOptions { dynamic: false, ..Default::default() }),
        ("smoothie", GcodeOptions { s_max: 1.0, flavor: Flavor::Smoothie, ..Default::default() }),
        ("marlin", GcodeOptions { s_max: 255.0, flavor: Flavor::Marlin, ..Default::default() }),
        ("rrf", GcodeOptions { s_max: 255.0, flavor: Flavor::Rrf, ..Default::default() }),
        ("snapmaker", GcodeOptions { s_max: 255.0, flavor: Flavor::Snapmaker, ..Default::default() }),
        ("marlin-fan", GcodeOptions { s_max: 255.0, flavor: Flavor::MarlinFan, ..Default::default() }),
    ] {
        std::fs::write(format!("{dir}/gburn__{name}.gcode"), gcode(&d, &plan, o)).unwrap();
    }
}
