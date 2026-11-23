//! Fonts for the Text tool: the built-in Liberation Sans, every font on this computer (found once, in the
//! background, by gcarve-ui's `system_fonts`), and font files the user loads (the browser has no font folders).
//! A font is named "Family|Style"; "" = the built-in one.

use gcarve_ui::glyphs::{self, FontFile};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use ttf_parser::Face;

static SYSTEM: OnceLock<Vec<FontFile>> = OnceLock::new();
#[cfg(not(target_arch = "wasm32"))]
static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// "Family|Style" → the font file's bytes and face index.
/// ponytail: bytes are leaked so faces can be 'static; a session only ever loads the fonts it uses
type Loaded = HashMap<String, (&'static [u8], u32)>;
static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);
/// single-line SHX fonts by "Name|Single line", kept (base64) in the store key `shxfonts` so designs find them again
type Shx = HashMap<String, std::sync::Arc<gburn_core::shx::ShxFont>>;
static SHX: Mutex<Option<Shx>> = Mutex::new(None);
const SHX_STYLE: &str = "Single line";

fn shx_fonts() -> std::sync::MutexGuard<'static, Option<Shx>> {
    let mut g = SHX.lock().unwrap();
    if g.is_none() {
        // the files loaded in earlier sessions
        let mut m = HashMap::new();
        let saved: Vec<String> = gcarve_ui::store::load("shxfonts").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        for b64 in saved {
            if let Ok(f) = gburn_core::shx::ShxFont::parse(&gcarve_core::photo::b64_decode(&b64)) {
                m.insert(key(&f.name, SHX_STYLE), std::sync::Arc::new(f));
            }
        }
        *g = Some(m);
    }
    g
}

/// The single-line font for a "Name|Single line" name, if it's one.
pub fn shx(name: &str) -> Option<std::sync::Arc<gburn_core::shx::ShxFont>> {
    if !name.ends_with(SHX_STYLE) {
        return None;
    }
    shx_fonts().as_ref()?.get(name).cloned()
}

pub fn key(family: &str, style: &str) -> String {
    format!("{family}|{style}")
}

/// Look for the computer's fonts (once; ~1 s on a Mac with 500 faces), then repaint.
pub fn start_scan(ctx: &eframe::egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    if !STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = SYSTEM.set(glyphs::system_fonts());
            ctx.request_repaint();
        });
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
        let _ = SYSTEM.set(Vec::new());
    }
}

pub fn ready() -> bool {
    SYSTEM.get().is_some()
}

/// (family, style) of every font to offer: the computer's and the loaded files', sorted, no repeats.
pub fn all() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = SYSTEM.get().map(|s| s.iter().map(|f| (f.family.clone(), f.style.clone())).collect()).unwrap_or_default();
    if let Some(m) = LOADED.lock().unwrap().as_ref() {
        v.extend(m.keys().filter_map(|k| k.split_once('|').map(|(a, b)| (a.to_string(), b.to_string()))));
    }
    if let Some(m) = shx_fonts().as_ref() {
        v.extend(m.keys().filter_map(|k| k.split_once('|').map(|(a, b)| (a.to_string(), b.to_string()))));
    }
    v.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then(a.1.cmp(&b.1)));
    v.dedup();
    v
}

/// A font file's faces become choosable; returns their names ("Family Style").
pub fn add_file(bytes: Vec<u8>) -> Vec<String> {
    if bytes.starts_with(b"AutoCAD-86") {
        // a single-line SHX font, remembered for next time
        let Ok(f) = gburn_core::shx::ShxFont::parse(&bytes) else { return Vec::new() };
        let name = f.name.clone();
        let mut g = shx_fonts();
        g.get_or_insert_with(HashMap::new).insert(key(&name, SHX_STYLE), std::sync::Arc::new(f));
        let mut saved: Vec<String> = gcarve_ui::store::load("shxfonts").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let b64 = gcarve_core::photo::b64_encode(&bytes);
        if !saved.contains(&b64) {
            saved.push(b64);
            gcarve_ui::store::save("shxfonts", &serde_json::to_string(&saved).unwrap_or_default());
        }
        return vec![format!("{name} {SHX_STYLE}")];
    }
    let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let mut names = Vec::new();
    let mut m = LOADED.lock().unwrap();
    let m = m.get_or_insert_with(HashMap::new);
    for (family, style, index) in glyphs::faces_in(data) {
        names.push(format!("{family} {style}"));
        m.insert(key(&family, &style), (data, index));
    }
    names
}

/// The face for a "Family|Style" name, and whether it was found (else the built-in one, bold or not).
pub fn face(name: &str, bold: bool) -> (Face<'static>, bool) {
    if name.is_empty() {
        return (glyphs::bundled(bold), true);
    }
    let hit = LOADED.lock().unwrap().as_ref().and_then(|m| m.get(name).copied());
    let hit = hit.or_else(|| {
        let (family, style) = name.split_once('|')?;
        let f = SYSTEM.get()?.iter().find(|f| f.family == family && f.style == style)?;
        let data: &'static [u8] = Box::leak(std::fs::read(&f.path).ok()?.into_boxed_slice());
        LOADED.lock().unwrap().get_or_insert_with(HashMap::new).insert(name.to_string(), (data, f.index));
        Some((data, f.index))
    });
    match hit.and_then(|(d, i)| Face::parse(d, i).ok()) {
        Some(f) => (f, true),
        None => (glyphs::bundled(bold), false),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_loaded_font_file_is_offered_and_used() {
        let names = super::add_file(gcarve_ui::glyphs::BOLD.to_vec());
        assert_eq!(names, ["Liberation Sans Bold"]);
        assert!(super::all().contains(&("Liberation Sans".to_string(), "Bold".to_string())));
        let (f, found) = super::face(&super::key("Liberation Sans", "Bold"), false);
        assert!(found && f.number_of_glyphs() > 100);
        let (_, found) = super::face("Nope|Regular", false);
        assert!(!found, "a missing font falls back to the built-in one");
    }
}
