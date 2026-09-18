//! HTTP validators for the admin read endpoints.
//!
//! The console polls /router/stats every couple of seconds and the answer is usually byte-for-byte
//! identical to the previous one, so the request is worth answering with 304.
//!
//! The validator cannot be built from the request counter - that counter (and therefore the body)
//! moves *because* the poll happened, so "did it change?" would always answer yes. It is built from
//! the part of the body that describes the world instead: the process counters, the per-endpoint
//! statistics, health, cooldown and quota readings. Those move when a request is served or when a
//! background probe reports something, which is exactly when the console has something new to show.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// Real wall-clock seconds. Deliberately not `util::now_secs()`: that honours the fake-clock
/// override used by tests and manual runs, which would freeze the time part of the validator.
fn real_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Weak validator for the /router/stats body.
///
/// `volatile` names the fields that move by themselves and must not take part: the clock and the
/// uptime, plus `counters.requests`, which counts the poll itself - keeping it would make every
/// poll look like a change, which is the whole thing this is trying to detect. The fields that say
/// anything real about traffic (proxied, streams, retries, errors, bytes, plans/cash) are all kept,
/// so the console still refreshes the moment a request is served.
///
/// The minute bucket is a backstop for the two slow-moving parts no counter covers: a cooldown
/// expiring on its own, and a quota percentage refreshed by a background probe.
pub fn stats_revision(body: &Value, volatile: &[&str]) -> String {
    let mut stable = body.clone();
    if let Some(obj) = stable.as_object_mut() {
        if let Some(counters) = obj.get_mut("counters").and_then(|c| c.as_object_mut()) {
            for key in volatile {
                counters.remove(*key);
            }
        }
        for key in volatile {
            obj.remove(*key);
        }
    }
    let mut hasher = DefaultHasher::new();
    // serde_json renders object keys in a stable order, so the same document always hashes the same.
    serde_json::to_string(&stable).unwrap_or_default().hash(&mut hasher);
    format!("W/\"t{}-{:016x}\"", real_secs() / 60, hasher.finish())
}

/// Does the client already hold this exact body? Handles the list form and the `W/` prefix.
pub fn matches(if_none_match: Option<&str>, etag: &str) -> bool {
    let Some(raw) = if_none_match else { return false };
    if raw.trim() == "*" {
        return true;
    }
    let want = etag.trim();
    let want_bare = want.strip_prefix("W/").unwrap_or(want).trim_matches('"');
    raw.split(',').any(|candidate| {
        let c = candidate.trim();
        let c = c.strip_prefix("W/").unwrap_or(c).trim_matches('"');
        c == want_bare
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body(proxied: u64, peak: bool) -> Value {
        json!({
            "now": "2026-01-01T00:00:00Z",
            "uptime_secs": 12,
            "counters": {"requests": 5, "proxied": proxied},
            "router": {"peak": peak},
            "accounts": [{"name": "a", "stats": {"requests": 3}}],
        })
    }

    const VOLATILE: &[&str] = &["now", "uptime_secs", "requests"];

    #[test]
    fn the_clock_alone_does_not_move_the_validator() {
        let a = stats_revision(&body(1, true), VOLATILE);
        let mut same = body(1, true);
        same["now"] = json!("2026-01-01T00:00:02Z");
        same["uptime_secs"] = json!(14);
        assert_eq!(stats_revision(&same, VOLATILE), a, "a bare clock tick is not a change");
    }

    #[test]
    fn the_poll_counting_itself_does_not_move_the_validator() {
        let a = stats_revision(&body(1, true), VOLATILE);
        let mut polled = body(1, true);
        polled["counters"]["requests"] = json!(99);
        assert_eq!(
            stats_revision(&polled, VOLATILE),
            a,
            "the request counter counts this very poll, so it cannot be part of the validator"
        );
    }

    #[test]
    fn a_moved_counter_or_a_moved_state_moves_the_validator() {
        let a = stats_revision(&body(1, true), VOLATILE);
        assert_ne!(stats_revision(&body(2, true), VOLATILE), a, "counters");
        assert_ne!(stats_revision(&body(1, false), VOLATILE), a, "peak state");
        let mut nested = body(1, true);
        nested["accounts"][0]["stats"]["requests"] = json!(4);
        assert_ne!(stats_revision(&nested, VOLATILE), a, "per-endpoint statistics");
    }

    #[test]
    fn if_none_match_accepts_lists_and_weak_prefixes() {
        let tag = "W/\"t5-abcd\"";
        assert!(matches(Some(tag), tag));
        assert!(matches(Some("\"t5-abcd\""), tag), "the W/ prefix is not significant");
        assert!(matches(Some("W/\"other\", W/\"t5-abcd\""), tag));
        assert!(matches(Some("*"), tag));
        assert!(!matches(Some("W/\"t5-other\""), tag));
        assert!(!matches(None, tag));
    }
}
