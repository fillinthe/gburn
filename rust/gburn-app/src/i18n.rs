//! The interface in other languages: `tr("English")` gives the chosen language's words, or the English when the table
//! has none (so a missing line never breaks anything). Tables are `lang/<code>.txt`: one `English<TAB>translation`
//! a line, `\n` for a line break inside either side, `#` lines ignored.

use std::collections::HashMap;
use std::sync::RwLock;

/// (code, the language's own name)
pub const LANGS: [(&str, &str); 12] = [
    ("en", "English"),
    ("es", "Español"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("it", "Italiano"),
    ("pt", "Português (Brasil)"),
    ("nl", "Nederlands"),
    ("pl", "Polski"),
    ("tr", "Türkçe"),
    ("ja", "日本語"),
    ("zh", "简体中文"),
    ("ko", "한국어"),
];

static TABLE: RwLock<Option<HashMap<&'static str, &'static str>>> = RwLock::new(None);

fn source(code: &str) -> &'static str {
    match code {
        "es" => include_str!("lang/es.txt"),
        "fr" => include_str!("lang/fr.txt"),
        "de" => include_str!("lang/de.txt"),
        "it" => include_str!("lang/it.txt"),
        "pt" => include_str!("lang/pt.txt"),
        "ja" => include_str!("lang/ja.txt"),
        "zh" => include_str!("lang/zh.txt"),
        "nl" => include_str!("lang/nl.txt"),
        "pl" => include_str!("lang/pl.txt"),
        "tr" => include_str!("lang/tr.txt"),
        "ko" => include_str!("lang/ko.txt"),
        _ => "",
    }
}

/// `\n` in a table stands for a line break (a side with one is copied once, at `set`)
fn unescape(s: &'static str) -> &'static str {
    if s.contains("\\n") {
        Box::leak(s.replace("\\n", "\n").into_boxed_str())
    } else {
        s
    }
}

/// a table's text → English → translation
pub fn parse(text: &'static str) -> HashMap<&'static str, &'static str> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('\t'))
        .filter(|(en, tr)| !en.is_empty() && !tr.trim().is_empty())
        .map(|(en, tr)| (unescape(en), unescape(tr.trim_end())))
        .collect()
}

/// switch the interface to `code` ("en" or unknown = English)
pub fn set(code: &str) {
    let table = Some(parse(source(code))).filter(|t| !t.is_empty());
    if let Ok(mut w) = TABLE.write() {
        *w = table;
    }
    // gcarve-ui's own words (the command search, dialog Close…) come from its packs
    gcarve_ui::i18n::set(code);
}

/// `s` in the chosen language
pub fn tr(s: &str) -> &str {
    TABLE.read().ok().and_then(|r| r.as_ref().and_then(|t| t.get(s).copied())).unwrap_or(s)
}

/// Noto Sans JP / SC / KR (OFL, THIRD_PARTY.md) cut down to the glyphs in each table (`fonts/subset.py`), plus two tiny
/// ones holding just "日本語" / "简体中文" / "한국어" for the language list in every language.
/// ponytail: CJK typed into a field outside the tables shows as boxes; ship the whole fonts if that matters
const CJK: [(&str, &[u8]); 3] =
    [("ja", include_bytes!("../fonts/cjk-ja.ttf")), ("zh", include_bytes!("../fonts/cjk-zh.ttf")), ("ko", include_bytes!("../fonts/cjk-ko.ttf"))];
const NAMES: [&[u8]; 3] = [include_bytes!("../fonts/names-ja.ttf"), include_bytes!("../fonts/names-zh.ttf"), include_bytes!("../fonts/names-ko.ttf")];

static FONT_LANG: RwLock<Option<String>> = RwLock::new(None);

/// called every frame; on a language change: the plain fonts, then that language's CJK font, then the names, all as
/// fallbacks after the UI font (CJK fonts carry Latin too, so they must never come first)
pub fn fonts(ctx: &eframe::egui::Context, code: &str) {
    use eframe::egui::{FontData, FontFamily};
    use eframe::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    use eframe::epaint::text::{FontTweak, VariationCoords};
    if FONT_LANG.read().is_ok_and(|l| l.as_deref() == Some(code)) {
        return;
    }
    if let Ok(mut l) = FONT_LANG.write() {
        *l = Some(code.to_string());
    }
    gcarve_ui::fonts::install(ctx);
    let both = || vec![FontFamily::Proportional, FontFamily::Name(gcarve_ui::fonts::BOLD.into())];
    let add = |name: String, d: FontData, families: Vec<FontFamily>| {
        ctx.add_font(FontInsert::new(&name, d, families.into_iter().map(|family| InsertFontFamily { family, priority: FontPriority::Lowest }).collect()));
    };
    if let Some((_, bytes)) = CJK.iter().find(|c| c.0 == code) {
        for (wght, family) in [(400.0, FontFamily::Proportional), (700.0, FontFamily::Name(gcarve_ui::fonts::BOLD.into()))] {
            let mut d = FontData::from_static(bytes);
            d.tweak = FontTweak { coords: VariationCoords::new([(b"wght", wght)]), ..Default::default() };
            add(format!("cjk-{code}-{wght}"), d, vec![family]);
        }
    }
    for (i, bytes) in NAMES.iter().enumerate() {
        add(format!("langnames-{i}"), FontData::from_static(bytes), both());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_reads_whole_and_keeps_its_tags() {
        for (code, _) in &LANGS[1..] {
            let src = source(code);
            let t = parse(src);
            let rows = src.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).count();
            assert_eq!(t.len(), rows, "{code}: a key appears twice or a line has no tab");
            assert!(t.len() > 700, "{code}: {} lines", t.len());
            for (en, tr) in &t {
                for tag in ["{", "⇧", "⌘", "%"] {
                    assert_eq!(en.matches(tag).count(), tr.matches(tag).count(), "{code}: {en} → {tr}");
                }
            }
        }
    }
}
