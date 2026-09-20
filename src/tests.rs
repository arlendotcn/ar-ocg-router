//! Unit tests for the pure logic (windows, model mapping, config, endpoint prices, SSE usage).

use crate::config;
use crate::models;
use crate::router;
use crate::sse::{self, UsageScanner};
use crate::timeutil;
use crate::util;

fn ts(s: &str) -> i64 {
    timeutil::parse_iso8601(s).expect("timestamp")
}

// ------------------------------------------------------------------ timeutil

#[test]
fn civil_time_round_trip() {
    for secs in [0i64, 1_700_000_000, 1_789_551_700, -86_400] {
        let c = timeutil::civil_from_unix(secs);
        let back = timeutil::days_from_civil(c.year, c.month, c.day) * 86_400
            + c.hour as i64 * 3600
            + c.min as i64 * 60
            + c.sec as i64;
        assert_eq!(back, secs);
    }
    let c = timeutil::civil_from_unix(0);
    assert_eq!((c.year, c.month, c.day), (1970, 1, 1));
    assert_eq!(c.weekday, 3, "1970-01-01 was a Thursday");
}

#[test]
fn iso8601_round_trip_including_offsets() {
    assert_eq!(ts("2026-09-16T02:00:00Z"), 1_789_524_000);
    assert_eq!(ts("2026-09-16T10:00:00+08:00"), 1_789_524_000);
    assert_eq!(ts("2026-09-16T01:00:00-01:00"), 1_789_524_000);
    assert_eq!(timeutil::iso8601(1_789_524_000), "2026-09-16T02:00:00Z");
}

#[test]
fn deepseek_peak_windows_are_mon_fri_01_04_and_06_10_utc() {
    let w = timeutil::parse_peak_spec(timeutil::DEFAULT_PEAK_SPEC).expect("default spec");
    // Wednesday
    assert!(timeutil::is_peak_at(ts("2026-09-16T01:00:00Z"), &w));
    assert!(timeutil::is_peak_at(ts("2026-09-16T03:59:00Z"), &w));
    assert!(!timeutil::is_peak_at(ts("2026-09-16T04:00:00Z"), &w));
    assert!(timeutil::is_peak_at(ts("2026-09-16T06:00:00Z"), &w));
    assert!(timeutil::is_peak_at(ts("2026-09-16T09:59:00Z"), &w));
    assert!(!timeutil::is_peak_at(ts("2026-09-16T10:00:00Z"), &w));
    assert!(!timeutil::is_peak_at(ts("2026-09-16T05:00:00Z"), &w));
    // Saturday and Sunday are fully off-peak
    assert!(!timeutil::is_peak_at(ts("2026-09-19T02:00:00Z"), &w));
    assert!(!timeutil::is_peak_at(ts("2026-09-20T07:00:00Z"), &w));
    // Monday is peak again
    assert!(timeutil::is_peak_at(ts("2026-09-21T07:00:00Z"), &w));
}

#[test]
fn peak_spec_supports_offsets_and_multiple_windows() {
    let w = timeutil::parse_peak_spec("Mon-Fri 09:00-12:00 +08:00; Sat,Sun 00:00-24:00 UTC").unwrap();
    assert_eq!(w.len(), 2);
    // 09:00 Beijing == 01:00 UTC -> peak
    assert!(timeutil::is_peak_at(ts("2026-09-16T01:30:00Z"), &w));
    assert!(!timeutil::is_peak_at(ts("2026-09-16T05:30:00Z"), &w));
    // Saturday is covered by the second window whatever the hour
    assert!(timeutil::is_peak_at(ts("2026-09-19T22:00:00Z"), &w));
    // Sunday 23:30 UTC is inside Saturday-window day? No: Sunday is in the day list.
    assert!(timeutil::is_peak_at(ts("2026-09-20T23:30:00Z"), &w));
}

#[test]
fn schedule_reports_next_transition() {
    let w = timeutil::parse_peak_spec(timeutil::DEFAULT_PEAK_SPEC).unwrap();
    let s = timeutil::eval_schedule(ts("2026-09-16T09:30:00Z"), &w);
    assert!(s.is_peak);
    assert_eq!(s.next_change, ts("2026-09-16T10:00:00Z"));
    let s = timeutil::eval_schedule(ts("2026-09-16T23:00:00Z"), &w);
    assert!(!s.is_peak);
    assert_eq!(s.next_change, ts("2026-09-17T01:00:00Z"));
}

#[test]
fn bad_peak_specs_are_rejected() {
    assert!(timeutil::parse_peak_spec("").is_err());
    assert!(timeutil::parse_peak_spec("Mon-Fri").is_err());
    assert!(timeutil::parse_peak_spec("Mon-Fri 25:00-26:00 UTC").is_err());
    assert!(timeutil::parse_peak_spec("Funday 01:00-02:00").is_err());
}

// ------------------------------------------------------------------ models

#[test]
fn provider_detection_and_parsing() {
    use crate::models::ProviderKind;
    assert_eq!(ProviderKind::detect("https://opencode.ai/zen/go/v1"), ProviderKind::OpencodeGo);
    assert_eq!(ProviderKind::detect("https://api.deepseek.com"), ProviderKind::DeepSeek);
    assert_eq!(ProviderKind::detect("https://ark.cn-beijing.volces.com/api/coding/v3"), ProviderKind::Generic);
    assert_eq!(ProviderKind::parse("opencode-go"), Some(ProviderKind::OpencodeGo));
    assert_eq!(ProviderKind::parse("ds"), Some(ProviderKind::DeepSeek));
    assert_eq!(ProviderKind::parse("nonsense"), None);
}

#[test]
fn forced_endpoint_parsing() {
    use crate::models::{parse_forced_endpoint, ForcedEndpoint, ProviderKind, ROUTER_MODEL};
    let names = vec!["go-dsf".to_string(), "vol-glm".to_string()];
    // no pin
    assert_eq!(parse_forced_endpoint(ROUTER_MODEL, &names), None);
    assert_eq!(parse_forced_endpoint("", &names), None);
    // unknown free-form model names are accepted and ignored
    assert_eq!(parse_forced_endpoint("deepseek-flash", &names), None);
    assert_eq!(parse_forced_endpoint("gpt-5.6-sol", &names), None);
    // pin by endpoint name (with or without a model suffix)
    assert_eq!(
        parse_forced_endpoint("go-dsf", &names),
        Some(ForcedEndpoint::Name("go-dsf".to_string()))
    );
    assert_eq!(
        parse_forced_endpoint("go-dsf/whatever", &names),
        Some(ForcedEndpoint::Name("go-dsf".to_string()))
    );
    // pin by provider/model
    assert_eq!(
        parse_forced_endpoint("generic/glm-5.3-flash", &names),
        Some(ForcedEndpoint::ProviderModel(
            ProviderKind::Generic,
            "glm-5.3-flash".to_string()
        ))
    );
    assert_eq!(
        parse_forced_endpoint("opencodego/deepseek-flash", &names),
        Some(ForcedEndpoint::ProviderModel(
            ProviderKind::OpencodeGo,
            "deepseek-flash".to_string()
        ))
    );
    // models payload advertises exactly one id
    let payload = crate::models::models_payload();
    let ids: Vec<&str> = payload["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![ROUTER_MODEL]);
}

#[test]
fn endpoint_name_rules() {
    use crate::models::normalize_name;
    assert_eq!(normalize_name("vol ark"), "vol-ark");
    assert_eq!(normalize_name("Vol/GLM"), "vol-glm");
    assert_eq!(normalize_name("  "), "endpoint");
    let fp = crate::util::key_fingerprint("sk-abcdef");
    assert_eq!(fp.len(), 4);
    assert_eq!(fp, crate::util::key_fingerprint("sk-abcdef"), "stable");
    assert_ne!(fp, crate::util::key_fingerprint("sk-abcdefg"));
}

#[test]
fn endpoint_detection_accepts_both_base_url_styles() {
    use models::{endpoint_of, normalize_path, Endpoint};
    assert_eq!(normalize_path("/v1/chat/completions"), "/chat/completions");
    assert_eq!(endpoint_of("/v1/chat/completions"), Endpoint::Chat);
    assert_eq!(endpoint_of("/chat/completions"), Endpoint::Chat);
    assert_eq!(endpoint_of("/v1/responses"), Endpoint::Responses);
    assert_eq!(endpoint_of("/responses?foo=1"), Endpoint::Responses);
    assert_eq!(endpoint_of("/v1/messages"), Endpoint::Anthropic);
    assert_eq!(endpoint_of("/v1/models"), Endpoint::Models);
    assert_eq!(endpoint_of("/v1/beta/completions"), Endpoint::Chat);
}

#[test]
fn upstream_url_join_handles_v1_prefixes() {
    let cfg = config::parse(SAMPLE_CONFIG, std::path::Path::new("test.yaml")).unwrap();
    let go = cfg.find("go-1").unwrap();
    assert_eq!(
        go.upstream_url("/v1/chat/completions"),
        "https://opencode.ai/zen/go/v1/chat/completions"
    );
    assert_eq!(go.upstream_url("/chat/completions"), "https://opencode.ai/zen/go/v1/chat/completions");
    let fb = cfg.find("deepseek-official").unwrap();
    assert_eq!(fb.upstream_url("/v1/chat/completions"), "https://api.deepseek.com/chat/completions");
    assert_eq!(fb.upstream_url("/v1/models?x=1"), "https://api.deepseek.com/models?x=1");
}

// ------------------------------------------------------------------ pricing

/// Rates are the endpoint's own, so this only has to prove the arithmetic and the peak rule -
/// whether a given number matches a provider's published price is the operator's business now.
/// A plan quoted in renminbi is money like any other: the unit labels the amount, it never
/// converts it. The alias set matters because configs are written by hand.
#[test]
fn rmb_is_a_money_unit_and_parses_its_aliases() {
    for alias in ["rmb", "RMB", "cny", "CNY", "yuan", "¥", "￥"] {
        assert_eq!(
            config::QuotaUnit::parse(alias),
            Some(config::QuotaUnit::Rmb),
            "{:?} must be accepted as renminbi",
            alias
        );
    }
    assert_eq!(config::QuotaUnit::Rmb.as_str(), "rmb", "one spelling on the way out");
    assert!(config::QuotaUnit::Rmb.is_money());
    assert!(config::QuotaUnit::Usd.is_money());
    assert!(!config::QuotaUnit::Tokens.is_money());
    assert!(!config::QuotaUnit::None.is_money());
    // The bare word "money" predates the second currency: it still means dollars, which is what
    // every config written before rmb existed meant by it.
    assert_eq!(config::QuotaUnit::parse("money"), Some(config::QuotaUnit::Usd));
}

#[test]
fn endpoint_prices_apply_the_peak_multiplier() {
    let p = config::PricesCfg {
        currency: "USD".to_string(),
        input: 0.15,
        output: 0.60,
        cached_input: 0.003,
        peak_multiplier: 2.0,
    };
    let off = p.cost_of(1000, 400, 200, false);
    let want_off = (600.0 * 0.15 + 400.0 * 0.003 + 200.0 * 0.60) / 1_000_000.0;
    assert!((off - want_off).abs() < 1e-15, "{} vs {}", off, want_off);
    let peak = p.cost_of(1000, 400, 200, true);
    assert!((peak - want_off * 2.0).abs() < 1e-15, "peak must be the multiplier times off-peak");
    // cached tokens can never exceed the prompt count
    let capped = p.cost_of(100, 500, 0, false);
    assert!((capped - (100.0 * 0.003 / 1_000_000.0)).abs() < 1e-18);
}

#[test]
fn absent_prices_record_no_money() {
    let p = config::PricesCfg::default();
    assert!(!p.is_set(), "an empty block must not look like a price");
    assert_eq!(p.cost_of(1000, 400, 200, true), 0.0);
    // A currency label without rates is still not a price.
    let labelled = config::PricesCfg { currency: "CNY".to_string(), ..config::PricesCfg::default() };
    assert!(!labelled.is_set());
    assert_eq!(labelled.cost_of(1000, 0, 100, false), 0.0);
}

// ------------------------------------------------------------------ sse usage

#[test]
fn usage_is_extracted_from_chat_and_responses_payloads() {
    let chat: serde_json::Value = serde_json::from_str(
        r#"{"usage":{"prompt_tokens":1200,"completion_tokens":300,"prompt_cache_hit_tokens":800}}"#,
    )
    .unwrap();
    let u = sse::extract_usage(&chat);
    assert_eq!((u.prompt, u.completion, u.cached), (1200, 300, 800));
    assert!(u.seen);

    let responses: serde_json::Value = serde_json::from_str(
        r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":2,"input_tokens_details":{"cached_tokens":4}}}}"#,
    )
    .unwrap();
    let u = sse::extract_usage(&responses);
    assert_eq!((u.prompt, u.completion, u.cached), (10, 2, 4));

    // OpenCode Go also reports the billed cost
    let with_cost: serde_json::Value =
        serde_json::from_str(r#"{"choices":[],"cost":"0.0008265"}"#).unwrap();
    let u = sse::extract_usage(&with_cost);
    assert!((u.upstream_cost - 0.0008265).abs() < 1e-12);
}

#[test]
fn usage_scanner_parses_a_stream_without_touching_it() {
    let mut sc = UsageScanner::new();
    let stream = concat!(
        "data: {\"id\":\"1\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
        "da",
        "ta: {\"id\":\"1\",\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20,\"prompt_tokens_details\":{\"cached_tokens\":60}},\"cost\":\"0.0001\"}\n\n",
        "data: [DONE]\n\n"
    );
    // feed in awkward slices to exercise partial-line buffering
    for chunk in stream.as_bytes().chunks(7) {
        sc.feed(chunk);
    }
    sc.finish();
    assert_eq!(sc.usage.prompt, 100);
    assert_eq!(sc.usage.completion, 20);
    assert_eq!(sc.usage.cached, 60);
    assert!((sc.usage.upstream_cost - 0.0001).abs() < 1e-12);
}

// ------------------------------------------------------------------ router rules

#[test]
fn fallback_rules_follow_peak_and_quota_state() {
    use config::Rule;
    use router::{rule_matches, RuleCtx};
    let peak_ample = RuleCtx { is_peak: true, plans_surplus: true, plans_available: true };
    let idle_ample = RuleCtx { is_peak: false, plans_surplus: true, plans_available: true };
    let idle_tight = RuleCtx { is_peak: false, plans_surplus: false, plans_available: true };
    let peak_gone = RuleCtx { is_peak: true, plans_surplus: false, plans_available: false };

    assert!(rule_matches(Rule::Always, peak_ample));
    assert!(rule_matches(Rule::Peak, peak_ample) && !rule_matches(Rule::Peak, idle_ample));
    assert!(rule_matches(Rule::OffPeak, idle_ample) && !rule_matches(Rule::OffPeak, peak_ample));
    assert!(!rule_matches(Rule::QuotaLow, idle_ample));
    assert!(rule_matches(Rule::QuotaLow, idle_tight));
    assert!(rule_matches(Rule::QuotaExhausted, peak_gone));
    assert!(rule_matches(Rule::PrimaryUnavailable, peak_gone));
    assert!(!rule_matches(Rule::Never, idle_ample));
}

// ------------------------------------------------------------------ config

const SAMPLE_CONFIG: &str = r#"
server:
  host: 0.0.0.0
  port: 9000
  client_keys: [sk-router-1]
log:
  level: debug
router:
  mode: auto
  peak_windows: "Mon-Fri 01:00-04:00, 06:00-10:00 UTC"
  idle_prefer: fallback
  surplus_max_pct: 70
plans:
  - name: go-1
    url: https://opencode.ai/zen/go/v1
    key: sk-go
    model: deepseek-flash
    inject_session: true
    mode: openai-completion
    weight: 60
    quota: { unit: usd, rolling: 12, weekly: 30, monthly: 60 }
fallback:
  - name: deepseek-official
    mode: openai-responses
    url: https://api.deepseek.com
    key: sk-ds
    model: deepseek-flash
    rule: offpeak, quota_low
    weight: 50
"#;

#[test]
fn sample_config_parses_with_expected_defaults() {
    let c = config::parse(SAMPLE_CONFIG, std::path::Path::new("test.yaml")).unwrap();
    assert_eq!(c.server.host, "0.0.0.0");
    assert_eq!(c.server.port, 9000);
    assert_eq!(c.server.client_keys, vec!["sk-router-1".to_string()]);
    assert_eq!(c.accounts.len(), 2);
    let go = c.find("go-1").unwrap();
    assert_eq!(go.kind, config::AccountKind::Plans);
    assert_eq!(go.modes, vec![models::Mode::Chat]);
    assert_eq!(go.provider, models::ProviderKind::OpencodeGo);
    assert_eq!(go.quota.probe, config::QuotaProbe::Usage, "plans probe the usage endpoint");
    assert_eq!(go.quota.unit, config::QuotaUnit::Usd);
    assert!((go.quota.monthly - 60.0).abs() < 1e-9);
    assert!(go.inject_session, "inject_session comes from the endpoint config");
    let fb = c.find("deepseek-official").unwrap();
    assert_eq!(fb.modes, vec![models::Mode::Responses]);
    assert_eq!(fb.rules, vec![config::Rule::OffPeak, config::Rule::QuotaLow]);
    assert_eq!(fb.provider, models::ProviderKind::DeepSeek);
    assert_eq!(fb.quota.probe, config::QuotaProbe::Balance);
    assert_eq!(fb.kind, config::AccountKind::Cash);
    assert!(!fb.inject_session);
    assert_eq!(c.router.surplus_max_pct, 70.0);
    assert!(
        !c.compat.developer_role_to_system && !c.compat.max_completion_tokens_to_max_tokens,
        "compat rewrites must default to off (lossless passthrough)"
    );
    assert!(c.compat.drop_params.is_empty());
    assert!(c.router.session_affinity);
    assert_eq!(c.router.session_fallback, config::SessionFallback::Process);
    assert!(!config::PROTOCOL_HEADERS.is_empty());
    assert!(c.warnings.is_empty(), "warnings: {:?}", c.warnings);
    assert!(c.describe_windows().contains("01:00-04:00"));
}

#[test]
fn inject_session_is_never_guessed_from_the_provider() {
    let no_flag = SAMPLE_CONFIG.replace("    inject_session: true\n", "");
    let c = config::parse(&no_flag, std::path::Path::new("t")).unwrap();
    let go = c.find("go-1").unwrap();
    assert_eq!(go.provider, models::ProviderKind::OpencodeGo);
    assert!(
        !go.inject_session,
        "an opencodego endpoint without inject_session must NOT get the header implicitly"
    );
}

#[test]
fn config_errors_are_reported() {
    assert!(config::parse("fallback: []", std::path::Path::new("t")).is_err(), "no accounts");
    // duplicates are no longer fatal: the later endpoint gets a suffixed name
    let dup = SAMPLE_CONFIG.replace("name: deepseek-official", "name: go-1");
    let c = config::parse(&dup, std::path::Path::new("t")).unwrap();
    assert!(c.find("go-1").is_some());
    assert!(c.find("go-1-2").is_some(), "second go-1 must be renamed");
    assert!(
        c.warnings.iter().any(|w| w.contains("already used")),
        "warnings: {:?}",
        c.warnings
    );
    let bad_url = SAMPLE_CONFIG.replace("https://api.deepseek.com", "ftp://x");
    assert!(config::parse(&bad_url, std::path::Path::new("t")).is_err(), "bad url");
    let no_key = SAMPLE_CONFIG.replace("key: sk-ds", "key: \"\"");
    assert!(config::parse(&no_key, std::path::Path::new("t")).is_err(), "empty key");
    let no_model = SAMPLE_CONFIG.replace("    model: deepseek-flash\n", "");
    assert!(
        config::parse(&no_model, std::path::Path::new("t")).is_err(),
        "an endpoint without a model must be rejected"
    );
}

#[test]
fn key_indirection_supports_env_and_file() {
    std::env::set_var("OCG_ROUTER_TEST_KEY", "sk-from-env");
    let c = config::parse(
        &SAMPLE_CONFIG.replace("key: sk-ds", "key: env:OCG_ROUTER_TEST_KEY"),
        std::path::Path::new("t"),
    )
    .unwrap();
    assert_eq!(c.find("deepseek-official").unwrap().key, "sk-from-env");
    std::env::remove_var("OCG_ROUTER_TEST_KEY");
    assert!(config::parse(
        &SAMPLE_CONFIG.replace("key: sk-ds", "key: env:OCG_ROUTER_TEST_MISSING"),
        std::path::Path::new("t"),
    )
    .is_err());
}

/// A retired key must not change anything, and must not make the file fail to load: silently
/// ignoring it is how a config keeps working while quietly behaving differently.
#[test]
fn mode_tolerates_alternate_spellings_and_a_retired_weight_is_ignored() {
    let cfg = r#"
plans:
  - name: go-a
    url: https://opencode.ai/zen/go/v1
    key: k
    model: deepseek-flash
    mode: openai-completion|openai-responses
    weight: 200
fallback:
  - name: fb-a
    url: https://api.deepseek.com
    key: k
    model: deepseek-flash
    mode: [anthropic-messages]
    rule: never_on_error
"#;
    let c = config::parse(cfg, std::path::Path::new("t")).unwrap();
    let go = c.find("go-a").unwrap();
    assert_eq!(go.modes.len(), 2);
    let fb = c.find("fb-a").unwrap();
    assert_eq!(fb.modes, vec![models::Mode::Anthropic]);
    assert!(fb.no_error_fallback);
    // list order is the default consumption order, and an unmeasurable plan is not "unsurplus"
    assert_eq!(go.order, 0);
    assert_eq!(go.kind, config::AccountKind::Plans);
    assert_eq!(go.quota.unit, config::QuotaUnit::Usd, "opencode.ai endpoints default to usd quota");
    assert_eq!(fb.order, 0);
    assert_eq!(fb.quota.probe, config::QuotaProbe::Balance);
}

// ------------------------------------------------------------------ misc utils

#[test]
fn redact_hides_the_secret_body() {
    let r = util::redact("sk-abcdefghijklmnop");
    assert!(r.starts_with("sk-abc"));
    assert!(r.ends_with("***"));
    assert!(!r.contains("efghij"));
    assert_eq!(util::redact("short"), "***");
}

#[test]
fn usd_formatting_is_readable() {
    assert_eq!(util::fmt_usd(0.0), "0");
    assert!(util::fmt_usd(0.0008265).starts_with("0.00082"), "{}", util::fmt_usd(0.0008265));
    assert_eq!(util::fmt_usd(12.5), "12.5000");
}

/// An empty window sums to -0.0 under IEEE rules, and the console prints that as "-0.00".
/// Zero money has no sign, so the accessor normalises it.
#[test]
fn an_empty_money_window_is_not_negative_zero() {
    use crate::state::LocalLedger;

    let l = LocalLedger::default();
    for (name, v) in [
        ("total", l.total_cost),
        ("rolling", l.window_cost(1_000_000, 5 * 3600)),
        ("weekly", l.window_cost(1_000_000, 7 * 86_400)),
        ("monthly", l.window_cost(1_000_000, 30 * 86_400)),
    ] {
        assert_eq!(v.to_string(), "0", "{name} must not render as -0.0");
        assert!(v.is_sign_positive(), "{name} must carry no negative sign");
    }
}

// ------------------------------------------------------------------ quota unit reporting

/// A plan budgeted in money with no probe must not claim its used/limit figures are unitless.
/// The unit is what the console prints next to the numbers, and "none" beside an amount of money
/// is worse than useless.
#[test]
fn a_money_budget_without_a_probe_reports_its_currency_as_the_unit() {
    use crate::config::{QuotaCfg, QuotaProbe, QuotaUnit};
    use crate::state::{Accounting, QuotaState};

    let q = QuotaState::default();
    let money = QuotaCfg {
        rolling: 0.0,
        weekly: 0.0,
        monthly: 200.0,
        probe: QuotaProbe::None,
        refresh_secs: 300,
        ..QuotaCfg::for_provider(crate::models::ProviderKind::Generic)
    };

    // 1. The Ark case as a user would write it: bounded windows, the currency only in prices.
    //    The unit defaults to "none" for a generic provider, yet the totals are RMB sums.
    let unstated = QuotaCfg { unit: QuotaUnit::None, ..money.clone() };
    let r1 = q.report(1_000_000, 3_600, 80.0, true, 99.0, &unstated);
    assert_eq!(r1.accounting, Accounting::Money, "bounded windows are money");
    assert_eq!(
        q.to_json(1_000_000, &r1, "RMB")["unit"],
        "rmb",
        "the console must be told the denomination, not shown a bare number"
    );
    // An endpoint with no currency label gets the unitless answer rather than an invented symbol.
    assert_eq!(q.to_json(1_000_000, &r1, "")["unit"], "none");

    // 2. A stated unit is already the answer; prices.currency must not override it.
    let stated = QuotaCfg { unit: QuotaUnit::Rmb, ..money.clone() };
    let r2 = q.report(1_000_000, 3_600, 80.0, true, 99.0, &stated);
    assert_eq!(r2.accounting, Accounting::Money);
    assert_eq!(q.to_json(1_000_000, &r2, "USD")["unit"], "rmb", "the config wins over the label");

    let tok = QuotaCfg { unit: QuotaUnit::Tokens, ..money.clone() };
    let r3 = q.report(1_000_000, 3_600, 80.0, true, 99.0, &tok);
    assert_eq!(r3.accounting, Accounting::Tokens);
    assert_eq!(q.to_json(1_000_000, &r3, "RMB")["unit"], "tokens");

    // 3. No windows at all: nothing to denominate, so it stays unitless even with prices present.
    let unbounded = QuotaCfg { unit: QuotaUnit::None, rolling: 0.0, weekly: 0.0, monthly: 0.0, ..money };
    let r4 = q.report(1_000_000, 3_600, 80.0, true, 99.0, &unbounded);
    assert_eq!(r4.accounting, Accounting::None, "an unbudgeted plan measures nothing");
    assert_eq!(q.to_json(1_000_000, &r4, "RMB")["unit"], "none");
}

/// A probe can express a provider-side unit, so it wins: a plan metered in tokens by the provider
/// stays tokens even when the config budgets it in money.
#[test]
fn a_remote_probe_keeps_its_own_unit() {
    use crate::config::{QuotaCfg, QuotaProbe, QuotaUnit};
    use crate::state::QuotaState;

    let cfg = QuotaCfg {
        unit: QuotaUnit::Tokens,
        rolling: 0.0,
        weekly: 0.0,
        monthly: 1000.0,
        probe: QuotaProbe::Usage,
        refresh_secs: 60,
    };
    let q = QuotaState::default();
    let report = q.report(1_000_000, 3_600, 80.0, true, 99.0, &cfg);
    assert_eq!(q.to_json(1_000_000, &report, "USD")["unit"], "tokens");
}

