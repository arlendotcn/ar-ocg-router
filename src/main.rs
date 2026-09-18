//! ar-ocg-router: single-binary multi-account router for OpenCode Go + DeepSeek official API.
//!
//! Busy (peak) hours prefer the prepaid OpenCode Go subscription; idle (off-peak) hours prefer
//! the cheap DeepSeek official API unless the OpenCode Go quota would otherwise go unused.

#![allow(dead_code)]

mod api;
mod config;
mod configbackup;
mod configwrite;
mod etag;
mod httpclient;
mod httpd;
mod library;
mod logger;
mod models;
mod pricing;
mod proxy;
mod persist;
mod quota;
mod router;
mod sse;
mod service;
mod state;
#[cfg(test)]
mod tests;
mod timeutil;
mod util;
mod webui;

use std::io::Write;
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::httpd::Connection;
use crate::logger::Level;

fn usage() -> String {
    const TEMPLATE: &str = r#"ar-OCG-Router __VERSION__   (code name: ar-ocg-router)
multi-account router for OpenCode Go + DeepSeek official

Usage: ar-ocg-router [options]

Options:
  -c, --config <path>     config file (default: ./config.yaml next to the binary)
      --host <addr>       listen address (default from config, usually 127.0.0.1)
      --port <port>       listen port (default from config, usually 8787)
      --log-level <lvl>   error|warn|info|debug|trace
      --log-file <path>   append logs to a file as well
      --selftest          probe every endpoint (key, /models, quota, one tiny request) and exit
      --dump-models       fetch each endpoint's /models and write it into a reference block at the
                          top of config.yaml (informational only; the router never reads it)
                          alias: --update-models
      --plan              print the routing decision for the current time and exit
      --check             validate the config file and exit
      --dump              print the effective config summary and exit
  -V, --version           print version
  -h, --help              this help

Windows service (run from an elevated prompt):
  --install-service       register ar-ocg-router as an auto-start Windows service
                          (uses the resolved --config path and --log-file)
  --uninstall-service     stop and delete the service
  --service-status        print the service state
  --service-start | --service-stop | --service-restart
  --service-name <name>   service name to manage (default: ar-ocg-router)
  --service-display-name <text>  display name (default: ar-OCG-Router)
  --service-account <user>       run as this account (default: LocalSystem);
                                 add --service-password <pw> for user accounts
  --service               internal: this is what the Service Control Manager runs

Linux: use dist/linux-x64/install.sh (systemd unit ar-ocg-router.service).

Endpoints: POST /v1/chat/completions, POST /v1/responses, POST /v1/messages, GET /v1/models
           GET /router/stats, GET /router/schedule, POST /router/reload, GET /health

Banner text: peak = 01:00-04:00 and 06:00-10:00 UTC Mon-Fri (DeepSeek official + OpenCode Go
share the same window). Peak prefers OpenCode Go; off-peak prefers the DeepSeek official API
unless the OpenCode Go quota would otherwise go unused.
"#;
    TEMPLATE.replace("__VERSION__", proxy::VERSION)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg_path: Option<String> = None;
    let mut host: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut log_level: Option<String> = None;
    let mut log_file: Option<String> = None;
    let mut mode_plan = false;
    let mut mode_check = false;
    let mut mode_dump = false;
    let mut mode_selftest = false;
    let mut mode_dump_models = false;
    let mut mode_service = false;
    let mut mode_install = false;
    let mut mode_uninstall = false;
    let mut mode_svc_status = false;
    let mut mode_svc_start = false;
    let mut mode_svc_stop = false;
    let mut mode_svc_restart = false;
    let mut service_name: Option<String> = None;
    let mut service_display: Option<String> = None;
    let mut service_account: Option<String> = None;
    let mut service_password: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let next = |i: &mut usize| -> Option<String> {
            *i += 1;
            args.get(*i).cloned()
        };
        match a {
            "-c" | "--config" => cfg_path = next(&mut i),
            "--host" | "--bind" => host = next(&mut i),
            "--port" => {
                port = next(&mut i).and_then(|p| p.parse::<u16>().ok());
            }
            "--log-level" | "--level" => log_level = next(&mut i),
            "--log-file" => log_file = next(&mut i),
            "--plan" => mode_plan = true,
            "--selftest" | "--self-test" => mode_selftest = true,
            "--dump-models" | "--update-models" => mode_dump_models = true,
            "--check" => mode_check = true,
            "--dump" => mode_dump = true,
            "--service" => mode_service = true,
            "--install-service" => mode_install = true,
            "--uninstall-service" => mode_uninstall = true,
            "--service-status" => mode_svc_status = true,
            "--service-start" => mode_svc_start = true,
            "--service-stop" => mode_svc_stop = true,
            "--service-restart" => mode_svc_restart = true,
            "--service-name" => service_name = next(&mut i),
            "--service-display-name" => service_display = next(&mut i),
            "--service-account" => service_account = next(&mut i),
            "--service-password" => service_password = next(&mut i),
            "-V" | "--version" => {
                println!(
                    "{} {} (code name: ar-ocg-router)",
                    service::PRODUCT_NAME,
                    proxy::VERSION
                );
                return;
            }
            "-h" | "--help" => {
                print!("{}", usage());
                return;
            }
            other => {
                eprintln!("unknown option: {}", other);
                eprint!("{}", usage());
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let svc_name = service_name
        .clone()
        .unwrap_or_else(|| service::DEFAULT_SERVICE_NAME.to_string());

    // Service management does not need a valid config file.
    if mode_uninstall || mode_svc_status || mode_svc_start || mode_svc_stop || mode_svc_restart {
        logger::init(Level::Info, None, false);
        let result = if mode_uninstall {
            service::uninstall(&svc_name)
        } else if mode_svc_status {
            service::status(&svc_name)
        } else if mode_svc_start {
            service::control(&svc_name, "start")
        } else if mode_svc_stop {
            service::control(&svc_name, "stop")
        } else {
            service::control(&svc_name, "restart")
        };
        match result {
            Ok(msg) => println!("{}", msg),
            Err(e) => {
                log_error!("{}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    let explicit_config = cfg_path.is_some();
    let path = config::default_config_path(cfg_path.as_deref());
    let mut cfg = match config::load(&path) {
        Ok(c) => c,
        Err(e) => {
            logger::init(Level::Info, None, false);
            log_error!("{}", e);
            if explicit_config {
                log_error!("(--config {})", path.display());
            } else {
                log_error!("searched (in order):");
                for cand in config::search_paths() {
                    log_error!("  {}", cand.display());
                }
                log_error!("put config.yaml next to the binary, or pass --config <path>");
            }
            std::process::exit(1);
        }
    };
    if let Some(h) = host {
        cfg.server.host = h;
    }
    if let Some(p) = port {
        cfg.server.port = p;
    }
    if let Some(l) = log_level {
        cfg.log.level = l;
    }
    if let Some(f) = log_file {
        cfg.log.file = Some(f);
    }

    if mode_service {
        // No console when started by the SCM: always keep a log file, never write to stdout.
        if cfg.log.file.is_none() {
            cfg.log.file = Some(service::default_log_path().display().to_string());
        }
        cfg.log.quiet = true;
    }

    let level = Level::parse(&cfg.log.level).unwrap_or(Level::Info);
    logger::init(level, cfg.log.file.as_deref(), cfg.log.quiet);

    if mode_install {
        let abs_cfg = std::fs::canonicalize(&cfg.path).unwrap_or_else(|_| cfg.path.clone());
        let log = match cfg.log.file.clone() {
            Some(f) => Some(abs_log_path(&f)),
            None => Some(service::default_log_path()),
        };
        match service::install(
            &abs_cfg,
            log.as_deref(),
            &svc_name,
            service_display.as_deref(),
            service_account.as_deref(),
            service_password.as_deref(),
        ) {
            Ok(msg) => {
                println!("{}", msg);
                return;
            }
            Err(e) => {
                log_error!("{}", e);
                std::process::exit(1);
            }
        }
    }

    if mode_check {
        println!("config OK: {}", cfg.path.display());
        for w in &cfg.warnings {
            println!("warning: {}", w);
        }
        println!("{}", cfg.summary());
        return;
    }
    if mode_dump {
        println!("{}", cfg.summary());
        println!();
        println!("windows : {}", cfg.describe_windows());
        println!(
            "accounts: {} ({} plans, {} fallback/cash)",
            cfg.accounts.len(),
            cfg.plans().len(),
            cfg.cash().len()
        );
        for a in &cfg.accounts {
            println!(
                "  [{}#{}] {:<18} {:<45} modes={:<26} quota={}/{}{} rules={}",
                a.kind.as_str(),
                a.order,
                a.name,
                a.base(),
                a.modes.iter().map(|m| m.as_str()).collect::<Vec<_>>().join(","),
                a.quota.unit.as_str(),
                a.quota.probe.as_str(),
                if a.quota.measures_something() {
                    format!("({}/{}/{})", a.quota.rolling, a.quota.weekly, a.quota.monthly)
                } else {
                    String::new()
                },
                a.rules.iter().map(|r| r.as_str()).collect::<Vec<_>>().join(",")
            );
        }
        for w in &cfg.warnings {
            println!("warning: {}", w);
        }
        return;
    }

    let state = proxy::AppState::new(cfg);
    {
        let cfg = state.cfg();
        let path = persist::state_path(&cfg.path);
        let saved = persist::load(&path);
        // Counters shown by the console cover the lifetime of the install, not of this process:
        // a restart continues from the file instead of dropping back to zero.
        persist::restore_counters(&state, saved.counters.unwrap_or_default());
        state.router.health.restore(saved.health);
        state.registry.restore_stats(&saved.stats);
        // write once at startup so the file exists (and is known-good) even before any failure
        persist::write(&state);
    }
    persist::spawn_flusher(state.clone());
    if mode_dump_models {
        run_dump_models(&state);
        return;
    }
    if mode_selftest {
        run_selftest(&state);
        return;
    }
    if mode_plan {
        print_plan(&state);
        return;
    }

    if mode_service {
        let cfg = state.cfg();
        let _ = SERVICE_CTX.set(((cfg.as_ref()).clone(), state.clone()));
        log_info!("running as a service ({})", svc_name);
        if let Err(e) = service::dispatch(&svc_name, service_runner) {
            log_error!("{}", e);
            // There is a console when a human runs --service by mistake: say it out loud too.
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
        return;
    }

    run_server(state);
}

/// State handed to the service main thread (Config is not shareable with a bare fn pointer).
static SERVICE_CTX: std::sync::OnceLock<(config::Config, Arc<proxy::AppState>)> = std::sync::OnceLock::new();

fn service_runner() {
    match SERVICE_CTX.get() {
        Some((_, state)) => run_server(state.clone()),
        None => log_error!("service context missing"),
    }
}

/// Make a log path absolute: relative paths resolve next to the binary, because a service
/// starts with the SCM working directory (usually C:\\Windows\\System32).
fn abs_log_path(raw: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(raw);
    if p.is_absolute() {
        return p;
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(&p)))
        .unwrap_or(p)
}

/// Bind the listener and serve until the process is asked to stop.
fn run_server(state: Arc<proxy::AppState>) {
    let cfg = state.cfg();
    log_info!(
        "ar-OCG-Router {} starting (code name: ar-ocg-router)",
        proxy::VERSION
    );
    log_info!("config: {}", cfg.path.display());
    log_info!("policy: {}", cfg.summary());
    if util::has_fake_now() {
        log_warn!(
            "AR_OCG_ROUTER_FAKE_NOW is set -> time is pinned to {} for testing",
            timeutil::iso8601(util::now_secs())
        );
    }
    for w in &cfg.warnings {
        log_warn!("config warning: {}", w);
    }

    proxy::spawn_refreshers(state.clone());
    proxy::spawn_config_watcher(state.clone());

    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            log_error!("cannot bind {}: {}", addr, e);
            std::process::exit(1);
        }
    };
    log_info!(
        "listening on http://{} (max_connections={})",
        addr,
        cfg.server.max_connections
    );
    log_info!(
        "point your client at http://{}/v1 (OpenAI base URL) or http://{} (also accepts /v1 prefixes)",
        addr,
        addr
    );
    let sched = timeutil::eval_schedule(util::now_secs(), &cfg.router.peak_windows);
    log_info!(
        "now={} state={} next_change={} (in {}s)",
        timeutil::iso8601(util::now_secs()),
        if sched.is_peak { "PEAK" } else { "OFF-PEAK" },
        timeutil::iso8601(sched.next_change),
        (sched.next_change - util::now_secs()).max(0)
    );

    let active = Arc::new(AtomicUsize::new(0));
    let max_conns = cfg.server.max_connections;
    let idle = cfg.server.idle_timeout_secs;
    let req_timeout = cfg.server.read_timeout_secs;
    let max_body = cfg.server.max_body_bytes;

    // Non-blocking accept so a stop request is noticed promptly; accepted sockets are put
    // back into blocking mode explicitly (Windows inherits the listening socket mode).
    if let Err(e) = listener.set_nonblocking(true) {
        log_warn!("cannot switch the listener to non-blocking mode: {}", e);
    }

    while !service::is_shutdown() {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let _ = stream.set_nonblocking(false);
                if active.load(Ordering::Relaxed) >= max_conns {
                    log_warn!("connection limit reached ({}), rejecting", max_conns);
                    let mut s = stream;
                    let _ = s.write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    continue;
                }
                active.fetch_add(1, Ordering::Relaxed);
                let st = state.clone();
                let counter = active.clone();
                let _ = std::thread::Builder::new()
                    .name("conn".to_string())
                    .spawn(move || {
                        serve_connection(st, stream, idle, req_timeout, max_body);
                        counter.fetch_sub(1, Ordering::Relaxed);
                    });
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            Err(e) => {
                log_warn!("accept failed: {}", e);
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        }
    }
    persist::flush_now(&state);
    log_info!("ar-OCG-Router stopped (shutdown requested)");
}

fn serve_connection(
    state: Arc<proxy::AppState>,
    stream: std::net::TcpStream,
    idle: u64,
    req_timeout: u64,
    max_body: usize,
) {
    let mut conn = match Connection::new(stream, idle, req_timeout) {
        Ok(c) => c,
        Err(e) => {
            log_warn!("connection setup failed: {}", e);
            return;
        }
    };
    loop {
        match conn.read_request(max_body) {
            Ok(Some(req)) => {
                let keep_alive = req.keep_alive;
                let mut responder = match conn.responder() {
                    Ok(r) => r,
                    Err(e) => {
                        log_warn!("responder failed: {}", e);
                        return;
                    }
                };
                proxy::handle(&state, &req, &mut responder);
                if !keep_alive {
                    return;
                }
            }
            Ok(None) => return,
            Err(e) => {
                if let Ok(mut responder) = conn.responder() {
                    let body = serde_json::json!({
                        "error": {"message": e.message, "type": "bad_request", "code": e.status}
                    });
                    let _ = responder.send_json(e.status, &body, false, "HTTP/1.1");
                }
                return;
            }
        }
    }
}

/// Print the current routing decision (for verification without sending traffic).
fn print_plan(state: &Arc<proxy::AppState>) {
    let cfg = state.cfg();
    let now = util::now_secs();
    let sched = timeutil::eval_schedule(now, &cfg.router.peak_windows);
    let accounts = state.account_list();
    println!("now        : {}", timeutil::iso8601(now));
    println!(
        "state      : {} (next change {} in {}s)",
        if sched.is_peak { "PEAK" } else { "OFF-PEAK" },
        timeutil::iso8601(sched.next_change),
        (sched.next_change - now).max(0)
    );
    println!("windows    : {}", cfg.describe_windows());
    println!("policy     : {}", cfg.summary().split(" | ").nth(1).unwrap_or(""));
    for model in ["deepseek-flash", "deepseek-v4.1-flash", "deepseek-v4-pro"] {
        for (name, endpoint) in [
            ("chat/completions", models::Endpoint::Chat),
            ("responses", models::Endpoint::Responses),
        ] {
            let plan = state.router.plan(&cfg, &accounts, endpoint, None, None, now);
            println!(
                "{:<22} {:<17} -> {}  [{}]",
                model,
                name,
                if plan.candidates.is_empty() {
                    "NO CANDIDATE".to_string()
                } else {
                    plan.candidates
                        .iter()
                        .map(|a| a.name().to_string())
                        .collect::<Vec<_>>()
                        .join(" > ")
                },
                plan.reason
            );
        }
    }
    if !accounts.is_empty() {
        println!();
        println!("accounts:");
        for acc in accounts.iter() {
            let report = acc.rt.quota_report(now, &cfg, &acc.cfg.quota);
            println!(
                "  [{}] {:<18} rules={} quota={} exhausted={} surplus={} cooldown={}s",
                acc.cfg.kind.as_str(),
                acc.name(),
                acc.cfg
                    .rules
                    .iter()
                    .map(|r| r.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                report.source.as_str(),
                report.exhausted,
                report.surplus,
                acc.rt.cooldown_left(now)
            );
        }
    }
}

/// Evidence-based suggestions for the compat/quota knobs of one endpoint.
/// The router never enables anything by itself: this only prints what was observed.
fn probe_suggestions(
    agent: &ureq::Agent,
    acc: &std::sync::Arc<state::Account>,
    cfg: &config::Config,
) -> Vec<String> {
    let mut out = Vec::new();
    let url = acc.cfg.upstream_url("/v1/chat/completions");
    // Mirror what a real request looks like: session header when the endpoint asks for one.
    let mut base_headers: Vec<(String, String)> = Vec::new();
    if acc.cfg.inject_session {
        // a stable probe session so both cache attempts share one upstream session scope
        base_headers.push(("x-opencode-session".to_string(), util::gen_session_id()));
    }
    for (k, v) in &acc.cfg.extra_headers {
        base_headers.push((k.clone(), v.clone()));
    }
    let body = serde_json::json!({
        "model": acc.cfg.model,
        "messages": [{"role": "user", "content": "Reply with exactly: ok"}],
        "max_tokens": 8,
        "stream": false,
    });
    // 1) does the endpoint report usage at all?
    let mut call = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {}", acc.cfg.key))
        .set("User-Agent", &cfg.router.user_agent);
    for (k, v) in &base_headers {
        call = call.set(k.as_str(), v.as_str());
    }
    match call.send_bytes(&serde_json::to_vec(&body).unwrap_or_default()) {
        Ok(resp) if (200..300).contains(&resp.status()) => {
            let text = resp.into_string().unwrap_or_default();
            let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
            let u = crate::sse::extract_usage(&v);
            if !u.seen {
                out.push(
                    "该端点不返回 usage —— 额度/缓存无法观测，quota.probe 只能填 none".to_string(),
                );
            } else {
                out.push(format!(
                    "usage 可用: in={} out={} cached={}",
                    u.prompt, u.completion, u.cached
                ));
            }
            // 2) prompt cache: same request twice, look at cached_tokens the 2nd time
            let mut second = agent
                .post(&url)
                .set("Content-Type", "application/json")
                .set("Authorization", &format!("Bearer {}", acc.cfg.key))
                .set("User-Agent", &cfg.router.user_agent);
            for (k, v) in &base_headers {
                second = second.set(k.as_str(), v.as_str());
            }
            // a longer shared prefix so a cacheable block exists at all
            let filler = "cache probe line: the quick brown fox jumps over the lazy dog.\n".repeat(120);
            let body2 = serde_json::json!({
                "model": acc.cfg.model,
                "messages": [
                    {"role": "system", "content": filler},
                    {"role": "user", "content": "Reply with exactly: ok"}
                ],
                "max_tokens": 8,
                "stream": false,
            });
            let _ = second.send_bytes(&serde_json::to_vec(&body2).unwrap_or_default());
            let mut third = agent
                .post(&url)
                .set("Content-Type", "application/json")
                .set("Authorization", &format!("Bearer {}", acc.cfg.key))
                .set("User-Agent", &cfg.router.user_agent);
            for (k, v) in &base_headers {
                third = third.set(k.as_str(), v.as_str());
            }
            if let Ok(resp3) = third.send_bytes(&serde_json::to_vec(&body2).unwrap_or_default()) {
                if (200..300).contains(&resp3.status()) {
                    let t3 = resp3.into_string().unwrap_or_default();
                    let v3: serde_json::Value =
                        serde_json::from_str(&t3).unwrap_or(serde_json::Value::Null);
                    let u3 = crate::sse::extract_usage(&v3);
                    let label = crate::proxy::cache_label(u3.prompt, u3.cached);
                    out.push(format!(
                        "缓存实测: 第 2 次同前缀请求 cached={}/{} ({})",
                        u3.cached, u3.prompt, label
                    ));
                }
            }
        }
        Ok(resp) => {
            let code = resp.status();
            let text = resp.into_string().unwrap_or_default();
            let lower = text.to_ascii_lowercase();
            if code == 400 && (lower.contains("max_completion_tokens") || lower.contains("max_tokens"))
            {
                out.push(
                    "建议 compat.max_completion_tokens_to_max_tokens: true —— 该端点拒绝 max_tokens 形态"
                        .to_string(),
                );
            }
            if code == 400 && lower.contains("developer") {
                out.push(
                    "建议 compat.developer_role_to_system: true —— 该端点不接受 role: developer"
                        .to_string(),
                );
            }
            out.push(format!("探测请求返回 HTTP {}: {}", code, util::truncate(&text, 160)));
        }
        Err(e) => out.push(format!("探测失败: {}", util::truncate(&format!("{}", e), 160))),
    }
    out
}

/// --dump-models: fetch every endpoint's /models and write the raw listing into a reference
/// block at the TOP of config.yaml. The router itself never reads that block; it exists so the
/// person editing the config can see which ids the provider actually advertises.
fn run_dump_models(state: &Arc<proxy::AppState>) {
    let cfg = state.cfg();
    let now = util::now_secs();
    let agent = httpclient::agent(10, 30, 30);
    let ua = cfg.router.user_agent.clone();
    let mut entries: Vec<(String, String, String, Vec<String>, Option<String>)> = Vec::new();
    println!("ar-OCG-Router {} --dump-models", proxy::VERSION);
    println!("config: {}", cfg.path.display());
    println!();
    for acc in state.account_list().iter() {
        let url = quota::models_url(&acc.cfg);
        let (ids, err) = match httpclient::get_json(&agent, &url, &acc.cfg.key, &ua) {
            Ok((200, body)) => {
                let ids = body
                    .get("data")
                    .and_then(|d| d.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|m| {
                                m.get("id").and_then(|x| x.as_str()).map(|s| s.to_string())
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                (ids, None)
            }
            Ok((code, body)) => (
                Vec::new(),
                Some(format!("HTTP {}: {}", code, util::truncate(&crate::util::json_compact(&body), 120))),
            ),
            Err(e) => (Vec::new(), Some(util::truncate(&e, 160))),
        };
        println!(
            "[{}#{}] {}  {}/{}",
            acc.cfg.kind.as_str(),
            acc.cfg.order,
            acc.cfg.name,
            acc.cfg.provider.as_str(),
            acc.cfg.model
        );
        println!("    url      : {}", acc.cfg.base());
        match &err {
            Some(e) => println!("    /models  : FAILED - {}", e),
            None => {
                println!("    /models  : {} id(s)", ids.len());
                if !ids.is_empty() {
                    println!("    ids      : {}", util::truncate(&ids.join(", "), 400));
                }
            }
        }
        println!();
        entries.push((acc.cfg.name.clone(), acc.cfg.url.clone(), acc.cfg.model.clone(), ids, err));
    }
    // ---- write the reference block
    let path = cfg.path.clone();
    match write_models_block(&path, &entries, now) {
        Ok(msg) => println!("{}", msg),
        Err(e) => {
            log_error!("cannot update {}: {}", path.display(), e);
            std::process::exit(1);
        }
    }
}

const MODELS_BLOCK_BEGIN: &str = "# === BEGIN generated by --dump-models (do not edit) ===";
const MODELS_BLOCK_END: &str = "# === END generated by --dump-models ===";

/// Replace (or insert at the very top) the informational models block, leaving every other byte
/// of the file - including all comments and formatting - untouched.
fn write_models_block(
    path: &std::path::Path,
    entries: &[(String, String, String, Vec<String>, Option<String>)],
    now: i64,
) -> Result<String, String> {
    let original = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut block = String::new();
    block.push_str(MODELS_BLOCK_BEGIN);
    block.push('\n');
    block.push_str(&format!(
        "# fetched: {}   (informational only: the router never reads this block)\n",
        timeutil::iso8601(now)
    ));
    block.push_str("# each endpoint serves exactly ONE model; that id is set with \"model:\" on the endpoint.\n");
    for (name, url, current, ids, err) in entries {
        block.push_str(&format!("#\n# {}\n#   url      : {}\n#   in use   : {}\n", name, url, current));
        match err {
            Some(e) => block.push_str(&format!("#   /models  : FAILED - {}\n", e)),
            None => {
                block.push_str(&format!("#   /models  : {} id(s)\n", ids.len()));
                for chunk in ids.chunks(6) {
                    block.push_str(&format!("#     {}\n", chunk.join(", ")));
                }
            }
        }
    }
    block.push_str(MODELS_BLOCK_END);
    let body = match (original.find(MODELS_BLOCK_BEGIN), original.find(MODELS_BLOCK_END)) {
        (Some(a), Some(b)) if b > a => {
            let mut out = String::with_capacity(original.len());
            out.push_str(&original[..a]);
            out.push_str(&block);
            out.push_str(&original[b + MODELS_BLOCK_END.len()..]);
            out
        }
        _ => {
            // Insert right after the leading banner comment block (the run of lines that start
            // with '#'), so the title/banner stays at the very top, then a blank line, then the
            // generated block, then the rest of the file.
            let mut split_at = 0usize;
            for (idx, _line) in original.match_indices('\n') {
                let s = &original[..idx];
                let last = s.rsplit('\n').next().unwrap_or("");
                if last.trim_start().starts_with('#') || last.trim().is_empty() {
                    split_at = idx + 1;
                } else {
                    break;
                }
            }
            let (head, tail) = original.split_at(split_at);
            format!("{}\n{}\n{}", head.trim_end(), block, tail.trim_start_matches('\n'))
        }
    };
    // atomic write: temp file in the same directory, then rename over the original
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, body.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(format!(
        "updated {} ({} endpoint(s) recorded between the BEGIN/END markers)",
        path.display(),
        entries.len()
    ))
}

/// Real upstream self-test: connectivity + auth + catalog + quota + one minimal request.
fn run_selftest(state: &Arc<proxy::AppState>) {
    let cfg = state.cfg();
    let now = util::now_secs();
    let sched = timeutil::eval_schedule(now, &cfg.router.peak_windows);
    println!("ar-OCG-Router {} self-test", proxy::VERSION);
    println!("config     : {}", cfg.path.display());
    println!("now        : {}", timeutil::iso8601(now));
    println!(
        "schedule   : {} (next change {} in {}s)",
        if sched.is_peak { "PEAK" } else { "OFF-PEAK" },
        timeutil::iso8601(sched.next_change),
        (sched.next_change - now).max(0)
    );
    println!("windows    : {}", cfg.describe_windows());
    println!();
    let agent = httpclient::agent(10, 30, 30);
    let ua = cfg.router.user_agent.clone();
    let mut ok = 0usize;
    let mut failed = 0usize;
    for acc in state.account_list().iter() {
        println!("[{}#{}] {} ({})", acc.cfg.kind.as_str(), acc.cfg.order, acc.cfg.name, acc.cfg.base());
        println!("    key      : {} ({} chars)", util::redact(&acc.cfg.key), acc.cfg.key.len());
        // ---- catalog
        let started = std::time::Instant::now();
        #[allow(unused_assignments)]
        let mut listed: Vec<String> = Vec::new();
        match httpclient::get_json(&agent, &quota::models_url(&acc.cfg), &acc.cfg.key, &ua) {
            Ok((200, body)) => {
                listed = body
                    .get("data")
                    .and_then(|d| d.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|m| m.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                println!(
                    "    /models  : 200 OK, {} models, {} ms",
                    listed.len(),
                    started.elapsed().as_millis()
                );
            }
            Ok((code, body)) => {
                failed += 1;
                println!(
                    "    /models  : HTTP {} -> {}",
                    code,
                    util::truncate(&httpclient::short_body(&body, "", 200), 200)
                );
            }
            Err(e) => {
                failed += 1;
                println!("    /models  : transport error -> {}", util::truncate(&e, 200));
            }
        }
        // ---- quota / balance
        if acc.cfg.quota.probe == crate::config::QuotaProbe::Usage {
            let status = quota::refresh_usage(&agent, &acc.cfg, &acc.rt, now, &ua);
            let report = acc.rt.quota_report(now, &cfg, &acc.cfg.quota);
            println!(
                "    /usage   : {} | rolling {:.0}% weekly {:.0}% monthly {:.0}% | source={} exhausted={} surplus={}",
                status,
                report.rolling.pct,
                report.weekly.pct,
                report.monthly.pct,
                report.source.as_str(),
                report.exhausted,
                report.surplus
            );
        }
        if acc.cfg.quota.probe == crate::config::QuotaProbe::Balance {
            let status = quota::refresh_balance(&agent, &acc.cfg, &acc.rt, now, &ua);
            println!("    /balance : {}", status);
        }
        // ---- one minimal real request through the account's own protocol
        let model = acc.cfg.model.clone();
        let body = serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 8,
            "stream": false
        });
        let url = acc.cfg.upstream_url("/v1/chat/completions");
        let mut call = agent
            .post(&url)
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {}", acc.cfg.key))
            .set("User-Agent", &ua);
        if acc.cfg.inject_session {
            call = call.set("x-opencode-session", &util::gen_session_id());
        }
        let started = std::time::Instant::now();
        match call.send_bytes(&serde_json::to_vec(&body).unwrap_or_default()) {
            Ok(resp) if (200..300).contains(&resp.status()) => {
                let text = resp.into_string().unwrap_or_default();
                let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
                let usage = crate::sse::extract_usage(&v);
                ok += 1;
                println!(
                    "    /chat    : 200 OK model={} ({} ms) tokens in={} out={}",
                    v.get("model").and_then(|m| m.as_str()).unwrap_or("-"),
                    started.elapsed().as_millis(),
                    usage.prompt,
                    usage.completion
                );
            }
            Ok(resp) => {
                failed += 1;
                let code = resp.status();
                let text = resp.into_string().unwrap_or_default();
                println!("    /chat    : HTTP {} -> {}", code, util::truncate(&text, 240));
            }
            Err(ureq::Error::Status(code, resp)) => {
                failed += 1;
                let text = resp.into_string().unwrap_or_default();
                println!("    /chat    : HTTP {} -> {}", code, util::truncate(&text, 240));
            }
            Err(e) => {
                failed += 1;
                println!("    /chat    : transport error -> {}", util::truncate(&format!("{}", e), 240));
            }
        }
        // evidence-based suggestions (never applied automatically)
        for s in probe_suggestions(&agent, acc, &cfg) {
            println!("    建议     : {}", s);
        }
        println!();
    }
    println!(
        "result: {} account(s) fully working, {} with failures",
        ok, failed
    );
    if failed > 0 {
        println!();
        println!("hints:");
        println!("  * 401/403 -> key 不对，或用了错误的产品前缀（Go 必须是 /zen/go/v1）");
        println!("  * 400 MissingSessionID -> 客户端缺少 x-opencode-session（本程序会自动带上）");
        println!("  * 10013 / WSAEACCES / connection refused -> 出网被沙箱或防火墙拦截，请换普通终端运行");
        std::process::exit(1);
    }
}
