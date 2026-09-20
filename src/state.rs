//! Runtime state per account: statistics, quota ledger, cooldown, health.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::config::{AccountCfg, AccountKind, QuotaCfg, QuotaUnit};
use crate::timeutil;

#[derive(Debug, Default, Clone)]
pub struct AccountStats {
    pub requests: u64,
    pub successes: u64,
    pub errors: u64,
    pub stream_requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    /// Cash value of the traffic served here (DeepSeek list price at the time of the request).
    pub cost_usd: f64,
    /// Cash that would have been paid to the DeepSeek official API (i.e. what quota saved).
    pub saved_usd: f64,
    pub latency_ms_total: u64,
    pub last_used: i64,
    pub last_error: Option<String>,
    pub last_status: u16,
    pub attempts_skipped: u64,
}

impl AccountStats {
    /// True when nothing has ever been recorded, so the state file can stay sparse.
    pub fn is_empty(&self) -> bool {
        self.requests == 0
            && self.successes == 0
            && self.errors == 0
            && self.stream_requests == 0
            && self.prompt_tokens == 0
            && self.completion_tokens == 0
            && self.cached_tokens == 0
            && self.cost_usd == 0.0
            && self.saved_usd == 0.0
            && self.latency_ms_total == 0
            && self.last_used == 0
            && self.last_error.is_none()
            && self.last_status == 0
            && self.attempts_skipped == 0
    }

    /// Serialised into ar-ocg-router.state.json. Additive only: an older file that lacks a field
    /// loads it as zero.
    pub fn to_state_json(&self) -> Value {
        json!({
            "requests": self.requests,
            "successes": self.successes,
            "errors": self.errors,
            "stream_requests": self.stream_requests,
            "prompt_tokens": self.prompt_tokens,
            "completion_tokens": self.completion_tokens,
            "cached_tokens": self.cached_tokens,
            // No currency in the field name: the unit is the endpoint's, and it is carried
            // alongside the statistics rather than baked into the key.
            "cost": self.cost_usd,
            "saved": self.saved_usd,
            "latency_ms_total": self.latency_ms_total,
            "last_used": self.last_used,
            "last_error": self.last_error,
            "last_status": self.last_status,
            "attempts_skipped": self.attempts_skipped,
        })
    }

    /// Inverse of to_state_json. Returns None when the entry is not an object at all.
    pub fn from_state_json(v: &Value) -> Option<AccountStats> {
        let o = v.as_object()?;
        let u = |k: &str| o.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        let f = |k: &str| o.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let s = |k: &str| o.get(k).and_then(|x| x.as_str()).map(|x| x.to_string());
        Some(AccountStats {
            requests: u("requests"),
            successes: u("successes"),
            errors: u("errors"),
            stream_requests: u("stream_requests"),
            prompt_tokens: u("prompt_tokens"),
            completion_tokens: u("completion_tokens"),
            cached_tokens: u("cached_tokens"),
            cost_usd: f("cost"),
            saved_usd: f("saved"),
            latency_ms_total: u("latency_ms_total"),
            last_used: o.get("last_used").and_then(|x| x.as_i64()).unwrap_or(0),
            last_error: s("last_error"),
            last_status: u("last_status") as u16,
            attempts_skipped: u("attempts_skipped"),
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct QuotaWindow {
    pub pct: f64,
    pub status: String,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaSource {
    Unknown,
    Remote,
    Local,
}

impl QuotaSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            QuotaSource::Unknown => "unknown",
            QuotaSource::Remote => "remote",
            QuotaSource::Local => "local",
        }
    }
}

/// Total observed usage in sliding windows, used when the provider has no usage API
/// (or in addition to it). Keeps both dollars and tokens so that plans budgeted either way
/// can be estimated without maintaining a price table.
#[derive(Debug, Default, Clone)]
pub struct LocalLedger {
    /// (timestamp, usd, tokens)
    pub samples: VecDeque<(i64, f64, u64)>,
    pub total_cost: f64,
    pub total_tokens: u64,
}

/// How many samples are written to the state file. The live ledger keeps far more, but the file is
/// rewritten every few seconds, and a sliding window never reaches back further than a month: 2000
/// samples cover any realistic request rate while keeping the file small.
const PERSISTED_SAMPLES: usize = 2000;

impl LocalLedger {
    /// Totals plus a bounded tail of samples, for the state file.
    ///
    /// The ledger used to be memory-only on the grounds that it is routing input, and persisting it
    /// would invent usage the provider never confirmed. That reasoning stopped holding once the
    /// statistics themselves became persistent: the same requests were already being written to the
    /// same file, so a memory-only ledger only meant two numbers on one screen counted different
    /// periods (the dashboard showed a lifetime total, the quota decision a since-restart one).
    pub fn to_state_json(&self) -> Value {
        let samples: Vec<Value> = self
            .samples
            .iter()
            .rev()
            .take(PERSISTED_SAMPLES)
            .rev()
            .map(|(ts, cost, tokens)| json!([ts, cost, tokens]))
            .collect();
        json!({
            "total_cost": self.total_cost,
            "total_tokens": self.total_tokens,
            "samples": samples,
        })
    }

    pub fn from_state_json(v: &Value) -> LocalLedger {
        let mut l = LocalLedger::default();
        let Some(o) = v.as_object() else { return l };
        l.total_cost = o.get("total_cost").and_then(|x| x.as_f64()).unwrap_or(0.0);
        l.total_tokens = o.get("total_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
        if let Some(list) = o.get("samples").and_then(|x| x.as_array()) {
            for item in list {
                let Some(t) = item.as_array() else { continue };
                if t.len() != 3 {
                    continue;
                }
                let ts = t[0].as_i64().unwrap_or(0);
                let cost = t[1].as_f64().unwrap_or(0.0);
                let tokens = t[2].as_u64().unwrap_or(0);
                if ts > 0 {
                    l.samples.push_back((ts, cost, tokens));
                }
            }
        }
        l
    }

    pub fn add(&mut self, now: i64, cost: f64, tokens: u64) {
        self.samples.push_back((now, cost, tokens));
        self.total_cost += cost;
        self.total_tokens += tokens;
        if self.samples.len() > 100_000 {
            self.samples.pop_front();
        }
        let cutoff = now - 31 * 86400;
        while let Some((ts, _, _)) = self.samples.front() {
            if *ts < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn window_cost(&self, now: i64, period_secs: i64) -> f64 {
        let from = now - period_secs;
        let sum: f64 = self
            .samples
            .iter()
            .rev()
            .take_while(|(ts, _, _)| *ts >= from)
            .map(|(_, c, _)| *c)
            .sum();
        // Summing an empty or all-zero window yields -0.0, which prints as "-0.00" in the console.
        // Zero money is zero in either sign; the negative zero is an artefact of IEEE addition.
        if sum == 0.0 {
            0.0
        } else {
            sum
        }
    }

    pub fn window_tokens(&self, now: i64, period_secs: i64) -> u64 {
        let from = now - period_secs;
        self.samples
            .iter()
            .rev()
            .take_while(|(ts, _, _)| *ts >= from)
            .map(|(_, _, t)| *t)
            .sum()
    }
}

#[derive(Debug, Clone)]
pub struct QuotaState {
    pub source: QuotaSource,
    pub fetched_at: i64,
    pub rolling: QuotaWindow,
    pub weekly: QuotaWindow,
    pub monthly: QuotaWindow,
    pub ledger: LocalLedger,
    pub last_error: Option<String>,
    /// Set when the upstream explicitly reported "out of quota" / 429-budget errors.
    pub exhausted_until: i64,
    /// Do not probe the usage endpoint again before this timestamp (backoff on 404/errors).
    pub probe_after: i64,
}

impl Default for QuotaState {
    fn default() -> Self {
        QuotaState {
            source: QuotaSource::Unknown,
            fetched_at: 0,
            rolling: QuotaWindow::default(),
            weekly: QuotaWindow::default(),
            monthly: QuotaWindow::default(),
            ledger: LocalLedger::default(),
            last_error: None,
            exhausted_until: 0,
            probe_after: 0,
        }
    }
}

/// A single quota window resolved for routing decisions.
#[derive(Debug, Clone, Copy)]
pub struct QuotaView {
    pub pct: f64,
    pub projected_pct: f64,
    pub resets_at: Option<i64>,
    pub has_data: bool,
    /// Absolute amount used inside this window (unit: see QuotaReport::unit).
    pub used: f64,
    /// Absolute limit of this window (0 = unknown / not bounded).
    pub limit: f64,
}

/// The measure behind a quota window's numbers, decided together with the local ledger below.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Accounting {
    /// The windows and the limits are both counted in tokens.
    Tokens,
    /// The windows are sums of money in the endpoint's own currency. Which currency is a separate
    /// label (`prices.currency`); amounts of different currencies are never combined.
    Money,
    /// There is nothing to account for; the endpoint's quota is unbounded or unused.
    None,
}

/// Decide what a quota block's numbers measure. "none" covers two different situations and they
/// must not be conflated: a plan whose windows are bounded but unitless is budgeted in money the
/// config never named (the prices block is the only currency on hand, so the totals are money),
/// while a plan with no windows at all has nothing to denominate and stays unitless.
fn accounting_of(quota: &QuotaCfg) -> Accounting {
    match quota.unit {
        QuotaUnit::Tokens => Accounting::Tokens,
        QuotaUnit::Usd | QuotaUnit::Rmb => Accounting::Money,
        QuotaUnit::None => {
            if quota.rolling > 0.0 || quota.weekly > 0.0 || quota.monthly > 0.0 {
                Accounting::Money
            } else {
                Accounting::None
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct QuotaReport {
    pub source: QuotaSource,
    pub unit: QuotaUnit,
    /// What the numbers in this report actually measure. `None` unit means the router cannot say
    /// anything about the windows, so the consumer should keep reporting `unit`; otherwise the
    /// ledger is denominated in `prices.currency` and this is what the windows are made of.
    pub accounting: Accounting,
    pub stale: bool,
    pub exhausted: bool,
    pub surplus: bool,
    pub rolling: QuotaView,
    pub weekly: QuotaView,
    pub monthly: QuotaView,
}

fn status_exhausted(s: &str) -> bool {
    let t = s.trim().to_ascii_lowercase();
    !t.is_empty() && t != "ok" && t != "normal" && t != "active"
}

const PERIOD_ROLLING: i64 = 5 * 3600;
const PERIOD_WEEKLY: i64 = 7 * 86400;
const PERIOD_MONTHLY: i64 = 30 * 86400;

fn view(
    win: &QuotaWindow,
    period: i64,
    now: i64,
    local: Option<(f64, f64)>,
    limit: f64,
    use_remote: bool,
) -> QuotaView {
    if use_remote && (win.pct > 0.0 || win.resets_at.is_some() || !win.status.is_empty()) {
        let frac = match win.resets_at {
            Some(r) => {
                let elapsed = (period - (r - now)).max(1) as f64;
                (elapsed / period as f64).clamp(0.08, 1.0)
            }
            None => 1.0,
        };
        return QuotaView {
            pct: win.pct,
            projected_pct: win.pct / frac,
            resets_at: win.resets_at,
            has_data: true,
            used: limit * win.pct / 100.0,
            limit,
        };
    }
    match local {
        Some((used, pct)) => QuotaView {
            pct,
            projected_pct: pct,
            resets_at: None,
            has_data: true,
            used,
            limit,
        },
        None => QuotaView {
            pct: 0.0,
            projected_pct: 0.0,
            resets_at: None,
            has_data: false,
            used: 0.0,
            limit,
        },
    }
}

impl QuotaState {
    /// Combine the remote usage API with the local ledger into a routing view.
    pub fn report(
        &self,
        now: i64,
        stale_after: i64,
        surplus_max_pct: f64,
        projection: bool,
        exhaust_at_pct: f64,
        quota: &QuotaCfg,
    ) -> QuotaReport {
        let stale = self.source != QuotaSource::Remote || now - self.fetched_at > stale_after;
        let use_remote = self.source == QuotaSource::Remote;
        // Local estimation in whatever unit the plan is budgeted in. Tokens need no price table.
        //
        // An empty ledger is only meaningful when the account has no remote probe at all: if a
        // probe is configured, "0 tokens seen by the router" says nothing about the provider's
        // counter (other tools may have consumed the plan), so we report "no data" and let the
        // router wait for the first reading instead of claiming the quota is unused.
        let trust_empty_ledger = quota.probe == crate::config::QuotaProbe::None;
        // What the windows are made of. A currency budget is spent from the ledger, whose amounts
        // are denominated in prices.currency; the "unit" label only names the measure.
        let accounting = accounting_of(quota);
        let local = |period: i64, limit: f64| -> Option<(f64, f64)> {
            if limit <= 0.0 || quota.unit == QuotaUnit::None {
                return None;
            }
            let used = match quota.unit {
                // Both currencies read the same amount: the unit is a label, never a conversion.
                // Listed explicitly rather than behind a guard, because a guard does not make a
                // match exhaustive and adding a currency would then fail to compile elsewhere.
                QuotaUnit::Usd | QuotaUnit::Rmb => self.ledger.window_cost(now, period),
                QuotaUnit::Tokens => self.ledger.window_tokens(now, period) as f64,
                QuotaUnit::None => return None,
            };
            if used <= 0.0 && !trust_empty_ledger {
                return None;
            }
            Some((used, (used / limit * 100.0).clamp(0.0, 999.0)))
        };
        let lp_rolling = local(PERIOD_ROLLING, quota.rolling);
        let lp_weekly = local(PERIOD_WEEKLY, quota.weekly);
        let lp_monthly = local(PERIOD_MONTHLY, quota.monthly);
        let has_local = lp_rolling.is_some() || lp_weekly.is_some() || lp_monthly.is_some();

        let rolling = view(&self.rolling, PERIOD_ROLLING, now, lp_rolling, quota.rolling, use_remote);
        let weekly = view(&self.weekly, PERIOD_WEEKLY, now, lp_weekly, quota.weekly, use_remote);
        let monthly = view(&self.monthly, PERIOD_MONTHLY, now, lp_monthly, quota.monthly, use_remote);

        let max_pct = rolling.pct.max(weekly.pct).max(monthly.pct);
        let max_projected = rolling.projected_pct.max(weekly.projected_pct).max(monthly.projected_pct);
        let exhausted = now < self.exhausted_until
            || (rolling.has_data && max_pct >= exhaust_at_pct)
            || status_exhausted(&self.rolling.status)
            || status_exhausted(&self.weekly.status)
            || status_exhausted(&self.monthly.status);
        let surplus = !exhausted
            && max_pct < surplus_max_pct
            && (!projection || max_projected < 100.0);
        QuotaReport {
            // The routing decision needs to know whether the numbers came from the provider
            // or from our own ledger, because "unknown" means something different from "0% used".
            source: match self.source {
                QuotaSource::Remote => QuotaSource::Remote,
                _ if has_local => QuotaSource::Local,
                other => other,
            },
            unit: quota.unit,
            accounting,
            stale,
            exhausted,
            surplus,
            rolling,
            weekly,
            monthly,
        }
    }

    /// `currency` is the endpoint's own money label (`prices.currency`, empty when unset). It is
    /// only consulted when the windows are money and the config could not name a unit, so that the
    /// console never shows amounts whose denomination it refuses to state.
    pub fn to_json(&self, now: i64, report: &QuotaReport, currency: &str) -> Value {
        let w = |v: &QuotaView| {
            json!({
                "percent": (v.pct * 10.0).round() / 10.0,
                "projected_percent": (v.projected_pct * 10.0).round() / 10.0,
                "used": (v.used * 100.0).round() / 100.0,
                "limit": v.limit,
                "resets_at": v.resets_at.map(timeutil::iso8601),
                "has_data": v.has_data,
            })
        };
        // Report the measure that actually produced the numbers below. The config's unit names a
        // unit only when a probe can express one; a ledger-budgeted plan has none, and echoing
        // "none" next to amounts of money is worse than useless.
        let unit = match (report.unit, report.accounting) {
            (QuotaUnit::None, Accounting::Money) if !currency.is_empty() => {
                currency.to_ascii_lowercase()
            }
            _ => report.unit.as_str().to_string(),
        };
        json!({
            "source": report.source.as_str(),
            "unit": unit,
            "stale": report.stale,
            "fetched_at": if self.fetched_at > 0 { Value::String(timeutil::iso8601(self.fetched_at)) } else { Value::Null },
            "exhausted": report.exhausted,
            "surplus": report.surplus,
            "exhausted_until": if self.exhausted_until > now { Value::String(timeutil::iso8601(self.exhausted_until)) } else { Value::Null },
            "rolling": w(&report.rolling),
            "weekly": w(&report.weekly),
            "monthly": w(&report.monthly),
            // The suffix is gone: the unit is the endpoint's currency, not necessarily dollars.
            "local_ledger": {
                "total": (self.ledger.total_cost * 1e6).round() / 1e6,
                "rolling": (self.ledger.window_cost(now, PERIOD_ROLLING) * 1e6).round() / 1e6,
                "weekly": (self.ledger.window_cost(now, PERIOD_WEEKLY) * 1e6).round() / 1e6,
                "monthly": (self.ledger.window_cost(now, PERIOD_MONTHLY) * 1e6).round() / 1e6,
            },
            "last_error": self.last_error,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct BalanceInfo {
    pub total: f64,
    pub currency: String,
    pub is_available: bool,
    pub fetched_at: i64,
}

#[derive(Debug)]
pub struct AccountRuntime {
    pub name: String,
    pub stats: Mutex<AccountStats>,
    pub quota: Mutex<QuotaState>,
    pub balance: Mutex<Option<BalanceInfo>>,
    pub models: Mutex<Option<(i64, Vec<String>)>>,
    pub cooldown_until: AtomicI64,
    pub cooldown_reason: Mutex<Option<String>>,

}

impl AccountRuntime {
    pub fn new(name: &str) -> AccountRuntime {
        AccountRuntime {
            name: name.to_string(),
            stats: Mutex::new(AccountStats::default()),
            quota: Mutex::new(QuotaState::default()),
            balance: Mutex::new(None),
            models: Mutex::new(None),
            cooldown_until: AtomicI64::new(0),
            cooldown_reason: Mutex::new(None),

        }
    }

    pub fn in_cooldown(&self, now: i64) -> bool {
        self.cooldown_until.load(Ordering::Relaxed) > now
    }

    pub fn cooldown_left(&self, now: i64) -> i64 {
        (self.cooldown_until.load(Ordering::Relaxed) - now).max(0)
    }

    pub fn set_cooldown(&self, until: i64, reason: &str) {
        let cur = self.cooldown_until.load(Ordering::Relaxed);
        if until > cur {
            self.cooldown_until.store(until, Ordering::Relaxed);
        }
        if let Ok(mut r) = self.cooldown_reason.lock() {
            *r = Some(reason.to_string());
        }
    }

    pub fn clear_cooldown(&self) {
        self.cooldown_until.store(0, Ordering::Relaxed);
        if let Ok(mut r) = self.cooldown_reason.lock() {
            *r = None;
        }
    }

    /// Has the account's quota been probed at least once (successfully or not)?
    /// Used so that an idle-hour decision is not made from a not-yet-available reading.
    pub fn quota_probe_attempted(&self) -> bool {
        let check = |q: &QuotaState| q.fetched_at > 0 || q.probe_after > 0 || q.last_error.is_some();
        match self.quota.lock() {
            Ok(q) => check(&q),
            Err(p) => check(&p.into_inner()),
        }
    }

    pub fn quota_report(&self, now: i64, cfg: &crate::config::Config, quota: &QuotaCfg) -> QuotaReport {
        let stale_after = (quota.refresh_secs as i64).max(cfg.router.quota_refresh_secs as i64) * 5;
        let blank = QuotaView {
            pct: 0.0,
            projected_pct: 0.0,
            resets_at: None,
            has_data: false,
            used: 0.0,
            limit: 0.0,
        };
        match self.quota.lock() {
            Ok(q) => q.report(
                now,
                stale_after,
                cfg.router.surplus_max_pct,
                cfg.router.surplus_projection,
                cfg.router.exhaust_at_pct,
                quota,
            ),
            Err(_) => QuotaReport {
                source: QuotaSource::Unknown,
                unit: quota.unit,
                // A poisoned lock is not a routing decision; the caller still renders the amounts
                // from the same ledger, so the measure behind them is unchanged.
                accounting: accounting_of(quota),
                stale: true,
                exhausted: false,
                surplus: false,
                rolling: blank,
                weekly: blank,
                monthly: blank,
            },
        }
    }

    pub fn record_success(&self, now: i64, usage: &UsageDelta, latency_ms: u64, stream: bool) {
        if let Ok(mut s) = self.stats.lock() {
            s.requests += 1;
            s.successes += 1;
            s.last_used = now;
            s.last_status = 200;
            s.latency_ms_total += latency_ms;
            if stream {
                s.stream_requests += 1;
            }
            s.prompt_tokens += usage.prompt_tokens;
            s.completion_tokens += usage.completion_tokens;
            s.cached_tokens += usage.cached_tokens;
            s.cost_usd += usage.cost_usd;
            s.saved_usd += usage.saved_usd;
        }
        let tokens = usage.prompt_tokens.saturating_add(usage.completion_tokens);
        if usage.cost_usd > 0.0 || tokens > 0 {
            if let Ok(mut q) = self.quota.lock() {
                q.ledger.add(now, usage.cost_usd, tokens);
            }
        }
    }

    pub fn record_error(&self, now: i64, status: u16, msg: &str, latency_ms: u64) {
        if let Ok(mut s) = self.stats.lock() {
            s.requests += 1;
            s.errors += 1;
            s.last_used = now;
            s.last_status = status;
            s.latency_ms_total += latency_ms;
            s.last_error = Some(format!("HTTP {} {}", status, crate::util::truncate(msg, 300)));
        }
    }

    pub fn to_json(&self, now: i64, cfg: &AccountCfg, report: &QuotaReport) -> Value {
        let s = self.stats.lock().map(|x| x.clone()).unwrap_or_default();
        let quota = self
            .quota
            .lock()
            .map(|q| q.to_json(now, report, cfg.prices.currency.as_str()))
            .unwrap_or(Value::Null);
        let balance = self.balance.lock().ok().and_then(|b| {
            b.as_ref().map(|v| {
                json!({
                    "total": v.total,
                    "currency": v.currency,
                    "is_available": v.is_available,
                    "fetched_at": timeutil::iso8601(v.fetched_at),
                })
            })
        });
        json!({
            "name": cfg.name,
            "kind": cfg.kind.as_str(),
            "order": cfg.order,
            "provider": match cfg.provider {
                crate::models::ProviderKind::OpencodeGo => "opencode-go",
                crate::models::ProviderKind::DeepSeek => "deepseek",
                crate::models::ProviderKind::Generic => "generic",
            },
            "url": cfg.base(),
            "modes": cfg.modes.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
            "rules": cfg.rules.iter().map(|r| r.as_str()).collect::<Vec<_>>(),
            "key": crate::util::redact(&cfg.key),
            "available": !self.in_cooldown(now),
            "cooldown_secs_left": self.cooldown_left(now),
            "cooldown_reason": self.cooldown_reason.lock().ok().and_then(|r| r.clone()),
            "balance": balance.unwrap_or(Value::Null),
            "quota": quota,
            "stats": {
                "requests": s.requests,
                "successes": s.successes,
                "errors": s.errors,
                "stream_requests": s.stream_requests,
                "prompt_tokens": s.prompt_tokens,
                "completion_tokens": s.completion_tokens,
                "cached_tokens": s.cached_tokens,
                // The unit travels with the number, so a reader never has to guess (or, worse,
                // assume dollars because that used to be the only possibility).
                "currency": cfg.prices.currency,
                "cost": (s.cost_usd * 1e6).round() / 1e6,
                "saved": (s.saved_usd * 1e6).round() / 1e6,
                "avg_latency_ms": if s.requests > 0 { s.latency_ms_total / s.requests } else { 0 },
                "last_used": if s.last_used > 0 { Value::String(timeutil::iso8601(s.last_used)) } else { Value::Null },
                "last_error": s.last_error,
            },
        })
    }
}

/// Token/cost delta of a single upstream exchange.
#[derive(Debug, Default, Clone, Copy)]
pub struct UsageDelta {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub cost_usd: f64,
    pub saved_usd: f64,
}

impl UsageDelta {
    pub fn merge(&mut self, other: UsageDelta) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.cached_tokens += other.cached_tokens;
        self.cost_usd += other.cost_usd;
        self.saved_usd += other.saved_usd;
    }
}

/// Registry of account runtime state, kept across config reloads by account name.
#[derive(Debug, Default)]
pub struct Registry {
    map: Mutex<HashMap<String, Arc<AccountRuntime>>>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry { map: Mutex::new(HashMap::new()) }
    }

    pub fn get_or_create(&self, name: &str) -> Arc<AccountRuntime> {
        let mut m = match self.map.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        m.entry(name.to_string())
            .or_insert_with(|| Arc::new(AccountRuntime::new(name)))
            .clone()
    }

    /// Drop runtime state for accounts that disappeared from the config.
    pub fn retain(&self, names: &[String]) {
        let mut m = match self.map.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        m.retain(|k, _| names.iter().any(|n| n == k));
    }

    pub fn all(&self) -> Vec<Arc<AccountRuntime>> {
        match self.map.lock() {
            Ok(m) => m.values().cloned().collect(),
            Err(p) => p.into_inner().values().cloned().collect(),
        }
    }

    /// Re-attach persisted counters at startup. Unknown names are dropped: an endpoint that no
    /// longer exists in the config has no runtime object to attach to.
    pub fn restore_stats(&self, stats: &HashMap<String, AccountStats>) {
        if stats.is_empty() {
            return;
        }
        let m = match self.map.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        let mut restored = 0usize;
        for (name, st) in stats {
            if let Some(rt) = m.get(name) {
                if let Ok(mut cur) = rt.stats.lock() {
                    *cur = st.clone();
                    restored += 1;
                }
            }
        }
        crate::log_info!("restored counters for {} endpoint(s)", restored);
    }

    /// Re-attach persisted ledgers at startup, so the quota view and the statistics cover the same
    /// period instead of one being since-restart.
    pub fn restore_ledgers(&self, ledgers: &HashMap<String, LocalLedger>) {
        if ledgers.is_empty() {
            return;
        }
        let m = match self.map.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        let mut restored = 0usize;
        for (name, l) in ledgers {
            let Some(rt) = m.get(name) else { continue };
            let mut q = match rt.quota.lock() {
                Ok(q) => q,
                Err(p) => p.into_inner(),
            };
            q.ledger = l.clone();
            restored += 1;
        }
        crate::log_info!("restored usage ledgers for {} endpoint(s)", restored);
    }

    /// Zero every per-endpoint counter. The local ledger and the cooldown clocks are left alone:
    /// the ledger is routing input (not a statistic), and clearing it would make a plan look
    /// unused and get burned preferentially.
    pub fn reset_stats(&self) {
        for rt in self.all() {
            if let Ok(mut s) = rt.stats.lock() {
                *s = AccountStats::default();
            }
        }
    }
}

/// Every endpoint's sliding-window ledger, keyed by name.
pub fn ledger_snapshot(state: &crate::proxy::AppState) -> HashMap<String, LocalLedger> {
    let mut out = HashMap::new();
    for rt in state.registry.all() {
        let l = match rt.quota.lock() {
            Ok(q) => q.ledger.clone(),
            Err(p) => p.into_inner().ledger.clone(),
        };
        if l.total_cost != 0.0 || l.total_tokens != 0 || !l.samples.is_empty() {
            out.insert(rt.name.clone(), l);
        }
    }
    out
}

/// Every endpoint that has recorded something, keyed by name.
pub fn stats_snapshot(state: &crate::proxy::AppState) -> HashMap<String, AccountStats> {
    let mut out = HashMap::new();
    for rt in state.registry.all() {
        let s = rt.stats.lock().map(|x| x.clone()).unwrap_or_default();
        if !s.is_empty() {
            out.insert(rt.name.clone(), s);
        }
    }
    out
}

/// Account = immutable config + shared runtime state.
#[derive(Debug)]
pub struct Account {
    pub cfg: AccountCfg,
    pub rt: Arc<AccountRuntime>,
}

impl Account {
    pub fn name(&self) -> &str {
        &self.cfg.name
    }

    pub fn kind(&self) -> AccountKind {
        self.cfg.kind
    }
}
