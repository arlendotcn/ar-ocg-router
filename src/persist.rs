//! Endpoint health persistence (ar-ocg-router.state.json).
//!
//! Reliability rules:
//!   * atomic writes: temp file in the same directory, flush, then rename over the target
//!   * the file is validated on load (version + JSON parse); anything invalid is moved aside as
//!     <name>.corrupt and ignored, never silently trusted
//!   * bounded write rate: changes are flushed at most once every 5 seconds, plus once on exit
//!   * the file is advisory only: losing it costs nothing but a few retry attempts

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::router::EndpointHealth;

pub const STATE_FILE: &str = "ar-ocg-router.state.json";
const STATE_VERSION: u32 = 1;

pub fn state_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|d| d.join(STATE_FILE))
        .unwrap_or_else(|| PathBuf::from(STATE_FILE))
}

/// Load persisted endpoint health. Returns an empty map on any problem (and quarantines the file).
pub fn load(path: &Path) -> HashMap<String, EndpointHealth> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return HashMap::new(),
    };
    let parse = |t: &str| -> Option<HashMap<String, EndpointHealth>> {
        let v: Value = serde_json::from_str(t).ok()?;
        if v.get("version").and_then(|x| x.as_u64())? != STATE_VERSION as u64 {
            return None;
        }
        let mut out = HashMap::new();
        for (name, e) in v.get("endpoints")?.as_object()? {
            out.insert(
                name.clone(),
                EndpointHealth {
                    streak: e.get("failure_streak").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                    skip_until: e.get("skip_until").and_then(|x| x.as_i64()).unwrap_or(0),
                    last_reason: e
                        .get("last_reason")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string()),
                    total_errors: e.get("total_errors").and_then(|x| x.as_u64()).unwrap_or(0),
                },
            );
        }
        Some(out)
    };
    match parse(&text) {
        Some(map) => {
            crate::log_info!(
                "restored endpoint state from {} ({} endpoint(s))",
                path.display(),
                map.len()
            );
            map
        }
        None => {
            let corrupt = path.with_extension("json.corrupt");
            let _ = std::fs::rename(path, &corrupt);
            crate::log_warn!(
                "{} was unreadable or from a different version; moved to {} and ignored",
                path.display(),
                corrupt.display()
            );
            HashMap::new()
        }
    }
}

/// Serialise + atomic replace.
pub fn save(path: &Path, health: &HashMap<String, EndpointHealth>, now: i64) -> Result<(), String> {
    let mut endpoints = serde_json::Map::new();
    for (name, h) in health {
        endpoints.insert(
            name.clone(),
            json!({
                "failure_streak": h.streak,
                "skip_until": h.skip_until,
                "last_reason": h.last_reason,
                "total_errors": h.total_errors,
            }),
        );
    }
    let doc = json!({
        "version": STATE_VERSION,
        "updated_at": crate::timeutil::iso8601(now),
        "note": "endpoint health only; safe to delete",
        "endpoints": Value::Object(endpoints),
    });
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        f.write_all(b"\n").map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

static DIRTY: AtomicBool = AtomicBool::new(false);

/// Mark the state as changed; the background flusher writes it at most every 5s.
pub fn mark_dirty() {
    DIRTY.store(true, Ordering::Relaxed);
}

pub fn take_dirty() -> bool {
    DIRTY.swap(false, Ordering::Relaxed)
}

/// Background flusher + one final write when the process is asked to stop.
pub fn spawn_flusher(state: Arc<crate::proxy::AppState>) {
    std::thread::Builder::new()
        .name("state-flush".to_string())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            if !take_dirty() {
                continue;
            }
            let cfg = state.cfg();
            let path = state_path(&cfg.path);
            let snapshot = state.router.health.snapshot();
            let now = crate::util::now_secs();
            if let Err(e) = save(&path, &snapshot, now) {
                crate::log_warn!("cannot persist {}: {}", path.display(), e);
            }
        })
        .ok();
}

/// Called on shutdown.
pub fn flush_now(state: &Arc<crate::proxy::AppState>) {
    let cfg = state.cfg();
    let path = state_path(&cfg.path);
    let snapshot = state.router.health.snapshot();
    let now = crate::util::now_secs();
    if let Err(e) = save(&path, &snapshot, now) {
        crate::log_warn!("cannot persist {}: {}", path.display(), e);
    }
}
