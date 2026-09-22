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
/// (or in addition to it).
///
/// The ledger stores **tokens, never money**. Money is a function of the prices in force at the
/// moment of reading, and the prices are configurable: folding them into the samples would freeze
/// whatever rate was configured when each request happened, so correcting a price could never fix
/// the history that rate produced, and re-deriving a price would have to rescale the whole ledger
/// to stay consistent. Keeping the counts means a price correction applies everywhere at once,
/// including backwards, and two models that share one allowance can be added up by pricing each
/// with its own rates.
#[derive(Debug, Default, Clone)]
pub struct LocalLedger {
    pub samples: VecDeque<LedgerSample>,
    pub total_tokens: u64,
}

/// One request's contribution to the ledger: when it happened and how many tokens of each class.
///
/// Per class rather than one total because the three are billed differently, and because the
/// calibration shows the user how much of each class the router forwarded since a baseline.
#[derive(Debug, Default, Clone, Copy)]
pub struct LedgerSample {
    pub ts: i64,
    pub prompt: u64,
    pub cached: u64,
    pub completion: u64,
}

/// The three token classes a request consumed, and the rates to price them with.
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    pub prompt: u64,
    pub cached: u64,
    pub completion: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.prompt.saturating_add(self.completion)
    }

    /// What these tokens cost at `prices`, in the endpoint's own currency.
    pub fn cost_at(&self, prices: &crate::config::PricesCfg, peak: bool) -> f64 {
        prices.cost_of(self.prompt, self.cached, self.completion, peak)
    }
}

impl LedgerSample {
    pub fn total_tokens(&self) -> u64 {
        self.prompt.saturating_add(self.completion)
    }

    pub fn usage(&self) -> Usage {
        Usage { prompt: self.prompt, cached: self.cached, completion: self.completion }
    }
}

/// How many ledger samples are written to the state file.
///
/// This used to be 2000 as a size cap. That was wrong: the calibration reads consumption as
/// `cost_between(baseline, now)`, and a truncated history makes that difference too small whenever
/// the baseline sits before the retained tail. The derived scale then comes out too large and the
/// whole ledger is rescaled by it - a wrong answer, not a rounding error. The samples are the only
/// record of *when* money was spent, so they have to survive restarts intact.
///
/// 20 000 samples is roughly 1 MB of JSON at typical sizes and covers a month of ordinary console
/// traffic with room to spare; beyond it the oldest samples are dropped, which degrades the rolling
/// windows oldest-first (exactly the ones that no longer matter).
const PERSISTED_SAMPLES: usize = 20_000;

/// How long an in-flight mark may stay set before it is treated as debris rather than traffic.
///
/// The longest legitimate hold is one upstream stream. `attempt_budget_secs` bounds a whole retry
/// chain but not an individual stream, so this is deliberately generous: an hour is far beyond any
/// productive generation and still short enough that a leaked mark clears itself within a session
/// rather than painting an idle endpoint busy until the next restart.
const IN_FLIGHT_MAX_HOLD_SECS: i64 = 3600;

impl LocalLedger {
    /// Totals plus a bounded tail of samples, for the state file.
    ///
    /// The ledger used to be memory-only on the grounds that it is routing input, and persisting it
    /// would invent usage the provider never confirmed. That reasoning stopped holding once the
    /// statistics themselves became persistent: the same requests were already being written to the
    /// same file, so a memory-only ledger only meant two numbers on one screen counted different
    /// periods (the dashboard showed a lifetime total, the quota decision a since-restart one).
    pub fn to_state_json(&self) -> Value {
        // Written oldest-first so the file stays readable in time order, and every sample that
        // survives the in-memory window is written - see PERSISTED_SAMPLES for why dropping the
        // old ones is not an option.
        let keep = self.samples.len().min(PERSISTED_SAMPLES);
        // [ts, prompt, cached, completion] - tokens only, in time order. Older files carried a
        // money column; `from_state_json` reads those too and keeps the counts, dropping the money.
        let samples: Vec<Value> = self
            .samples
            .iter()
            .skip(self.samples.len() - keep)
            .map(|s| json!([s.ts, s.prompt, s.cached, s.completion]))
            .collect();
        json!({
            "total_tokens": self.total_tokens,
            "samples": samples,
        })
    }

    pub fn from_state_json(v: &Value) -> LocalLedger {
        let mut l = LocalLedger::default();
        let Some(o) = v.as_object() else { return l };
        l.total_tokens = o.get("total_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
        if let Some(list) = o.get("samples").and_then(|x| x.as_array()) {
            for item in list {
                let Some(t) = item.as_array() else { continue };
                if t.len() < 2 {
                    continue;
                }
                let ts = t[0].as_i64().unwrap_or(0);
                if ts <= 0 {
                    continue;
                }
                // Three shapes exist on disk:
                //   [ts, cost, tokens]                        - money + a bare total
                //   [ts, cost, tokens, prompt, cached, completion] - money + the split
                //   [ts, prompt, cached, completion]          - the current, tokens-only form
                // All are read for their token counts, which is all the ledger means now. The money
                // column is discarded rather than reinterpreted: a rate frozen in an old file
                // cannot be trusted to price anything today.
                let (prompt, cached, completion) = if t.len() == 4 {
                    (
                        t[1].as_u64().unwrap_or(0),
                        t[2].as_u64().unwrap_or(0),
                        t[3].as_u64().unwrap_or(0),
                    )
                } else if t.len() >= 6 {
                    let (p, c, o) = (
                        t[3].as_u64().unwrap_or(0),
                        t[4].as_u64().unwrap_or(0),
                        t[5].as_u64().unwrap_or(0),
                    );
                    // An early version wrote the token total in column 2 and left the split at
                    // zero. That total is real usage and must not be dropped, or every such request
                    // silently disappears from the quota the moment rates are applied. Priced as
                    // output, the dearest class, so an unknown split never understates the spend.
                    if p == 0 && c == 0 && o == 0 {
                        (0, 0, t[2].as_u64().unwrap_or(0))
                    } else {
                        (p, c, o)
                    }
                } else if t.len() == 3 {
                    // Same case in its older, shorter form.
                    (0, 0, t[2].as_u64().unwrap_or(0))
                } else {
                    continue;
                };
                l.samples.push_back(LedgerSample { ts, prompt, cached, completion });
            }
        }
        l
    }

    /// Record one request's token counts. Money is not an input: it is computed from these when
    /// read, against whatever prices are configured at that moment.
    pub fn add_usage(&mut self, now: i64, usage: Usage) {
        self.samples.push_back(LedgerSample {
            ts: now,
            prompt: usage.prompt,
            cached: usage.cached,
            completion: usage.completion,
        });
        self.total_tokens += usage.total();
        // The in-memory cap and the persisted cap are the same number on purpose. If memory kept
        // more than the file accepts, a long-running process would answer from a longer history
        // than a restarted one and the two would disagree about the same baseline.
        while self.samples.len() > PERSISTED_SAMPLES {
            self.samples.pop_front();
        }
        let cutoff = now - 31 * 86400;
        while let Some(s) = self.samples.front() {
            if s.ts < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// Tokens of each class recorded at or after `since`, priced at `prices`.
    ///
    /// Every money figure the ledger reports goes through here, so a price correction applies to
    /// the whole retained history at once - that is the point of storing counts instead of money.
    pub fn cost_since(&self, since: i64, prices: &crate::config::PricesCfg, peak: bool) -> f64 {
        let mut prompt = 0u64;
        let mut cached = 0u64;
        let mut completion = 0u64;
        for s in self.samples.iter().rev().take_while(|s| s.ts >= since) {
            prompt += s.prompt;
            cached += s.cached;
            completion += s.completion;
        }
        let c = prices.cost_of(prompt, cached, completion, peak);
        // Summing nothing yields 0.0; the guard exists so a zero never prints as "-0.00".
        if c == 0.0 {
            0.0
        } else {
            c
        }
    }

    /// Cost recorded in the sliding window ending now, priced at `prices`.
    pub fn window_cost(&self, now: i64, period_secs: i64, prices: &crate::config::PricesCfg, peak: bool) -> f64 {
        self.cost_since(now - period_secs, prices, peak)
    }

    /// Tokens of each class recorded at or after `since`.
    pub fn usage_since_usage(&self, since: i64) -> Usage {
        let mut u = Usage::default();
        for s in self.samples.iter().rev().take_while(|s| s.ts >= since) {
            u.prompt += s.prompt;
            u.cached += s.cached;
            u.completion += s.completion;
        }
        u
    }

    pub fn tokens_since(&self, since: i64) -> u64 {
        self.samples
            .iter()
            .rev()
            .take_while(|s| s.ts >= since)
            .map(|s| s.total_tokens())
            .sum()
    }

    /// Per-class token totals recorded at or after `since`.
    ///
    /// This is the progress figure the calibration shows: it comes from the same samples the money
    /// does, so it cannot disagree with the L that the derivation divides by.
    pub fn usage_since(&self, since: i64) -> (u64, u64, u64) {
        let mut prompt = 0u64;
        let mut cached = 0u64;
        let mut completion = 0u64;
        for s in self.samples.iter().rev().take_while(|s| s.ts >= since) {
            prompt += s.prompt;
            cached += s.cached;
            completion += s.completion;
        }
        (prompt, cached, completion)
    }

    /// Consumption recorded in the half-open interval [from, to), priced at `prices`.
    ///
    /// Half-open because both ends are reading instants: a request stamped exactly at `from` was
    /// forwarded after that reading was taken (readings happen between requests, never inside one),
    /// while one stamped exactly at `to` belongs to the next window. Summing the two open ends
    /// would double-count a boundary sample.
    pub fn cost_between(&self, from: i64, to: i64, prices: &crate::config::PricesCfg, peak: bool) -> f64 {
        self.cost_since(from, prices, peak) - self.cost_since(to, prices, peak)
    }

    /// Combine several ledgers into one, in time order.
    ///
    /// Used when endpoints that used to keep separate records turn out to share an allowance: each
    /// record holds a different model's share of the same plan, so the shares add up. Samples that
    /// are identical in every field are kept once - a partially migrated file can hold the same
    /// request under both its endpoint name and its group, and counting it twice would inflate the
    /// plan by real, spendable money.
    pub fn merge(parts: Vec<LocalLedger>) -> LocalLedger {
        let mut all: Vec<LedgerSample> = Vec::new();
        for p in parts {
            all.extend(p.samples.into_iter());
        }
        all.sort_by_key(|s| s.ts);
        all.dedup_by(|a, b| {
            a.ts == b.ts && a.prompt == b.prompt && a.cached == b.cached && a.completion == b.completion
        });
        let total_tokens = all.iter().map(|s| s.total_tokens()).sum();
        let mut out = LocalLedger { samples: VecDeque::from(all), total_tokens };
        // The merged history can exceed the persistence cap; the same rule as `add_usage` applies.
        while out.samples.len() > PERSISTED_SAMPLES {
            out.samples.pop_front();
        }
        out
    }

    pub fn window_tokens(&self, now: i64, period_secs: i64) -> u64 {
        let from = now - period_secs;
        self.samples
            .iter()
            .rev()
            .take_while(|s| s.ts >= from)
            .map(|s| s.total_tokens())
            .sum()
    }
}

/// The baseline half of a calibration: the console's percentages at one moment.
///
/// Only `at` and the percentages are needed. Progress since the reading is read from the ledger by
/// timestamp, which is why no counter snapshot is stored here: a counter baseline went stale the
/// moment "reset data" zeroed those counters, and the difference it produced was silently the whole
/// counter value instead of the consumption since the reading.
#[derive(Debug, Clone, Default)]
pub struct QuotaReading {
    pub at: i64,
    pub pct_5h: f64,
    pub pct_week: f64,
    pub pct_month: f64,
}

impl QuotaReading {
    pub fn to_json(&self) -> Value {
        json!({
            "at": self.at,
            "pct_5h": self.pct_5h,
            "pct_week": self.pct_week,
            "pct_month": self.pct_month,
        })
    }
    pub fn from_json(v: &Value) -> Option<QuotaReading> {
        let o = v.as_object()?;
        let g = |k: &str| o.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let gi = |k: &str| o.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
        Some(QuotaReading {
            at: gi("at"),
            pct_5h: g("pct_5h"),
            pct_week: g("pct_week"),
            pct_month: g("pct_month"),
        })
    }
}

/// Everything a calibration flow keeps per endpoint, persisted as one block.
#[derive(Debug, Clone, Default)]
pub struct CalibrationEntry {
    pub reading: Option<QuotaReading>,
    pub derived: Option<QuotaCalibration>,
    pub anchors: QuotaAnchors,
}

/// The moments each provider window resets, copied from the console's countdowns.
///
/// Kept apart from the calibration on purpose: they are not part of the derivation math, they are
/// the bucket model that lets local percentages line up with the provider's. The user can correct
/// them at any time and the change applies the moment it arrives - a countdown copied off a console
/// and submitted two hours later describes the past, so there is no "pending" state to hold it.
#[derive(Debug, Clone, Default)]
pub struct QuotaAnchors {
    pub bucket_5h: i64,
    pub week_reset: i64,
}

impl QuotaAnchors {
    pub fn to_json(&self) -> Value {
        json!({ "bucket_5h": self.bucket_5h, "week_reset": self.week_reset })
    }
    pub fn from_json(v: &Value) -> QuotaAnchors {
        let gi = |k: &str| v.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
        QuotaAnchors { bucket_5h: gi("bucket_5h"), week_reset: gi("week_reset") }
    }
}

/// What a pair of readings proved about the plan, in true money (the plan price anchors it).
///
/// `rolling_total`/`weekly_total` are display facts: the 5-hour and weekly buckets are
/// deliberately kept out of the routing decision, because the upstream answers 40x on its own when
/// a bucket runs dry and the failover path handles that. They exist so the console can show the
/// same percentages the provider does.
#[derive(Debug, Clone, Default)]
pub struct QuotaCalibration {
    pub calibrated_at: i64,
    pub rolling_total: f64,
    pub weekly_total: f64,
    /// Set by a third reading agreeing with the prediction. Until then the derivation is unproven.
    pub verified_at: i64,
    /// Predicted minus actual, in percentage points, from the last verification reading.
    pub residual_pp: f64,
    /// The percentages and moment of the reference reading, so a verification compares deltas
    /// (pure forwarded traffic) rather than absolutes (which also contain usage the router never
    /// saw, e.g. whatever was spent before the endpoint was configured).
    ///
    /// These are also the level anchors for every derived window: the percentage the console
    /// displayed at `ref_at` is ground truth for that instant, and everything after it is the
    /// ledger's correctly-priced consumption. Nothing before the reference is ever needed again -
    /// a ledger that began mid-cycle is as good as a complete one once the reference exists.
    pub ref_pct_month: f64,
    pub ref_pct_5h: f64,
    pub ref_pct_week: f64,
    pub ref_at: i64,
}

impl QuotaCalibration {
    pub fn to_json(&self) -> Value {
        json!({
            "calibrated_at": self.calibrated_at,
            "rolling_total": self.rolling_total,
            "weekly_total": self.weekly_total,
            "verified_at": if self.verified_at > 0 { json!(self.verified_at) } else { Value::Null },
            "residual_pp": if self.verified_at > 0 { json!(self.residual_pp) } else { Value::Null },
            "ref_pct_month": self.ref_pct_month,
            "ref_pct_5h": self.ref_pct_5h,
            "ref_pct_week": self.ref_pct_week,
            "ref_at": self.ref_at,
        })
    }
    pub fn from_json(v: &Value) -> Option<QuotaCalibration> {
        let o = v.as_object()?;
        let gi = |k: &str| o.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
        let calibrated_at = gi("calibrated_at");
        // A derivation is stamped when it completes; an entry without that stamp is a phantom
        // (e.g. resurrected from a null by an earlier load bug) and must not come back.
        if calibrated_at <= 0 {
            return None;
        }
        let g = |k: &str| o.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
        Some(QuotaCalibration {
            calibrated_at,
            rolling_total: g("rolling_total"),
            weekly_total: g("weekly_total"),
            verified_at: gi("verified_at"),
            residual_pp: g("residual_pp"),
            ref_pct_month: g("ref_pct_month"),
            ref_pct_5h: g("ref_pct_5h"),
            ref_pct_week: g("ref_pct_week"),
            ref_at: gi("ref_at"),
        })
    }
}

/// Roll a recorded reset forward until it lies in the future, by whole periods.
pub fn roll_forward(reset: i64, period: i64, now: i64) -> Option<i64> {
    let mut r = reset;
    if r <= 0 {
        return None;
    }
    while r <= now {
        r += period;
    }
    Some(r)
}

#[derive(Debug, Clone)]
pub struct QuotaState {
    pub source: QuotaSource,
    pub fetched_at: i64,
    pub rolling: QuotaWindow,
    pub weekly: QuotaWindow,
    pub monthly: QuotaWindow,
    pub ledger: LocalLedger,
    /// A first reading awaiting its second, and the derivation of a completed pair.
    pub reading: Option<QuotaReading>,
    pub calibration: Option<QuotaCalibration>,
    /// Bucket reset moments copied from the provider's console; editable at any time.
    pub anchors: QuotaAnchors,
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
            reading: None,
            calibration: None,
            anchors: QuotaAnchors::default(),
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
    /// The start of the current billing cycle, when the plan has one. Carried on the report because
    /// the ledger views are cycle-aligned: a consumer computing an amount from the ledger needs the
    /// same window boundary the percentage above it used, and the boundary is knowable only here.
    pub cycle_start: Option<i64>,
}

/// The active billing cycle (start, next start) when the plan has one, else None.
///
/// Only the monthly window can be a cycle: the provider restarts the whole allowance on the
/// anchor day, whereas a 5-hour or weekly budget is a rolling limit with no such restart.
fn cycle_window(now: i64, quota: &QuotaCfg) -> Option<(i64, i64)> {
    if !quota.has_cycle() || quota.monthly <= 0.0 {
        return None;
    }
    Some(crate::timeutil::cycle_bounds(now, quota.cycle_day))
}

fn status_exhausted(s: &str) -> bool {
    let t = s.trim().to_ascii_lowercase();
    !t.is_empty() && t != "ok" && t != "normal" && t != "active"
}

pub const PERIOD_ROLLING: i64 = 5 * 3600;
pub const PERIOD_WEEKLY: i64 = 7 * 86400;
const PERIOD_MONTHLY: i64 = 30 * 86400;

/// `cycle` is the (start, next start) of a fixed billing cycle, when the plan has one. Unlike a
/// rolling window, those instants are knowable locally (they are anchor days), so the console can
/// show a countdown and a projection that match the provider's own.
fn view(
    win: &QuotaWindow,
    period: i64,
    now: i64,
    local: Option<(f64, f64)>,
    limit: f64,
    use_remote: bool,
    cycle: Option<(i64, i64)>,
    observed_from: Option<i64>,
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
        Some((used, pct)) => {
            // Projection needs the elapsed fraction of the window. For a fixed cycle we know both
            // ends outright; for a rolling one we assume the caller just started observing it, as
            // before, which is why the sliding case has never reported a projection.
            // `resets_at` is a fact about the provider's calendar and is reported whenever the
            // cycle is known, independently of whether a projection can be justified: the console
            // shows the countdown either way.
            let (projected, resets_at) = match cycle {
                Some((start, end)) if end > now => {
                    // The cycle's real length, not the nominal month: a billing period can be 28
                    // days (Jan 31 -> Feb 28), and projecting against 30 would understate it.
                    let len = (end - start).max(1) as f64;
                    let elapsed = (now - start).max(1) as f64;
                    // Extrapolating from a window we only started observing partway through is
                    // meaningless: the numerator is what the ledger saw, the denominator is the
                    // whole cycle, so a ledger that began yesterday reports a rate it never
                    // measured. `observed_from` is the ledger's own first sample; when it starts
                    // after the cycle does, there is no honest projection to give and the
                    // projection is suppressed (the reset instant is still reported).
                    if matches!(observed_from, Some(from) if from > start) {
                        (pct, Some(end))
                    } else {
                        // The anchor day itself makes `elapsed` a second or two, which pushed the
                        // ratio into the 999 cap. The remote branch has always floored its fraction
                        // at 8%; the local one needs the same floor to stay comparable.
                        let frac = (elapsed / len).clamp(0.08, 1.0);
                        (pct / frac, Some(end))
                    }
                }
                _ => (pct, None),
            };
            QuotaView {
                pct,
                projected_pct: projected.clamp(0.0, 999.0),
                resets_at,
                has_data: true,
                used,
                limit,
            }
        }
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
    /// `prices` and `peak` price the ledger's token counts. They are passed in rather than stored
    /// because the ledger keeps counts only: a money figure is a function of the rates in force when
    /// it is read, so a corrected rate fixes the whole retained history at once.
    pub fn report(
        &self,
        now: i64,
        stale_after: i64,
        surplus_max_pct: f64,
        projection: bool,
        exhaust_at_pct: f64,
        quota: &QuotaCfg,
        prices: &crate::config::PricesCfg,
        peak: bool,
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
        // A subscription plan restarts its allowance on a fixed day-of-month, so its window has an
        // absolute start rather than a trailing period. Everything else keeps sliding.
        let cycle = cycle_window(now, quota);
        let ledger_sum = |from: i64| match quota.unit {
            // Both currencies read the same amount: the unit is a label, never a conversion.
            // Listed explicitly rather than behind a guard, because a guard does not make a
            // match exhaustive and adding a currency would then fail to compile elsewhere.
            QuotaUnit::Usd | QuotaUnit::Rmb => self.ledger.cost_since(from, prices, peak),
            QuotaUnit::Tokens => self.ledger.tokens_since(from) as f64,
            QuotaUnit::None => 0.0,
        };
        let local = |period: i64, limit: f64, ref_anchor: Option<(i64, f64)>| -> Option<(f64, f64)> {
            if limit <= 0.0 || quota.unit == QuotaUnit::None {
                return None;
            }
            // Inside a fixed cycle the window runs from the cycle start, not a trailing period.
            //
            // A calibration reference replaces the ledger's own history for the level: the ledger
            // only reaches back to when it started recording, while the provider's console counts
            // the whole cycle, so summing the ledger from the cycle start undercounts by everything
            // that happened before the router first saw traffic (an entire month, when the ledger
            // was rebuilt mid-cycle). The provider's reading at `ref_at` is the level's ground
            // truth; the ledger adds what it forwarded since. Once the cycle rolls over, the
            // reference describes a window that no longer exists and the plain sum takes over -
            // by then the ledger has covered that new cycle from its start.
            let used = match cycle {
                Some((cyc_start, _)) => match ref_anchor {
                    Some((ref_at, ref_pct)) if ref_at >= cyc_start => {
                        limit * ref_pct / 100.0 + ledger_sum(ref_at)
                    }
                    _ => ledger_sum(cyc_start),
                },
                None => ledger_sum(now - period),
            };
            if used <= 0.0 && !trust_empty_ledger {
                return None;
            }
            Some((used, (used / limit * 100.0).clamp(0.0, 999.0)))
        };
        let lp_rolling = local(PERIOD_ROLLING, quota.rolling, None);
        let lp_weekly = local(PERIOD_WEEKLY, quota.weekly, None);
        // The monthly window anchors on the calibration reference when one is live: the ledger
        // sums only what it saw, and a ledger that began mid-cycle cannot know the level.
        let ref_anchor = match &self.calibration {
            Some(c) if c.ref_pct_month > 0.0 && c.ref_at > 0 => {
                Some((c.ref_at, c.ref_pct_month))
            }
            _ => None,
        };
        let lp_monthly = local(PERIOD_MONTHLY, quota.monthly, ref_anchor);
        let has_local = lp_rolling.is_some() || lp_weekly.is_some() || lp_monthly.is_some();

        // Only the monthly window has a cycle; the shorter windows stay rolling sums.
        // When the ledger's own history starts. A projection is only as good as the span it
        // measured, so a window the ledger did not cover from its beginning reports no projection.
        let observed_from = self.ledger.samples.front().map(|s| s.ts);
        let rolling = view(&self.rolling, PERIOD_ROLLING, now, lp_rolling, quota.rolling, use_remote, None, observed_from);
        let weekly = view(&self.weekly, PERIOD_WEEKLY, now, lp_weekly, quota.weekly, use_remote, None, observed_from);
        let monthly = view(&self.monthly, PERIOD_MONTHLY, now, lp_monthly, quota.monthly, use_remote, cycle, observed_from);

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
            cycle_start: cycle.map(|(s, _)| s),
        }
    }

    /// `currency` is the endpoint's own money label (`prices.currency`, empty when unset). It is
    /// only consulted when the windows are money and the config could not name a unit, so that the
    /// console never shows amounts whose denomination it refuses to state.
    pub fn to_json(
        &self,
        now: i64,
        report: &QuotaReport,
        currency: &str,
        _stats: &AccountStats,
        prices: &crate::config::PricesCfg,
        peak: bool,
    ) -> Value {
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
        // The 5-hour and weekly buckets that a calibration derived are display-only: routing never
        // sees them (their config limits stay 0, so the report carries no data for them), so the
        // views here are built straight from the derivation. The reference instant is shared by
        // all three windows: it is when the second reading was taken.
        let ref_at = self.calibration.as_ref().map(|c| c.ref_at).unwrap_or(0);
        //
        // The level comes from the reference reading, not from a bucket-aligned sum: the console's
        // percentage at `ref_at` is ground truth for that instant, and the ledger adds everything
        // forwarded since. Summing the bucket instead would need the ledger to cover the whole
        // bucket, which a mid-cycle ledger never does for the first week. Once the bucket rolls
        // over past the reference, the sum is both available and exact, so it takes over.
        let derived_view = |total: f64, window: i64, next_reset: Option<i64>, ref_pct: f64| -> Option<Value> {
            let reset = next_reset?;
            if total <= 0.0 || reset <= now {
                return None;
            }
            let start = reset - window;
            let ref_inside = ref_pct > 0.0 && ref_at > 0 && ref_at >= start && ref_at < reset;
            let used = if ref_inside {
                total * ref_pct / 100.0 + self.ledger.cost_since(ref_at, prices, peak)
            } else {
                // No reference in this bucket: fall back to the bucket sum, which is only honest
                // when the ledger reaches the bucket's start.
                if self.ledger.samples.front().map(|s| s.ts).is_some_and(|first| first > start) {
                    return None;
                }
                self.ledger.cost_since(start, prices, peak)
            };
            let pct = (used / total * 100.0).clamp(0.0, 999.0);
            Some(json!({
                "percent": (pct * 10.0).round() / 10.0,
                "projected_percent": (pct * 10.0).round() / 10.0,
                "used": (used * 100.0).round() / 100.0,
                "limit": total,
                "resets_at": timeutil::iso8601(reset),
                "has_data": true,
            }))
        };
        let rolling_view = match &self.calibration {
            Some(c) => {
                derived_view(c.rolling_total, PERIOD_ROLLING, roll_forward(self.anchors.bucket_5h, PERIOD_ROLLING, now), c.ref_pct_5h)
                    .unwrap_or_else(|| w(&report.rolling))
            }
            None => w(&report.rolling),
        };
        let weekly_view = match &self.calibration {
            Some(c) => {
                derived_view(c.weekly_total, PERIOD_WEEKLY, roll_forward(self.anchors.week_reset, PERIOD_WEEKLY, now), c.ref_pct_week)
                    .unwrap_or_else(|| w(&report.weekly))
            }
            None => w(&report.weekly),
        };
        json!({
            "source": report.source.as_str(),
            "unit": unit,
            "stale": report.stale,
            "fetched_at": if self.fetched_at > 0 { Value::String(timeutil::iso8601(self.fetched_at)) } else { Value::Null },
            "exhausted": report.exhausted,
            "surplus": report.surplus,
            "exhausted_until": if self.exhausted_until > now { Value::String(timeutil::iso8601(self.exhausted_until)) } else { Value::Null },
            "rolling": rolling_view,
            "weekly": weekly_view,
            "monthly": w(&report.monthly),
            // The suffix is gone: the unit is the endpoint's currency, not necessarily dollars.
            //
            // `monthly` is cycle-aligned when the plan has a cycle, matching the percentage above
            // it. A trailing 30-day sum would still hold the previous cycle on the day the provider
            // resets, so the amount and the percentage would disagree at exactly the moment the
            // operator looks: the percentage would read 0% beside the whole of last month's spend.
            "local_ledger": {
                "total": (self.ledger.cost_since(0, prices, peak) * 1e6).round() / 1e6,
                "rolling": (self.ledger.window_cost(now, PERIOD_ROLLING, prices, peak) * 1e6).round() / 1e6,
                "weekly": (self.ledger.window_cost(now, PERIOD_WEEKLY, prices, peak) * 1e6).round() / 1e6,
                "monthly": (self
                    .ledger
                    .cost_since(report.cycle_start.unwrap_or(now - PERIOD_MONTHLY), prices, peak)
                    * 1e6)
                    .round()
                    / 1e6,
            },
            "calibration": {
                "anchors": self.anchors.to_json(),
                "pending": self.reading.as_ref().map(|r| {
                    // What the router forwarded since the reading was taken, per token class: the
                    // progress signal that tells the user when the second reading is worth entering.
                    //
                    // Read from the ledger, not from the statistics counters. The counters are
                    // zeroed by "reset data" while this baseline survives, and subtracting a
                    // pre-reset baseline from a post-reset counter produced a figure that was
                    // silently the whole counter value - progress that looked plausible and was
                    // simply wrong. The ledger is also what the derivation divides by, so keeping
                    // both on one source means the progress shown and the maths applied agree.
                    let (prompt, cached, completion) = self.ledger.usage_since(r.at);
                    json!({
                        "at": r.at,
                        "pct_5h": r.pct_5h,
                        "pct_week": r.pct_week,
                        "pct_month": r.pct_month,
                        "accumulated": {
                            "cost": (self.ledger.cost_since(r.at, prices, peak) * 1e6).round() / 1e6,
                            "prompt": prompt,
                            "cached": cached,
                            "completion": completion,
                            "total": prompt + completion,
                        },
                    })
                }),
                "derived": self.calibration.as_ref().map(|c| c.to_json()),
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
    /// The allowance this endpoint draws on.
    ///
    /// Shared, not owned: endpoints configured against the same provider credential are two models
    /// on one plan, so they consume one allowance. Per-endpoint state made the router see two
    /// half-used budgets - failing to notice a plan run dry, and reporting the same consumption
    /// twice. The rates that price each model's tokens stay per endpoint, because the provider
    /// charges them differently; only the money they add up to is shared.
    pub quota: Arc<Mutex<QuotaState>>,
    /// The allowance group this endpoint belongs to (empty = its own). Quota state is keyed by
    /// group when persisted, so a restart reconstructs the same sharing.
    pub group: String,
    pub balance: Mutex<Option<BalanceInfo>>,
    pub models: Mutex<Option<(i64, Vec<String>)>>,
    pub cooldown_until: AtomicI64,
    pub cooldown_reason: Mutex<Option<String>>,
    /// Requests dispatched upstream but not yet finished. A long stream is invisible in the
    /// completion-time statistics for its whole duration, which reads as "this endpoint has been
    /// idle for minutes" while it is in fact mid-generation.
    pub in_flight: AtomicI64,
    /// When the newest in-flight mark was taken (`in_flight` counter), so a leaked mark can be
    /// aged out instead of misread as a live request.
    pub in_flight_since: AtomicI64,
}

impl AccountRuntime {
    pub fn new(name: &str) -> AccountRuntime {
        AccountRuntime::with_quota(name, String::new(), Arc::new(Mutex::new(QuotaState::default())))
    }

    /// Build a runtime bound to an existing allowance cell. Two endpoints given the same cell share
    /// everything the cell holds (readings, calibration, the token ledger) while keeping their own
    /// statistics, cooldowns and in-flight marks.
    pub fn with_quota(name: &str, group: String, quota: Arc<Mutex<QuotaState>>) -> AccountRuntime {
        AccountRuntime {
            name: name.to_string(),
            stats: Mutex::new(AccountStats::default()),
            quota,
            group,
            balance: Mutex::new(None),
            models: Mutex::new(None),
            cooldown_until: AtomicI64::new(0),
            cooldown_reason: Mutex::new(None),
            in_flight: AtomicI64::new(0),
            in_flight_since: AtomicI64::new(0),
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

    /// Store the first calibration reading (the console percentages, copied as-is).
    pub fn set_reading(&self, r: QuotaReading) {
        if let Ok(mut q) = self.quota.lock() {
            q.reading = Some(r);
        }
    }

    /// Read the pending first reading without consuming it.
    ///
    /// A `finish` that fails validation must not eat the reading: the user would have to re-copy
    /// percentages from the console for nothing. It is taken only once the derivation succeeds.
    pub fn peek_reading(&self) -> Option<QuotaReading> {
        self.quota.lock().ok().and_then(|q| q.reading.clone())
    }

    pub fn clear_reading(&self) {
        if let Ok(mut q) = self.quota.lock() {
            q.reading = None;
        }
    }

    /// Discard a derivation (used by `cancel` and when a fresh calibration replaces it).
    pub fn clear_calibration(&self) {
        if let Ok(mut q) = self.quota.lock() {
            q.calibration = None;
        }
    }

    /// Store the derivation of a completed reading pair.
    pub fn set_calibration(&self, c: QuotaCalibration) {
        if let Ok(mut q) = self.quota.lock() {
            q.calibration = Some(c);
        }
    }

    /// Update the bucket anchors (the console's reset moments). Applies immediately: a countdown
    /// copied off a console describes that instant, not whenever some form is eventually submitted.
    pub fn set_anchors(&self, a: QuotaAnchors) {
        if let Ok(mut q) = self.quota.lock() {
            q.anchors = a;
        }
    }

    /// Mark a request as dispatched upstream. The returned guard clears it on drop, so every exit
    /// path - success, upstream error, client abort - is covered by construction.
    ///
    /// This exists because the completion-time statistics are blind for the whole duration of a
    /// stream: a request that runs for a minute shows nothing anywhere, and the endpoint reads as
    /// idle while it is mid-generation.
    pub fn in_flight_guard(&self) -> crate::state::InFlightGuard<'_> {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        // Stamp the mark so a reader can tell "a request started long ago and its guard never
        // dropped" apart from "a request is running right now". Without this the two are the same
        // number and the stale one is indistinguishable from live traffic.
        self.in_flight_since
            .store(crate::util::now_secs(), Ordering::Relaxed);
        crate::state::InFlightGuard { rt: self, armed: true }
    }

    /// In-flight requests right now.
    ///
    /// The mark is a counter, not a set, so it cannot list which requests are running. It is also
    /// only as good as its guards: a connection thread killed mid-write, or any exit path that
    /// bypasses the guard destructor, leaves the counter high for good. An aged mark is therefore
    /// treated as debris and reported as zero rather than as traffic - a stale count is worse than
    /// no count, because it makes an idle endpoint look busy forever.
    pub fn in_flight(&self) -> i64 {
        let n = self.in_flight.load(Ordering::Relaxed);
        if n <= 0 {
            return 0;
        }
        let since = self.in_flight_since.load(Ordering::Relaxed);
        if since > 0 && crate::util::now_secs() - since > IN_FLIGHT_MAX_HOLD_SECS {
            return 0;
        }
        n
    }

    /// The raw counter, debris included. Diagnostics only.
    pub fn in_flight_raw(&self) -> i64 {
        self.in_flight.load(Ordering::Relaxed)
    }

    pub fn calibration(&self) -> Option<QuotaCalibration> {
        self.quota.lock().ok().and_then(|q| q.calibration.clone())
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

    pub fn quota_report(
        &self,
        now: i64,
        cfg: &crate::config::Config,
        quota: &QuotaCfg,
        prices: &crate::config::PricesCfg,
    ) -> QuotaReport {
        let stale_after = (quota.refresh_secs as i64).max(cfg.router.quota_refresh_secs as i64) * 5;
        let peak = cfg.is_peak(now);
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
                prices,
                peak,
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
                cycle_start: cycle_window(now, quota).map(|(s, _)| s),
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
        if tokens > 0 {
            if let Ok(mut q) = self.quota.lock() {
                // Only the counts go in. What they are worth is decided when the ledger is read, so
                // a price correction applies to this request too, however long ago it happened.
                q.ledger.add_usage(
                    now,
                    Usage {
                        prompt: usage.prompt_tokens,
                        cached: usage.cached_tokens,
                        completion: usage.completion_tokens,
                    },
                );
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

    pub fn to_json(&self, now: i64, cfg: &AccountCfg, report: &QuotaReport, peak: bool) -> Value {
        let s = self.stats.lock().map(|x| x.clone()).unwrap_or_default();
        let quota = self
            .quota
            .lock()
            .map(|q| q.to_json(now, report, cfg.prices.currency.as_str(), &s, &cfg.prices, peak))
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
    /// Allowance groups: group id -> the shared quota cell. See `get_or_create_in_group`.
    groups: Mutex<HashMap<String, Arc<Mutex<QuotaState>>>>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry { map: Mutex::new(HashMap::new()), groups: Mutex::new(HashMap::new()) }
    }

    /// Create (or fetch) an endpoint runtime, sharing an allowance with any endpoint already in
    /// `group`.
    ///
    /// `group` identifies the allowance, not the endpoint: endpoints that share a provider
    /// credential are two models on one plan and must consume one budget. The first member creates
    /// the cell; later members are handed it. An empty `group` leaves the endpoint on its own
    /// allowance, which is how every endpoint behaved before this existed.
    pub fn get_or_create_in_group(&self, name: &str, group: &str) -> Arc<AccountRuntime> {
        let mut m = match self.map.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        if let Some(existing) = m.get(name) {
            return existing.clone();
        }
        let cell = if group.is_empty() {
            Arc::new(Mutex::new(QuotaState::default()))
        } else {
            let mut g = match self.groups.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            g.entry(group.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(QuotaState::default())))
                .clone()
        };
        let rt = Arc::new(AccountRuntime::with_quota(name, group.to_string(), cell));
        m.insert(name.to_string(), rt.clone());
        rt
    }

    /// The key under which this endpoint's allowance is persisted: the group id when it shares
    /// one, its own name otherwise. Two endpoints on one plan therefore store and load one record,
    /// which is what keeps the sharing correct across a restart.
    pub fn quota_key(rt: &AccountRuntime) -> String {
        if rt.group.is_empty() {
            rt.name.clone()
        } else {
            rt.group.clone()
        }
    }

    /// The shared cell a group uses, if the group exists.
    pub fn group_quota(&self, group: &str) -> Option<Arc<Mutex<QuotaState>>> {
        let g = match self.groups.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        g.get(group).cloned()
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
        // Gather what belongs to each allowance. A file written before grouping lists one record per
        // endpoint, and each holds only that model's share of the plan; the shares must be merged
        // rather than one of them winning, or grouping would discard real consumption. Records are
        // merged by timestamp so a sample is never counted twice.
        let mut by_cell: HashMap<usize, (Arc<Mutex<QuotaState>>, Vec<LocalLedger>)> = HashMap::new();
        for rt in m.values() {
            let key = Registry::quota_key(rt);
            let mut parts: Vec<LocalLedger> = Vec::new();
            if let Some(l) = ledgers.get(&key) {
                parts.push(l.clone());
            }
            // Also pick up any per-endpoint record under this endpoint's own name: an old file has
            // no group record at all, and a partially migrated one may have both.
            if rt.name != key {
                if let Some(l) = ledgers.get(&rt.name) {
                    parts.push(l.clone());
                }
            }
            if parts.is_empty() {
                continue;
            }
            let id = Arc::as_ptr(&rt.quota) as usize;
            by_cell.entry(id).or_insert_with(|| (rt.quota.clone(), Vec::new())).1.extend(parts);
        }
        let mut restored = 0usize;
        for (_, (cell, parts)) in by_cell {
            let merged = LocalLedger::merge(parts);
            let mut q = match cell.lock() {
                Ok(q) => q,
                Err(p) => p.into_inner(),
            };
            q.ledger = merged;
            restored += 1;
        }
        crate::log_info!("restored usage ledgers for {} allowance(s)", restored);
    }

    /// Re-attach calibration flows at startup: a pending first reading, or a completed derivation.
    pub fn restore_calibrations(&self, map: &HashMap<String, CalibrationEntry>) {
        if map.is_empty() {
            return;
        }
        let m = match self.map.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        // Same keying as the ledger: one calibration per allowance, shared by its models.
        let mut done: Vec<usize> = Vec::new();
        let mut restored = 0usize;
        for rt in m.values() {
            let key = Registry::quota_key(rt);
            let Some(entry) = map.get(&key).or_else(|| map.get(&rt.name)) else { continue };
            let id = Arc::as_ptr(&rt.quota) as usize;
            if done.contains(&id) {
                continue;
            }
            done.push(id);
            let mut q = match rt.quota.lock() {
                Ok(q) => q,
                Err(p) => p.into_inner(),
            };
            q.reading = entry.reading.clone();
            q.calibration = entry.derived.clone();
            q.anchors = entry.anchors.clone();
            restored += 1;
        }
        crate::log_info!("restored calibration state for {} allowance(s)", restored);
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

    /// Drop pending calibration baselines. They are measured against the lifetime token counters,
    /// which a statistics reset zeroes - keeping a baseline afterwards would clamp the accumulated
    /// progress to zero until the counters re-climbed past it. Completed derivations stay: their
    /// totals are plan properties, and their verification runs off the ledger, which survives.
    pub fn clear_pending_readings(&self) {
        for rt in self.all() {
            rt.clear_reading();
        }
    }
}

/// Every allowance's sliding-window ledger, keyed by allowance (see `Registry::quota_key`).
///
/// A plan shared by several models writes one record: the map is keyed by group, so the second
/// model's write lands on the same key with the same value rather than duplicating the history.
pub fn ledger_snapshot(state: &crate::proxy::AppState) -> HashMap<String, LocalLedger> {
    let mut out = HashMap::new();
    for rt in state.registry.all() {
        let l = match rt.quota.lock() {
            Ok(q) => q.ledger.clone(),
            Err(p) => p.into_inner().ledger.clone(),
        };
        if l.total_tokens != 0 || !l.samples.is_empty() {
            out.insert(Registry::quota_key(&rt), l);
        }
    }
    out
}

/// The calibration flow per allowance: a pending first reading and/or a completed derivation.
/// Both survive restarts - the wizard spans a consumption window that can outlive the process.
/// A plan is calibrated once, so the entry is keyed by allowance and shared by its models.
pub fn calibration_snapshot(state: &crate::proxy::AppState) -> HashMap<String, CalibrationEntry> {
    let mut out = HashMap::new();
    for rt in state.registry.all() {
        let q = match rt.quota.lock() {
            Ok(q) => q,
            Err(p) => p.into_inner(),
        };
        if q.reading.is_some() || q.calibration.is_some() || q.anchors.bucket_5h > 0 || q.anchors.week_reset > 0 {
            out.insert(
                Registry::quota_key(&rt),
                CalibrationEntry {
                    reading: q.reading.clone(),
                    derived: q.calibration.clone(),
                    anchors: q.anchors.clone(),
                },
            );
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

/// Clears one in-flight mark on drop. Held for the whole upstream request, stream included.
///
/// The destructor is the normal path and covers success, upstream error and early return alike.
/// What it cannot cover is a thread that never unwinds normally - a connection thread torn down
/// mid-write can leave the mark behind. `AccountRuntime::in_flight` ages such debris out; the
/// explicit `clear` here is the belt to that pair of braces, so an ordinary early return drops the
/// count immediately instead of waiting for the age limit.
pub struct InFlightGuard<'a> {
    rt: &'a AccountRuntime,
    armed: bool,
}

impl InFlightGuard<'_> {
    /// Release the mark now. Idempotent: a second call is a no-op, so a caller may clear on the
    /// way out and still let the destructor run.
    pub fn clear(&mut self) {
        if self.armed {
            self.armed = false;
            self.rt.in_flight.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.clear();
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
