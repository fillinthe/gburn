//! The machine list (`machines.json`, compiled in): brand, model, controller family, work area, home corner,
//! connection and, for galvos, the lens field, from the makers' published specs. Machine Settings › Choose your
//! machine… fills the settings from one entry.

use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Machine {
    pub brand: String,
    pub model: String,
    /// ruida · grbl · marlin · smoothie · galvo · other (a controller GBurn doesn't drive yet)
    pub kind: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub watts: f64,
    /// work area, mm (0 = not published)
    #[serde(default)]
    pub bed: [f64; 2],
    /// rear-left · rear-right · front-left · front-right · "" (not published)
    #[serde(default)]
    pub home: String,
    #[serde(default)]
    pub connect: String,
    /// galvo: the standard lens's field, mm
    #[serde(default)]
    pub lens_mm: f64,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub url: String,
}

impl Machine {
    pub fn name(&self) -> String {
        format!("{} {}", self.brand, self.model)
    }
    /// GBurn drives this controller family
    pub fn supported(&self) -> bool {
        matches!(self.kind.as_str(), "ruida" | "grbl" | "marlin" | "smoothie" | "rrf" | "snapmaker" | "galvo" | "k40" | "xtool" | "xtool_f1" | "xtool_m1")
    }
}

pub fn all() -> &'static [Machine] {
    static ALL: OnceLock<Vec<Machine>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut v: Vec<Machine> = serde_json::from_str(include_str!("../machines.json")).unwrap_or_default();
        v.sort_by_key(|m| (m.brand.to_lowercase(), m.model.to_lowercase()));
        v
    })
}

/// Entries whose brand, model, kind, source or notes hold every word of `q` (any case)
pub fn search(q: &str) -> Vec<&'static Machine> {
    let words: Vec<String> = q.to_lowercase().split_whitespace().map(String::from).collect();
    all()
        .iter()
        .filter(|m| {
            let hay = format!("{} {} {} {} {} {}w", m.brand, m.model, m.kind, m.source, m.notes, m.watts).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_reads_and_every_entry_is_usable() {
        let all = all();
        assert!(!all.is_empty(), "{} machines", all.len());
        for m in all {
            assert!(!m.brand.is_empty() && !m.model.is_empty(), "{m:?}");
            assert!(["ruida", "grbl", "marlin", "smoothie", "rrf", "snapmaker", "galvo", "k40", "xtool", "xtool_f1", "xtool_m1", "other"].contains(&m.kind.as_str()), "{m:?}");
            assert!(m.bed[0] >= 0.0 && m.bed[0] < 5000.0 && m.bed[1] >= 0.0 && m.bed[1] < 5000.0, "{m:?}");
            assert!(["", "rear-left", "rear-right", "front-left", "front-right"].contains(&m.home.as_str()), "{m:?}");
            if m.kind == "galvo" {
                assert!(m.lens_mm > 0.0 || m.bed[0] > 0.0, "a galvo with no field: {m:?}");
            }
        }
        assert!(!search("thunder nova").is_empty());
    }
}
