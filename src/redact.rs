//! Redaction of a request body for the error log.
//!
//! Some upstreams answer "a parameter specified in the request is not valid" without saying which
//! parameter, and the console shows only the upstream's own words. This module exists for that
//! case: it turns a request body into a shape report - field names, value kinds, and the numeric
//! parameters verbatim - so the question "is max_output_tokens out of range?" can be answered
//! without writing the user's prompt into a log file.
//!
//! Rules:
//!   * a one-element array keeps reporting its single element's shape, so the innermost unknown
//!     object still shows up as {"type":"input_text","text":"<str:37>"}
//!   * strings become "<str:N>", never their contents
//!   * numbers are kept as-is: they are what upstream parameter validation is about
//!   * booleans, nulls and key lists are kept

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// Numbers longer than this are summarised instead of printed, so a stray 1e300 cannot bloat a line.
const MAX_ARRAY_ITEMS_REPORTED: usize = 8;

pub fn describe(v: &Value) -> Value {
    match v {
        Value::Null => json!("null"),
        Value::Bool(b) => json!(b),
        Value::Number(n) => json!(n),
        Value::String(s) => json!(format!("<str:{}>", s.chars().count())),
        Value::Array(items) => {
            if items.is_empty() {
                return json!("[]");
            }
            // A one-element array collapses to its element's shape: encoding clients wrap single
            // values in arrays, and the interesting object is one level down.
            if items.len() == 1 {
                return describe(&items[0]);
            }
            let mut kinds: Vec<String> = items.iter().take(MAX_ARRAY_ITEMS_REPORTED).map(kind_of).collect();
            if items.len() > MAX_ARRAY_ITEMS_REPORTED {
                kinds.push(format!("+{} more", items.len() - MAX_ARRAY_ITEMS_REPORTED));
            }
            json!(format!("[{}]", kinds.join(",")))
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, val) in map {
                // A conversation array is the one place where element *kinds* answer nothing: every
                // item is an object, while what decides whether an upstream accepts the request is
                // which kind each item is and whether it carries the fields that upstream requires
                // (a reasoning item with no `content`, for instance). Reported per item instead.
                let conversation = matches!(k.as_str(), "input" | "messages")
                    && matches!(val, Value::Array(a) if a.len() > 1);
                if conversation {
                    out.insert(k.clone(), describe_conversation(val.as_array().unwrap()));
                } else {
                    out.insert(k.clone(), describe(val));
                }
            }
            Value::Object(out)
        }
    }
}

/// A conversation, reported as a tally plus the items at the end of it.

/// The tail is what matters when a long conversation is rejected: it is where the current turn and
/// the most recent provider-specific artifacts are. Text never appears - only what each item is and
/// which of the fields an upstream may insist on are present.
fn describe_conversation(items: &[Value]) -> Value {
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let (mut text, mut summary, mut encrypted) = (0usize, 0usize, 0usize);
    for it in items {
        *kinds.entry(item_kind(it)).or_insert(0) += 1;
        if item_kind(it) == "reasoning" {
            if has_non_empty(it.get("content")) {
                text += 1;
            }
            if has_non_empty(it.get("summary")) {
                summary += 1;
            }
            if has_non_empty(it.get("encrypted_content")) {
                encrypted += 1;
            }
        }
    }
    let tally = kinds
        .iter()
        .map(|(k, n)| format!("{}:{}", k, n))
        .collect::<Vec<_>>()
        .join(" ");
    let tail: Vec<String> = items
        .iter()
        .rev()
        .take(8)
        .map(item_note)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    json!({
        "items": items.len(),
        "kinds": tally,
        "reasoning_with_text_or_summary_or_encrypted": format!("{}/{}/{}", text, summary, encrypted),
        "last": tail,
    })
}

fn item_kind(it: &Value) -> String {
    match it.get("type").and_then(|t| t.as_str()) {
        Some(t) => t.to_string(),
        // Chat-completions messages carry no `type`; their role is their kind.
        None => "message".to_string(),
    }
}

fn has_non_empty(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Object(m)) => !m.is_empty(),
        _ => false,
    }
}

/// One item, as `kind(details)`: for a reasoning item the details are the fields an upstream may
/// require (the plaintext text, the summary, the encrypted blob); for a message, its role.
fn item_note(it: &Value) -> String {
    let role = it.get("role").and_then(|r| r.as_str()).unwrap_or("");
    match item_kind(it).as_str() {
        "reasoning" => {
            let mut parts: Vec<String> = Vec::new();
            if has_non_empty(it.get("content")) {
                parts.push("text".to_string());
            }
            if has_non_empty(it.get("summary")) {
                parts.push("summary".to_string());
            }
            if has_non_empty(it.get("encrypted_content")) {
                parts.push("encrypted".to_string());
            }
            if parts.is_empty() {
                parts.push("empty".to_string());
            }
            format!("reasoning({})", parts.join(","))
        }
        "message" if !role.is_empty() => format!("message({})", role),
        other => other.to_string(),
    }
}

fn kind_of(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::Number(_) => "num".to_string(),
        Value::String(_) => "str".to_string(),
        Value::Array(_) => "array".to_string(),
        Value::Object(_) => "object".to_string(),
    }
}

/// One log line: the top-level field names first (that alone often answers the question), then the
/// redacted shape of the whole body.
pub fn describe_for_log(body: &Value) -> (String, String) {
    let names: Vec<String> = body
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let keys = names.join(",");
    let shape = serde_json::to_string(&describe(body)).unwrap_or_else(|_| "<unprintable>".to_string());
    (keys, shape)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_text_never_reaches_the_log() {
        let body = json!({
            "model": "glm-5.3-flash",
            "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "my secret plan"}]}],
            "max_output_tokens": 200000,
            "stream": true,
        });
        let (keys, shape) = describe_for_log(&body);
        assert!(keys.contains("max_output_tokens"));
        assert!(!shape.contains("my secret plan"), "prompt text leaked: {}", shape);
        assert!(!shape.contains("secret"));
        // "my secret plan" is 14 characters; the exact count is not the point, that it is a count
        // and not the text is.
        assert!(shape.contains("<str:14>"), "the string was not redacted: {}", shape);
        // The numbers are the whole point of the exercise: they must survive.
        assert!(shape.contains("200000"), "numeric parameters must be kept: {}", shape);
        assert!(shape.contains("true"));
        // A one-element array collapses to its element, so the innermost object's *fields*
        // survive even though the wrapper key ("input_text") is consumed by the collapse.
        assert!(shape.contains("\"text\""), "shape too coarse: {}", shape);
        assert!(shape.contains("\"content\""), "shape too coarse: {}", shape);
    }

    #[test]
    fn arrays_larger_than_one_report_their_element_kinds() {
        let body = json!({"tools": [{"a": 1}, {"b": 2}, {"c": 3}], "empty": [], "params": {"temperature": 5}});
        let (_, shape) = describe_for_log(&body);
        assert!(shape.contains("[object,object,object]"), "{}", shape);
        assert!(shape.contains("\"empty\":\"[]\""), "{}", shape);
        assert!(shape.contains("temperature"), "{}", shape);
    }
}

    #[test]
    fn a_conversation_reports_its_items_rather_than_their_kinds() {
        // The case that motivated this: an upstream rejects a conversation because a reasoning item
        // arrived without the text it requires. `[object,object,...]` cannot show that; the per-item
        // report can, and it still keeps every string out of the log.
        let body = json!({
            "model": "deepseek-flash",
            "input": [
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "my secret plan"}]},
                {"type": "reasoning", "id": "rs_1", "content": null, "summary": [{"type": "summary_text", "text": "private thoughts"}]},
                {"type": "function_call", "name": "get_time", "arguments": "{\"zone\":\"UTC\"}"},
                {"type": "function_call_output", "call_id": "call_1", "output": "12:00"},
                {"type": "reasoning", "id": "rs_2", "content": [{"type": "reasoning_text", "text": "more private thoughts"}]},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "done"}]}
            ],
            "stream": true,
        });
        let (keys, shape) = describe_for_log(&body);
        assert!(keys.contains("input"));
        assert!(shape.contains("\"items\":6"), "{}", shape);
        assert!(shape.contains("reasoning:2"), "{}", shape);
        assert!(shape.contains("message:2"), "{}", shape);
        // The tally that answers "did we send the text it wants?": one item with text, one with only
        // a summary, none encrypted.
        assert!(shape.contains("1/1/0"), "{}", shape);
        // The tail shows what each item actually is.
        assert!(shape.contains("reasoning(text)"), "{}", shape);
        assert!(shape.contains("reasoning(summary)"), "{}", shape);
        assert!(shape.contains("message(assistant)"), "{}", shape);
        // And no prompt text, as ever.
        assert!(!shape.contains("secret"), "{}", shape);
        assert!(!shape.contains("private"), "{}", shape);
        assert!(!shape.contains("12:00"), "{}", shape);
    }

    #[test]
    fn a_chat_conversation_is_reported_by_role() {
        let body = json!({
            "messages": [
                {"role": "system", "content": "be brief"},
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "hi"}
            ]
        });
        let (_, shape) = describe_for_log(&body);
        assert!(shape.contains("message(system)"), "{}", shape);
        assert!(shape.contains("message(assistant)"), "{}", shape);
        assert!(!shape.contains("be brief"), "{}", shape);
    }

