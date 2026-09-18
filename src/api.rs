//! Admin API for the embedded Web UI (`/api/*`).
//!
//! Everything the console can do goes through here. Two rules are enforced throughout:
//!   * the config file on disk stays the single source of truth - the API validates a candidate
//!     document, writes it atomically, then triggers the normal reload path, so the running
//!     router and the file can never drift apart silently;
//!   * secrets are write-only: a key is never echoed back to the browser.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::config::{AccountCfg, AccountKind, Config, QuotaProbe};
use crate::httpd::{Request, Responder};
use crate::models::{Mode, ProviderKind};
use crate::proxy::AppState;
use crate::util;
use crate::{log_info, log_warn};

/// Placeholder the UI sends back when it did not touch the key field.
const KEY_UNCHANGED: &str = "********";

fn json_response(req: &Request, out: &mut Responder, status: u16, body: &Value) {
    let _ = out.send_json(status, body, req.keep_alive, &req.version);
}

fn error_response(req: &Request, out: &mut Responder, status: u16, message: &str) {
    json_response(
        req,
        out,
        status,
        &json!({"error": {"message": message, "type": "admin_error", "code": status}}),
    );
}

fn ok_text(req: &Request, out: &mut Responder, status: u16, text: &str, ctype: &str) {
    let _ = out.send_text(status, text, ctype, req.keep_alive, &req.version);
}

fn body_json(req: &Request) -> Result<Value, String> {
    if req.body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice::<Value>(&req.body).map_err(|e| format!("invalid JSON body: {}", e))
}

/// Route an /api/... request. Returns false when the path is not ours, so the caller can
/// fall through to the static file handler.
pub fn handle(state: &Arc<AppState>, req: &Request, out: &mut Responder) -> bool {
    let trimmed = req.path.trim_end_matches('/');
    let path = if trimmed.is_empty() { "/" } else { trimmed };
    let method = req.method.as_str();

    match (method, path) {
        ("GET", "/api/config") => {
            let cfg = state.cfg();
            json_response(req, out, 200, &config_json(&cfg, state));
        }
        ("PUT", "/api/config") => save_from_json(state, req, out),
        ("GET", "/api/config/raw") => {
            let cfg = state.cfg();
            match std::fs::read_to_string(&cfg.path) {
                Ok(text) => json_response(
                    req,
                    out,
                    200,
                    &json!({"path": cfg.path.display().to_string(), "text": text}),
                ),
                Err(e) => error_response(req, out, 500, &format!("cannot read config: {}", e)),
            }
        }
        ("PUT", "/api/config/raw") => save_raw(state, req, out),
        ("GET", "/api/config/export") => {
            let cfg = state.cfg();
            let header = crate::configwrite::read_header(&cfg.path);
            let text = crate::configwrite::to_text(&crate::configwrite::build_doc(&cfg, &header));
            json_response(
                req,
                out,
                200,
                &json!({"path": cfg.path.display().to_string(), "text": text}),
            );
        }
        ("POST", "/api/config/import") => import_config(state, req, out),
        ("POST", "/api/stats/reset") => {
            reset_stats(state, req, out);
        }
        ("POST", "/api/plan/preview") => plan_preview(state, req, out),
        ("GET", "/api/library") => library_get(state, req, out),
        ("PUT", "/api/library") => library_put(state, req, out),
        ("GET", "/api/backups") => {
            let cfg = state.cfg();
            let list: Vec<Value> = crate::configbackup::list(&cfg.path)
                .iter()
                .map(|b| b.to_json(util::now_secs()))
                .collect();
            json_response(req, out, 200, &json!({"backups": list}));
        }
        ("POST", "/api/backups") => {
            let cfg = state.cfg();
            let tag = body_json(req)
                .ok()
                .and_then(|v| v.get("tag").and_then(|t| t.as_str()).map(|s| s.to_string()))
                .unwrap_or_else(|| "manual".to_string());
            match crate::configbackup::create(&cfg.path, &tag) {
                Ok(entry) => json_response(req, out, 200, &entry.to_json(util::now_secs())),
                Err(e) => error_response(req, out, 500, &e),
            }
        }
        _ => {
            if let Some(name) = path.strip_prefix("/api/backups/") {
                if let Some(name) = name.strip_suffix("/restore") {
                    return restore_backup(state, req, out, name);
                }
                return match method {
                    "GET" => download_backup(state, req, out, name),
                    "DELETE" => delete_backup(state, req, out, name),
                    _ => {
                        error_response(req, out, 405, "method not allowed");
                        true
                    }
                };
            }
            if let Some(rest) = path.strip_prefix("/api/endpoints/") {
                if let Some(name) = rest.strip_suffix("/state") {
                    return set_endpoint_state(state, req, out, name);
                }
                if let Some(name) = rest.strip_suffix("/duplicate") {
                    return duplicate_endpoint(state, req, out, name);
                }
            }
            if let Some(name) = path.strip_prefix("/api/test/") {
                return test_endpoint(state, req, out, name);
            }
            if let Some(name) = path.strip_prefix("/api/endpoints/") {
                if let Some(name) = name.strip_suffix("/models") {
                    return endpoint_models(state, req, out, name);
                }
            }
            return false;
        }
    }
    true
}

// ------------------------------------------------------------------- statistics

/// POST /api/stats/reset - zero the statistics and the endpoint health memory, then checkpoint the
/// result before answering.
///
/// What is NOT reset, deliberately:
///   * the local quota ledger - it is routing input, and clearing it would make a plan look fresh
///     and get burned preferentially;
///   * cooldown clocks and quota readings - those are current facts about the upstream, not
///     history, and clearing them would send traffic straight back into a cooling endpoint.
fn reset_stats(state: &Arc<AppState>, req: &Request, out: &mut Responder) {
    state.registry.reset_stats();
    state.router.health.clear();
    crate::persist::reset_counters(state);
    // The dispatcher counted this request before the reset ran, so the counter sits at 1 and the
    // console would greet the user with "1 request" right after being told everything is zero.
    // Discarding it here is exact, not an approximation: concurrent traffic cannot have incremented
    // the counter between the reset above and this store.
    state.statics.requests.store(0, std::sync::atomic::Ordering::Relaxed);
    // Write now, not on the next 5s tick: the console refreshes immediately after this returns, and
    // a crash before the flush would otherwise bring the old totals back.
    crate::persist::write(state);
    log_info!("statistics and endpoint health were reset by the console");
    json_response(
        req,
        out,
        200,
        &json!({
            "reset": true,
            "note": "statistics and endpoint health cleared; the quota ledger was kept",
        }),
    );
}

// ------------------------------------------------------------------- config view

/// The account as the editor needs it. The key is never the real key: when the endpoint
/// exists on disk we send a placeholder, and a save that returns the placeholder keeps
/// whatever is stored.
fn account_cfg_json(a: &AccountCfg) -> Value {
    let mut headers = serde_json::Map::new();
    for (k, v) in &a.extra_headers {
        headers.insert(k.clone(), json!(v));
    }
    json!({
        "name": a.name,
        "kind": a.kind.as_str(),
        "order": a.order,
        "provider": match a.provider {
            ProviderKind::OpencodeGo => "opencodego",
            ProviderKind::DeepSeek => "deepseek",
            ProviderKind::Generic => "generic",
        },
        "url": a.base(),
        // Only an env:/file: reference is safe to echo; a literal key never leaves the server.
        "key": if is_secret_ref(&a.key) { a.key.clone() } else { KEY_UNCHANGED.to_string() },
        "model": a.model,
        "modes": modes_json(&a.modes),
        "rules": a.rules.iter().map(|r| r.as_str()).collect::<Vec<_>>(),
        "no_error_fallback": a.no_error_fallback,
        "inject_session": a.inject_session,
        "headers": Value::Object(headers),
        "drop_params": a.drop_params,
        "quota": {
            "unit": a.quota.unit.as_str(),
            "rolling": a.quota.rolling,
            "weekly": a.quota.weekly,
            "monthly": a.quota.monthly,
            "probe": a.quota.probe.as_str(),
            "refresh_secs": a.quota.refresh_secs,
        },
        "enabled": a.is_enabled(),
    })
}

fn modes_json(modes: &[Mode]) -> Value {
    if modes.contains(&Mode::Chat) && modes.contains(&Mode::Responses) && modes.len() == 2 {
        return json!(["both"]);
    }
    json!(modes.iter().map(|m| m.as_str()).collect::<Vec<_>>())
}

fn config_json(cfg: &Config, state: &Arc<AppState>) -> Value {
    let endpoints: Vec<Value> = cfg.accounts.iter().map(account_cfg_json).collect();
    json!({
        "server": {
            "host": cfg.server.host,
            "port": cfg.server.port,
            "max_connections": cfg.server.max_connections,
            "client_keys": cfg.server.client_keys,
            "max_body_bytes": cfg.server.max_body_bytes,
            "idle_timeout_secs": cfg.server.idle_timeout_secs,
            "read_timeout_secs": cfg.server.read_timeout_secs,
            "stream": cfg.server.stream,
        },
        "log": {
            "level": cfg.log.level,
            "file": cfg.log.file.clone().unwrap_or_default(),
            "quiet": cfg.log.quiet,
        },
        "router": router_json(cfg),
        "compat": {
            "developer_role_to_system": cfg.compat.developer_role_to_system,
            "max_completion_tokens_to_max_tokens": cfg.compat.max_completion_tokens_to_max_tokens,
            "drop_params": cfg.compat.drop_params,
            "forward_headers": cfg.compat.forward_headers,
        },
        "endpoints": endpoints,
        "warnings": cfg.warnings,
        "ui": {
            "managed": true,
            "path_locked": true,
            "accounts_live": state.account_list().len(),
        },
    })
}

fn router_json(cfg: &Config) -> Value {
    let r = &cfg.router;
    json!({
        "mode": match r.mode {
            crate::config::RouterMode::Auto => "auto",
            crate::config::RouterMode::Plans => "plans",
            crate::config::RouterMode::Fallback => "fallback",
        },
        "peak_windows": r.peak_spec,
        "idle_prefer": match r.idle_prefer {
            crate::config::IdlePrefer::SurplusFirst => "surplus_first",
            crate::config::IdlePrefer::Plans => "plans",
            crate::config::IdlePrefer::Fallback => "fallback",
        },
        "surplus_max_pct": r.surplus_max_pct,
        "surplus_projection": r.surplus_projection,
        "exhaust_at_pct": r.exhaust_at_pct,
        "quota_refresh_secs": r.quota_refresh_secs,
        "cooldown_secs": r.cooldown_secs,
        "auth_cooldown_secs": r.auth_cooldown_secs,
        "server_error_cooldown_secs": r.server_error_cooldown_secs,
        "retry_on_model_error": r.retry_on_model_error,
        "skip_after_failures": r.skip_after_failures,
        "skip_secs": r.skip_secs,
        "attempt_budget_secs": r.attempt_budget_secs,
        "session_affinity": r.session_affinity,
        "session_affinity_ttl_secs": r.session_affinity_ttl_secs,
        "session_fallback": r.session_fallback.as_str(),
        "inject_stream_usage": r.inject_stream_usage,
        "session_headers": r.session_headers,
        "user_agent": r.user_agent,
        "request_timeout_secs": r.request_timeout_secs,
        "stream_idle_timeout_secs": r.stream_idle_timeout_secs,
    })
}


// ------------------------------------------------------------------- config write

/// Turn an editor document into YAML text that the real parser accepts. Reusing the parser as
/// the validator (rather than hand-checking every field) is what keeps UI and runtime in step:
/// anything the console can save is something the router can load.
fn doc_to_yaml(doc: &Value, existing: &Config) -> Result<String, String> {
    let server = doc.get("server").ok_or("server section is required")?;
    let router = doc.get("router").ok_or("router section is required")?;
    let compat = doc.get("compat").unwrap_or(&Value::Null);
    let log = doc.get("log").unwrap_or(&Value::Null);
    let endpoints = doc
        .get("endpoints")
        .and_then(|v| v.as_array())
        .ok_or("endpoints must be an array")?;
    if endpoints.is_empty() {
        return Err("at least one endpoint is required".to_string());
    }

    let old_by_name: std::collections::HashMap<&str, &AccountCfg> =
        existing.accounts.iter().map(|a| (a.name.as_str(), a)).collect();

    let mut out = String::new();
    out.push_str("# This file is managed by the ar-OCG-Router web console.\n");
    out.push_str("# Hand-written comments are NOT preserved: back it up before editing by hand.\n");
    out.push_str("# One endpoint = one merchant + one model.\n\n");
    out.push_str("server:\n");
    out.push_str(&format!("  host: {}\n", yaml_str(str_at(server, "host").unwrap_or("127.0.0.1"))));
    out.push_str(&format!("  port: {}\n", int_at(server, "port").unwrap_or(8787)));
    out.push_str(&format!("  max_connections: {}\n", int_at(server, "max_connections").unwrap_or(64)));
    // Must match the parser's default (config.rs: 64 MiB). A mismatched fallback here would
    // silently change a setting the user never touched whenever a partial document is saved.
    out.push_str(&format!(
        "  max_body_bytes: {}\n",
        int_at(server, "max_body_bytes").unwrap_or(existing.server.max_body_bytes as i64)
    ));
    out.push_str(&format!("  idle_timeout_secs: {}\n", int_at(server, "idle_timeout_secs").unwrap_or(1800)));
    out.push_str(&format!("  read_timeout_secs: {}\n", int_at(server, "read_timeout_secs").unwrap_or(600)));
    let keys: Vec<String> = server
        .get("client_keys")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(|s| s.to_string()).collect())
        .unwrap_or_default();
    if !keys.is_empty() {
        out.push_str("  client_keys:\n");
        for k in &keys {
            out.push_str(&format!("    - {}\n", yaml_str(k)));
        }
    }

    out.push_str("\nlog:\n");
    out.push_str(&format!("  level: {}\n", yaml_str(str_at(log, "level").unwrap_or("info"))));
    let logfile = str_at(log, "file").unwrap_or("");
    if !logfile.trim().is_empty() {
        out.push_str(&format!("  file: {}\n", yaml_str(logfile)));
    }

    out.push_str("\nrouter:\n");
    out.push_str(&format!("  mode: {}\n", yaml_str(str_at(router, "mode").unwrap_or("auto"))));
    // Read from the document. A partial document falls back to what is live, never to the
    // built-in constant: that constant is a default for a *new* config, not for a save.
    out.push_str(&format!(
        "  peak_windows: {}\n",
        yaml_str(str_at(router, "peak_windows").unwrap_or(existing.router.peak_spec.as_str()))
    ));
    out.push_str(&format!("  idle_prefer: {}\n", yaml_str(str_at(router, "idle_prefer").unwrap_or("surplus_first"))));
    out.push_str(&format!("  surplus_max_pct: {}\n", num_at(router, "surplus_max_pct").unwrap_or(80.0)));
    out.push_str(&format!("  surplus_projection: {}\n", bool_at(router, "surplus_projection").unwrap_or(true)));
    out.push_str(&format!("  exhaust_at_pct: {}\n", num_at(router, "exhaust_at_pct").unwrap_or(99.0)));
    out.push_str(&format!("  quota_refresh_secs: {}\n", int_at(router, "quota_refresh_secs").unwrap_or(60)));
    out.push_str(&format!("  cooldown_secs: {}\n", int_at(router, "cooldown_secs").unwrap_or(30)));
    out.push_str(&format!("  auth_cooldown_secs: {}\n", int_at(router, "auth_cooldown_secs").unwrap_or(600)));
    out.push_str(&format!("  server_error_cooldown_secs: {}\n", int_at(router, "server_error_cooldown_secs").unwrap_or(20)));
    out.push_str(&format!("  retry_on_model_error: {}\n", bool_at(router, "retry_on_model_error").unwrap_or(true)));
    out.push_str(&format!("  skip_after_failures: {}\n", int_at(router, "skip_after_failures").unwrap_or(3)));
    out.push_str(&format!("  skip_secs: {}\n", int_at(router, "skip_secs").unwrap_or(300)));
    out.push_str(&format!("  attempt_budget_secs: {}\n", int_at(router, "attempt_budget_secs").unwrap_or(120)));
    out.push_str(&format!("  session_affinity: {}\n", bool_at(router, "session_affinity").unwrap_or(true)));
    out.push_str(&format!("  session_affinity_ttl_secs: {}\n", int_at(router, "session_affinity_ttl_secs").unwrap_or(1800)));
    out.push_str(&format!("  session_fallback: {}\n", yaml_str(str_at(router, "session_fallback").unwrap_or("process"))));
    out.push_str(&format!("  inject_stream_usage: {}\n", bool_at(router, "inject_stream_usage").unwrap_or(false)));
    let headers: Vec<String> = router
        .get("session_headers")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(|s| s.to_string()).collect())
        .unwrap_or_default();
    if !headers.is_empty() {
        out.push_str("  session_headers:\n");
        for h in &headers {
            out.push_str(&format!("    - {}\n", yaml_str(h)));
        }
    }
    out.push_str(&format!(
        "  user_agent: {}\n",
        yaml_str(str_at(router, "user_agent").unwrap_or(existing.router.user_agent.as_str()))
    ));
    out.push_str(&format!(
        "  request_timeout_secs: {}\n",
        int_at(router, "request_timeout_secs").unwrap_or(existing.router.request_timeout_secs as i64)
    ));
    out.push_str(&format!("  stream_idle_timeout_secs: {}\n", int_at(router, "stream_idle_timeout_secs").unwrap_or(300)));

    out.push_str("\ncompat:\n");
    out.push_str(&format!("  developer_role_to_system: {}\n", bool_at(compat, "developer_role_to_system").unwrap_or(false)));
    out.push_str(&format!("  max_completion_tokens_to_max_tokens: {}\n", bool_at(compat, "max_completion_tokens_to_max_tokens").unwrap_or(false)));
    push_str_list(&mut out, "  drop_params", compat.get("drop_params"));
    push_str_list(&mut out, "  forward_headers", compat.get("forward_headers"));

    for (section, kind) in [("plans", AccountKind::Plans), ("fallback", AccountKind::Cash)] {
        out.push_str(&format!("\n{}:\n", section));
        let mut any = false;
        let mut position = 0;
        for e in endpoints {
            // A missing kind belongs to the section being emitted, and "cash" is accepted as an
            // alias for the wire name "fallback": defaulting a nameless endpoint to plans moved a
            // cash account to the wrong side of the peak/off-peak policy.
            let declared = str_at(e, "kind").unwrap_or(kind.as_str());
            let declared = if declared == "cash" { "fallback" } else { declared };
            if declared != kind.as_str() {
                continue;
            }
            any = true;
            position += 1;
            out.push_str(&endpoint_yaml(e, &old_by_name, position)?);
        }
        if !any {
            out.push_str("  []\n");
        }
    }

    Ok(out)
}

/// One endpoint block. The plans/fallback split is the caller's: within a block the two kinds
/// are written identically, because the section the block sits in is what decides the kind.
fn endpoint_yaml(
    e: &Value,
    old: &std::collections::HashMap<&str, &AccountCfg>,
    // 1-based position inside its own section; becomes "order".
    position: i32,
) -> Result<String, String> {
    let name = str_at(e, "name").unwrap_or("").trim().to_string();
    let url = str_at(e, "url").unwrap_or("").trim().trim_end_matches('/').to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("endpoint {:?}: url must start with http:// or https://", label(&name)));
    }
    let model = str_at(e, "model").unwrap_or("").trim().to_string();
    if model.is_empty() {
        return Err(format!("endpoint {:?}: model is required (one endpoint serves one model)", label(&name)));
    }

    // A key equal to the placeholder means "the browser never saw the real one"; carry the stored
    // value over so editing an unrelated field cannot silently wipe a credential.
    //
    // The lookup is by name, but a rename changes the name before the write reaches us, so the
    // editor also sends the previous name. Without it a rename could never be saved: the
    // placeholder stopped resolving and the write was rejected as a new endpoint without a key.
    let raw_key = str_at(e, "key").unwrap_or("").trim().to_string();
    let key = if raw_key == KEY_UNCHANGED {
        let previous = str_at(e, "renamed_from").map(str::trim).filter(|s| !s.is_empty());
        previous
            .and_then(|p| old.get(p))
            .or_else(|| old.get(name.as_str()))
            .map(|a| a.key.clone())
            .ok_or_else(|| format!("endpoint {:?}: key is required for a new endpoint", label(&name)))?
    } else if raw_key.is_empty() {
        return Err(format!("endpoint {:?}: key is empty", label(&name)));
    } else {
        raw_key
    };

    let mut s = String::new();
    s.push_str("  - ");
    let cont = "    ";
    if !name.is_empty() {
        s.push_str(&format!("name: {}\n", yaml_str(&name)));
        s.push_str(cont);
    }
    s.push_str(&format!("url: {}\n", yaml_str(&url)));
    s.push_str(cont);
    s.push_str(&format!("provider: {}\n", yaml_str(str_at(e, "provider").unwrap_or("generic"))));
    s.push_str(cont);
    s.push_str(&format!("key: {}\n", yaml_str(&key)));
    s.push_str(cont);
    s.push_str(&format!("model: {}\n", yaml_str(&model)));
    s.push_str(cont);
    let modes: Vec<String> = e
        .get("modes")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.to_string()).collect())
        .unwrap_or_else(|| vec!["both".to_string()]);
    let modes = if modes.is_empty() { vec!["both".to_string()] } else { modes };
    s.push_str(&format!("mode: {}\n", yaml_str(&modes.join(","))));
    s.push_str(cont);
    // Derived, never taken from the document: the console renumbers from the row position, so
    // the sequence the user arranged by dragging is what the file says and what the router uses.
    s.push_str(&format!("order: {}\n", position * 10));
    s.push_str(cont);
    // Written for every kind. The parser applies the rule list to plans and cash accounts alike,
    // so emitting it only for cash silently reset a plan's rules to [always] on the next save.
    // no_error_fallback is a *rule token* to the parser - writing it as a top-level key stored a
    // value nothing ever read back.
    let mut rules: Vec<String> = e
        .get("rules")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.to_string()).collect())
        .unwrap_or_else(|| vec!["always".to_string()]);
    rules.retain(|r| r != "no_error_fallback");
    if bool_at(e, "no_error_fallback").unwrap_or(false) {
        rules.push("no_error_fallback".to_string());
    }
    let rules = if rules.is_empty() { vec!["always".to_string()] } else { rules };
    s.push_str(&format!("rule: [{}]\n", rules.join(", ")));
    s.push_str(cont);
    // Written explicitly so the file explains itself, and read back by is_enabled().
    if !bool_at(e, "enabled").unwrap_or(true) {
        s.push_str("enabled: false\n");
        s.push_str(cont);
    }
    if bool_at(e, "inject_session").unwrap_or(false) {
        s.push_str("inject_session: true\n");
        s.push_str(cont);
    }
    let q = e.get("quota").cloned().unwrap_or(Value::Null);
    s.push_str(&format!(
        "quota: {{ unit: {}, rolling: {}, weekly: {}, monthly: {}, probe: {}, refresh_secs: {} }}\n",
        yaml_str(str_at(&q, "unit").unwrap_or("none")),
        num_at(&q, "rolling").unwrap_or(0.0),
        num_at(&q, "weekly").unwrap_or(0.0),
        num_at(&q, "monthly").unwrap_or(0.0),
        yaml_str(str_at(&q, "probe").unwrap_or("none")),
        int_at(&q, "refresh_secs").unwrap_or(300)
    ));
    let drop: Vec<String> = e
        .get("drop_params")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.to_string()).collect())
        .unwrap_or_default();
    let header_map = e.get("headers").and_then(|v| v.as_object()).cloned();
    let has_headers = header_map.as_ref().map(|h| !h.is_empty()).unwrap_or(false);
    if has_headers || !drop.is_empty() {
        // the inline quota map ends the scalar run; nested maps start clean on the next line
        s.push_str("headers:\n");
        if let Some(h) = &header_map {
            for (k, v) in h {
                s.push_str(&format!("      {}: {}\n", k, yaml_str(v.as_str().unwrap_or(""))));
            }
        }
        s.push_str(cont);
        if !drop.is_empty() {
            s.push_str(&format!("drop_params: [{}]\n", drop.join(", ")));
        }
    }
    Ok(s)
}

fn label(name: &str) -> &str {
    if name.is_empty() {
        "(new endpoint)"
    } else {
        name
    }
}

fn push_str_list(out: &mut String, key: &str, v: Option<&Value>) {
    let list: Vec<String> = v
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|i| i.as_str()).map(|s| s.to_string()).collect())
        .unwrap_or_default();
    if list.is_empty() {
        out.push_str(&format!("{}: []\n", key));
    } else {
        out.push_str(&format!("{}:\n", key));
        for item in &list {
            out.push_str(&format!("    - {}\n", yaml_str(item)));
        }
    }
}

fn str_at<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}
fn int_at(v: &Value, k: &str) -> Option<i64> {
    v.get(k)
        .and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)))
}
fn num_at(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64())
}
fn bool_at(v: &Value, k: &str) -> Option<bool> {
    v.get(k).and_then(|x| x.as_bool())
}

/// A conservative YAML scalar: quote whenever the value could be read as anything else.
fn yaml_str(s: &str) -> String {
    let quote = s.is_empty() || s.chars().any(is_special) || s.starts_with(' ') || s.ends_with(' ') || is_reserved(s);
    if quote {
        let mut out = String::from("\"");
        for c in s.chars() {
            if c == '\\' || c == '"' {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
        out
    } else {
        s.to_string()
    }
}

fn is_special(c: char) -> bool {
    matches!(c, ':' | '#' | '{' | '}' | '[' | ']' | ',' | '&' | '*' | '!' | '|' | '>' | '%' | '@' | '`' | '"' | '\'')
}

fn is_reserved(s: &str) -> bool {
    ["true", "false", "null", "yes", "no", "on", "off", "~"]
        .iter()
        .any(|k| s.eq_ignore_ascii_case(k))
}

// ------------------------------------------------------------------- write paths

fn save_from_json(state: &Arc<AppState>, req: &Request, out: &mut Responder) {
    let doc = match body_json(req) {
        Ok(v) => v,
        Err(e) => return error_response(req, out, 400, &e),
    };
    let existing = state.cfg();
    let text = match doc_to_yaml(&doc, &existing) {
        Ok(t) => t,
        Err(e) => return error_response(req, out, 400, &e),
    };
    // Validate before writing: a broken config must never replace a working file.
    let parsed = match crate::config::parse(&text, &existing.path) {
        Ok(c) => c,
        Err(e) => return error_response(req, out, 400, &format!("config rejected: {}", e)),
    };
    if let Err(e) = crate::configbackup::create(&existing.path, "autosave") {
        log_warn!("pre-save backup failed: {}", e);
    }
    if let Err(e) = crate::configbackup::write(&existing.path, &text) {
        return error_response(req, out, 500, &format!("cannot write config: {}", e));
    }
    finish_write(state, req, out, parsed.warnings.clone(), "config saved");
}

fn save_raw(state: &Arc<AppState>, req: &Request, out: &mut Responder) {
    let doc = match body_json(req) {
        Ok(v) => v,
        Err(e) => return error_response(req, out, 400, &e),
    };
    let text = match str_at(&doc, "text") {
        Some(t) => t.to_string(),
        None => return error_response(req, out, 400, "text is required"),
    };
    let existing = state.cfg();
    let parsed = match crate::config::parse(&text, &existing.path) {
        Ok(c) => c,
        Err(e) => return error_response(req, out, 400, &format!("config rejected: {}", e)),
    };
    if let Err(e) = crate::configbackup::create(&existing.path, "pre-raw-edit") {
        log_warn!("pre-save backup failed: {}", e);
    }
    if let Err(e) = crate::configbackup::write(&existing.path, &text) {
        return error_response(req, out, 500, &format!("cannot write config: {}", e));
    }
    finish_write(state, req, out, parsed.warnings.clone(), "config saved");
}

fn import_config(state: &Arc<AppState>, req: &Request, out: &mut Responder) {
    let doc = match body_json(req) {
        Ok(v) => v,
        Err(e) => return error_response(req, out, 400, &e),
    };
    let text = str_at(&doc, "text").unwrap_or("").to_string();
    if text.trim().is_empty() {
        return error_response(req, out, 400, "the imported file is empty");
    }
    let existing = state.cfg();
    // Accept either YAML or the JSON shape the console itself exports.
    let yaml = if text.trim_start().starts_with('{') {
        match serde_json::from_str::<Value>(&text) {
            Ok(v) => match doc_to_yaml(&v, &existing) {
                Ok(t) => t,
                Err(e) => return error_response(req, out, 400, &format!("import rejected: {}", e)),
            },
            Err(e) => return error_response(req, out, 400, &format!("invalid JSON: {}", e)),
        }
    } else {
        text.clone()
    };
    let parsed = match crate::config::parse(&yaml, &existing.path) {
        Ok(c) => c,
        Err(e) => return error_response(req, out, 400, &format!("import rejected: {}", e)),
    };
    if bool_at(&doc, "backup").unwrap_or(true) {
        if let Err(e) = crate::configbackup::create(&existing.path, "pre-import") {
            log_warn!("pre-import backup failed: {}", e);
        }
    }
    if let Err(e) = crate::configbackup::write(&existing.path, &yaml) {
        return error_response(req, out, 500, &format!("cannot write config: {}", e));
    }
    finish_write(state, req, out, parsed.warnings.clone(), "config imported");
}

/// Common tail of every config write: reload into the live state and report what happened.
fn finish_write(
    state: &Arc<AppState>,
    req: &Request,
    out: &mut Responder,
    warnings: Vec<String>,
    message: &str,
) {
    match state.reload() {
        Ok(summary) => {
            log_info!("{} via web ui: {}", message, summary);
            let cfg = state.cfg();
            json_response(
                req,
                out,
                200,
                &json!({
                    "saved": true,
                    "reloaded": true,
                    "summary": summary,
                    "warnings": warnings,
                    "message": message,
                    "config": config_json(&cfg, state),
                }),
            );
        }
        Err(e) => {
            log_warn!("config written but reload failed: {}", e);
            json_response(
                req,
                out,
                200,
                &json!({"saved": true, "reloaded": false, "warning": e, "warnings": warnings}),
            );
        }
    }
}

// ------------------------------------------------------------------- backups

fn restore_backup(state: &Arc<AppState>, req: &Request, out: &mut Responder, name: &str) -> bool {
    let cfg = state.cfg();
    match crate::configbackup::restore(&cfg.path, name) {
        Ok(()) => match state.reload() {
            Ok(summary) => json_response(
                req,
                out,
                200,
                &json!({"restored": name, "reloaded": true, "summary": summary}),
            ),
            Err(e) => json_response(
                req,
                out,
                200,
                &json!({"restored": name, "reloaded": false, "warning": e}),
            ),
        },
        Err(e) => error_response(req, out, 400, &e),
    }
    true
}

fn safe_backup_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && name.starts_with("config-")
        && name.ends_with(".yaml")
}

fn delete_backup(state: &Arc<AppState>, req: &Request, out: &mut Responder, name: &str) -> bool {
    if !safe_backup_name(name) {
        error_response(req, out, 400, "invalid backup name");
        return true;
    }
    let cfg = state.cfg();
    let path = crate::configbackup::backup_dir(&cfg.path).join(name);
    match std::fs::remove_file(&path) {
        Ok(()) => json_response(req, out, 200, &json!({"deleted": name})),
        Err(e) => error_response(req, out, 500, &format!("cannot delete: {}", e)),
    }
    true
}

fn download_backup(state: &Arc<AppState>, req: &Request, out: &mut Responder, name: &str) -> bool {
    if !safe_backup_name(name) {
        error_response(req, out, 400, "invalid backup name");
        return true;
    }
    let cfg = state.cfg();
    let path = crate::configbackup::backup_dir(&cfg.path).join(name);
    match std::fs::read_to_string(&path) {
        Ok(text) => ok_text(req, out, 200, &text, "text/yaml; charset=utf-8"),
        Err(e) => error_response(req, out, 404, &format!("cannot read: {}", e)),
    }
    true
}

// ------------------------------------------------------------------- live actions

/// Rewrite the live config from an in-memory account list. Used by enable/disable and
/// duplicate, which must take effect immediately rather than waiting for the editor to save.
fn persist_accounts(state: &Arc<AppState>, all: &[AccountCfg], tag: &str) -> Result<(), String> {
    let cfg = state.cfg();
    let mut doc = config_json(&cfg, state);
    let mut endpoints: Vec<Value> = all.iter().map(account_cfg_json).collect();
    // The placeholder keys must not be written back; patch the real ones in.
    for (i, a) in all.iter().enumerate() {
        endpoints[i]["key"] = json!(a.key);
    }
    doc["endpoints"] = Value::Array(endpoints);
    let text = doc_to_yaml(&doc, &cfg)?;
    crate::config::parse(&text, &cfg.path)?;
    crate::configbackup::create(&cfg.path, tag)?;
    crate::configbackup::write(&cfg.path, &text)
}

fn set_endpoint_state(state: &Arc<AppState>, req: &Request, out: &mut Responder, name: &str) -> bool {
    let doc = body_json(req).unwrap_or(Value::Null);
    let enabled = bool_at(&doc, "enabled").unwrap_or(true);
    let cfg = state.cfg();
    let Some(idx) = cfg.accounts.iter().position(|a| a.name == name) else {
        error_response(req, out, 404, &format!("unknown endpoint {:?}", name));
        return true;
    };
    let mut all = cfg.accounts.clone();
    let mut updated = all[idx].clone();
    updated.disabled = !enabled;
    all[idx] = updated;
    match persist_accounts(state, &all, "pre-toggle") {
        Ok(()) => {
            let reloaded = match state.reload() {
                Ok(_) => true,
                Err(e) => {
                    log_warn!("toggle written but reload failed: {}", e);
                    false
                }
            };
            log_info!("endpoint {} {}", name, if enabled { "enabled" } else { "disabled" });
            json_response(req, out, 200, &json!({"name": name, "enabled": enabled, "reloaded": reloaded}));
        }
        Err(e) => error_response(req, out, 500, &e),
    }
    true
}

fn duplicate_endpoint(state: &Arc<AppState>, req: &Request, out: &mut Responder, name: &str) -> bool {
    let doc = body_json(req).unwrap_or(Value::Null);
    let cfg = state.cfg();
    let Some(source) = cfg.accounts.iter().find(|a| a.name == name).cloned() else {
        error_response(req, out, 404, &format!("unknown endpoint {:?}", name));
        return true;
    };
    let base = {
        let given = str_at(&doc, "new_name").unwrap_or("").trim().to_string();
        if given.is_empty() {
            let stem = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '-');
            let stem = if stem.is_empty() { name } else { stem };
            format!("{}-copy", stem)
        } else {
            given
        }
    };
    let mut candidate = base.clone();
    let mut n = 2;
    while cfg.accounts.iter().any(|a| a.name == candidate) {
        candidate = format!("{}-{}", base, n);
        n += 1;
        if n > 999 {
            break;
        }
    }
    if cfg.accounts.iter().any(|a| a.name == candidate) {
        error_response(req, out, 409, "cannot find a free name for the copy");
        return true;
    }

    let mut copy = source.clone();
    copy.name = candidate.clone();
    let mut all = cfg.accounts.clone();
    let idx = all.iter().position(|a| a.name == name).unwrap();
    all.insert(idx + 1, copy);
    match persist_accounts(state, &all, "pre-duplicate") {
        Ok(()) => {
            let reloaded = state.reload().is_ok();
            log_info!("endpoint {} duplicated as {}", name, candidate);
            json_response(req, out, 200, &json!({"source": name, "name": candidate, "reloaded": reloaded}));
        }
        Err(e) => error_response(req, out, 500, &e),
    }
    true
}

// ------------------------------------------------------------------- plan preview

fn plan_preview(state: &Arc<AppState>, req: &Request, out: &mut Responder) {
    let doc = body_json(req).unwrap_or(Value::Null);
    let endpoint = match str_at(&doc, "endpoint").unwrap_or("chat") {
        "responses" => crate::models::Endpoint::Responses,
        "anthropic" | "messages" => crate::models::Endpoint::Anthropic,
        _ => crate::models::Endpoint::Chat,
    };
    let cfg = state.cfg();
    let accounts = state.account_list();
    let known: Vec<String> = accounts.iter().map(|a| a.name().to_string()).collect();
    let forced = str_at(&doc, "forced")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .and_then(|s| crate::models::parse_forced_endpoint(&s, &known));

    let now = util::now_secs();
    let plan = state.router.plan(&cfg, &accounts, endpoint, forced.as_ref(), None, now);
    let candidates: Vec<Value> = plan
        .candidates
        .iter()
        .map(|a| {
            json!({
                "name": a.name(),
                "kind": a.cfg.kind.as_str(),
                "model": a.cfg.model,
                "url": a.cfg.base(),
            })
        })
        .collect();
    let skipped: Vec<Value> = plan
        .skipped
        .iter()
        .map(|(n, r)| json!({"name": n, "reason": r}))
        .collect();
    json_response(
        req,
        out,
        200,
        &json!({
            "now": crate::timeutil::iso8601(now),
            // Which config produced this answer. Always the running one: the console edits a draft
            // that only reaches the router after a save, so the UI has to be able to say so
            // instead of showing a plan that silently disagrees with what is on screen.
            "source": "live",
            "peak": plan.is_peak,
            "preferred": plan.preferred.as_str(),
            "reason": plan.reason,
            "plans_surplus": plan.plans_surplus,
            "plans_available": plan.plans_available,
            "candidates": candidates,
            "skipped": skipped,
        }),
    );
}

// ------------------------------------------------------------------- live test

/// Probe one endpoint exactly the way the router would use it: catalog, quota, then one minimal
/// real request. This is the button that answers "is this endpoint actually usable?".
fn test_endpoint(state: &Arc<AppState>, req: &Request, out: &mut Responder, name: &str) -> bool {
    let cfg = state.cfg();
    let Some(acc) = cfg.accounts.iter().find(|a| a.name == name).cloned() else {
        error_response(req, out, 404, &format!("unknown endpoint {:?}", name));
        return true;
    };
    let agent = crate::httpclient::agent(10, 30, 30);
    let ua = cfg.router.user_agent.clone();
    let now = util::now_secs();

    let models = match crate::httpclient::get_json(&agent, &crate::quota::models_url(&acc), &acc.key, &ua) {
        Ok((200, body)) => {
            let ids: Vec<String> = body
                .get("data")
                .and_then(|d| d.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "ok": true,
                "count": ids.len(),
                "ids": ids.iter().take(80).cloned().collect::<Vec<_>>(),
                "error": Value::Null,
            })
        }
        Ok((code, body)) => json!({
            "ok": false,
            "count": 0,
            "ids": [],
            "error": format!("HTTP {} {}", code, util::truncate(&crate::httpclient::short_body(&body, "", 200), 200)),
        }),
        Err(e) => json!({"ok": false, "count": 0, "ids": [], "error": util::truncate(&e, 200)}),
    };

    let rt = state.registry.get_or_create(&acc.name);
    // The background refresher already polls these endpoints; asking again can answer "backoff"
    // (rate limited) instead of the value. So: try once, then report what is actually stored.
    let probe_status = if acc.quota.probe == QuotaProbe::Usage {
        Some(("usage", crate::quota::refresh_usage(&agent, &acc, &rt, now, &ua)))
    } else if acc.quota.probe == QuotaProbe::Balance {
        Some(("balance", crate::quota::refresh_balance(&agent, &acc, &rt, now, &ua)))
    } else {
        None
    };
    let quota = probe_status.map(|(kind, status)| {
        let report = rt.quota_report(now, &cfg, &acc.quota);
        let stale = report.source != crate::state::QuotaSource::Remote;
        // "backoff" is not a failure: the value is simply fresh already.
        let ok = !stale && !status.starts_with("error") && !status.starts_with("http");
        let detail = if status == "backoff" || status.starts_with("ok ") {
            format!(
                "rolling {:.0}% / weekly {:.0}% / monthly {:.0}% (source={})",
                report.rolling.pct,
                report.weekly.pct,
                report.monthly.pct,
                report.source.as_str()
            )
        } else if stale {
            format!("{} (no live reading; source={})", status, report.source.as_str())
        } else {
            status
        };
        json!({"kind": kind, "ok": ok, "detail": detail})
    });

    let url = acc.upstream_url("/v1/chat/completions");
    let body = json!({
        "model": acc.model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 8,
        "stream": false,
    });
    let mut call = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {}", acc.key))
        .set("User-Agent", &ua);
    if acc.inject_session {
        call = call.set("x-opencode-session", &util::gen_session_id());
    }
    for (k, v) in &acc.extra_headers {
        call = call.set(k, v);
    }
    let started = std::time::Instant::now();
    let chat = match call.send_bytes(&serde_json::to_vec(&body).unwrap_or_default()) {
        Ok(resp) if (200..300).contains(&resp.status()) => {
            let text = resp.into_string().unwrap_or_default();
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            json!({
                "ok": true,
                "status": 200,
                "model": v.get("model").and_then(|m| m.as_str()).unwrap_or(&acc.model),
                "latency_ms": started.elapsed().as_millis(),
                "error": Value::Null,
            })
        }
        Ok(resp) => {
            let code = resp.status();
            let text = resp.into_string().unwrap_or_default();
            json!({"ok": false, "status": code, "model": acc.model, "latency_ms": started.elapsed().as_millis(), "error": util::truncate(&text, 300)})
        }
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            json!({"ok": false, "status": code, "model": acc.model, "latency_ms": started.elapsed().as_millis(), "error": util::truncate(&text, 300)})
        }
        Err(e) => json!({"ok": false, "status": 0, "model": acc.model, "latency_ms": started.elapsed().as_millis(), "error": util::truncate(&format!("{}", e), 300)}),
    };

    // Evidence-based hints only: the router never changes a setting on its own.
    let mut suggestions: Vec<String> = Vec::new();
    if acc.provider == ProviderKind::OpencodeGo && !acc.inject_session {
        suggestions.push("OpenCode Go 需要 inject_session: true，否则上游会返回 400 MissingSessionID".to_string());
    }
    if quota.is_none() && acc.kind == AccountKind::Plans {
        suggestions.push("这个端点没有额度探测方式，闲时判定会退化为本地记账估算（quota.probe: none）".to_string());
    }
    if chat["status"] == json!(429) {
        suggestions.push("上游返回 429：额度可能已用尽或被限流，可以调大 order 让它晚点被选中".to_string());
    }
    if chat["status"] == json!(401) || chat["status"] == json!(403) {
        suggestions.push("鉴权失败：检查 key 是否正确，以及该套餐的 URL 前缀（Go 是 /zen/go/v1）".to_string());
    }
    if models["ok"] == json!(true) {
        let listed = models["ids"].as_array().cloned().unwrap_or_default();
        let hit = listed.iter().any(|v| v.as_str() == Some(acc.model.as_str()));
        if !listed.is_empty() && !hit {
            suggestions.push(format!("该端点的 model {:?} 不在上游 /models 列表里；列表常常不可信，但如果最小请求也失败，就先核对模型 id", acc.model));
        }
    }

    json_response(
        req,
        out,
        200,
        &json!({
            "name": acc.name,
            "key_ok": !acc.key.trim().is_empty(),
            "models": models,
            "quota": quota,
            "chat": chat,
            "suggestions": suggestions,
        }),
    );
    true
}

/// Whether the console is compiled in; main.rs uses it to decide what to advertise.
pub fn ui_available() -> bool {
    crate::webui::is_embedded()
}

/// True for key values that are references rather than secrets (env:VAR, file:path).
/// Those are safe to show in the editor; a literal credential is not.
fn is_secret_ref(v: &str) -> bool {
    let v = v.trim();
    v.starts_with("env:") || v.starts_with("file:")
}

// ------------------------------------------------------------------- models & library

/// GET /api/endpoints/<name>/models - the endpoint's live model catalog.
///
/// This is what makes the "Model ID" field in the console a picker instead of a typing test.
/// The catalog is advisory (upstreams list entries they cannot serve and omit ones they can -
/// see README 10.1), so the UI presents it as suggestions and keeps free text possible.
fn endpoint_models(state: &Arc<AppState>, req: &Request, out: &mut Responder, name: &str) -> bool {
    let cfg = state.cfg();
    let Some(acc) = cfg.accounts.iter().find(|a| a.name == name).cloned() else {
        error_response(req, out, 404, &format!("unknown endpoint {:?}", name));
        return true;
    };
    let agent = crate::httpclient::agent(10, 20, 20);
    let ua = cfg.router.user_agent.clone();
    let ctx = crate::library::load(&cfg.path);
    match crate::httpclient::get_json(&agent, &crate::quota::models_url(&acc), &acc.key, &ua) {
        Ok((200, body)) => {
            let ids: Vec<String> = body
                .get("data")
                .and_then(|d| d.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let ids = crate::library::rank_models(&ids, &ctx);
            json_response(
                req,
                out,
                200,
                &json!({
                    "name": acc.name,
                    "ok": true,
                    "count": ids.len(),
                    "models": ids,
                    "current": acc.model,
                }),
            );
        }
        Ok((code, body)) => json_response(
            req,
            out,
            200,
            &json!({
                "name": acc.name,
                "ok": false,
                "count": 0,
                "models": [],
                "current": acc.model,
                "error": format!("HTTP {} {}", code, util::truncate(&crate::httpclient::short_body(&body, "", 200), 200)),
            }),
        ),
        Err(e) => json_response(
            req,
            out,
            200,
            &json!({
                "name": acc.name,
                "ok": false,
                "count": 0,
                "models": [],
                "current": acc.model,
                "error": util::truncate(&e, 200),
            }),
        ),
    }
    true
}

fn library_get(state: &Arc<AppState>, req: &Request, out: &mut Responder) {
    let cfg = state.cfg();
    let lib = crate::library::load(&cfg.path);
    let stamp = crate::library::updated_stamp(&cfg.path);
    json_response(req, out, 200, &crate::library::to_json(&lib, stamp.as_deref()));
}

fn library_put(state: &Arc<AppState>, req: &Request, out: &mut Responder) {
    let doc = match body_json(req) {
        Ok(v) => v,
        Err(e) => return error_response(req, out, 400, &e),
    };
    let cfg = state.cfg();
    match crate::library::save(&cfg.path, &doc) {
        Ok(lib) => {
            log_info!("model library updated ({} entries)", lib.entries.len());
            let stamp = crate::library::updated_stamp(&cfg.path);
            json_response(req, out, 200, &crate::library::to_json(&lib, stamp.as_deref()));
        }
        Err(e) => error_response(req, out, 400, &e),
    }
}

