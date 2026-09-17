//! OpenCode Go quota (/zen/go/v1/usage), DeepSeek balance (/user/balance) and model listing probes.

use std::sync::Arc;

use serde_json::Value;

use crate::config::AccountCfg;
use crate::httpclient;
use crate::state::{AccountRuntime, BalanceInfo, QuotaSource, QuotaWindow};
use crate::util;

/// OpenCode Go's (undocumented but live) quota endpoint:
///   GET https://opencode.ai/zen/go/v1/usage
///   {"usage":{"rolling":{"status":"ok","percent":8,"resetsAt":"..."},"weekly":{...},"monthly":{...}}}
pub fn usage_url(cfg: &AccountCfg) -> String {
    format!("{}/usage", cfg.base())
}

pub fn balance_url(cfg: &AccountCfg) -> String {
    format!("{}/user/balance", cfg.base())
}

pub fn models_url(cfg: &AccountCfg) -> String {
    format!("{}/models", cfg.base())
}

fn window_from(v: Option<&Value>) -> Option<QuotaWindow> {
    let v = v?;
    let obj = v.as_object()?;
    let pct = obj
        .get("percent")
        .or_else(|| obj.get("pct"))
        .or_else(|| obj.get("used_percent"))
        .or_else(|| obj.get("usage_percent"))
        .and_then(httpclient::pct)
        .unwrap_or(0.0);
    let status = obj
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let resets_at = obj
        .get("resetsAt")
        .or_else(|| obj.get("resets_at"))
        .or_else(|| obj.get("reset_at"))
        .and_then(|s| s.as_str())
        .and_then(crate::timeutil::parse_iso8601)
        .or_else(|| {
            obj.get("resetsAt")
                .or_else(|| obj.get("resets_at"))
                .and_then(|s| s.as_i64())
        });
    Some(QuotaWindow {
        pct,
        status,
        resets_at,
    })
}

fn window_any<'a>(usage: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| usage.get(*k))
}

/// Parse a /usage payload into (rolling, weekly, monthly).
pub fn parse_usage(v: &Value) -> Option<(QuotaWindow, QuotaWindow, QuotaWindow)> {
    let usage = v.get("usage").unwrap_or(v);
    let rolling = window_from(window_any(
        usage,
        &["rolling", "window_5h", "window5h", "five_hour", "5h"],
    ))?;
    let weekly = window_from(window_any(usage, &["weekly", "window_weekly", "week"]))?;
    let monthly = window_from(window_any(usage, &["monthly", "window_monthly", "month"]))?;
    Some((rolling, weekly, monthly))
}

/// Probe the quota endpoint and store the result. Returns a short status line for logging.
pub fn refresh_usage(agent: &ureq::Agent, cfg: &AccountCfg, rt: &Arc<AccountRuntime>, now: i64, ua: &str) -> String {
    {
        let q = match rt.quota.lock() {
            Ok(q) => q,
            Err(p) => p.into_inner(),
        };
        if now < q.probe_after {
            return "backoff".to_string();
        }
    }
    let url = usage_url(cfg);
    match httpclient::get_json(agent, &url, &cfg.key, ua) {
        Ok((200, body)) => match parse_usage(&body) {
            Some((rolling, weekly, monthly)) => {
                let mut q = match rt.quota.lock() {
                    Ok(q) => q,
                    Err(p) => p.into_inner(),
                };
                q.source = QuotaSource::Remote;
                q.fetched_at = now;
                q.rolling = rolling;
                q.weekly = weekly;
                q.monthly = monthly;
                q.last_error = None;
                q.probe_after = 0;
                format!(
                    "ok rolling={:.0}% weekly={:.0}% monthly={:.0}%",
                    q.rolling.pct, q.weekly.pct, q.monthly.pct
                )
            }
            None => {
                let mut q = match rt.quota.lock() {
                    Ok(q) => q,
                    Err(p) => p.into_inner(),
                };
                q.last_error = Some(format!("unexpected /usage payload: {}", util::truncate(&util::json_compact(&body), 200)));
                q.probe_after = now + 300;
                "unparseable".to_string()
            }
        },
        Ok((code, body)) => {
            let mut q = match rt.quota.lock() {
                Ok(q) => q,
                Err(p) => p.into_inner(),
            };
            if q.source != QuotaSource::Remote {
                q.source = QuotaSource::Local;
            }
            q.last_error = Some(format!(
                "usage endpoint HTTP {}: {}",
                code,
                util::truncate(&util::json_compact(&body), 160)
            ));
            // Undocumented endpoint: back off hard on 404/405, softer on 5xx.
            q.probe_after = now + if code == 404 || code == 405 { 1800 } else { 120 };
            format!("http {}", code)
        }
        Err(e) => {
            let mut q = match rt.quota.lock() {
                Ok(q) => q,
                Err(p) => p.into_inner(),
            };
            if q.source != QuotaSource::Remote {
                q.source = QuotaSource::Local;
            }
            q.last_error = Some(format!("usage probe failed: {}", util::truncate(&e, 160)));
            q.probe_after = now + 60;
            format!("error {}", util::truncate(&e, 120))
        }
    }
}

/// DeepSeek official balance probe: GET /user/balance.
pub fn refresh_balance(agent: &ureq::Agent, cfg: &AccountCfg, rt: &Arc<AccountRuntime>, now: i64, ua: &str) -> String {
    let url = balance_url(cfg);
    match httpclient::get_json(agent, &url, &cfg.key, ua) {
        Ok((200, body)) => {
            let is_available = body
                .get("is_available")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let info = body
                .get("balance_infos")
                .and_then(|v| v.as_array())
                .and_then(|list| list.first().cloned())
                .map(|first| {
                    let total = first
                        .get("total_balance")
                        .and_then(|v| {
                            v.as_f64()
                                .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
                        })
                        .unwrap_or(0.0);
                    let currency = first
                        .get("currency")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    (total, currency)
                })
                .unwrap_or((0.0, String::new()));
            if let Ok(mut b) = rt.balance.lock() {
                *b = Some(BalanceInfo {
                    total: info.0,
                    currency: info.1.clone(),
                    is_available,
                    fetched_at: now,
                });
            }
            format!("ok {:.2} {}", info.0, info.1)
        }
        Ok((code, body)) => {
            format!("http {} {}", code, util::truncate(&util::json_compact(&body), 120))
        }
        Err(e) => format!("error {}", util::truncate(&e, 120)),
    }
}

/// GET /models -> list of model ids (used for /v1/models aggregation).
pub fn fetch_models(agent: &ureq::Agent, cfg: &AccountCfg, rt: &Arc<AccountRuntime>, now: i64, ttl: i64, ua: &str) -> Vec<String> {
    if ttl > 0 {
        if let Ok(m) = rt.models.lock() {
            if let Some((ts, list)) = m.as_ref() {
                if now - *ts < ttl {
                    return list.clone();
                }
            }
        }
    }
    let url = models_url(cfg);
    let list = match httpclient::get_json(agent, &url, &cfg.key, ua) {
        Ok((200, body)) => body
            .get("data")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if let Ok(mut guard) = rt.models.lock() {
        *guard = Some((now, list.clone()));
    }
    list
}

/// Mark the account as out of quota (from a 429 / quota error response).
pub fn mark_exhausted(rt: &Arc<AccountRuntime>, until: i64, reason: &str) {
    let mut q = match rt.quota.lock() {
        Ok(q) => q,
        Err(p) => p.into_inner(),
    };
    q.exhausted_until = until;
    q.last_error = Some(reason.to_string());
    q.probe_after = 0;
}
