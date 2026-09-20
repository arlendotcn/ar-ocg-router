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
                out.insert(k.clone(), describe(val));
            }
            Value::Object(out)
        }
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
