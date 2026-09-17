//! Endpoint identity: every configured endpoint serves exactly ONE hard-coded model.
//!
//! The client-visible model name carries no routing meaning: GET /v1/models advertises the
//! single id "ar-ocg-router" and whatever the client sends in "model" is ignored (it may
//! optionally force a specific endpoint, see `parse_forced_endpoint`).
//!
//! What matters is the endpoint's own `provider` + `model`, which is what actually goes on the
//! wire. Nothing is translated, normalised or guessed at runtime: if the configured id is wrong,
//! the upstream says so and the router falls back to the next endpoint.

use serde_json::{json, Value};

/// The one model id this router advertises to clients.
pub const ROUTER_MODEL: &str = "ar-ocg-router";

/// Which product an endpoint belongs to (decides defaults + how quota is probed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpencodeGo,
    DeepSeek,
    Generic,
}

impl ProviderKind {
    /// Detect the product from the endpoint host. Deliberately url-only: a prepaid plan can point
    /// at a bridge/proxy, and the plan itself says nothing about which vendor protocol it speaks.
    /// Use the endpoint's "provider:" field to override.
    pub fn detect(url: &str) -> ProviderKind {
        let u = url.to_ascii_lowercase();
        if u.contains("opencode.ai") {
            ProviderKind::OpencodeGo
        } else if u.contains("deepseek.com") {
            ProviderKind::DeepSeek
        } else {
            ProviderKind::Generic
        }
    }

    pub fn parse(s: &str) -> Option<ProviderKind> {
        let t = s.trim().to_ascii_lowercase().replace('_', "-");
        match t.as_str() {
            "opencodego" | "opencode-go" | "go" | "zen" | "ocg" => Some(ProviderKind::OpencodeGo),
            "deepseek" | "ds" | "official" | "deepseek-official" => Some(ProviderKind::DeepSeek),
            "generic" | "openai" | "openai-compatible" | "other" | "custom" => {
                Some(ProviderKind::Generic)
            }
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::OpencodeGo => "opencodego",
            ProviderKind::DeepSeek => "deepseek",
            ProviderKind::Generic => "generic",
        }
    }
}

/// Wire protocol of an endpoint / of an incoming request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// OpenAI chat completions (/chat/completions)
    Chat,
    /// OpenAI responses (/responses)
    Responses,
    /// Anthropic messages (/messages)
    Anthropic,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Vec<Mode>> {
        let t = s.trim().to_ascii_lowercase().replace('_', "-");
        let out = match t.as_str() {
            "openai-completion" | "openai-completions" | "openai-chat" | "chat"
            | "chat-completions" => vec![Mode::Chat],
            "openai-responses" | "openai-response" | "responses" | "response" => {
                vec![Mode::Responses]
            }
            "anthropic" | "anthropic-messages" | "messages" | "claude" => vec![Mode::Anthropic],
            "both" | "openai" | "all-openai" | "auto" => vec![Mode::Chat, Mode::Responses],
            "any" | "all" => vec![Mode::Chat, Mode::Responses, Mode::Anthropic],
            _ => return None,
        };
        Some(out)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Chat => "openai-completion",
            Mode::Responses => "openai-responses",
            Mode::Anthropic => "anthropic-messages",
        }
    }
}

/// What the client asked for, derived from the request path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Chat,
    Responses,
    Anthropic,
    Models,
    Other,
}

impl Endpoint {
    pub fn required_mode(&self) -> Option<Mode> {
        match self {
            Endpoint::Chat => Some(Mode::Chat),
            Endpoint::Responses => Some(Mode::Responses),
            Endpoint::Anthropic => Some(Mode::Anthropic),
            _ => None,
        }
    }
}

/// Strip a leading /v1 (or /openai) prefix so both base-URL styles work.
pub fn normalize_path(path: &str) -> String {
    let p = path.trim();
    let p = p.split('?').next().unwrap_or(p);
    let mut cur = p;
    for pre in ["/v1", "/openai/v1", "/api/v1"] {
        if let Some(rest) = cur.strip_prefix(pre) {
            if rest.is_empty() {
                cur = "/";
            } else {
                cur = rest;
                break;
            }
        }
    }
    if cur.is_empty() {
        "/".to_string()
    } else if !cur.starts_with('/') {
        format!("/{}", cur)
    } else {
        cur.to_string()
    }
}

pub fn endpoint_of(path: &str) -> Endpoint {
    let p = normalize_path(path);
    if p == "/models" || p.starts_with("/models/") {
        return Endpoint::Models;
    }
    if p.contains("/chat/completions") {
        return Endpoint::Chat;
    }
    if p.contains("/responses") {
        return Endpoint::Responses;
    }
    if p.ends_with("/messages") || p.contains("/messages?") {
        return Endpoint::Anthropic;
    }
    // Legacy/FIM/other OpenAI-compatible POST endpoints ride the chat protocol.
    if p.contains("/completions") || p.contains("/embeddings") || p.contains("/beta") {
        return Endpoint::Chat;
    }
    Endpoint::Other
}

/// Optional request to pin one endpoint for this call: "name/model", "name" or "provider/model".
/// This is the ONLY thing a client-supplied model string can influence; it never selects a
/// combination of provider and model other than the one that endpoint was configured with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForcedEndpoint {
    /// Match the endpoint's configured name (spaces/slashes normalised like the auto names).
    Name(String),
    /// Match provider + model (first endpoint wins).
    ProviderModel(ProviderKind, String),
}

/// Parse a client model string that may pin an endpoint.
/// Accepted forms (anything else is ignored and routed normally):
///   "ar-ocg-router"          -> no pin
///   "<name>/<model>"         -> pin by endpoint name (name gets normalised)
///   "<name>"                 -> pin by endpoint name when it is a configured name
///   "<provider>/<model>"     -> pin by provider+model
pub fn parse_forced_endpoint(model: &str, known_names: &[String]) -> Option<ForcedEndpoint> {
    let m = model.trim();
    if m.is_empty() || m.eq_ignore_ascii_case(ROUTER_MODEL) {
        return None;
    }
    let normalize = |s: &str| {
        s.trim()
            .to_ascii_lowercase()
            .replace([' ', '/'], "-")
            .trim_matches('-')
            .to_string()
    };
    // "<name>" without a slash: only pin when it names a configured endpoint.
    if !m.contains('/') {
        let norm = normalize(m);
        if known_names.iter().any(|n| *n == norm) {
            return Some(ForcedEndpoint::Name(norm));
        }
        return None;
    }
    let (head, tail) = m.split_once('/').unwrap();
    let head_raw = head.trim();
    let tail_raw = tail.trim();
    // provider/model ?
    if let Some(p) = ProviderKind::parse(head_raw) {
        if !tail_raw.is_empty() {
            return Some(ForcedEndpoint::ProviderModel(p, tail_raw.to_string()));
        }
    }
    // name/model ?
    let head_norm = normalize(head_raw);
    if known_names.iter().any(|n| *n == head_norm) {
        return Some(ForcedEndpoint::Name(head_norm));
    }
    // name only (e.g. "vol-ark/") or a name the parser normalised differently
    if tail_raw.is_empty() && known_names.iter().any(|n| *n == head_norm) {
        return Some(ForcedEndpoint::Name(head_norm));
    }
    None
}

/// Normalise a user supplied endpoint name the same way auto-generated names are built.
pub fn normalize_name(s: &str) -> String {
    let n = s
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '/'], "-")
        .trim_matches('-')
        .to_string();
    if n.is_empty() {
        "endpoint".to_string()
    } else {
        n
    }
}

/// The single-model payload advertised on GET /v1/models.
pub fn models_payload() -> Value {
    json!({
        "object": "list",
        "data": [
            {
                "id": ROUTER_MODEL,
                "object": "model",
                "created": 0,
                "owned_by": "ar-ocg-router",
            }
        ]
    })
}
