//! Runtime state persistence (ar-ocg-router.state.json): endpoint health + statistics counters.
//!
//! Reliability rules:
//!   * atomic writes: temp file in the same directory, flush, then rename over the target
//!   * the file is validated on load (version + JSON parse); anything invalid is moved aside as
//!     <name>.corrupt and ignored, never silently trusted
//!   * bounded write rate: changes are flushed at most once every 5 seconds, plus once on exit
//!   * the file is advisory only: losing it costs nothing but a few retry attempts and the
//!     counters restarting from zero

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::router::EndpointHealth;

pub const STATE_FILE: &str = "ar-ocg-router.state.json";
/// Bump only when an old file would be *misread*. Adding a block (as the counters block did) does
/// not qualify: an older file simply loads with those counters at zero.
const STATE_VERSION: u32 = 1;

pub fn state_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|d| d.join(STATE_FILE))
        .unwrap_or_else(|| PathBuf::from(STATE_FILE))
}

/// Everything that survives a restart. The local quota ledger is deliberately absent: it is
/// routing input, not a statistic, and inventing usage the provider never confirmed would
/// mis-route the plans.
#[derive(Debug, Default)]
pub struct Persisted {
    pub health: HashMap<String, EndpointHealth>,
    pub stats: HashMap<String, crate::state::AccountStats>,
    /// Process-wide counters (requests / streams / retries / bytes).
    pub counters: Option<crate::httpd::StaticsSnapshot>,
}

/// Load persisted state. Returns an empty value on any problem (and quarantines the file).
pub fn load(path: &Path) -> Persisted {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Persisted::default(),
    };
    let parse = |t: &str| -> Option<Persisted> {
        let v: Value = serde_json::from_str(t).ok()?;
        if v.get("version").and_then(|x| x.as_u64())? != STATE_VERSION as u64 {
            return None;
        }
        let mut health = HashMap::new();
        if let Some(eps) = v.get("endpoints").and_then(|x| x.as_object()) {
            for (name, e) in eps {
                health.insert(
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
        }
        let mut stats = HashMap::new();
        if let Some(xs) = v.get("stats").and_then(|x| x.as_object()) {
            for (name, s) in xs {
                if let Some(st) = crate::state::AccountStats::from_state_json(s) {
                    stats.insert(name.clone(), st);
                }
            }
        }
        let counters = v
            .get("counters")
            .and_then(crate::httpd::StaticsSnapshot::from_json);
        Some(Persisted { health, stats, counters })
    };
    match parse(&text) {
        Some(p) => {
            crate::log_info!(
                "restored runtime state from {} ({} endpoint(s), {} with counters)",
                path.display(),
                p.health.len(),
                p.stats.len()
            );
            p
        }
        None => {
            let corrupt = path.with_extension("json.corrupt");
            let _ = std::fs::rename(path, &corrupt);
            crate::log_warn!(
                "{} was unreadable or from a different version; moved to {} and ignored",
                path.display(),
                corrupt.display()
            );
            Persisted::default()
        }
    }
}

/// Serialise + atomic replace.
pub fn save(path: &Path, data: &Persisted, now: i64) -> Result<(), String> {
    let mut doc = json!({
        "version": STATE_VERSION,
        "updated_at": crate::timeutil::iso8601(now),
        "note": "endpoint health + statistics counters; safe to delete",
    });
    let obj = doc.as_object_mut().ok_or("state document is not an object")?;

    // Health is written for every configured endpoint, including the healthy zeros, so that the
    // file stays readable as the full roster. Counters are sparse: an endpoint that never served
    // a request stores nothing.
    let mut endpoints = serde_json::Map::new();
    for (name, h) in &data.health {
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
    let mut stats = serde_json::Map::new();
    for (name, s) in &data.stats {
        if s.is_empty() {
            continue;
        }
        stats.insert(name.clone(), s.to_state_json());
    }
    obj.insert("endpoints".to_string(), Value::Object(endpoints));
    obj.insert("stats".to_string(), Value::Object(stats));
    obj.insert(
        "counters".to_string(),
        data.counters.map(|c| c.to_json()).unwrap_or(Value::Null),
    );

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

/// Take a consistent snapshot of everything worth persisting.
///
/// The counters are written as running totals, not as deltas: a total can be checkpointed as often
/// as we like without the reader needing to know when the last write happened, so a restart can
/// neither double-count nor lose a request. The counter field is absent only until the first
/// request is served, which keeps an untouched installation free of a meaningless zero block.
pub fn snapshot(state: &Arc<crate::proxy::AppState>) -> Persisted {
    let counters = state.statics.snapshot();
    Persisted {
        health: state.router.health.snapshot(),
        stats: crate::state::stats_snapshot(state),
        counters: if counters == crate::httpd::StaticsSnapshot::default() {
            None
        } else {
            Some(counters)
        },
    }
}

/// Adopt the totals read from the state file, so the console's counters cover the lifetime of the
/// installation rather than the lifetime of this process.
pub fn restore_counters(state: &Arc<crate::proxy::AppState>, totals: crate::httpd::StaticsSnapshot) {
    state.statics.restore(totals);
}

/// Zero the process-wide counters. The caller must checkpoint immediately afterwards.
pub fn reset_counters(state: &Arc<crate::proxy::AppState>) {
    state.statics.reset();
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
            write(&state);
        })
        .ok();
}

/// Called on shutdown.
pub fn flush_now(state: &Arc<crate::proxy::AppState>) {
    write(state);
}

/// One flush; shared by the periodic flusher and shutdown.
pub fn write(state: &Arc<crate::proxy::AppState>) {
    let cfg = state.cfg();
    let path = state_path(&cfg.path);
    let data = snapshot(state);
    if let Err(e) = save(&path, &data, crate::util::now_secs()) {
        crate::log_warn!("cannot persist {}: {}", path.display(), e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AccountStats;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ocg-persist-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trip_keeps_health_counters_and_stats() {
        let dir = temp_dir("round");
        let path = dir.join(STATE_FILE);
        let mut data = Persisted::default();
        data.health.insert(
            "a".to_string(),
            EndpointHealth {
                streak: 2,
                skip_until: 1234,
                last_reason: Some("HTTP 404".to_string()),
                total_errors: 7,
            },
        );
        let mut st = AccountStats::default();
        st.requests = 9;
        st.successes = 8;
        st.errors = 1;
        st.stream_requests = 3;
        st.prompt_tokens = 1_000;
        st.completion_tokens = 200;
        st.cached_tokens = 64;
        st.cost_usd = 0.125;
        st.saved_usd = 1.5;
        st.latency_ms_total = 900;
        st.last_used = 1700;
        st.last_status = 200;
        st.last_error = Some("HTTP 500 boom".to_string());
        st.attempts_skipped = 4;
        data.stats.insert("a".to_string(), st);
        data.counters = Some(crate::httpd::StaticsSnapshot {
            requests: 11,
            proxied: 10,
            errors: 1,
            streams: 3,
            body_bytes_out: 4096,
            plans_used: 10,
            cash_used: 0,
            cold_starts: 2,
            retries: 1,
        });
        save(&path, &data, 1700).unwrap();

        let back = load(&path);
        let h = &back.health["a"];
        assert_eq!(h.streak, 2);
        assert_eq!(h.skip_until, 1234);
        assert_eq!(h.last_reason.as_deref(), Some("HTTP 404"));
        assert_eq!(h.total_errors, 7);
        let s = &back.stats["a"];
        assert_eq!(s.requests, 9);
        assert_eq!(s.successes, 8);
        assert_eq!(s.stream_requests, 3);
        assert_eq!(s.prompt_tokens, 1_000);
        assert_eq!(s.cached_tokens, 64);
        assert_eq!(s.saved_usd, 1.5);
        assert_eq!(s.latency_ms_total, 900);
        assert_eq!(s.attempts_skipped, 4);
        assert_eq!(s.last_error.as_deref(), Some("HTTP 500 boom"));
        let c = back.counters.expect("counters survive");
        assert_eq!(c.requests, 11);
        assert_eq!(c.body_bytes_out, 4096);
        assert_eq!(c.cold_starts, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_older_file_without_counters_still_loads() {
        let dir = temp_dir("older");
        let path = dir.join(STATE_FILE);
        std::fs::write(
            &path,
            r#"{"version":1,"endpoints":{"a":{"failure_streak":1,"skip_until":5,"total_errors":1}}}"#,
        )
        .unwrap();
        let data = load(&path);
        assert_eq!(data.health.len(), 1);
        assert_eq!(data.health["a"].streak, 1);
        assert!(data.stats.is_empty());
        assert!(data.counters.is_none());
        // the file is intact, not quarantined
        assert!(!dir.join("ar-ocg-router.state.json.corrupt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_idle_endpoint_is_not_written_to_the_stats_block() {
        let dir = temp_dir("sparse");
        let path = dir.join(STATE_FILE);
        let mut data = Persisted::default();
        data.health.insert("idle".to_string(), EndpointHealth::default());
        data.stats.insert("idle".to_string(), AccountStats::default());
        save(&path, &data, 1).unwrap();
        let back = load(&path);
        assert!(back.stats.is_empty());
        assert_eq!(back.health.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_quarantined_and_ignored() {
        let dir = temp_dir("corrupt");
        let path = dir.join(STATE_FILE);
        std::fs::write(&path, "{not json").unwrap();
        let data = load(&path);
        assert!(data.health.is_empty() && data.stats.is_empty() && data.counters.is_none());
        assert!(dir.join("ar-ocg-router.state.json.corrupt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
