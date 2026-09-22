//! Request handling: policy-driven account selection, upstream calls, retries, streaming.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use serde_json::{json, Value};

use crate::config::{AccountKind, Config, IdlePrefer, QuotaProbe, RouterMode};
use crate::httpd::{reason_phrase, Request, Responder, Statics};
use crate::models::{
    endpoint_of, normalize_path, parse_forced_endpoint, Endpoint, ForcedEndpoint, Mode,
    ROUTER_MODEL,
};
use crate::router::Router;
use crate::sse::{extract_usage, UsageScanner};
use crate::state::{Account, Registry, UsageDelta};
use crate::timeutil;
use crate::util;
use crate::{log_debug, log_info, log_warn};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const BANNER: &str = r#"ar-OCG-Router __VERSION__   (code name: ar-ocg-router)
multi-account router for OpenCode Go + DeepSeek official

Base URL for clients : http://HOST:PORT/v1  (a bare http://HOST:PORT also works)
OpenAI-compatible    : POST /v1/chat/completions, POST /v1/responses, GET /v1/models
Anthropic-compatible : POST /v1/messages
Admin                : GET /router/stats, GET /router/schedule, POST /router/reload, GET /health
Web console          : http://HOST:PORT/  (open it in a browser)
"#;

pub struct AppState {
    pub config: RwLock<Arc<Config>>,
    pub accounts: RwLock<Arc<Vec<Arc<Account>>>>,
    pub registry: Arc<Registry>,
    pub router: Router,
    pub agent: ureq::Agent,
    pub probe_agent: ureq::Agent,
    pub started_at: i64,
    pub statics: Statics,
    /// When the statistics counters started accumulating, as unix seconds. Zero = unknown.
    ///
    /// Held next to the counters rather than derived, because every other timestamp in reach means
    /// something else: `started_at` is this process, and the state file's `updated_at` is the last
    /// flush. Both would misreport a lifetime total as a recent one.
    pub stats_since: std::sync::atomic::AtomicI64,
    pub process_session: String,
}

impl AppState {
    pub fn new(cfg: Config) -> Arc<AppState> {
        let registry = Arc::new(Registry::new());
        let accounts = build_accounts(&cfg, &registry);
        let stream_timeout = cfg.router.stream_idle_timeout_secs.max(60);
        Arc::new(AppState {
            config: RwLock::new(Arc::new(cfg)),
            accounts: RwLock::new(Arc::new(accounts)),
            registry,
            router: Router::new(),
            agent: crate::httpclient::agent(20, stream_timeout, 120),
            probe_agent: crate::httpclient::agent(10, 20, 20),
            started_at: util::now_secs(),
            statics: Statics::default(),
            // Set properly at startup by persist::restore_stats_since; a fresh process that never
            // restores anything begins counting now.
            stats_since: std::sync::atomic::AtomicI64::new(util::now_secs()),
            process_session: util::gen_session_id(),
        })
    }

    pub fn cfg(&self) -> Arc<Config> {
        self.config
            .read()
            .map(|c| c.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    pub fn account_list(&self) -> Arc<Vec<Arc<Account>>> {
        self.accounts
            .read()
            .map(|a| a.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    /// Re-read config.yaml. Keeps per-account runtime state for accounts that survive.
    pub fn reload(&self) -> Result<String, String> {
        let path = self.cfg().path.clone();
        let cfg = crate::config::load(&path)?;
        let accounts = build_accounts(&cfg, &self.registry);
        self.registry
            .retain(&cfg.accounts.iter().map(|a| a.name.clone()).collect::<Vec<_>>());
        if let Ok(mut w) = self.config.write() {
            *w = Arc::new(cfg.clone());
        }
        if let Ok(mut w) = self.accounts.write() {
            *w = Arc::new(accounts);
        }
        Ok(cfg.summary())
    }
}

pub fn build_accounts(cfg: &Config, registry: &Arc<Registry>) -> Vec<Arc<Account>> {
    cfg.accounts
        .iter()
        .map(|a| {
            Arc::new(Account {
                cfg: a.clone(),
                rt: registry.get_or_create(&a.name),
            })
        })
        .collect()
}

fn error_value(status: u16, kind: &str, message: &str) -> Value {
    json!({
        "error": {
            "message": message,
            "type": kind,
            "code": status,
            "router": format!("ar-ocg-router {}", VERSION),
        }
    })
}

fn json_response(req: &Request, out: &mut Responder, status: u16, body: &Value) {
    let _ = out.send_json(status, body, req.keep_alive, &req.version);
}

// --------------------------------------------------------------------- routing

pub fn handle(state: &Arc<AppState>, req: &Request, out: &mut Responder) {
    state.statics.requests.fetch_add(1, Ordering::Relaxed);
    let cfg = state.cfg();
    let path = normalize_path(&req.path);

    if !cfg.server.client_keys.is_empty() && path != "/health" {
        let ok = req
            .bearer_token()
            .map(|t| cfg.server.client_keys.iter().any(|k| k == &t))
            .unwrap_or(false);
        if !ok {
            json_response(
                req,
                out,
                401,
                &error_value(401, "unauthorized", "missing or invalid router API key"),
            );
            return;
        }
    }

    match (req.method.as_str(), path.as_str()) {
        ("GET", "/") => {
            // When the console is compiled in it owns "/"; the plain-text banner stays
            // available at /router/banner (and scripted clients can always use /health).
            if crate::webui::serve(req, out) {
                return;
            }
            let text = BANNER.replace("__VERSION__", VERSION);
            let _ = out.send_text(
                200,
                &text,
                "text/plain; charset=utf-8",
                req.keep_alive,
                &req.version,
            );
        }
        ("GET", "/router/banner") => {
            let text = BANNER.replace("__VERSION__", VERSION);
            let _ = out.send_text(
                200,
                &text,
                "text/plain; charset=utf-8",
                req.keep_alive,
                &req.version,
            );
        }
        ("GET", "/health") => {
            json_response(req, out, 200, &json!({"status": "ok", "version": VERSION}));
        }
        ("GET", "/router/stats") | ("GET", "/stats") => {
            // Conditional GET: the console polls this every couple of seconds and the answer is
            // usually unchanged, so tell it so instead of shipping 8 KB again. The validator is a
            // hash of this very body minus the fields that move on their own (the clock, the
            // uptime), so a 304 can never hide a change the console would have drawn.
            let body = stats_json(state);
            // "requests" is excluded because it counts the polls themselves; the fields that
            // describe real traffic (proxied, streams, retries, bytes) are what make it move.
            let revision = crate::etag::stats_revision(&body, &["now", "uptime_secs", "requests"]);
            if crate::etag::matches(req.header("if-none-match"), &revision) {
                let _ = out.send_head(304, &[("ETag".to_string(), revision)], Some(0), req.keep_alive, &req.version);
                let _ = out.finish();
            } else {
                let _ = out.send_json_with(
                    200,
                    &body,
                    &[("ETag".to_string(), revision), ("Cache-Control".to_string(), "no-cache".to_string())],
                    req.keep_alive,
                    &req.version,
                );
            }
        }
        ("GET", "/router/schedule") => {
            let body = schedule_json(&cfg, util::now_secs());
            json_response(req, out, 200, &body);
        }
        ("POST", "/router/reload") => match state.reload() {
            Ok(summary) => {
                log_info!("config reloaded on request: {}", summary);
                json_response(req, out, 200, &json!({"status": "reloaded", "summary": summary}));
            }
            Err(e) => {
                log_warn!("config reload failed: {}", e);
                json_response(req, out, 400, &error_value(400, "reload_failed", &e));
            }
        },
        ("GET", "/models") => {
            let body = models_json();
            json_response(req, out, 200, &body);
        }
        // ---- admin API for the embedded web console -------------------------
        ("GET", _) | ("POST", _) | ("PUT", _) | ("PATCH", _) | ("DELETE", _)
            if path == "/api" || path.starts_with("/api/") =>
        {
            if crate::api::handle(state, req, out) {
                return;
            }
            json_response(req, out, 404, &error_value(404, "not_found", "unknown admin route"));
        }
        // ---- the console itself ---------------------------------------------
        // The console owns every non-/v1 path: a miss here (favicon, typo, stale asset) must
        // answer 404 rather than fall through and be forwarded upstream as a fake request.
        ("GET", _) if !path.starts_with("/v1") => {
            if !crate::webui::serve(req, out) {
                json_response(
                    req,
                    out,
                    404,
                    &error_value(404, "not_found", "no such resource"),
                );
            }
        }
        ("POST", _) | ("PUT", _) | ("PATCH", _) | ("GET", _) => {
            proxy(state, req, out);
        }
        _ => {
            json_response(
                req,
                out,
                405,
                &error_value(405, "method_not_allowed", "unsupported method"),
            );
        }
    }
}

fn schedule_json(cfg: &Config, now: i64) -> Value {
    let state = timeutil::eval_schedule(now, &cfg.router.peak_windows);
    let mut transitions = Vec::new();
    let mut t = now;
    let mut cur = state.is_peak;
    for _ in 0..16 {
        let s = timeutil::eval_schedule(t, &cfg.router.peak_windows);
        if s.is_peak != cur || transitions.is_empty() {
            cur = s.is_peak;
            transitions.push(json!({
                "at": timeutil::iso8601(s.next_change),
                "becomes": if s.is_peak { "offpeak" } else { "peak" },
            }));
        }
        if s.next_change <= t {
            break;
        }
        t = s.next_change;
    }
    json!({
        "now": timeutil::iso8601(now),
        "peak": state.is_peak,
        "windows": cfg.describe_windows(),
        "next_change": timeutil::iso8601(state.next_change),
        "seconds_to_change": (state.next_change - now).max(0),
        "transitions": transitions,
    })
}

fn stats_json(state: &Arc<AppState>) -> Value {
    let cfg = state.cfg();
    let accounts = state.account_list();
    let now = util::now_secs();
    let sched = timeutil::eval_schedule(now, &cfg.router.peak_windows);
    let mut acc_json = Vec::new();
    for acc in accounts.iter() {
        let report = acc.rt.quota_report(now, &cfg, &acc.cfg.quota, &acc.cfg.prices);
        let mut v = acc.rt.to_json(now, &acc.cfg, &report, cfg.is_peak(now));
        let health = state.router.health.get(acc.name());
        if let Some(obj) = v.as_object_mut() {
            obj.insert(
                "health".to_string(),
                json!({
                    "failure_streak": health.streak,
                    "total_errors": health.total_errors,
                    "skipped": health.skip_until > now,
                    "skip_secs_left": (health.skip_until - now).max(0),
                    "last_reason": health.last_reason,
                }),
            );
        }
        if let Some(obj) = v.as_object_mut() {
            obj.insert("model".to_string(), json!(acc.cfg.model));
            obj.insert(
                "identity".to_string(),
                json!(format!("{}/{}", acc.cfg.provider.as_str(), acc.cfg.model)),
            );
            // The roster in the console has to say which endpoints are actually in the routing
            // pool. Without this the dashboard could only guess from the available flag, which is
            // about cooldowns and quota, not about the enable switch.
            obj.insert("enabled".to_string(), json!(acc.cfg.is_enabled()));
            // The reported count is the aged one; the raw counter travels alongside it so a leak is
            // visible in the API instead of having to be inferred from a number that never drops.
            // They differ only when a mark outlived the age limit, which is a bug worth seeing.
            obj.insert("in_flight".to_string(), json!(acc.rt.in_flight()));
            obj.insert("in_flight_raw".to_string(), json!(acc.rt.in_flight_raw()));
        }
        acc_json.push(v);
    }
    let since = state.stats_since.load(Ordering::Relaxed);
    json!({
        "router": {
            "version": VERSION,
            "pid": std::process::id(),
            "uptime_secs": now - state.started_at,
            // The window the counters describe. Null, not a guess, when the state file predates
            // this field: the totals are real but their start is genuinely unknown.
            "stats_since": {
                "at": since,
                "iso": if since > 0 { Value::String(timeutil::iso8601(since)) } else { Value::Null },
                "secs": if since > 0 { json!((now - since).max(0)) } else { Value::Null },
            },
            "now": timeutil::iso8601(now),
            "fake_now": util::has_fake_now(),
            "peak": sched.is_peak,
            "next_change": timeutil::iso8601(sched.next_change),
            "seconds_to_change": (sched.next_change - now).max(0),
            "peak_windows": cfg.describe_windows(),
            "mode": match cfg.router.mode {
                RouterMode::Auto => "auto",
                RouterMode::Plans => "plans",
                RouterMode::Fallback => "fallback",
            },
            "idle_prefer": match cfg.router.idle_prefer {
                IdlePrefer::Fallback => "fallback",
                IdlePrefer::Plans => "plans",
                IdlePrefer::SurplusFirst => "surplus_first",
            },
            "surplus_max_pct": cfg.router.surplus_max_pct,
            "surplus_projection": cfg.router.surplus_projection,
            "config_path": cfg.path.display().to_string(),
        },
        "counters": {
            "requests": state.statics.requests.load(Ordering::Relaxed),
            "proxied": state.statics.proxied.load(Ordering::Relaxed),
            "errors": state.statics.errors.load(Ordering::Relaxed),
            "streams": state.statics.streams.load(Ordering::Relaxed),
            "retries": state.statics.retries.load(Ordering::Relaxed),
            "plans_used": state.statics.plans_used.load(Ordering::Relaxed),
            "cold_starts": state.statics.cold_starts.load(Ordering::Relaxed),
            "cash_used": state.statics.cash_used.load(Ordering::Relaxed),
            "bytes_out": state.statics.body_bytes_out.load(Ordering::Relaxed),
        },
        "accounts": acc_json,
        "warnings": cfg.warnings,
    })
}

/// GET /v1/models: exactly one id. It identifies "a model reached through ar-ocg-router"; which
/// upstream model actually serves a request is a configuration detail (see README §10.1).
fn models_json() -> Value {
    crate::models::models_payload()
}

// ----------------------------------------------------------------------- proxy

#[derive(Debug, Clone)]
struct Attempt {
    account: String,
    status: u16,
    error: String,
}

fn parse_body_json(req: &Request) -> Option<Value> {
    if req.body.is_empty() {
        return None;
    }
    let ct = req.header("content-type").unwrap_or("");
    let looks_json = ct.contains("json") || req.body.iter().take(64).any(|b| *b == b'{');
    if !looks_json {
        return None;
    }
    serde_json::from_slice::<Value>(&req.body).ok()
}

fn wants_stream(req: &Request, body: Option<&Value>) -> bool {
    if let Some(b) = body {
        if b.get("stream").and_then(|v| v.as_bool()).unwrap_or(false) {
            return true;
        }
    }
    req.header("accept")
        .map(|a| a.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false)
}

fn client_session(cfg: &Config, req: &Request, body: Option<&Value>) -> Option<String> {
    for h in &cfg.router.session_headers {
        if let Some(v) = req.header(h) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    if let Some(b) = body {
        for key in ["prompt_cache_key", "session_id", "conversation_id"] {
            if let Some(v) = b.get(key).and_then(|x| x.as_str()) {
                if !v.trim().is_empty() {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// What a clamp changed, for the log line.
pub struct Clamped {
    pub from: u64,
    pub to: u64,
}

fn apply_compat(
    cfg: &Config,
    acc: &Account,
    endpoint: Endpoint,
    body: &Value,
    upstream_model: &str,
    streaming: bool,
) -> (Value, Option<Clamped>) {
    let mut clamped: Option<Clamped> = None;
    let mut v = body.clone();
    if let Some(obj) = v.as_object_mut() {
        obj.insert("model".to_string(), json!(upstream_model));
        for p in cfg.compat.drop_params.iter().chain(acc.cfg.drop_params.iter()) {
            obj.remove(p);
        }
        if cfg.compat.max_completion_tokens_to_max_tokens && endpoint == Endpoint::Chat {
            if let Some(mc) = obj.remove("max_completion_tokens") {
                obj.entry("max_tokens".to_string()).or_insert(mc);
            }
        }
        if cfg.compat.developer_role_to_system {
            for key in ["messages", "input"] {
                if let Some(Value::Array(items)) = obj.get_mut(key) {
                    for m in items.iter_mut() {
                        if let Some(mo) = m.as_object_mut() {
                            if mo.get("role").and_then(|r| r.as_str()) == Some("developer") {
                                mo.insert("role".to_string(), json!("system"));
                            }
                        }
                    }
                }
            }
        }
        // Clamp before anything else looks at the body: an upstream that rejects an oversized
        // value does it with a generic error that names no parameter, so this is the difference
        // between "the endpoint is unusable" and "the endpoint served the request".
        if acc.cfg.max_output_tokens_limit > 0 {
            let limit = acc.cfg.max_output_tokens_limit;
            if let Some(v) = obj.get("max_output_tokens").and_then(|x| x.as_u64()) {
                if v > limit {
                    obj.insert("max_output_tokens".to_string(), json!(limit));
                    clamped = Some(Clamped { from: v, to: limit });
                }
            }
        }
        if cfg.router.inject_stream_usage && streaming && endpoint == Endpoint::Chat {
            let so = obj
                .entry("stream_options".to_string())
                .or_insert_with(|| json!({}));
            if let Some(so) = so.as_object_mut() {
                so.insert("include_usage".to_string(), json!(true));
            }
        }
    }
    (v, clamped)
}

fn looks_like_model_error(body: &Value) -> bool {
    let msg = body
        .get("error")
        .and_then(|e| {
            e.get("message")
                .and_then(|m| m.as_str())
                .or_else(|| e.as_str())
        })
        .or_else(|| body.get("message").and_then(|m| m.as_str()))
        .unwrap_or("")
        .to_ascii_lowercase();
    let model_words = msg.contains("model") || msg.contains("engine") || msg.contains("deployment");
    let bad_words = msg.contains("not found")
        || msg.contains("not exist")
        || msg.contains("not support")
        || msg.contains("unsupported")
        || msg.contains("unknown")
        || msg.contains("invalid")
        || msg.contains("no such")
        || msg.contains("not available")
        || msg.contains("does not exist");
    model_words && bad_words
}

fn looks_like_quota_error(status: u16, body: &Value) -> bool {
    if status == 402 {
        return true;
    }
    let msg = body
        .get("error")
        .and_then(|e| {
            e.get("message")
                .and_then(|m| m.as_str())
                .or_else(|| e.as_str())
        })
        .or_else(|| body.get("message").and_then(|m| m.as_str()))
        .unwrap_or("")
        .to_ascii_lowercase();
    [
        "quota",
        "usage limit",
        "limit reached",
        "limits reached",
        "insufficient",
        "out of credit",
        "exceeded your current",
        "budget",
        "billing",
    ]
    .iter()
    .any(|k| msg.contains(k))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Retry,
    Client,
}

fn classify(cfg: &Config, status: u16, body: &Value) -> Verdict {
    match status {
        401 | 403 | 429 => Verdict::Retry,
        408 | 500 | 502 | 503 | 504 | 529 => Verdict::Retry,
        400 | 404 | 405 | 409 | 422 => {
            if cfg.router.retry_on_model_error && looks_like_model_error(body) {
                Verdict::Retry
            } else {
                Verdict::Client
            }
        }
        _ => Verdict::Client,
    }
}

fn cooldown_for(cfg: &Config, status: u16) -> u64 {
    match status {
        401 | 403 => cfg.router.auth_cooldown_secs,
        429 => cfg.router.cooldown_secs,
        0 => cfg.router.cooldown_secs,
        s if s >= 500 => cfg.router.server_error_cooldown_secs,
        _ => cfg.router.cooldown_secs / 2,
    }
}

fn usage_delta(
    acc: &Account,
    upstream_model: &str,
    is_peak: bool,
    raw: crate::sse::RawUsage,
) -> UsageDelta {
    let _ = upstream_model;
    let mut d = UsageDelta {
        prompt_tokens: raw.prompt,
        completion_tokens: raw.completion,
        cached_tokens: raw.cached,
        cost_usd: 0.0,
        saved_usd: 0.0,
    };
    // Priority: what the upstream itself reports > what the endpoint's config says > nothing.
    // The upstream number is the only one that cannot be stale or mis-typed; the configured rates
    // are the operator's statement about a provider that does not return a cost at all (DeepSeek
    // official and Ark both report tokens only). With neither, no money is recorded - a guess
    // would be worse than a blank.
    let cost = if raw.upstream_cost > 0.0 {
        raw.upstream_cost
    } else if acc.cfg.prices.is_set() {
        acc.cfg
            .prices
            .cost_of(raw.prompt, raw.cached, raw.completion, is_peak)
    } else {
        0.0
    };
    d.cost_usd = cost;
    if acc.kind() == AccountKind::Plans {
        d.saved_usd = cost;
    }
    d
}

fn endpoint_mode_name(e: Endpoint) -> &'static str {
    match e {
        Endpoint::Chat => "chat",
        Endpoint::Responses => "responses",
        Endpoint::Anthropic => "messages",
        Endpoint::Models => "models",
        Endpoint::Other => "other",
    }
}

fn proxy(state: &Arc<AppState>, req: &Request, out: &mut Responder) {
    let cfg = state.cfg();
    let now = util::now_secs();
    let endpoint = endpoint_of(&req.path);
    let req_id = util::rand_hex(8);

    let body_json = parse_body_json(req);
    let client_model = body_json
        .as_ref()
        .and_then(|b| b.get("model").and_then(|m| m.as_str()))
        .unwrap_or("")
        .to_string();
    // The client's model string carries no routing meaning: it may optionally pin one configured
    // endpoint (see models::parse_forced_endpoint); anything else is accepted and only logged.
    let accounts = state.account_list();
    let known_names: Vec<String> = accounts.iter().map(|a| a.name().to_string()).collect();
    let forced: Option<ForcedEndpoint> = if client_model.is_empty() {
        None
    } else {
        parse_forced_endpoint(&client_model, &known_names)
    };
    if let Some(f) = &forced {
        log_info!("[{}] client pinned endpoint: {:?}", req_id, f);
    } else if !client_model.is_empty() && !client_model.eq_ignore_ascii_case(ROUTER_MODEL) {
        log_debug!(
            "[{}] client model {:?} ignored (routing is endpoint-driven)",
            req_id,
            client_model
        );
    }

    let streaming = wants_stream(req, body_json.as_ref()) && req.method == "POST";
    // Session value handed to upstreams that want one (OpenCode Go requires
    // an x-opencode-session value on every request, so fall back to a per-process id.
    let client_sid = client_session(&cfg, req, body_json.as_ref());
    let sid = match (&client_sid, cfg.router.session_fallback) {
        (Some(s), _) => s.clone(),
        (None, crate::config::SessionFallback::Process) => state.process_session.clone(),
        (None, crate::config::SessionFallback::PerRequest) => util::gen_session_id(),
    };
    let affinity_key = client_sid.clone();

    let plan = state.router.plan(
        &cfg,
        &accounts,
        endpoint,
        forced.as_ref(),
        affinity_key.as_deref(),
        now,
    );

    log_debug!(
        "[{}] {} {} model={} stream={} -> {} candidates={:?} skipped={:?}",
        req_id,
        req.method,
        req.path,
        if client_model.is_empty() { "-" } else { &client_model },
        streaming,
        plan.reason,
        plan.candidates.iter().map(|a| a.name()).collect::<Vec<_>>(),
        plan.skipped
    );

    if plan.candidates.is_empty() {
        state.statics.errors.fetch_add(1, Ordering::Relaxed);
        json_response(
            req,
            out,
            503,
            &error_value(
                503,
                "no_account_available",
                &format!(
                    "no account can serve {} ({}) skipped={:?}",
                    if client_model.is_empty() { req.path.as_str() } else { &client_model },
                    plan.reason,
                    plan.skipped
                ),
            ),
        );
        return;
    }

    let budget = std::time::Duration::from_secs(cfg.router.attempt_budget_secs.max(5));
    let started_chain = Instant::now();
    let mut attempts: Vec<Attempt> = Vec::new();

    for (idx, acc) in plan.candidates.iter().enumerate() {
        if idx > 0 && started_chain.elapsed() >= budget {
            log_warn!(
                "[{}] retry budget ({}s) exhausted after {} attempt(s)",
                req_id,
                cfg.router.attempt_budget_secs,
                idx
            );
            break;
        }
        // A skipped endpoint is not tried at all - not even as the first candidate - unless the
        // client pinned it explicitly. Otherwise every request would pay one doomed attempt.
        if forced.is_none() && state.router.is_skipped(acc.name(), now) {
            let health = state.router.health.get(acc.name());
            log_warn!(
                "[{}] skipping {} for {}s ({} consecutive failures: {})",
                req_id,
                acc.name(),
                health.skip_until - now,
                health.streak,
                health.last_reason.clone().unwrap_or_default()
            );
            attempts.push(Attempt {
                account: acc.name().to_string(),
                status: 0,
                error: format!("skipped ({} consecutive failures)", health.streak),
            });
            continue;
        }
        let mode = match endpoint.required_mode() {
            Some(m) => m,
            None => acc.cfg.modes.first().copied().unwrap_or(Mode::Chat),
        };
        if !acc.cfg.supports_mode(mode) {
            continue;
        }
        // The endpoint's own model wins, always: that is what makes it a node.
        let upstream_model = acc.cfg.model.clone();

        let url = acc.cfg.upstream_url(&req.path);
        // Keep the rewritten document around as well as its bytes: if the upstream rejects the
        // request with a bad-parameter error, this is the only record of what was actually sent.
        let mut clamped: Option<Clamped> = None;
        let sent_json: Option<Value> = body_json.as_ref().map(|b| {
            let (v, c) = apply_compat(&cfg, acc, endpoint, b, &upstream_model, streaming);
            clamped = c;
            v
        });
        if let Some(c) = clamped {
            log_info!(
                "[{}] {}: max_output_tokens {} -> {} (endpoint limit)",
                req_id,
                acc.name(),
                c.from,
                c.to
            );
        }
        let body_bytes = match (&sent_json, &body_json) {
            (Some(v), _) => serde_json::to_vec(v).unwrap_or_else(|_| req.body.clone()),
            (None, _) => req.body.clone(),
        };

        let mut call = state
            .agent
            .post(&url)
            .set("Content-Type", "application/json")
            .set(
                "Accept",
                if streaming {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .set("User-Agent", &cfg.router.user_agent)
            .set("Authorization", &format!("Bearer {}", acc.cfg.key));
        if mode == Mode::Anthropic {
            call = call
                .set("x-api-key", &acc.cfg.key)
                .set("anthropic-version", "2023-06-01");
        }
        if acc.cfg.inject_session {
            call = call.set("x-opencode-session", &sid);
        }
        // Protocol headers go through verbatim; the router never rewrites their values.
        let client_headers: Vec<&str> = crate::config::PROTOCOL_HEADERS
            .iter()
            .copied()
            .chain(cfg.compat.forward_headers.iter().map(|s| s.as_str()))
            .collect();
        for h in client_headers {
            if let Some(v) = req.header(h) {
                // never let a client override the endpoint credentials
                if h.eq_ignore_ascii_case("authorization") {
                    continue;
                }
                call = call.set(h, v);
            }
        }
        for (k, v) in &acc.cfg.extra_headers {
            call = call.set(k, v);
        }

        // From here to the end of the attempt the endpoint is in use - including the whole stream,
        // which is exactly the window the completion-time statistics cannot see. The guard is the
        // RAII mark; the explicit clear below covers the paths where an unwinding write error would
        // otherwise leave it set (see the note on InFlightGuard::disarm).
        let mut in_flight = acc.rt.in_flight_guard();
        let started = Instant::now();
        log_info!(
            "[{}] -> {} {} model={} stream={} ({})",
            req_id,
            acc.name(),
            if idx == 0 { "primary" } else { "retry" },
            upstream_model,
            streaming,
            endpoint_mode_name(endpoint)
        );

        let response = call.send_bytes(&body_bytes);
        let latency_ms = started.elapsed().as_millis() as u64;

        match response {
            Ok(resp) if (200..300).contains(&resp.status()) => {
                if let Err(e) = deliver_success(
                    state,
                    req,
                    out,
                    acc,
                    resp,
                    latency_ms,
                    streaming,
                    cfg.is_peak(now),
                    &upstream_model,
                    &sid,
                    endpoint,
                    &req_id,
                    affinity_key.as_deref(),
                ) {
                    log_debug!("[{}] client write ended: {}", req_id, e);
                }
                state.statics.proxied.fetch_add(1, Ordering::Relaxed);
                match acc.kind() {
                    AccountKind::Plans => state.statics.plans_used.fetch_add(1, Ordering::Relaxed),
                    AccountKind::Cash => state.statics.cash_used.fetch_add(1, Ordering::Relaxed),
                };
                in_flight.clear();
                return;
            }
            Ok(resp) => {
                // 3xx (after redirects) and anything else ureq surfaced as Ok.
                let status = resp.status();
                let text = read_capped(resp, 1 << 20);
                let parsed: Value = serde_json::from_slice(&text).unwrap_or(Value::Null);
                if handle_upstream_error(
                    state,
                    req,
                    out,
                    acc,
                    status,
                    &text,
                    &parsed,
                    latency_ms,
                    now,
                    &mut attempts,
                    &req_id,
                    sent_json.as_ref(),
                ) == ErrorAction::Return
                {
                    return;
                }
            }
            Err(ureq::Error::Status(status, resp)) => {
                let text = read_capped(resp, 1 << 20);
                let parsed: Value = serde_json::from_slice(&text).unwrap_or(Value::Null);
                if handle_upstream_error(
                    state,
                    req,
                    out,
                    acc,
                    status,
                    &text,
                    &parsed,
                    latency_ms,
                    now,
                    &mut attempts,
                    &req_id,
                    sent_json.as_ref(),
                ) == ErrorAction::Return
                {
                    return;
                }
            }
            Err(e) => {
                let msg = format!("{}", e);
                acc.rt.record_error(now, 0, &msg, latency_ms);
                acc.rt.set_cooldown(
                    now + cfg.router.cooldown_secs as i64,
                    &format!("transport error: {}", util::truncate(&msg, 120)),
                );
                crate::persist::mark_dirty();
                state.router.health.record_failure(
                    acc.name(),
                    now,
                    &format!("transport: {}", util::truncate(&msg, 120)),
                    cfg.router.skip_after_failures,
                    cfg.router.skip_secs,
                );
                hint_egress_blocked(&msg);
                log_warn!(
                    "[{}] {} transport error: {}",
                    req_id,
                    acc.name(),
                    util::truncate(&msg, 220)
                );
                attempts.push(Attempt {
                    account: acc.name().to_string(),
                    status: 0,
                    error: util::truncate(&msg, 200),
                });
                state.statics.retries.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    state.statics.errors.fetch_add(1, Ordering::Relaxed);
    let detail = attempts
        .iter()
        .map(|a| format!("{}: HTTP {} {}", a.account, a.status, a.error))
        .collect::<Vec<_>>()
        .join(" | ");
    json_response(
        req,
        out,
        502,
        &error_value(
            502,
            "all_accounts_failed",
            &format!("every candidate account failed: {}", detail),
        ),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorAction {
    Return,
    Retry,
}

fn handle_upstream_error(
    state: &Arc<AppState>,
    req: &Request,
    out: &mut Responder,
    acc: &Arc<Account>,
    status: u16,
    text: &[u8],
    parsed: &Value,
    latency_ms: u64,
    now: i64,
    attempts: &mut Vec<Attempt>,
    req_id: &str,
    sent_body: Option<&Value>,
) -> ErrorAction {
    let cfg = state.cfg();
    let brief = crate::sse::brief(parsed);
    acc.rt.record_error(now, status, &brief, latency_ms);
    maybe_cooldown(state, acc, status, now, parsed);
    // Track "this endpoint cannot serve it" failures: without this a misconfigured endpoint only
    // shows up as a slower request (the retry chain hides it).
    if classify(&cfg, status, parsed) == Verdict::Retry {
        crate::persist::mark_dirty();
        let skipped_now = state.router.health.record_failure(
            acc.name(),
            now,
            &format!("HTTP {}: {}", status, util::truncate(&brief, 120)),
            cfg.router.skip_after_failures,
            cfg.router.skip_secs,
        );
        if skipped_now {
            log_warn!(
                "endpoint {} disabled for {}s after {} consecutive failures",
                acc.name(),
                cfg.router.skip_secs,
                cfg.router.skip_after_failures
            );
        }
    }
    log_warn!(
        "[{}] {} returned HTTP {}: {}",
        req_id,
        acc.name(),
        status,
        util::truncate(&brief, 220)
    );
    // Some upstreams reject a request with "a parameter specified in the request is not valid" and
    // an empty "param": the only way to find out which one is to look at what we sent. Off by
    // default; the shape report keeps prompts out of the log (see crate::redact).
    if cfg.log.dump_error_request && (400..500).contains(&status) {
        if let Some(sent) = sent_body {
            let (keys, shape) = crate::redact::describe_for_log(sent);
            log_warn!("[{}] request fields: {}", req_id, keys);
            log_warn!("[{}] request shape: {}", req_id, util::truncate(&shape, 4000));
        }
    }
    attempts.push(Attempt {
        account: acc.name().to_string(),
        status,
        error: brief,
    });
    if classify(&cfg, status, parsed) == Verdict::Client {
        state.statics.errors.fetch_add(1, Ordering::Relaxed);
        return_client_error(req, out, status, parsed, text);
        return ErrorAction::Return;
    }
    state.statics.retries.fetch_add(1, Ordering::Relaxed);
    ErrorAction::Retry
}

/// Classify how much of the prompt the upstream served from its cache.
///   hit  : a meaningful share was cached
///   cold : the upstream reported usage but billed (almost) the whole prompt
///   unknown: the upstream did not report token details at all
pub fn cache_label(prompt: u64, cached: u64) -> &'static str {
    if prompt == 0 {
        return "unknown";
    }
    let ratio = cached as f64 / prompt as f64;
    if ratio >= 0.10 {
        "hit"
    } else {
        "cold"
    }
}

/// Windows reports a blocked outbound connection as WSAEACCES (os error 10013). That is a
/// firewall/EDR policy problem, not an upstream problem, so say so once per process.
fn hint_egress_blocked(msg: &str) {
    static HINTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let blocked = msg.contains("10013") || msg.contains("forbidden by its access permissions");
    if !blocked {
        return;
    }
    if HINTED.swap(true, Ordering::Relaxed) {
        return;
    }
    log_warn!(
        "outbound connections are blocked by policy (WSAEACCES/10013). Allow this program through the \
         firewall, e.g. as administrator: netsh advfirewall firewall add rule name=\"ar-OCG-Router out\" \
         dir=out action=allow program=\"<full path to ar-ocg-router.exe>\" enable=yes"
    );
}

fn maybe_cooldown(state: &Arc<AppState>, acc: &Arc<Account>, status: u16, now: i64, body: &Value) {
    let cfg = state.cfg();
    let secs = cooldown_for(&cfg, status);
    acc.rt
        .set_cooldown(now + secs as i64, &format!("HTTP {}", status));
    if status == 429 && acc.kind() == AccountKind::Plans && looks_like_quota_error(status, body) {
        crate::quota::mark_exhausted(
            &acc.rt,
            now + 300,
            &format!("429 quota: {}", crate::sse::brief(body)),
        );
    }
    if status == 401 || status == 403 {
        log_warn!(
            "account {} disabled for {}s after auth error (HTTP {})",
            acc.name(),
            secs,
            status
        );
    }
}

fn read_capped(resp: ureq::Response, cap: usize) -> Vec<u8> {
    let mut reader = resp.into_reader();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16384];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() >= cap {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    buf
}

fn return_client_error(req: &Request, out: &mut Responder, status: u16, parsed: &Value, raw: &[u8]) {
    if parsed.is_null() {
        let body = String::from_utf8_lossy(raw).to_string();
        let _ = out.send_text(
            status,
            &body,
            "application/json; charset=utf-8",
            req.keep_alive,
            &req.version,
        );
        return;
    }
    json_response(req, out, status, parsed);
}

#[allow(clippy::too_many_arguments)]
fn deliver_success(
    state: &Arc<AppState>,
    req: &Request,
    out: &mut Responder,
    acc: &Arc<Account>,
    resp: ureq::Response,
    latency_ms: u64,
    streaming: bool,
    is_peak: bool,
    upstream_model: &str,
    #[allow(unused_variables)] sid: &str,
    endpoint: Endpoint,
    req_id: &str,
    affinity_key: Option<&str>,
) -> std::io::Result<()> {
    let status = resp.status();
    let upstream_ct = resp
        .header("content-type")
        .unwrap_or(if streaming {
            "text/event-stream"
        } else {
            "application/json"
        })
        .to_string();

    let extra_headers = vec![
        ("Content-Type".to_string(), upstream_ct),
        ("x-router-account".to_string(), acc.name().to_string()),
        ("x-router-kind".to_string(), acc.cfg.kind.as_str().to_string()),
        ("x-router-endpoint".to_string(), acc.name().to_string()),
        ("x-router-model".to_string(), upstream_model.to_string()),
        (
            "x-router-peak".to_string(),
            if is_peak {
                "peak".to_string()
            } else {
                "offpeak".to_string()
            },
        ),
        ("x-router-request-id".to_string(), req_id.to_string()),
    ];

    let now = util::now_secs();
    // Cache state is derived from what the upstream reported, never assumed: "cold" means the
    // upstream billed (nearly) the whole prompt, so a session hop or an endpoint without prompt
    // caching is visible to whoever is debugging cost.
    let mut cache_state = "unknown";
    let mut cache_tokens = 0u64;

    if streaming {
        let mut headers = extra_headers.clone();
        headers.push(("x-router-cache".to_string(), cache_state.to_string()));
        out.send_head(status, &headers, None, req.keep_alive, &req.version)?;
        state.statics.streams.fetch_add(1, Ordering::Relaxed);
        let mut reader = resp.into_reader();
        let mut scanner = UsageScanner::new();
        let mut buf = vec![0u8; 16 * 1024];
        let mut bytes = 0u64;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    scanner.feed(&buf[..n]);
                    out.send_body(&buf[..n])?;
                    bytes += n as u64;
                }
                Err(e) => {
                    log_warn!(
                        "[{}] upstream stream error after {} bytes: {}",
                        req_id,
                        bytes,
                        util::truncate(&format!("{}", e), 160)
                    );
                    break;
                }
            }
        }
        scanner.finish();
        out.finish()?;
        state.statics.body_bytes_out.fetch_add(bytes, Ordering::Relaxed);
        if cache_state == "cold" && cache_tokens == 0 {
            state.statics.cold_starts.fetch_add(1, Ordering::Relaxed);
        }
        let delta = usage_delta(acc, upstream_model, is_peak, scanner.usage);
        // NOTE: on a streamed response the head is already on the wire, so the cache state can
        // only be reported in the log and in the counters (see x-router-cache for non-stream).
        let stream_cache = cache_label(scanner.usage.prompt, scanner.usage.cached);
        log_info!(
            "[{}] <- {} {} {} {}ms tokens in={} out={} cached={} cache={} cost={} usd",
            req_id,
            acc.name(),
            status,
            endpoint_mode_name(endpoint),
            latency_ms,
            delta.prompt_tokens,
            delta.completion_tokens,
            delta.cached_tokens,
            stream_cache,
            util::fmt_usd(delta.cost_usd)
        );
        acc.rt.record_success(now, &delta, latency_ms, true);
        state.router.health.record_success(acc.name());
        if let Some(key) = affinity_key {
            state.router.affinity.put(key, acc.name(), now);
        }
        crate::persist::mark_dirty();
        return Ok(());
    }

    let raw = {
        let mut reader = resp.into_reader();
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; 32 * 1024];
        let cap = 64 * 1024 * 1024usize;
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() >= cap {
                        break;
                    }
                }
                Err(e) => {
                    log_warn!(
                        "[{}] upstream read error: {}",
                        req_id,
                        util::truncate(&format!("{}", e), 160)
                    );
                    break;
                }
            }
        }
        buf
    };
    let parsed: Option<Value> = serde_json::from_slice(&raw).ok();
    let raw_usage = parsed
        .as_ref()
        .map(extract_usage)
        .unwrap_or_default();
    let delta = usage_delta(acc, upstream_model, is_peak, raw_usage);
    cache_tokens = raw_usage.cached;
    cache_state = cache_label(raw_usage.prompt, raw_usage.cached);
    let mut headers = extra_headers.clone();
    headers.push(("x-router-cache".to_string(), cache_state.to_string()));
    out.send_head(status, &headers, Some(raw.len()), req.keep_alive, &req.version)?;
    out.send_body(&raw)?;
    out.finish()?;
    state
        .statics
        .body_bytes_out
        .fetch_add(raw.len() as u64, Ordering::Relaxed);
    if cache_state == "cold" && cache_tokens == 0 {
        state.statics.cold_starts.fetch_add(1, Ordering::Relaxed);
    }
    log_info!(
        "[{}] <- {} {} {} {}ms tokens in={} out={} cached={} cache={} cost={} usd",
        req_id,
        acc.name(),
        status,
        endpoint_mode_name(endpoint),
        latency_ms,
        delta.prompt_tokens,
        delta.completion_tokens,
        delta.cached_tokens,
        cache_state,
        util::fmt_usd(delta.cost_usd)
    );
    acc.rt.record_success(now, &delta, latency_ms, false);
    state.router.health.record_success(acc.name());
    if let Some(key) = affinity_key {
        state.router.affinity.put(key, acc.name(), now);
    }
    crate::persist::mark_dirty();
    Ok(())
}

/// Background refresher: OpenCode Go /usage + DeepSeek /user/balance.
pub fn spawn_refreshers(state: Arc<AppState>) {
    std::thread::Builder::new()
        .name("quota-refresh".to_string())
        .spawn(move || loop {
            let cfg = state.cfg();
            let now = util::now_secs();
            for acc in state.account_list().iter() {
                if acc.cfg.quota.probe == QuotaProbe::Usage {
                    let last = {
                        let q = match acc.rt.quota.lock() {
                            Ok(q) => q,
                            Err(p) => p.into_inner(),
                        };
                        q.fetched_at
                    };
                    let due = last == 0
                        || now - last >= acc.cfg.quota.refresh_secs.max(cfg.router.quota_refresh_secs) as i64;
                    if due {
                        let status = crate::quota::refresh_usage(
                            &state.probe_agent,
                            &acc.cfg,
                            &acc.rt,
                            now,
                            &cfg.router.user_agent,
                        );
                        log_debug!("quota {}: {}", acc.name(), status);
                    }
                }
                if acc.cfg.quota.probe == QuotaProbe::Balance {
                    crate::quota::refresh_balance(
                        &state.probe_agent,
                        &acc.cfg,
                        &acc.rt,
                        now,
                        &cfg.router.user_agent,
                    );
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        })
        .ok();
}

/// Watch config.yaml and reload it automatically when it changes.
pub fn spawn_config_watcher(state: Arc<AppState>) {
    std::thread::Builder::new()
        .name("config-watch".to_string())
        .spawn(move || {
            let mut last_signature = file_signature(&state.cfg().path);
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3));
                let path = state.cfg().path.clone();
                let sig = file_signature(&path);
                if sig.is_some() && sig != last_signature {
                    last_signature = sig;
                    match state.reload() {
                        Ok(summary) => log_info!("config reloaded: {}", summary),
                        Err(e) => log_warn!("config reload failed: {}", e),
                    }
                }
            }
        })
        .ok();
}

fn file_signature(path: &std::path::Path) -> Option<(u64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some((meta.len(), mtime))
}

pub fn counters_snapshot(state: &Arc<AppState>) -> Value {
    json!({
        "requests": state.statics.requests.load(Ordering::Relaxed),
        "proxied": state.statics.proxied.load(Ordering::Relaxed),
        "errors": state.statics.errors.load(Ordering::Relaxed),
    })
}

/// Keep reason_phrase / AtomicU64 referenced even if a build trims them.
pub fn _keep_references(a: &AtomicU64) -> u16 {
    let _ = reason_phrase(200);
    a.load(Ordering::Relaxed) as u16
}
