//! Shared blocking HTTP client (ureq) used for upstream calls and probes.

use std::time::Duration;

use serde_json::Value;

use crate::util;

/// Build an agent. read_timeout is applied per socket read, so long SSE streams stay alive
/// as long as the upstream keeps sending keep-alives/tokens.
pub fn agent(connect_secs: u64, read_secs: u64, write_secs: u64) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(connect_secs.max(1)))
        .timeout_read(Duration::from_secs(read_secs.max(1)))
        .timeout_write(Duration::from_secs(write_secs.max(1)))
        .redirects(2)
        .user_agent("ar-ocg-router")
        .build()
}

/// GET a JSON document with a bearer token. Returns (status, parsed body) on HTTP responses and
/// Err(transport message) when the request never completed.
pub fn get_json(agent: &ureq::Agent, url: &str, key: &str, ua: &str) -> Result<(u16, Value), String> {
    let resp = agent
        .get(url)
        .set("Authorization", &format!("Bearer {}", key))
        .set("Accept", "application/json")
        .set("User-Agent", ua)
        .call();
    match resp {
        Ok(r) => {
            let status = r.status();
            let text = r.into_string().unwrap_or_default();
            let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
            Ok((status, parsed))
        }
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
            Ok((code, parsed))
        }
        Err(e) => Err(format!("{}", e)),
    }
}

/// Percent values arrive as 0..100 floats (OpenCode Go /zen/go/v1/usage).
pub fn pct(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

pub fn short_body(v: &Value, raw: &str, max: usize) -> String {
    if v.is_null() {
        return util::truncate(raw.trim(), max);
    }
    util::truncate(&util::json_compact(v), max)
}
