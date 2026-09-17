//! SSE / JSON usage extraction. Used to account tokens and cost without altering the byte
//! stream that flows back to the client.

use serde_json::Value;

use crate::util;

#[derive(Debug, Default, Clone, Copy)]
pub struct RawUsage {
    pub prompt: u64,
    pub completion: u64,
    pub cached: u64,
    /// Cost reported by the upstream itself (OpenCode Go returns {"cost":"0.0012"}).
    pub upstream_cost: f64,
    pub seen: bool,
}

impl RawUsage {
    fn merge(&mut self, other: RawUsage) {
        self.prompt = self.prompt.max(other.prompt);
        self.completion = self.completion.max(other.completion);
        self.cached = self.cached.max(other.cached);
        if other.upstream_cost > self.upstream_cost {
            self.upstream_cost = other.upstream_cost;
        }
        self.seen |= other.seen;
    }
}

fn num(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse::<u64>().unwrap_or(0),
        _ => 0,
    }
}

fn fnum(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Extract usage from an OpenAI chat-completions / responses payload (either the final
/// JSON object or a single SSE data event).
pub fn extract_usage(v: &Value) -> RawUsage {
    let mut u = RawUsage::default();
    let usage = match v.get("usage") {
        Some(Value::Object(_)) => v.get("usage"),
        // SSE "response.completed"/"response.incomplete" events wrap the response object.
        _ => v
            .get("response")
            .and_then(|r| r.get("usage"))
            .filter(|x| x.is_object())
            .or_else(|| v.get("message").and_then(|m| m.get("usage"))),
    };
    if let Some(usage) = usage {
        let prompt = usage
            .get("prompt_tokens")
            .or_else(|| usage.get("input_tokens"));
        let completion = usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"));
        u.prompt = num(prompt);
        u.completion = num(completion);
        u.cached = num(usage
            .get("prompt_cache_hit_tokens")
            .or_else(|| usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens")))
            .or_else(|| usage
                .get("input_tokens_details")
                .and_then(|d| d.get("cached_tokens"))));
        u.seen = true;
    }
    let cost = v
        .get("cost")
        .or_else(|| v.get("response").and_then(|r| r.get("cost")));
    if let Some(c) = cost {
        let c = fnum(Some(c));
        if c > 0.0 {
            u.upstream_cost = c;
        }
    }
    u
}

/// Streams bytes through unmodified while scanning SSE data lines for usage.
#[derive(Debug)]
pub struct UsageScanner {
    buf: Vec<u8>,
    pub usage: RawUsage,
    events: u64,
}

impl Default for UsageScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl UsageScanner {
    pub fn new() -> UsageScanner {
        UsageScanner {
            buf: Vec::with_capacity(8192),
            usage: RawUsage::default(),
            events: 0,
        }
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        loop {
            let pos = match self.buf.iter().position(|b| *b == b'\n') {
                Some(p) => p,
                None => break,
            };
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.consume_line(&line);
        }
        // Safety valve: never let a pathological stream grow the buffer.
        if self.buf.len() > 512 * 1024 {
            self.buf.clear();
        }
    }

    fn consume_line(&mut self, line: &[u8]) {
        let s = String::from_utf8_lossy(line);
        let s = s.trim();
        if s.is_empty() || s.starts_with(':') {
            return;
        }
        let payload = match s.strip_prefix("data:") {
            Some(rest) => rest.trim(),
            None => {
                // Non-SSE JSON body (only the first line is scanned for those).
                if !s.starts_with('{') {
                    return;
                }
                s
            }
        };
        if payload == "[DONE]" || payload.is_empty() {
            return;
        }
        if !payload.starts_with('{') {
            return;
        }
        // Cheap pre-filter before paying for a full parse.
        if !(payload.contains("usage") || payload.contains("cost")) {
            return;
        }
        self.events += 1;
        if let Ok(v) = serde_json::from_str::<Value>(payload) {
            let u = extract_usage(&v);
            self.usage.merge(u);
        }
    }

    pub fn finish(&mut self) {
        if !self.buf.is_empty() {
            let rest: Vec<u8> = self.buf.drain(..).collect();
            self.consume_line(&rest);
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "tokens in={} out={} cached={} events={}",
            self.usage.prompt, self.usage.completion, self.usage.cached, self.events
        )
    }
}

/// Error body preview for logs.
pub fn brief(v: &Value) -> String {
    let msg = v
        .get("error")
        .and_then(|e| {
            e.get("message")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
                .or_else(|| e.as_str().map(|s| s.to_string()))
        })
        .or_else(|| v.get("message").and_then(|m| m.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| util::json_compact(v));
    util::truncate(&msg, 300)
}
