//! Writing config.yaml back out (admin Web UI).
//!
//! Round-trip rule: whatever this module produces must parse back into the same Config with
//! config::parse. That is why the body is built as a serde_yaml::Mapping and rendered by hand
//! instead of by serde_yaml: the top-level sections must come out in a fixed order, and
//! serde_yaml only keeps insertion order because its Mapping happens to be backed by an
//! IndexMap (a default-feature implementation detail, not a documented promise).
//!
//! Only the config body is generated. Hand-written comments in the file are NOT preserved -
//! only the leading comment banner is carried over (read_header).

use std::path::Path;

use serde_yaml::{Mapping, Value as Y};

use crate::config::{
    AccountCfg, AccountKind, CompatCfg, Config, IdlePrefer, LogCfg, QuotaCfg, RouterCfg,
    RouterMode, Rule, ServerCfg,
};
use crate::models::{Mode, ProviderKind};

/// Comment banner used when the config file has no header of its own.
pub const DEFAULT_HEADER: &str = "\
# ar-OCG-Router config.yaml
# Written by ar-OCG-Router: hand-written comments in the body are NOT preserved (back up first).
# Values can still be edited by hand; the next save rewrites the body from the parsed config.
# One endpoint = one merchant + one model: set \"url:\" and the exact \"model:\" id it serves.
# Nothing is translated or guessed: a wrong model id fails loudly at the upstream.
# Default peak windows: Mon-Fri 01:00-04:00 and 06:00-10:00 UTC (DeepSeek official + OpenCode Go
# share the same window); peak prefers the prepaid plans, off-peak the cheap cash accounts.
# Quota windows are 5h rolling / weekly / monthly, in the unit given by quota.unit.
";

// ---------------------------------------------------------------- scalars

fn ystr(s: &str) -> Y {
    Y::String(s.to_string())
}

fn ybool(b: bool) -> Y {
    Y::Bool(b)
}

fn yu64(v: u64) -> Y {
    Y::Number(serde_yaml::Number::from(v))
}

fn yusize(v: usize) -> Y {
    Y::Number(serde_yaml::Number::from(v as u64))
}

fn yi64(v: i64) -> Y {
    Y::Number(serde_yaml::Number::from(v))
}

/// Floats that are whole numbers are written out as integers (12.0 -> 12), which is what a
/// hand-written config would say; the parser reads both forms identically.
fn yfloat(v: f64) -> Y {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.0e15 {
        Y::Number(serde_yaml::Number::from(v as i64))
    } else if v.is_finite() {
        Y::Number(serde_yaml::Number::from(v))
    } else {
        // YAML has no infinity/NaN literal the parser accepts; keep it loadable.
        Y::Number(serde_yaml::Number::from(0))
    }
}

fn yseq_str<I: IntoIterator<Item = S>, S: AsRef<str>>(items: I) -> Y {
    Y::Sequence(items.into_iter().map(|s| ystr(s.as_ref())).collect())
}

fn ymap(pairs: Vec<(&str, Y)>) -> Y {
    let mut m = Mapping::new();
    for (k, v) in pairs {
        m.insert(ystr(k), v);
    }
    Y::Mapping(m)
}

// ---------------------------------------------------------------- enums

fn router_mode_str(m: RouterMode) -> &'static str {
    match m {
        RouterMode::Auto => "auto",
        RouterMode::Plans => "plans",
        RouterMode::Fallback => "fallback",
    }
}

fn idle_prefer_str(p: IdlePrefer) -> &'static str {
    match p {
        IdlePrefer::SurplusFirst => "surplus_first",
        IdlePrefer::Plans => "plans",
        IdlePrefer::Fallback => "fallback",
    }
}

/// ","-joined Mode::as_str() list, with exactly {Chat, Responses} collapsed to "both".
fn modes_str(modes: &[Mode]) -> String {
    if modes == [Mode::Chat, Mode::Responses] {
        return "both".to_string();
    }
    modes.iter().map(|m| m.as_str()).collect::<Vec<_>>().join(",")
}

fn quota_map(q: &QuotaCfg) -> Y {
    let mut m = vec![
        ("unit", ystr(q.unit.as_str())),
        ("rolling", yfloat(q.rolling)),
        ("weekly", yfloat(q.weekly)),
        ("monthly", yfloat(q.monthly)),
        ("probe", ystr(q.probe.as_str())),
        ("refresh_secs", yu64(q.refresh_secs)),
    ];
    // Written only when set, so an endpoint the user never calibrated keeps the short form and the
    // file stays readable. A cycle day of 0 is the documented "no cycle" value, not a thing to
    // spell out.
    if q.has_cycle() {
        m.push(("cycle_day", yu64(q.cycle_day as u64)));
    }
    ymap(m)
}

/// True when the rules are exactly the parser default (a single Rule::Always).
fn rules_are_default(rules: &[Rule]) -> bool {
    rules.len() == 1 && rules[0] == Rule::Always
}

// ---------------------------------------------------------------- accounts

/// One account as the canonical YAML mapping the parser accepts.
///
/// compat_drop contains compat.drop_params: the parser merges them into every account, so they
/// must not be written a second time on the account itself.
pub fn account_to_yaml(a: &AccountCfg, compat_drop: &[String]) -> Y {
    let mut m = Mapping::new();
    // Always present.
    m.insert(ystr("name"), ystr(&a.name));
    m.insert(ystr("url"), ystr(&a.url));
    // Only when it differs from what the URL already implies.
    let detected = ProviderKind::detect(&a.url);
    if a.provider != detected {
        m.insert(ystr("provider"), ystr(a.provider.as_str()));
    }
    m.insert(ystr("key"), ystr(&a.key));
    m.insert(ystr("model"), ystr(&a.model));
    m.insert(ystr("mode"), ystr(&modes_str(&a.modes)));
    // Written for transparency (a human can read the sequence off the file), but it is derived:
    // the console renumbers it from the row position on every save, and it is never edited by hand.
    m.insert(ystr("order"), Y::Number(serde_yaml::Number::from(a.order)));
    // Rules only matter for cash accounts; plans use them implicitly.
    if a.kind == AccountKind::Cash && !rules_are_default(&a.rules) {
        m.insert(ystr("rule"), yseq_str(a.rules.iter().map(|r| r.as_str())));
    }
    m.insert(ystr("quota"), quota_map(&a.quota));
    if a.inject_session {
        m.insert(ystr("inject_session"), ybool(true));
    }
    if !a.extra_headers.is_empty() {
        m.insert(
            ystr("headers"),
            ymap(a.extra_headers.iter().map(|(k, v)| (k.as_str(), ystr(v))).collect()),
        );
    }
    // The parser folds the global compat.drop_params into every account, so writing the raw
    // field back would duplicate them (and would double the compat list on the next load).
    let own_drop: Vec<String> = a
        .drop_params
        .iter()
        .filter(|p| !compat_drop.iter().any(|c| c == *p))
        .cloned()
        .collect();
    if !own_drop.is_empty() {
        m.insert(ystr("drop_params"), yseq_str(&own_drop));
    }
    Y::Mapping(m)
}

// ---------------------------------------------------------------- document

/// The rendered config document: leading comment banner + YAML body.
pub struct ConfigDoc {
    pub header: String,
    pub body: String,
}

fn server_yaml(s: &ServerCfg) -> Y {
    let mut m = Mapping::new();
    m.insert(ystr("host"), ystr(&s.host));
    m.insert(ystr("port"), Y::Number(serde_yaml::Number::from(s.port as u64)));
    m.insert(ystr("max_connections"), yusize(s.max_connections));
    if !s.client_keys.is_empty() {
        m.insert(ystr("client_keys"), yseq_str(&s.client_keys));
    }
    m.insert(ystr("max_body_bytes"), yusize(s.max_body_bytes));
    m.insert(ystr("idle_timeout_secs"), yu64(s.idle_timeout_secs));
    m.insert(ystr("read_timeout_secs"), yu64(s.read_timeout_secs));
    m.insert(ystr("stream"), ybool(s.stream));
    Y::Mapping(m)
}

fn log_yaml(l: &LogCfg) -> Y {
    let mut m = Mapping::new();
    m.insert(ystr("level"), ystr(&l.level));
    if let Some(f) = l.file.as_ref().filter(|f| !f.trim().is_empty()) {
        m.insert(ystr("file"), ystr(f));
    }
    if l.quiet {
        m.insert(ystr("quiet"), ybool(true));
    }
    // Only written when set: this is a debugging switch, and a config file is easier to read when
    // the rare options are not spelled out at their defaults.
    if l.dump_error_request {
        m.insert(ystr("dump_error_request"), ybool(true));
    }
    Y::Mapping(m)
}

fn router_yaml(r: &RouterCfg) -> Y {
    ymap(vec![
        ("mode", ystr(router_mode_str(r.mode))),
        ("peak_windows", ystr(&r.peak_spec)),
        ("idle_prefer", ystr(idle_prefer_str(r.idle_prefer))),
        ("surplus_max_pct", yfloat(r.surplus_max_pct)),
        ("surplus_projection", ybool(r.surplus_projection)),
        ("exhaust_at_pct", yfloat(r.exhaust_at_pct)),
        ("quota_refresh_secs", yu64(r.quota_refresh_secs)),
        ("cooldown_secs", yu64(r.cooldown_secs)),
        ("auth_cooldown_secs", yu64(r.auth_cooldown_secs)),
        ("server_error_cooldown_secs", yu64(r.server_error_cooldown_secs)),
        ("retry_on_model_error", ybool(r.retry_on_model_error)),
        ("rate_limit_retries", Y::Number(serde_yaml::Number::from(r.rate_limit_retries as u64))),
        ("rate_retry_delay_ms", yu64(r.rate_retry_delay_ms)),
        ("skip_after_failures", Y::Number(serde_yaml::Number::from(r.skip_after_failures as u64))),
        ("skip_secs", yu64(r.skip_secs)),
        ("attempt_budget_secs", yu64(r.attempt_budget_secs)),
        ("session_affinity", ybool(r.session_affinity)),
        ("session_affinity_ttl_secs", yu64(r.session_affinity_ttl_secs)),
        ("session_fallback", ystr(r.session_fallback.as_str())),
        ("inject_stream_usage", ybool(r.inject_stream_usage)),
        ("session_headers", yseq_str(&r.session_headers)),
        ("user_agent", ystr(&r.user_agent)),
        ("request_timeout_secs", yu64(r.request_timeout_secs)),
        ("stream_idle_timeout_secs", yu64(r.stream_idle_timeout_secs)),
    ])
}

fn compat_yaml(c: &CompatCfg) -> Y {
    ymap(vec![
        ("developer_role_to_system", ybool(c.developer_role_to_system)),
        (
            "max_completion_tokens_to_max_tokens",
            ybool(c.max_completion_tokens_to_max_tokens),
        ),
        ("drop_params", yseq_str(&c.drop_params)),
        ("forward_headers", yseq_str(&c.forward_headers)),
    ])
}

fn accounts_of(cfg: &Config, kind: AccountKind) -> Vec<&AccountCfg> {
    cfg.accounts.iter().filter(|a| a.kind == kind).collect()
}

/// Build the document. Top-level order is fixed: server, log, router, compat, plans, fallback.
pub fn build_doc(cfg: &Config, header: &str) -> ConfigDoc {
    let mut root = Mapping::new();
    root.insert(ystr("server"), server_yaml(&cfg.server));
    root.insert(ystr("log"), log_yaml(&cfg.log));
    root.insert(ystr("router"), router_yaml(&cfg.router));
    root.insert(ystr("compat"), compat_yaml(&cfg.compat));
    root.insert(
        ystr("plans"),
        Y::Sequence(
            accounts_of(cfg, AccountKind::Plans)
                .iter()
                .map(|a| account_to_yaml(a, &cfg.compat.drop_params))
                .collect(),
        ),
    );
    root.insert(
        ystr("fallback"),
        Y::Sequence(
            accounts_of(cfg, AccountKind::Cash)
                .iter()
                .map(|a| account_to_yaml(a, &cfg.compat.drop_params))
                .collect(),
        ),
    );
    ConfigDoc {
        header: header.to_string(),
        body: render(&Y::Mapping(root)),
    }
}

// ---------------------------------------------------------------- yaml text writer
// Hand-rolled so the key order above is guaranteed regardless of serde_yaml's internals.
// Strings are always double-quoted, so no scalar can be mistaken for a key or a comment.

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn quoted(s: &str) -> String {
    format!("\"{}\"", escape(s))
}

fn scalar(v: &Y) -> String {
    match v {
        Y::Null => "null".to_string(),
        Y::Bool(b) => b.to_string(),
        Y::Number(n) => n.to_string(),
        Y::String(s) => quoted(s),
        _ => quoted(""),
    }
}

fn indent(n: usize) -> String {
    "  ".repeat(n)
}

fn render(v: &Y) -> String {
    let mut out = String::new();
    match v {
        Y::Mapping(m) => render_map(m, 0, &mut out),
        Y::Sequence(s) => render_seq(s, 0, &mut out),
        other => {
            out.push_str(&scalar(other));
            out.push('\n');
        }
    }
    out
}

fn render_map(m: &Mapping, depth: usize, out: &mut String) {
    if m.is_empty() {
        out.push_str(&format!("{}{{}}\n", indent(depth)));
        return;
    }
    for (k, v) in m.iter() {
        let key = scalar(k);
        match v {
            Y::Sequence(seq) if seq.is_empty() => {
                out.push_str(&format!("{}{}: []\n", indent(depth), key));
            }
            Y::Sequence(seq) => {
                out.push_str(&format!("{}{}:\n", indent(depth), key));
                render_seq(seq, depth + 1, out);
            }
            Y::Mapping(inner) if inner.is_empty() => {
                out.push_str(&format!("{}{}: {{}}\n", indent(depth), key));
            }
            Y::Mapping(inner) => {
                out.push_str(&format!("{}{}:\n", indent(depth), key));
                render_map(inner, depth + 1, out);
            }
            other => {
                out.push_str(&format!("{}{}: {}\n", indent(depth), key, scalar(other)));
            }
        }
    }
}

fn render_seq(seq: &[Y], depth: usize, out: &mut String) {
    for item in seq {
        match item {
            Y::Mapping(m) if !m.is_empty() => {
                let mut buf = String::new();
                render_map(m, depth + 1, &mut buf);
                let body = buf.strip_prefix(&indent(depth + 1)).unwrap_or(&buf);
                out.push_str(&format!("{}- {}\n", indent(depth), body.trim_end_matches('\n')));
            }
            other => {
                out.push_str(&format!("{}- {}\n", indent(depth), scalar(other)));
            }
        }
    }
}

// ---------------------------------------------------------------- text + save

/// Header comment block, one blank line, then the YAML body, ending in exactly one newline.
pub fn to_text(doc: &ConfigDoc) -> String {
    let mut out = String::new();
    let header = doc.header.trim_end_matches('\n');
    if !header.trim().is_empty() {
        out.push_str(header);
        out.push('\n');
        out.push('\n');
    }
    out.push_str(doc.body.trim_end_matches('\n'));
    out.push('\n');
    out
}

/// Leading run of lines that are comments ('#') or blank; DEFAULT_HEADER when there is none.
pub fn read_header(path: &Path) -> String {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return DEFAULT_HEADER.to_string(),
    };
    read_header_from(&text)
}

fn read_header_from(text: &str) -> String {
    let mut head = String::new();
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let body = body.strip_suffix('\r').unwrap_or(body);
        if body.trim_start().starts_with('#') || body.trim().is_empty() {
            head.push_str(line);
        } else {
            break;
        }
    }
    let head = head.trim_end_matches(['\n', '\r', ' ', '\t']);
    if head.trim().is_empty() {
        DEFAULT_HEADER.to_string()
    } else {
        head.to_string()
    }
}

/// Atomic replace, same discipline as persist::save: temp file in the same directory, fsync,
/// rename over the target.
pub fn save(path: &Path, cfg: &Config, header: &str) -> Result<(), String> {
    let text = to_text(&build_doc(cfg, header));
    let tmp = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => path.with_extension(format!("{}.tmp", ext)),
        None => path.with_extension("tmp"),
    };
    {
        use std::io::Write;
        let mut f =
            std::fs::File::create(&tmp).map_err(|e| format!("cannot write {}: {}", tmp.display(), e))?;
        f.write_all(text.as_bytes())
            .map_err(|e| format!("cannot write {}: {}", tmp.display(), e))?;
        f.sync_all()
            .map_err(|e| format!("cannot flush {}: {}", tmp.display(), e))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {}", path.display(), e))?;
    crate::log_info!(
        "wrote config {} ({} bytes, {} account(s))",
        path.display(),
        text.len(),
        cfg.accounts.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// Unique scratch directory per test: temp_dir + pid + clock + counter.
    fn tmpdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ar-ocg-cfgwrite-{}-{}-{}",
            std::process::id(),
            crate::util::real_now_secs(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Exercises every field this module writes: two plans with different providers, one cash
    /// account with rules and headers.
    const FIXTURE: &str = r#"# hand written banner
# second line
server:
  host: 0.0.0.0
  port: 9012
  max_connections: 12
  client_keys:
    - "sk-a"
    - "sk-b"
  max_body_bytes: 1048576
  idle_timeout_secs: 111
  read_timeout_secs: 222
  stream: false
log:
  level: debug
  quiet: true
router:
  mode: fallback
  peak_windows: "Mon-Fri 01:00-04:00"
  idle_prefer: plans
  surplus_max_pct: 55
  surplus_projection: false
  exhaust_at_pct: 88
  quota_refresh_secs: 91
  cooldown_secs: 7
  auth_cooldown_secs: 8
  server_error_cooldown_secs: 9
  retry_on_model_error: false
  skip_after_failures: 4
  skip_secs: 33
  attempt_budget_secs: 44
  session_affinity: false
  session_affinity_ttl_secs: 55
  session_fallback: per-request
  inject_stream_usage: true
  session_headers:
    - x-hdr-one
    - x-hdr-two
  user_agent: "fixture-agent"
  request_timeout_secs: 66
  stream_idle_timeout_secs: 77
compat:
  developer_role_to_system: true
  max_completion_tokens_to_max_tokens: true
  drop_params:
    - n
    - logprobs
  forward_headers:
    - x-fwd
plans:
  - name: plan-one
    url: https://opencode.ai/zen/v1
    key: sk-plan-one
    model: plan-one-model
    mode: openai-completion
    order: 1
    quota:
      unit: usd
      rolling: 3
      weekly: 7
      monthly: 11
      probe: usage
      refresh_secs: 45
      cycle_day: 26
  - name: plan-two
    url: https://plan.example.com/v1
    key: sk-plan-two
    model: plan-two-model
    mode: anthropic-messages
    order: 0
    inject_session: true
    headers:
      x-plan-two: "yes"
    quota:
      unit: tokens
      rolling: 100
      weekly: 200
      monthly: 300
      probe: none
      refresh_secs: 60
fallback:
  - name: cash-one
    url: https://api.deepseek.com/v1
    key: sk-cash
    model: deepseek-chat
    mode: both
    order: 3
    rule:
      - peak
      - quota_low
    drop_params:
      - temperature
    headers:
      x-cash: "1"
    inject_session: true
    quota:
      unit: usd
      rolling: 1.5
      weekly: 2.5
      monthly: 5.5
      probe: balance
      refresh_secs: 30
"#;

    fn fixture() -> Config {
        config::parse(FIXTURE, Path::new("fixture.yaml")).expect("fixture parses")
    }

    fn modes_of(a: &AccountCfg) -> Vec<&'static str> {
        a.modes.iter().map(|m| m.as_str()).collect()
    }

    fn rules_of(a: &AccountCfg) -> Vec<&'static str> {
        a.rules.iter().map(|r| r.as_str()).collect()
    }

    /// The same rule main.rs uses: the leading run of comment/blank lines.
    fn header_of(text: &str) -> String {
        read_header_from(text)
    }

    #[test]
    fn round_trip_preserves_every_field() {
        let original = fixture();
        let text = to_text(&build_doc(&original, &header_of(FIXTURE)));
        let reparsed = config::parse(&text, Path::new("roundtrip.yaml"))
            .unwrap_or_else(|e| panic!("regenerated config must reload: {}\n---\n{}", e, text));

        // ---- server
        assert_eq!(reparsed.server.host, original.server.host);
        assert_eq!(reparsed.server.port, original.server.port);
        assert_eq!(reparsed.server.max_connections, original.server.max_connections);
        assert_eq!(reparsed.server.client_keys, original.server.client_keys);
        assert_eq!(reparsed.server.max_body_bytes, original.server.max_body_bytes);
        assert_eq!(reparsed.server.idle_timeout_secs, original.server.idle_timeout_secs);
        assert_eq!(reparsed.server.read_timeout_secs, original.server.read_timeout_secs);
        assert_eq!(reparsed.server.stream, original.server.stream);

        // ---- log
        assert_eq!(reparsed.log.level, original.log.level);
        assert_eq!(reparsed.log.file, original.log.file);
        assert_eq!(reparsed.log.quiet, original.log.quiet);

        // ---- router (enum-typed fields compared through as_str)
        assert_eq!(router_mode_str(reparsed.router.mode), router_mode_str(original.router.mode));
        assert_eq!(reparsed.router.peak_spec, original.router.peak_spec);
        assert_eq!(reparsed.router.peak_windows.len(), original.router.peak_windows.len());
        assert_eq!(
            idle_prefer_str(reparsed.router.idle_prefer),
            idle_prefer_str(original.router.idle_prefer)
        );
        assert_eq!(reparsed.router.surplus_max_pct, original.router.surplus_max_pct);
        assert_eq!(reparsed.router.surplus_projection, original.router.surplus_projection);
        assert_eq!(reparsed.router.exhaust_at_pct, original.router.exhaust_at_pct);
        assert_eq!(reparsed.router.quota_refresh_secs, original.router.quota_refresh_secs);
        assert_eq!(reparsed.router.cooldown_secs, original.router.cooldown_secs);
        assert_eq!(reparsed.router.auth_cooldown_secs, original.router.auth_cooldown_secs);
        assert_eq!(
            reparsed.router.server_error_cooldown_secs,
            original.router.server_error_cooldown_secs
        );
        assert_eq!(reparsed.router.retry_on_model_error, original.router.retry_on_model_error);
        assert_eq!(reparsed.router.rate_limit_retries, original.router.rate_limit_retries);
        assert_eq!(reparsed.router.rate_retry_delay_ms, original.router.rate_retry_delay_ms);
        assert_eq!(reparsed.router.skip_after_failures, original.router.skip_after_failures);
        assert_eq!(reparsed.router.skip_secs, original.router.skip_secs);
        assert_eq!(reparsed.router.attempt_budget_secs, original.router.attempt_budget_secs);
        assert_eq!(reparsed.router.session_affinity, original.router.session_affinity);
        assert_eq!(
            reparsed.router.session_affinity_ttl_secs,
            original.router.session_affinity_ttl_secs
        );
        assert_eq!(
            reparsed.router.session_fallback.as_str(),
            original.router.session_fallback.as_str()
        );
        assert_eq!(reparsed.router.inject_stream_usage, original.router.inject_stream_usage);
        assert_eq!(reparsed.router.session_headers, original.router.session_headers);
        assert_eq!(reparsed.router.user_agent, original.router.user_agent);
        assert_eq!(reparsed.router.request_timeout_secs, original.router.request_timeout_secs);
        assert_eq!(
            reparsed.router.stream_idle_timeout_secs,
            original.router.stream_idle_timeout_secs
        );

        // ---- compat
        assert_eq!(
            reparsed.compat.developer_role_to_system,
            original.compat.developer_role_to_system
        );
        assert_eq!(
            reparsed.compat.max_completion_tokens_to_max_tokens,
            original.compat.max_completion_tokens_to_max_tokens
        );
        assert_eq!(reparsed.compat.drop_params, original.compat.drop_params);
        assert_eq!(reparsed.compat.forward_headers, original.compat.forward_headers);

        // ---- accounts, field by field, in order
        assert_eq!(reparsed.accounts.len(), original.accounts.len());
        for (got, want) in reparsed.accounts.iter().zip(original.accounts.iter()) {
            assert_eq!(got.name, want.name);
            assert_eq!(got.kind.as_str(), want.kind.as_str());
            assert_eq!(got.order, want.order);
            assert_eq!(got.provider.as_str(), want.provider.as_str());
            assert_eq!(got.url, want.url);
            assert_eq!(got.key, want.key);
            assert_eq!(got.model, want.model);
            assert_eq!(modes_of(got), modes_of(want));
            assert_eq!(rules_of(got), rules_of(want));
            assert_eq!(got.extra_headers, want.extra_headers);
            assert_eq!(got.quota.unit.as_str(), want.quota.unit.as_str());
            assert_eq!(got.quota.probe.as_str(), want.quota.probe.as_str());
            assert_eq!(got.quota.rolling, want.quota.rolling);
            assert_eq!(got.quota.weekly, want.quota.weekly);
            assert_eq!(got.quota.monthly, want.quota.monthly);
            assert_eq!(got.quota.refresh_secs, want.quota.refresh_secs);
            assert_eq!(got.quota.cycle_day, want.quota.cycle_day);
            assert_eq!(got.inject_session, want.inject_session);
            assert_eq!(got.drop_params, want.drop_params);
        }

        // Regenerating from the reparsed config must change nothing.
        assert_eq!(to_text(&build_doc(&reparsed, &header_of(&text))), text);
        assert!(
            reparsed.warnings.is_empty(),
            "regenerated config warns: {:?}\n{}",
            reparsed.warnings,
            text
        );

        // ---- framing: header verbatim, one blank line, exactly one trailing newline
        assert!(text.starts_with("# hand written banner\n# second line\n\n"));
        // exactly one trailing newline: the file ends right after the last scalar
        assert_eq!(text.lines().last(), Some("      - \"temperature\""));
        assert!(text.ends_with('\n') && !text.ends_with("\n\n"));
        assert_eq!(text.matches("\n\n").count(), 1, "header/body separator only");

        // ---- fixed top-level section order
        let keys: Vec<String> = serde_yaml::from_str::<Y>(&text)
            .unwrap()
            .as_mapping()
            .unwrap()
            .keys()
            .map(|k| k.as_str().unwrap().to_string())
            .collect();
        assert_eq!(keys, vec!["server", "log", "router", "compat", "plans", "fallback"]);
    }

    #[test]
    fn optional_keys_are_omitted_and_client_keys_sits_after_max_connections() {
        let text = to_text(&build_doc(&fixture(), DEFAULT_HEADER));
        let root: Y = serde_yaml::from_str(&text).unwrap();
        let root = root.as_mapping().unwrap();

        let srv = root.get(Y::String("server".into())).unwrap().as_mapping().unwrap();
        let srv_keys: Vec<&str> = srv.keys().map(|k| k.as_str().unwrap()).collect();
        assert_eq!(
            srv_keys,
            vec![
                "host",
                "port",
                "max_connections",
                "client_keys",
                "max_body_bytes",
                "idle_timeout_secs",
                "read_timeout_secs",
                "stream"
            ]
        );

        let log = root.get(Y::String("log".into())).unwrap().as_mapping().unwrap();
        assert!(log.get(Y::String("file".into())).is_none(), "no log.file configured");
        assert_eq!(log.get(Y::String("quiet".into())).unwrap().as_bool(), Some(true));

        let plans = root.get(Y::String("plans".into())).unwrap().as_sequence().unwrap();
        let plan_one = plans[0].as_mapping().unwrap();
        // provider is written only when it differs from the URL-detected one
        assert!(plan_one.get(Y::String("provider".into())).is_none());
        assert!(plan_one.get(Y::String("rule".into())).is_none());
        assert!(plan_one.get(Y::String("headers".into())).is_none());
        assert!(plan_one.get(Y::String("inject_session".into())).is_none());
        assert_eq!(
            plan_one.get(Y::String("mode".into())).unwrap().as_str(),
            Some("openai-completion"),
            "a single mode is not collapsed to both"
        );
        // keys come out in the documented order
        let acc_keys: Vec<&str> = plan_one.keys().map(|k| k.as_str().unwrap()).collect();
        assert_eq!(acc_keys, vec!["name", "url", "key", "model", "mode", "order", "quota"]);

        let cash = root.get(Y::String("fallback".into())).unwrap().as_sequence().unwrap();
        let cash_one = cash[0].as_mapping().unwrap();
        assert_eq!(
            cash_one.get(Y::String("mode".into())).unwrap().as_str(),
            Some("both"),
            "{{Chat, Responses}} collapses to both"
        );
        let rules: Vec<&str> = cash_one
            .get(Y::String("rule".into()))
            .unwrap()
            .as_sequence()
            .unwrap()
            .iter()
            .map(|r| r.as_str().unwrap())
            .collect();
        assert_eq!(rules, vec!["peak", "quota_low"]);
    }

    #[test]
    fn provider_is_written_only_when_it_differs_from_the_url() {
        let mut cfg = fixture();
        // A key pointing somewhere else: detect() says generic, the field says opencodego.
        let cash = cfg.accounts.iter_mut().find(|a| a.name == "cash-one").unwrap();
        cash.provider = ProviderKind::OpencodeGo;
        let text = to_text(&build_doc(&cfg, DEFAULT_HEADER));
        let root: Y = serde_yaml::from_str(&text).unwrap();
        let accounts = root
            .as_mapping()
            .unwrap()
            .get(Y::String("fallback".into()))
            .unwrap()
            .as_sequence()
            .unwrap()
            .clone();
        assert_eq!(
            accounts[0]
                .as_mapping()
                .unwrap()
                .get(Y::String("provider".into()))
                .unwrap()
                .as_str(),
            Some("opencodego")
        );
        let reparsed = config::parse(&text, Path::new("x.yaml")).unwrap();
        assert_eq!(
            reparsed.find("cash-one").unwrap().provider.as_str(),
            ProviderKind::OpencodeGo.as_str()
        );
    }

    #[test]
    fn read_header_uses_the_leading_comment_run_and_falls_back() {
        assert_eq!(read_header_from("server:\n  host: 1.2.3.4\n"), DEFAULT_HEADER);
        assert_eq!(read_header_from(""), DEFAULT_HEADER);
        assert_eq!(read_header_from("\n\n  # indented comment\nserver:\n"), "\n\n  # indented comment");
        assert_eq!(read_header_from("# only a comment\n"), "# only a comment");
        // DEFAULT_HEADER is a comment-only banner, so it is its own header (up to the trailing NL).
        assert_eq!(header_of(DEFAULT_HEADER), DEFAULT_HEADER.trim_end_matches('\n'));
        let lines = DEFAULT_HEADER.lines().count();
        assert!((8..=10).contains(&lines), "default header has {} lines", lines);
        assert!(DEFAULT_HEADER.lines().all(|l| l.starts_with('#')));
        // read_header() on disk: the banner is carried over, a missing file falls back.
        let d = tmpdir();
        let p = d.join("config.yaml");
        std::fs::write(&p, "# banner\n\nserver: {}\n").unwrap();
        assert_eq!(read_header(&p), "# banner");
        assert_eq!(read_header(&d.join("absent.yaml")), DEFAULT_HEADER);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn save_replaces_atomically_and_keeps_the_banner() {
        let d = tmpdir();
        let p = d.join("config.yaml");
        let cfg = fixture();
        let banner = read_header_from(FIXTURE);
        save(&p, &cfg, &banner).unwrap();

        let written = std::fs::read_to_string(&p).unwrap();
        assert!(written.starts_with("# hand written banner\n# second line\n"));
        assert!(written.ends_with('\n'));
        // no temp file is left behind
        assert!(!d.join("config.yaml.tmp").exists());
        assert_eq!(written, to_text(&build_doc(&cfg, &banner)));
        // and it reloads as an equivalent config
        let reloaded = config::load(&p).unwrap();
        assert_eq!(reloaded.account_names(), cfg.account_names());

        // a second save over an existing file works the same way
        save(&p, &reloaded, &read_header(&p)).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), written);
        let _ = std::fs::remove_dir_all(&d);
    }
}
