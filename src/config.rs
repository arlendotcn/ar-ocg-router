//! config.yaml parsing (tolerant, hand-written on top of serde_yaml::Value) + validation.

use std::path::{Path, PathBuf};

use serde_yaml::Value as Y;

use crate::models::{Mode, ProviderKind};
use crate::timeutil;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Two kinds of upstream account:
///   * Plans - a prepaid subscription/quota bucket (OpenCode Go, 火山方舟 Coding Plan, 百炼 Token Plan,
///     腾讯 TokenHub ...). Consuming them costs no cash until the quota runs out, so the router
///     tries to use them first and keeps track of how much is left.
///   * Cash  - pay-as-you-go accounts, billed per token (DeepSeek official). Their price varies
///     with the peak/off-peak window, which is what the whole peak-vs-idle policy is about.
pub enum AccountKind {
    Plans,
    Cash,
}

impl AccountKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountKind::Plans => "plans",
            AccountKind::Cash => "fallback",
        }
    }
}

/// When a fallback account may be *preferred* (it always stays usable as an error fallback
/// unless no_error_fallback is set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    Always,
    Peak,
    OffPeak,
    QuotaLow,
    QuotaExhausted,
    PrimaryUnavailable,
    Never,
}

impl Rule {
    pub fn parse_token(tok: &str) -> Option<Rule> {
        let t = tok.trim().to_ascii_lowercase().replace(' ', "_");
        let t = t.replace('-', "_");
        let r = match t.as_str() {
            "always" | "any" | "*" | "all_hours" | "anytime" => Rule::Always,
            "peak" | "busy" | "onpeak" | "on_peak" => Rule::Peak,
            "offpeak" | "idle" | "off_peak" | "low" | "cheap" => Rule::OffPeak,
            "quota_low" | "quota_scarce" | "scarcity" | "low_quota" => Rule::QuotaLow,
            "quota_exhausted" | "exhausted" | "no_quota" => Rule::QuotaExhausted,
            "primary_unavailable" | "no_primary" | "no_opencodego" => Rule::PrimaryUnavailable,
            "never" | "none" | "disabled" => Rule::Never,
            _ => return None,
        };
        Some(r)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Rule::Always => "always",
            Rule::Peak => "peak",
            Rule::OffPeak => "offpeak",
            Rule::QuotaLow => "quota_low",
            Rule::QuotaExhausted => "quota_exhausted",
            Rule::PrimaryUnavailable => "primary_unavailable",
            Rule::Never => "never",
        }
    }
}

/// What the account's quota is measured in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaUnit {
    /// Dollars (OpenCode Go tracks DeepSeek list prices).
    Usd,
    /// Plain tokens (most domestic coding/token plans, e.g. 腾讯 Hy Token Plan).
    Tokens,
    /// Nothing measurable: the router only counts what it sent.
    None,
}

impl QuotaUnit {
    pub fn as_str(&self) -> &'static str {
        match self {
            QuotaUnit::Usd => "usd",
            QuotaUnit::Tokens => "tokens",
            QuotaUnit::None => "none",
        }
    }
    pub fn parse(s: &str) -> Option<QuotaUnit> {
        match s.trim().to_ascii_lowercase().as_str() {
            "usd" | "$" | "dollar" | "dollars" | "money" => Some(QuotaUnit::Usd),
            "token" | "tokens" | "tok" | "credit" | "credits" => Some(QuotaUnit::Tokens),
            "none" | "off" | "na" | "n/a" => Some(QuotaUnit::None),
            _ => None,
        }
    }
}

/// How quota information is obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaProbe {
    /// Provider usage endpoint (OpenCode Go /usage).
    Usage,
    /// Provider balance endpoint (DeepSeek /user/balance).
    Balance,
    /// No endpoint: rely on the local ledger only.
    None,
}

impl QuotaProbe {
    pub fn as_str(&self) -> &'static str {
        match self {
            QuotaProbe::Usage => "usage",
            QuotaProbe::Balance => "balance",
            QuotaProbe::None => "none",
        }
    }
    pub fn parse(s: &str) -> Option<QuotaProbe> {
        match s.trim().to_ascii_lowercase().as_str() {
            "usage" | "quota" | "auto" => Some(QuotaProbe::Usage),
            "balance" | "credit" => Some(QuotaProbe::Balance),
            "none" | "off" | "local" => Some(QuotaProbe::None),
            _ => None,
        }
    }
}

/// Sliding-window quota of one account.
#[derive(Debug, Clone)]
pub struct QuotaCfg {
    pub unit: QuotaUnit,
    /// 5 hour / rolling window limit (in 'unit').
    pub rolling: f64,
    /// weekly limit
    pub weekly: f64,
    /// monthly limit
    pub monthly: f64,
    pub probe: QuotaProbe,
    /// How often the probe runs.
    pub refresh_secs: u64,
}

impl QuotaCfg {
    /// Defaults per provider: OpenCode Go tracks dollar windows (5h 20% / week 50% / month 100%
    /// of a $60 monthly plan), DeepSeek official has no quota (balance instead), everything else
    /// is unmeasurable until the user says otherwise.
    pub fn for_provider(provider: ProviderKind) -> QuotaCfg {
        match provider {
            ProviderKind::OpencodeGo => QuotaCfg {
                unit: QuotaUnit::Usd,
                rolling: 12.0,
                weekly: 30.0,
                monthly: 60.0,
                probe: QuotaProbe::Usage,
                refresh_secs: 60,
            },
            ProviderKind::DeepSeek => QuotaCfg {
                unit: QuotaUnit::None,
                rolling: 0.0,
                weekly: 0.0,
                monthly: 0.0,
                probe: QuotaProbe::Balance,
                refresh_secs: 300,
            },
            ProviderKind::Generic => QuotaCfg {
                unit: QuotaUnit::None,
                rolling: 0.0,
                weekly: 0.0,
                monthly: 0.0,
                probe: QuotaProbe::None,
                refresh_secs: 300,
            },
        }
    }

    pub fn measures_something(&self) -> bool {
        self.unit != QuotaUnit::None && (self.rolling > 0.0 || self.weekly > 0.0 || self.monthly > 0.0)
    }
}

#[derive(Debug, Clone)]
pub struct AccountCfg {
    pub name: String,
    pub kind: AccountKind,
    /// Consumption order inside its kind (smaller first; the list order is the default).
    pub order: i32,
    pub provider: ProviderKind,
    pub url: String,
    pub key: String,
    /// The ONE model this endpoint serves. Sent verbatim on every request: nothing is
    /// translated, normalised or guessed, so a wrong id fails loudly at the upstream.
    pub model: String,
    pub modes: Vec<Mode>,
    pub rules: Vec<Rule>,
    pub no_error_fallback: bool,
    pub drop_params: Vec<String>,
    pub extra_headers: Vec<(String, String)>,
    pub quota: QuotaCfg,
    pub inject_session: bool,
    /// Explicit opt-out written by the console ("enabled: false"). Kept separate from the rule
    /// list because a prepaid plan's rules are not a preference list in the same way.
    pub disabled: bool,
}

impl AccountCfg {
    /// "provider/model" -- the endpoint identity used in logs, stats and forced pinning.
    pub fn identity(&self) -> String {
        format!("{}/{}", self.provider.as_str(), self.model)
    }
}

impl AccountCfg {
    pub fn supports_mode(&self, m: Mode) -> bool {
        self.modes.contains(&m)
    }

    /// Whether this endpoint may be selected at all.
    ///
    /// The console's disable toggle is a real switch, not a hint: a disabled endpoint keeps its
    /// configuration but is dropped from every candidate list, including the error-fallback pass.
    /// Cash accounts express it as `rule: [never]`; prepaid plans have no rule list to speak of,
    /// so they use the explicit flag.
    pub fn is_enabled(&self) -> bool {
        if self.disabled {
            return false;
        }
        !matches!(self.rules.as_slice(), [Rule::Never])
    }

    /// Base URL with any trailing slash removed.
    pub fn base(&self) -> String {
        self.url.trim_end_matches('/').to_string()
    }

    /// Build the upstream URL for a normalized request path (both /v1 and bare styles work).
    pub fn upstream_url(&self, path_with_query: &str) -> String {
        let base = self.base();
        let (path, query) = match path_with_query.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (path_with_query, None),
        };
        let normalized = crate::models::normalize_path(path);
        let mut suffix = normalized;
        if base.ends_with("/v1") {
            if let Some(rest) = suffix.strip_prefix("/v1") {
                suffix = rest.to_string();
            }
        }
        let url = format!("{}{}", base, suffix);
        match query {
            Some(q) => format!("{}?{}", url, q),
            None => url,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterMode {
    Auto,
    Plans,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdlePrefer {
    /// Off-peak: use the cheap cash account unless some plan's quota would otherwise go unused.
    SurplusFirst,
    /// Off-peak: always prefer the paid plans.
    Plans,
    /// Off-peak: always prefer cash.
    Fallback,
}

/// Fallback identity when a client sends no session header at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFallback {
    /// One stable id for the whole process: keeps upstream caches warm, at the cost of sharing
    /// one cache/session domain between all clients.
    Process,
    /// A fresh id per request: no cross-talk, but no upstream cache reuse.
    PerRequest,
}

impl SessionFallback {
    pub fn parse(s: &str) -> Option<SessionFallback> {
        match s.trim().to_ascii_lowercase().as_str() {
            "process" | "shared" | "global" => Some(SessionFallback::Process),
            "per-request" | "per_request" | "request" | "none" => Some(SessionFallback::PerRequest),
            _ => None,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionFallback::Process => "process",
            SessionFallback::PerRequest => "per-request",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServerCfg {
    pub host: String,
    pub port: u16,
    pub max_connections: usize,
    pub client_keys: Vec<String>,
    pub max_body_bytes: usize,
    pub idle_timeout_secs: u64,
    pub read_timeout_secs: u64,
    /// Forward streaming responses chunk-by-chunk instead of buffering.
    pub stream: bool,
}

impl Default for ServerCfg {
    fn default() -> Self {
        ServerCfg {
            host: "127.0.0.1".to_string(),
            port: 8787,
            max_connections: 64,
            client_keys: Vec::new(),
            max_body_bytes: 64 * 1024 * 1024,
            idle_timeout_secs: 1800,
            read_timeout_secs: 600,
            stream: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogCfg {
    pub level: String,
    pub file: Option<String>,
    pub quiet: bool,
}

impl Default for LogCfg {
    fn default() -> Self {
        LogCfg {
            level: "info".to_string(),
            file: None,
            quiet: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RouterCfg {
    pub mode: RouterMode,
    pub peak_spec: String,
    pub peak_windows: Vec<timeutil::PeakWindow>,
    pub idle_prefer: IdlePrefer,
    pub surplus_max_pct: f64,
    pub surplus_projection: bool,
    pub exhaust_at_pct: f64,
    pub quota_refresh_secs: u64,
    pub cooldown_secs: u64,
    pub auth_cooldown_secs: u64,
    pub server_error_cooldown_secs: u64,
    pub retry_on_model_error: bool,
    /// After this many consecutive "this endpoint cannot serve it" failures the endpoint is
    /// skipped entirely for skip_secs (on top of the normal cooldown).
    pub skip_after_failures: u32,
    pub skip_secs: u64,
    /// Wall-clock budget for the whole retry chain. Streaming requests only count the time until
    /// the first byte, so long generations are never cut off by it.
    pub attempt_budget_secs: u64,
    /// Keep a conversation on one endpoint. This is deterministic coordination, not optimisation:
    /// upstreams that scope prompt caches (or server-side conversation state) per session lose
    /// that feature when the session hops between endpoints. Only honoured while the endpoint is
    /// healthy: cooling/skipped endpoints are never kept.
    pub session_affinity: bool,
    pub session_affinity_ttl_secs: u64,
    /// What to use as the session key when the client sends no session header.
    pub session_fallback: SessionFallback,
    pub inject_stream_usage: bool,
    pub session_headers: Vec<String>,
    pub user_agent: String,
    pub request_timeout_secs: u64,
    pub stream_idle_timeout_secs: u64,
}

impl Default for RouterCfg {
    fn default() -> Self {
        RouterCfg {
            mode: RouterMode::Auto,
            peak_spec: timeutil::DEFAULT_PEAK_SPEC.to_string(),
            peak_windows: timeutil::parse_peak_spec(timeutil::DEFAULT_PEAK_SPEC).unwrap_or_default(),
            idle_prefer: IdlePrefer::SurplusFirst,
            surplus_max_pct: 80.0,
            surplus_projection: true,
            exhaust_at_pct: 99.0,
            quota_refresh_secs: 60,
            cooldown_secs: 30,
            auth_cooldown_secs: 600,
            server_error_cooldown_secs: 20,
            retry_on_model_error: true,
            skip_after_failures: 3,
            skip_secs: 300,
            attempt_budget_secs: 120,
            session_affinity: true,
            session_affinity_ttl_secs: 1800,
            session_fallback: SessionFallback::Process,
            inject_stream_usage: false,
            session_headers: vec![
                "x-opencode-session".to_string(),
                "x-session-id".to_string(),
                "session-id".to_string(),
                "x-dsh-session".to_string(),
                "x-dsh-session-id".to_string(),
                "x-claude-code-session-id".to_string(),
                "x-conversation-id".to_string(),
                "conversation_id".to_string(),
                "x-thread-id".to_string(),
                "thread-id".to_string(),
                "x-request-session".to_string(),
            ],
            user_agent: format!("ar-ocg-router/{} (multi-account-router)", env!("CARGO_PKG_VERSION")),
            request_timeout_secs: 900,
            stream_idle_timeout_secs: 300,
        }
    }
}

#[derive(Debug, Clone)]
/// Compatibility rewrites. Everything here is OFF by default: the router forwards the request
/// body byte-for-byte (except "model", which IS the endpoint identity) and forwards protocol
/// headers unchanged. If an upstream rejects something, the configuration author decides which
/// rewrite to enable - the router does not guess.
pub struct CompatCfg {
    /// Rewrite role:"developer" to "system" in messages/input.
    pub developer_role_to_system: bool,
    /// Rename max_completion_tokens to max_tokens on chat requests.
    pub max_completion_tokens_to_max_tokens: bool,
    /// Extra body fields to drop (defaults to none).
    pub drop_params: Vec<String>,
    /// Extra request headers forwarded verbatim from the client (protocol headers are always
    /// forwarded; see PROTOCOL_HEADERS).
    pub forward_headers: Vec<String>,
}

/// Request headers that are forwarded to the upstream verbatim, without interpreting them.
/// Deliberately protocol/feature oriented: beta switches, API versions, session and idempotency
/// hints. Authorization is never forwarded (the endpoint key is used instead).
pub const PROTOCOL_HEADERS: [&str; 16] = [
    "anthropic-version",
    "anthropic-beta",
    "anthropic-dangerous-direct-browser-access",
    "openai-beta",
    "openai-organization",
    "openai-project",
    "x-api-key",
    "x-app",
    "x-client-version",
    "x-opencode-session",
    "x-session-id",
    "session-id",
    "x-dsh-session",
    "x-dsh-session-id",
    "x-stainless-arch",
    "x-stainless-os",
];

impl Default for CompatCfg {
    fn default() -> Self {
        CompatCfg {
            // OFF by default: see the struct docs (lossless passthrough first).
            developer_role_to_system: false,
            max_completion_tokens_to_max_tokens: false,
            drop_params: Vec::new(),
            forward_headers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub path: PathBuf,
    pub server: ServerCfg,
    pub log: LogCfg,
    pub router: RouterCfg,
    pub compat: CompatCfg,
    pub accounts: Vec<AccountCfg>,
    pub warnings: Vec<String>,
}

impl Config {
    pub fn plans(&self) -> Vec<&AccountCfg> {
        let mut v: Vec<&AccountCfg> = self
            .accounts
            .iter()
            .filter(|a| a.kind == AccountKind::Plans)
            .collect();
        v.sort_by_key(|a| a.order);
        v
    }

    pub fn cash(&self) -> Vec<&AccountCfg> {
        let mut v: Vec<&AccountCfg> = self
            .accounts
            .iter()
            .filter(|a| a.kind == AccountKind::Cash)
            .collect();
        v.sort_by_key(|a| a.order);
        v
    }

    pub fn account_names(&self) -> Vec<String> {
        self.accounts.iter().map(|a| a.name.clone()).collect()
    }

    pub fn find(&self, name: &str) -> Option<&AccountCfg> {
        self.accounts
            .iter()
            .find(|a| a.name == name || a.name.eq_ignore_ascii_case(name))
    }

    pub fn is_peak(&self, now: i64) -> bool {
        timeutil::is_peak_at(now, &self.router.peak_windows)
    }

    pub fn describe_windows(&self) -> String {
        timeutil::describe_spec(&self.router.peak_spec)
    }

    pub fn summary(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "server {}:{} max_conns={} client_keys={}",
            self.server.host,
            self.server.port,
            self.server.max_connections,
            if self.server.client_keys.is_empty() { 0 } else { self.server.client_keys.len() }
        ));
        s.push_str(&format!(
            " | peak={} idle_prefer={:?} surplus<{:.0}%",
            self.describe_windows(),
            self.router.idle_prefer,
            self.router.surplus_max_pct
        ));
        for a in &self.accounts {
            s.push_str(&format!(
                " | [{}] {} url={} mode={} order={} quota={}/{} rules={} key={}",
                a.kind.as_str(),
                a.name,
                a.base(),
                a.modes.iter().map(|m| m.as_str()).collect::<Vec<_>>().join("+"),
                a.order,
                a.quota.unit.as_str(),
                a.quota.probe.as_str(),
                a.rules.iter().map(|r| r.as_str()).collect::<Vec<_>>().join(","),
                crate::util::redact(&a.key)
            ));
        }
        s
    }
}

// ------------------------------------------------------------------ yaml helpers

fn ymap(v: &Y) -> Option<&serde_yaml::Mapping> {
    v.as_mapping()
}

fn yget<'a>(m: &'a serde_yaml::Mapping, key: &str) -> Option<&'a Y> {
    m.get(Y::String(key.to_string()))
}

fn yget_any<'a>(m: &'a serde_yaml::Mapping, keys: &[&str]) -> Option<&'a Y> {
    keys.iter().find_map(|k| yget(m, k))
}

fn ystr(v: &Y) -> Option<String> {
    match v {
        Y::String(s) => Some(s.clone()),
        Y::Number(n) => Some(n.to_string()),
        Y::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn ybool(v: &Y, default: bool) -> bool {
    match v {
        Y::Bool(b) => *b,
        Y::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => true,
            "false" | "no" | "off" | "0" => false,
            _ => default,
        },
        Y::Number(n) => n.as_i64().map(|x| x != 0).unwrap_or(default),
        _ => default,
    }
}

fn yint(v: &Y, default: i64) -> i64 {
    match v {
        Y::Number(n) => n.as_i64().unwrap_or(default),
        Y::String(s) => s.trim().parse().unwrap_or(default),
        _ => default,
    }
}

fn yfloat(v: &Y, default: f64) -> f64 {
    match v {
        Y::Number(n) => n.as_f64().unwrap_or(default),
        Y::String(s) => s.trim().parse().unwrap_or(default),
        _ => default,
    }
}

/// Split a scalar or sequence into trimmed tokens (comma / space / semicolon / pipe).
fn ytokens(v: &Y) -> Vec<String> {
    match v {
        Y::Sequence(_) => ystr_list(v)
            .into_iter()
            .flat_map(|s| {
                s.split([',', ' ', ';', '|'])
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>()
            })
            .collect(),
        _ => ystr(v)
            .unwrap_or_default()
            .split([',', ' ', ';', '|'])
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
    }
}

fn ystr_list(v: &Y) -> Vec<String> {
    match v {
        Y::String(s) => s
            .split([',', ' ', ';'])
            .map(|x| x.trim())
            .filter(|x| !x.is_empty())
            .map(|x| x.to_string())
            .collect(),
        Y::Sequence(seq) => seq.iter().filter_map(ystr).collect(),
        _ => Vec::new(),
    }
}

/// key: literal | "env:NAME" | "file:path"
fn resolve_secret(v: &Y) -> Result<String, String> {
    let s = ystr(v).ok_or_else(|| "key must be a string".to_string())?;
    let t = s.trim();
    if let Some(name) = t.strip_prefix("env:") {
        return std::env::var(name.trim())
            .map_err(|_| format!("environment variable {} is not set", name.trim()));
    }
    if let Some(path) = t.strip_prefix("file:") {
        return std::fs::read_to_string(path.trim())
            .map(|x| x.trim().to_string())
            .map_err(|e| format!("cannot read key file {}: {}", path.trim(), e));
    }
    Ok(t.to_string())
}

/// Locations searched for config.yaml when --config is not given, in order:
///   1. next to the binary (<exe dir>/config.yaml, then .yml)
///   2. the current working directory (config.yaml, then .yml)
pub fn search_paths() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("config.yaml"));
            out.push(dir.join("config.yml"));
        }
    }
    out.push(PathBuf::from("config.yaml"));
    out.push(PathBuf::from("config.yml"));
    out
}

pub fn default_config_path(explicit: Option<&str>) -> PathBuf {
    if let Some(p) = explicit {
        return PathBuf::from(p);
    }
    let candidates = search_paths();
    for cand in &candidates {
        if cand.exists() {
            return cand.clone();
        }
    }
    // Nothing found: report the most likely place (next to the binary).
    candidates
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("config.yaml"))
}

pub fn load(path: &Path) -> Result<Config, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read config {}: {}", path.display(), e))?;
    parse(&raw, path)
}

pub fn parse(raw: &str, path: &Path) -> Result<Config, String> {
    let doc: Y = serde_yaml::from_str(raw).map_err(|e| format!("invalid YAML: {}", e))?;
    let root = doc
        .as_mapping()
        .ok_or_else(|| "config root must be a mapping".to_string())?;

    let mut warnings: Vec<String> = Vec::new();
    let mut server = ServerCfg::default();
    let mut log = LogCfg::default();
    let mut router = RouterCfg::default();
    let mut compat = CompatCfg::default();

    if let Some(m) = yget(root, "server").and_then(ymap) {
        if let Some(v) = yget_any(m, &["host", "bind"]) {
            server.host = ystr(v).unwrap_or(server.host);
        }
        if let Some(v) = yget_any(m, &["port", "listen_port"]) {
            let p = yint(v, server.port as i64);
            if p > 0 && p < 65536 {
                server.port = p as u16;
            } else {
                warnings.push(format!("server.port {} out of range, using {}", p, server.port));
            }
        }
        if let Some(v) = yget_any(m, &["max_connections", "threads", "concurrency"]) {
            server.max_connections = yint(v, server.max_connections as i64).max(1) as usize;
        }
        if let Some(v) = yget_any(m, &["client_keys", "api_keys", "auth_keys"]) {
            server.client_keys = ystr_list(v);
        }
        if let Some(v) = yget_any(m, &["max_body_mb", "max_body_size_mb"]) {
            server.max_body_bytes = (yfloat(v, 64.0).max(0.1) * 1024.0 * 1024.0) as usize;
        } else if let Some(v) = yget_any(m, &["max_body_bytes", "max_body_size_bytes"]) {
            // The bytes spelling is what the console, the export writer and the docs use. It used
            // to be ignored, so every console save reverted a hand-set max_body_mb to the default.
            server.max_body_bytes = yint(v, 64 * 1024 * 1024).max(1024) as usize;
        }
        if let Some(v) = yget_any(m, &["idle_timeout_secs", "keep_alive_secs"]) {
            server.idle_timeout_secs = yint(v, server.idle_timeout_secs as i64).max(1) as u64;
        }
        if let Some(v) = yget_any(m, &["read_timeout_secs", "request_timeout_secs"]) {
            server.read_timeout_secs = yint(v, server.read_timeout_secs as i64).max(1) as u64;
        }
        if let Some(v) = yget(m, "stream") {
            server.stream = ybool(v, server.stream);
        }
    }

    if let Some(m) = yget(root, "log").and_then(ymap) {
        if let Some(v) = yget_any(m, &["level", "log_level"]) {
            log.level = ystr(v).unwrap_or(log.level);
        }
        if let Some(v) = yget_any(m, &["file", "log_file"]) {
            let f = ystr(v).unwrap_or_default();
            if !f.trim().is_empty() {
                log.file = Some(f.trim().to_string());
            }
        }
        if let Some(v) = yget(m, "quiet") {
            log.quiet = ybool(v, false);
        }
    }

    if let Some(m) = yget(root, "router").and_then(ymap) {
        if let Some(v) = yget_any(m, &["mode", "policy"]) {
            let t = ystr(v).unwrap_or_default().to_ascii_lowercase().replace('_', "-");
            router.mode = match t.as_str() {
                "auto" | "" => RouterMode::Auto,
                "plans" | "plan" | "prepaid" | "opencodego" | "go" | "primary" => RouterMode::Plans,
                "fallback" | "fallback-only" | "official" | "deepseek" => RouterMode::Fallback,
                other => {
                    warnings.push(format!("router.mode {:?} unknown, using auto", other));
                    RouterMode::Auto
                }
            };
        }
        if let Some(v) = yget_any(m, &["peak_windows", "peak_spec", "peak"]) {
            let spec = ystr(v).unwrap_or_default();
            match timeutil::parse_peak_spec(&spec) {
                Ok(w) => {
                    router.peak_spec = spec.clone();
                    router.peak_windows = w;
                }
                Err(e) => warnings.push(format!("router.peak_windows ignored: {}", e)),
            }
        }
        if let Some(v) = yget_any(m, &["idle_prefer", "offpeak_prefer"]) {
            let t = ystr(v).unwrap_or_default().to_ascii_lowercase().replace('_', "-");
            router.idle_prefer = match t.as_str() {
                "fallback" | "official" | "deepseek" => IdlePrefer::Fallback,
                "plans" | "prepaid" | "opencodego" | "go" => IdlePrefer::Plans,
                "surplus" | "surplus-first" | "surplus_first" | "auto" | "" => IdlePrefer::SurplusFirst,
                other => {
                    warnings.push(format!("router.idle_prefer {:?} unknown, using surplus-first", other));
                    IdlePrefer::SurplusFirst
                }
            };
        }
        if let Some(v) = yget_any(m, &["surplus_max_pct", "idle_use_below_pct"]) {
            router.surplus_max_pct = yfloat(v, router.surplus_max_pct).clamp(1.0, 100.0);
        }
        if let Some(v) = yget_any(m, &["surplus_projection", "idle_use_projection"]) {
            router.surplus_projection = ybool(v, router.surplus_projection);
        }
        if let Some(v) = yget_any(m, &["exhaust_at_pct", "block_at_pct"]) {
            router.exhaust_at_pct = yfloat(v, router.exhaust_at_pct).clamp(1.0, 100.0);
        }
        if let Some(v) = yget_any(m, &["quota_refresh_secs", "quota_refresh", "quota_ttl_secs"]) {
            router.quota_refresh_secs = yint(v, router.quota_refresh_secs as i64).max(5) as u64;
        }
        if let Some(v) = yget_any(m, &["skip_after_failures", "skip_after"]) {
            router.skip_after_failures = yint(v, router.skip_after_failures as i64).clamp(1, 100) as u32;
        }
        if let Some(v) = yget_any(m, &["skip_secs", "skip_for_secs"]) {
            router.skip_secs = yint(v, router.skip_secs as i64).max(5) as u64;
        }
        if let Some(v) = yget_any(m, &["attempt_budget_secs", "budget_secs"]) {
            router.attempt_budget_secs = yint(v, router.attempt_budget_secs as i64).max(5) as u64;
        }
        if let Some(v) = yget_any(m, &["cooldown_secs"]) {
            router.cooldown_secs = yint(v, router.cooldown_secs as i64).max(0) as u64;
        }
        if let Some(v) = yget_any(m, &["auth_cooldown_secs"]) {
            router.auth_cooldown_secs = yint(v, router.auth_cooldown_secs as i64).max(0) as u64;
        }
        if let Some(v) = yget_any(m, &["server_error_cooldown_secs"]) {
            router.server_error_cooldown_secs =
                yint(v, router.server_error_cooldown_secs as i64).max(0) as u64;
        }
        if let Some(v) = yget_any(m, &["retry_on_model_error"]) {
            router.retry_on_model_error = ybool(v, router.retry_on_model_error);
        }
        if let Some(v) = yget_any(m, &["session_affinity", "sticky_session"]) {
            router.session_affinity = ybool(v, router.session_affinity);
        }
        if let Some(v) = yget_any(m, &["session_affinity_ttl_secs"]) {
            router.session_affinity_ttl_secs =
                yint(v, router.session_affinity_ttl_secs as i64).max(0) as u64;
        }
        if let Some(v) = yget_any(m, &["session_fallback"]) {
            let raw = ystr(v).unwrap_or_default();
            match SessionFallback::parse(&raw) {
                Some(sf) => router.session_fallback = sf,
                None => warnings.push(format!(
                    "router.session_fallback {:?} unknown (process|per-request), keeping {}",
                    raw,
                    router.session_fallback.as_str()
                )),
            }
        }
        if let Some(v) = yget_any(m, &["inject_stream_usage", "stream_usage"]) {
            router.inject_stream_usage = ybool(v, router.inject_stream_usage);
        }
        if let Some(v) = yget_any(m, &["session_headers", "session_header"]) {
            let list = ystr_list(v);
            if !list.is_empty() {
                router.session_headers = list;
            }
        }
        if let Some(v) = yget_any(m, &["user_agent", "ua"]) {
            let ua = ystr(v).unwrap_or_default();
            if !ua.trim().is_empty() {
                router.user_agent = ua.trim().to_string();
            }
        }
        if let Some(v) = yget_any(m, &["request_timeout_secs"]) {
            router.request_timeout_secs = yint(v, router.request_timeout_secs as i64).max(1) as u64;
        }
        if let Some(v) = yget_any(m, &["stream_idle_timeout_secs", "read_timeout_secs"]) {
            router.stream_idle_timeout_secs =
                yint(v, router.stream_idle_timeout_secs as i64).max(1) as u64;
        }
    }

    if let Some(m) = yget(root, "compat").and_then(ymap) {
        if let Some(v) = yget_any(m, &["developer_role_to_system", "developer_role"]) {
            compat.developer_role_to_system = ybool(v, compat.developer_role_to_system);
        }
        if let Some(v) = yget_any(
            m,
            &["max_completion_tokens_to_max_tokens", "max_completion_tokens"],
        ) {
            compat.max_completion_tokens_to_max_tokens =
                ybool(v, compat.max_completion_tokens_to_max_tokens);
        }
        if let Some(v) = yget_any(m, &["drop_params", "drop"]) {
            compat.drop_params = ystr_list(v);
        }
        if let Some(v) = yget_any(m, &["forward_headers"]) {
            compat.forward_headers = ystr_list(v);
        }
    }

    // Unknown top-level sections are almost always typos (opencodego: instead of plans: ...).
    const KNOWN_SECTIONS: [&str; 5] = ["server", "log", "router", "compat", "plans"];
    for (k, _) in root.iter() {
        if let Some(key) = ystr(k) {
            if key == "fallback" || KNOWN_SECTIONS.contains(&key.as_str()) {
                continue;
            }
            warnings.push(format!(
                "unknown top-level section {:?} (known: server, log, router, compat, plans, fallback) - ignored",
                key
            ));
        }
    }

    // -------------------------------------------------------------- accounts
    let mut accounts: Vec<AccountCfg> = Vec::new();
    // Endpoint names must be unique (runtime state is keyed by name); duplicates fall back to
    // an auto-generated suffix instead of failing the whole config.
    let mut used_names: Vec<String> = Vec::new();
    for (key, kind) in [("plans", AccountKind::Plans), ("fallback", AccountKind::Cash)] {
        let list = match yget(root, key) {
            Some(v) => v,
            None => continue,
        };
        let seq = match list {
            Y::Sequence(seq) => seq,
            Y::Mapping(_) => {
                // Single-account shorthand: plans: {url: ..., key: ...}
                return Err(format!(
                    "{} must be a list of accounts, e.g. {}: [{{url: ..., key: ...}}]",
                    key, key
                ));
            }
            _ => {
                warnings.push(format!("{} must be a list, ignoring", key));
                continue;
            }
        };
        for (idx, item) in seq.iter().enumerate() {
            let m = item
                .as_mapping()
                .ok_or_else(|| format!("{}[{}] must be a mapping", key, idx))?;
            let url = yget_any(m, &["url", "base_url", "baseURL", "endpoint"])
                .and_then(ystr)
                .ok_or_else(|| format!("{}[{}].url is required", key, idx))?
                .trim()
                .trim_end_matches('/')
                .to_string();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(format!("{}[{}].url must start with http:// or https://", key, idx));
            }
            let keyval = yget_any(m, &["key", "api_key", "apiKey", "token"])
                .ok_or_else(|| format!("{}[{}].key is required", key, idx))
                .and_then(resolve_secret)?;
            if keyval.trim().is_empty() {
                return Err(format!("{}[{}].key is empty", key, idx));
            }
            let explicit_name = yget_any(m, &["name", "id", "label", "display_name"])
                .and_then(ystr)
                .map(|s| crate::models::normalize_name(&s))
                .filter(|s| !s.is_empty());

            let mut modes: Vec<Mode> = Vec::new();
            if let Some(v) = yget_any(m, &["mode", "modes", "api", "protocol"]) {
                for tok in ytokens(v) {
                    match Mode::parse(&tok) {
                        Some(ms) => {
                            for mm in ms {
                                if !modes.contains(&mm) {
                                    modes.push(mm);
                                }
                            }
                        }
                        None => warnings.push(format!(
                            "{}[{}].mode {:?} unknown (expected openai-completion|openai-responses|anthropic-messages|both|any)",
                            key, idx, tok
                        )),
                    }
                }
            }
            if modes.is_empty() {
                // Default: OpenCode Go deepseek models speak chat completions; the official
                // DeepSeek API speaks both. Unless told otherwise assume both OpenAI shapes.
                modes = vec![Mode::Chat, Mode::Responses];
                warnings.push(format!(
                    "{}[{}] has no mode, assuming openai-completion+openai-responses",
                    key, idx
                ));
            }

            let mut rules: Vec<Rule> = Vec::new();
            let mut no_error_fallback = false;
            if let Some(v) = yget_any(m, &["rule", "rules", "when"]) {
                for tok in ytokens(v) {
                    let t = tok.trim().to_ascii_lowercase().replace('-', "_");
                    match t.as_str() {
                        "on_error" | "error" | "failover" => {}
                        "no_error_fallback" | "never_on_error" => no_error_fallback = true,
                        _ => match Rule::parse_token(&t) {
                            Some(r) => {
                                if !rules.contains(&r) {
                                    rules.push(r);
                                }
                            }
                            None => warnings.push(format!(
                                "{}[{}].rule {:?} unknown (always|peak|offpeak|quota_low|quota_exhausted|primary_unavailable|never|on_error)",
                                key, idx, tok
                            )),
                        },
                    }
                }
            }
            if rules.is_empty() {
                rules.push(Rule::Always);
            }

            let mut drop_params = compat.drop_params.clone();
            if let Some(v) = yget_any(m, &["drop_params", "drop"]) {
                drop_params.extend(ystr_list(v));
            }

            let mut extra_headers: Vec<(String, String)> = Vec::new();
            if let Some(v) = yget_any(m, &["headers", "extra_headers"]) {
                if let Some(hm) = v.as_mapping() {
                    for (k, val) in hm.iter() {
                        if let (Some(k), Some(val)) = (ystr(k), ystr(val)) {
                            extra_headers.push((k, val));
                        }
                    }
                }
            }

            let detected = ProviderKind::detect(&url);
            let provider = match yget_any(m, &["provider", "kind", "product"]).and_then(ystr) {
                Some(s) => {
                    let t = s.trim().to_ascii_lowercase().replace('_', "-");
                    match t.as_str() {
                        "opencodego" | "opencode-go" | "go" | "zen" => ProviderKind::OpencodeGo,
                        "deepseek" | "ds" | "official" | "deepseek-official" => ProviderKind::DeepSeek,
                        "generic" | "openai" | "openai-compatible" | "other" => ProviderKind::Generic,
                        other => {
                            warnings.push(format!(
                                "{}[{}].provider {:?} unknown, using the URL-detected one",
                                key, idx, other
                            ));
                            detected
                        }
                    }
                }
                None => detected,
            };
            // ---- quota: unit / per-window limits / how to probe it ----------------
            let mut quota = QuotaCfg::for_provider(provider);
            if kind == AccountKind::Cash {
                quota.probe = QuotaProbe::Balance;
            }
            if let Some(v) = yget_any(m, &["quota", "limits"]) {
                if let Some(qm) = v.as_mapping() {
                    if let Some(x) = yget_any(qm, &["unit", "currency", "measure"]) {
                        let raw = ystr(x).unwrap_or_default();
                        match QuotaUnit::parse(&raw) {
                            Some(u) => quota.unit = u,
                            None => warnings.push(format!(
                                "{}[{}].quota.unit {:?} unknown (usd|tokens|none), keeping {}",
                                key, idx, raw, quota.unit.as_str()
                            )),
                        }
                    }
                    if let Some(x) = yget_any(qm, &["probe", "usage_api", "how"]) {
                        let raw = ystr(x).unwrap_or_default();
                        match QuotaProbe::parse(&raw) {
                            Some(p) => quota.probe = p,
                            None => warnings.push(format!(
                                "{}[{}].quota.probe {:?} unknown (usage|balance|none), keeping {}",
                                key, idx, raw, quota.probe.as_str()
                            )),
                        }
                    }
                    if let Some(x) = yget_any(
                        qm,
                        &["rolling", "rolling_usd", "rolling_tokens", "5h", "5h_usd", "hourly"],
                    ) {
                        quota.rolling = yfloat(x, quota.rolling);
                    }
                    if let Some(x) = yget_any(qm, &["weekly", "weekly_usd", "weekly_tokens"]) {
                        quota.weekly = yfloat(x, quota.weekly);
                    }
                    if let Some(x) = yget_any(qm, &["monthly", "monthly_usd", "monthly_tokens"]) {
                        quota.monthly = yfloat(x, quota.monthly);
                    }
                    if let Some(x) = yget_any(qm, &["refresh_secs", "refresh", "fetch_seconds"]) {
                        quota.refresh_secs = yint(x, quota.refresh_secs as i64).max(5) as u64;
                    }
                    // tokens are a plain count: keep them integral-ish but allow floats
                    if quota.unit == QuotaUnit::Tokens {
                        quota.rolling = quota.rolling.max(0.0);
                        quota.weekly = quota.weekly.max(0.0);
                        quota.monthly = quota.monthly.max(0.0);
                    }
                } else if let Some(x) = v.as_f64() {
                    // shorthand: quota: 60  (monthly amount, split like the Go plan)
                    quota.unit = QuotaUnit::Usd;
                    quota.monthly = x;
                    quota.weekly = x * 0.5;
                    quota.rolling = x * 0.2;
                }
            }
            if quota.unit == QuotaUnit::None && quota.probe == QuotaProbe::Usage {
                warnings.push(format!(
                    "{}[{}] probes a usage API but has no quota unit/limits, so the reading will be ignored",
                    key, idx
                ));
            }
            // consumption order inside this kind (list order by default)
            let order = yget_any(m, &["order", "priority", "rank", "seq"])
                .map(|v| yint(v, idx as i64) as i32)
                .unwrap_or(idx as i32);
            // Explicit only: guessing from the provider silently breaks when the same endpoint
            // is reached through a bridge/proxy.
            let inject_session = yget_any(m, &["inject_session", "opencode_session", "send_session"])
                .map(|v| ybool(v, false))
                .unwrap_or(false);
            // "enabled: false" is the console's switch; while parsing, also honour the natural
            // "enabled: true" so a hand-written file reads the way it looks.
            let disabled = yget_any(m, &["enabled"])
                .map(|v| !ybool(v, true))
                .unwrap_or(false);
            // The one model this endpoint serves. Required: no guessing, no aliasing.
            let model = yget_any(m, &["model", "model_id", "modelId"])
                .and_then(ystr)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    format!(
                        "{}[{}].model is required (one endpoint = one model; e.g. model: deepseek-flash)",
                        key, idx
                    )
                })?;

            // --- endpoint name (Q9): explicit wins; auto = <provider>-<model>-<keyfp>-<n>
            //     first occurrence of a duplicate wins, later ones fall back to the next free n
            let name = match explicit_name {
                Some(want) => {
                    if used_names.iter().any(|n| *n == want) {
                        let mut n = 2;
                        loop {
                            let cand = format!("{}-{}", want, n);
                            if !used_names.iter().any(|u| *u == cand) {
                                warnings.push(format!(
                                    "{}[{}]: name {:?} already used, endpoint is named {:?} instead",
                                    key, idx, want, cand
                                ));
                                break cand;
                            }
                            n += 1;
                        }
                    } else {
                        want
                    }
                }
                None => {
                    let base = format!(
                        "{}-{}-{}",
                        provider.as_str(),
                        crate::models::normalize_name(&model),
                        crate::util::key_fingerprint(&keyval)
                    );
                    if !used_names.iter().any(|u| *u == base) {
                        base
                    } else {
                        let mut n = 2;
                        loop {
                            let cand = format!("{}-{}", base, n);
                            if !used_names.iter().any(|u| *u == cand) {
                                break cand;
                            }
                            n += 1;
                        }
                    }
                }
            };
            used_names.push(name.clone());

            accounts.push(AccountCfg {
                name,
                kind,
                order,
                provider,
                url,
                key: keyval.trim().to_string(),
                modes,
                rules,
                no_error_fallback,
                drop_params,
                extra_headers,
                quota,
                inject_session,
                disabled,
                model,
            });
        }
    }

    if accounts.is_empty() {
        return Err("no accounts configured: add at least one opencodego: or fallback: entry".to_string());
    }

    if !accounts.iter().any(|a| a.kind == AccountKind::Plans) {
        warnings.push("no 'plans' account configured; the router will always use the cash accounts".to_string());
    }
    if !accounts.iter().any(|a| a.kind == AccountKind::Cash) {
        warnings.push("no 'fallback' (cash) account configured; the router will always use the prepaid plans".to_string());
    }
    if accounts.is_empty() {
        return Err("no accounts configured: add at least one plans: or fallback: entry".to_string());
    }

    Ok(Config {
        path: path.to_path_buf(),
        server,
        log,
        router,
        compat,
        accounts,
        warnings,
    })
}
