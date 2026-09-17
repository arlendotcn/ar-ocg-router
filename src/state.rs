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

impl LocalLedger {
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
        self.samples
            .iter()
            .rev()
            .take_while(|(ts, _, _)| *ts >= from)
            .map(|(_, c, _)| *c)
            .sum()
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

#[derive(Debug, Clone, Copy)]
pub struct QuotaReport {
    pub source: QuotaSource,
    pub unit: QuotaUnit,
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
        let local = |period: i64, limit: f64| -> Option<(f64, f64)> {
            if limit <= 0.0 || quota.unit == QuotaUnit::None {
                return None;
            }
            let used = match quota.unit {
                QuotaUnit::Usd => self.ledger.window_cost(now, period),
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
            stale,
            exhausted,
            surplus,
            rolling,
            weekly,
            monthly,
        }
    }

    pub fn to_json(&self, now: i64, report: &QuotaReport) -> Value {
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
        json!({
            "source": report.source.as_str(),
            "unit": report.unit.as_str(),
            "stale": report.stale,
            "fetched_at": if self.fetched_at > 0 { Value::String(timeutil::iso8601(self.fetched_at)) } else { Value::Null },
            "exhausted": report.exhausted,
            "surplus": report.surplus,
            "exhausted_until": if self.exhausted_until > now { Value::String(timeutil::iso8601(self.exhausted_until)) } else { Value::Null },
            "rolling": w(&report.rolling),
            "weekly": w(&report.weekly),
            "monthly": w(&report.monthly),
            "local_ledger": {
                "total_usd": (self.ledger.total_cost * 1e6).round() / 1e6,
                "rolling_usd": (self.ledger.window_cost(now, PERIOD_ROLLING) * 1e6).round() / 1e6,
                "weekly_usd": (self.ledger.window_cost(now, PERIOD_WEEKLY) * 1e6).round() / 1e6,
                "monthly_usd": (self.ledger.window_cost(now, PERIOD_MONTHLY) * 1e6).round() / 1e6,
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
            .map(|q| q.to_json(now, report))
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
                "cost_usd": (s.cost_usd * 1e6).round() / 1e6,
                "saved_usd": (s.saved_usd * 1e6).round() / 1e6,
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
