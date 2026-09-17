//! The model library: reference metadata for the models this router can serve.
//!
//! Why it exists: an endpoint is "one merchant + one model", and the same model is sold under
//! different ids by different merchants with different real limits. This file records what is
//! *known* about each model (context window, max output, reasoning levels, modalities, and which
//! provider ids map to it) so the console can show it beside the endpoint and offer a model id
//! picker instead of a typing test.
//!
//! Stored beside config.yaml as `models.library.json`, NOT inside config.yaml: the console
//! regenerates config.yaml on every save and must not clobber this. It is advisory only - the
//! router never routes on it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub const LIBRARY_FILE: &str = "models.library.json";
const VERSION: u32 = 1;

#[derive(Debug, Clone, Default)]
pub struct Entry {
    /// Canonical id used inside the router (what the operator thinks of as "the model").
    pub id: String,
    pub label: String,
    /// Merchant-specific ids that all mean this model: "deepseek-flash" on DeepSeek official vs
    /// "deepseek-v4.1-flash" on a domestic plan is exactly the case this maps.
    pub aliases: Vec<String>,
    pub context_tokens: u64,
    pub max_output_tokens: u64,
    /// Reasoning/thinking effort levels the model accepts, weakest first.
    pub reasoning_levels: Vec<String>,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    pub notes: String,
    /// Free-form facts that do not deserve a column yet (pricing, release date, limits...).
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Library {
    pub entries: Vec<Entry>,
}

pub fn path_for(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|d| d.join(LIBRARY_FILE))
        .unwrap_or_else(|| PathBuf::from(LIBRARY_FILE))
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|i| i.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn u64_at(v: &Value, k: &str) -> u64 {
    v.get(k)
        .and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f as u64)))
        .unwrap_or(0)
}

/// Parse the library document. Entries without an id are skipped rather than fatal: the library
/// is hand-maintained and a half-written entry must not break the console.
pub fn parse(doc: &Value) -> Library {
    let mut entries = Vec::new();
    let arr = doc
        .get("models")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    for m in arr {
        let id = m.get("id").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
        if id.is_empty() {
            continue;
        }
        entries.push(Entry {
            label: m
                .get("label")
                .and_then(|x| x.as_str())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(&id)
                .to_string(),
            aliases: str_list(m.get("aliases")),
            context_tokens: u64_at(&m, "context_tokens"),
            max_output_tokens: u64_at(&m, "max_output_tokens"),
            reasoning_levels: str_list(m.get("reasoning_levels")),
            input_modalities: str_list(m.get("input_modalities")),
            output_modalities: str_list(m.get("output_modalities")),
            notes: m.get("notes").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            tags: str_list(m.get("tags")),
            id,
        });
    }
    Library { entries }
}

/// The library shipped inside the binary, used until the operator saves their own. It covers the
/// DeepSeek family this router is built around plus the common plan models, so a fresh install
/// shows useful limits instead of empty columns.
pub fn builtin() -> Library {
    let raw = include_str!("../assets/models.library.json");
    match serde_json::from_str::<Value>(raw) {
        Ok(v) => parse(&v),
        Err(_) => Library::default(),
    }
}

pub fn load(config_path: &Path) -> Library {
    let p = path_for(config_path);
    let Ok(text) = std::fs::read_to_string(&p) else {
        return builtin();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(v) => parse(&v),
        Err(e) => {
            crate::log_warn!(
                "{} is not valid JSON ({}), using the built-in library",
                p.display(),
                e
            );
            builtin()
        }
    }
}

pub fn to_json(lib: &Library) -> Value {
    let models: Vec<Value> = lib
        .entries
        .iter()
        .map(|e| {
            json!({
                "id": e.id,
                "label": e.label,
                "aliases": e.aliases,
                "context_tokens": e.context_tokens,
                "max_output_tokens": e.max_output_tokens,
                "reasoning_levels": e.reasoning_levels,
                "input_modalities": e.input_modalities,
                "output_modalities": e.output_modalities,
                "notes": e.notes,
                "tags": e.tags,
            })
        })
        .collect();
    json!({ "version": VERSION, "models": models })
}

/// Validate + atomically write. Duplicate ids are rejected: the whole point of the library is to
/// be one authoritative list.
pub fn save(config_path: &Path, doc: &Value) -> Result<Library, String> {
    let lib = parse(doc);
    if lib.entries.is_empty() {
        return Err("the library must contain at least one model".to_string());
    }
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    for e in &lib.entries {
        if seen.insert(e.id.to_ascii_lowercase(), ()).is_some() {
            return Err(format!("duplicate model id {:?}", e.id));
        }
    }
    let text = serde_json::to_string_pretty(&to_json(&lib)).map_err(|e| e.to_string())?;
    let p = path_for(config_path);
    let tmp = p.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        f.write_all(b"\n").map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
    Ok(lib)
}

/// Order a live catalog: models the library knows come first (largest context first, then
/// library order), then unknown ones alphabetically. With a 130-entry plan catalog this is what
/// puts the likely choices at the top of the picker.
pub fn rank_models(ids: &[String], lib: &Library) -> Vec<String> {
    let mut known: Vec<(u64, i64, String)> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for id in ids {
        match find(lib, id) {
            Some(e) => {
                let pos = lib
                    .entries
                    .iter()
                    .position(|x| x.id.eq_ignore_ascii_case(&e.id))
                    .unwrap_or(0) as i64;
                known.push((e.context_tokens, -pos, id.clone()));
            }
            None => unknown.push(id.clone()),
        }
    }
    known.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    unknown.sort_by_key(|s| s.to_ascii_lowercase());
    let mut out: Vec<String> = known.into_iter().map(|(_, _, id)| id).collect();
    out.extend(unknown);
    out
}

/// Look up one model id (canonical or alias) so the UI can annotate an endpoint.
pub fn find<'a>(lib: &'a Library, id: &str) -> Option<&'a Entry> {
    lib.entries
        .iter()
        .find(|e| e.id.eq_ignore_ascii_case(id) || e.aliases.iter().any(|a| a.eq_ignore_ascii_case(id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib() -> Library {
        parse(&json!({
            "models": [
                {"id": "big", "context_tokens": 200000, "aliases": ["big-alias"], "reasoning_levels": ["low", "high"], "input_modalities": ["text", "image"]},
                {"id": "small", "context_tokens": 32000},
                {"id": "mid", "context_tokens": 128000}
            ]
        }))
    }

    #[test]
    fn parse_skips_entries_without_an_id() {
        let l = parse(&json!({"models": [{"label": "nope"}, {"id": "ok"}]}));
        assert_eq!(l.entries.len(), 1);
        assert_eq!(l.entries[0].id, "ok");
        assert_eq!(l.entries[0].label, "ok", "a missing label falls back to the id");
    }

    #[test]
    fn rank_puts_known_models_first_and_keeps_everything() {
        let ids: Vec<String> = ["zzz-unknown", "small", "aaa-unknown", "big", "mid"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let ranked = rank_models(&ids, &lib());
        assert_eq!(ranked, vec!["big", "mid", "small", "aaa-unknown", "zzz-unknown"]);
        assert_eq!(ranked.len(), ids.len(), "no model may be dropped");
    }

    #[test]
    fn find_matches_an_alias_case_insensitively() {
        let l = lib();
        assert_eq!(find(&l, "BIG-ALIAS").map(|e| e.id.as_str()), Some("big"));
        assert!(find(&l, "missing").is_none());
    }

    #[test]
    fn save_rejects_duplicates_and_persists_atomically() {
        let dir = std::env::temp_dir().join(format!("ar-ocg-lib-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.yaml");

        assert!(save(&cfg, &json!({"models": [{"id": "a"}, {"id": "A"}]})).is_err());
        assert!(save(&cfg, &json!({"models": []})).is_err());

        let ok = json!({"models": [{"id": "a", "context_tokens": 1000, "reasoning_levels": ["low"]}]});
        save(&cfg, &ok).unwrap();
        let back = load(&cfg);
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries[0].context_tokens, 1000);
        assert_eq!(back.entries[0].reasoning_levels, vec!["low"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builtin_library_parses_and_covers_the_deepseek_family() {
        let l = builtin();
        assert!(!l.entries.is_empty(), "the shipped library must parse");
        for id in ["deepseek-flash", "deepseek-v4-pro"] {
            assert!(find(&l, id).is_some(), "{} should ship in the built-in library", id);
        }
    }
}
