//! Endpoint ordering: peak/off-peak buckets, the order field, quota awareness, endpoint health.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::{AccountKind, Config, IdlePrefer, Rule};
use crate::models::{Endpoint, ForcedEndpoint};
use crate::state::Account;

/// Buckets are tried in policy order, and each bucket is consumed by "order:" internally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    /// Every healthy prepaid plan (used while busy / when the user forces plans).
    Plans,
    /// Prepaid plans whose quota would otherwise go unused (used off-peak).
    PlansSurplus,
    /// Prepaid plans we would rather keep for the busy hours.
    PlansTight,
    /// Pay-as-you-go accounts (fallback:).
    Cash,
}

impl Bucket {
    pub fn as_str(&self) -> &'static str {
        match self {
            Bucket::Plans => "plans",
            Bucket::PlansSurplus => "plans-surplus",
            Bucket::PlansTight => "plans-tight",
            Bucket::Cash => "fallback",
        }
    }
}

/// How a prepaid plan looks right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanClass {
    Exhausted,
    Surplus,
    Tight,
}

/// Classify a plan from its quota report. "Nothing measurable" counts as surplus because the
/// quota is already paid for, while "a probe is configured but has not answered yet" does not:
/// we would rather wait one probe than declare a plan unused.
pub fn classify_plan(acc: &Account, report: &crate::state::QuotaReport) -> PlanClass {
    if report.exhausted {
        return PlanClass::Exhausted;
    }
    match report.source {
        crate::state::QuotaSource::Remote | crate::state::QuotaSource::Local => {
            if report.surplus {
                PlanClass::Surplus
            } else {
                PlanClass::Tight
            }
        }
        crate::state::QuotaSource::Unknown => {
            if acc.cfg.quota.probe != crate::config::QuotaProbe::None && !acc.rt.quota_probe_attempted()
            {
                PlanClass::Tight
            } else {
                PlanClass::Surplus
            }
        }
    }
}

fn bucket_holds(bucket: Bucket, acc: &Arc<Account>, class: PlanClass) -> bool {
    match bucket {
        Bucket::Cash => acc.cfg.kind == AccountKind::Cash,
        Bucket::Plans => acc.cfg.kind == AccountKind::Plans,
        Bucket::PlansSurplus => acc.cfg.kind == AccountKind::Plans && class == PlanClass::Surplus,
        Bucket::PlansTight => acc.cfg.kind == AccountKind::Plans && class == PlanClass::Tight,
    }
}

#[derive(Debug, Clone)]
pub struct RoutePlan {
    /// Ordered candidate list (most preferred first).
    pub candidates: Vec<Arc<Account>>,
    pub is_peak: bool,
    /// Some plan's quota would still be left over at the end of its window.
    pub plans_surplus: bool,
    pub plans_available: bool,
    pub preferred: Side,
    pub reason: String,
    /// Accounts filtered out before selection, with the reason (for logging/stats).
    pub skipped: Vec<(String, String)>,
}

/// Which side of the configuration is being preferred for this request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Prepaid quota buckets (plans:)
    Plans,
    /// Pay-as-you-go cash accounts (fallback:)
    Cash,
}

impl Side {
    pub fn as_str(&self) -> &'static str {
        match self {
            Side::Plans => "plans",
            Side::Cash => "fallback",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RuleCtx {
    pub is_peak: bool,
    pub plans_surplus: bool,
    pub plans_available: bool,
}

pub fn rule_matches(rule: Rule, ctx: RuleCtx) -> bool {
    match rule {
        Rule::Always => true,
        Rule::Peak => ctx.is_peak,
        Rule::OffPeak => !ctx.is_peak,
        Rule::QuotaLow => !ctx.plans_surplus,
        Rule::QuotaExhausted => !ctx.plans_available,
        Rule::PrimaryUnavailable => !ctx.plans_available,
        Rule::Never => false,
    }
}

/// Per-endpoint health memory that is NOT part of the account config: consecutive failures of the
/// "this endpoint cannot serve it" kind, and an optional skip window on top of the normal cooldown.
/// Without it a misconfigured endpoint would look merely "slow" (the retry chain hides the error).
#[derive(Debug, Default, Clone)]
pub struct EndpointHealth {
    /// consecutive failures that look like a configuration/upstream-capability problem
    pub streak: u32,
    /// endpoint is skipped entirely until this timestamp
    pub skip_until: i64,
    pub last_reason: Option<String>,
    pub total_errors: u64,
}

#[derive(Debug, Default)]
pub struct HealthMap {
    map: Mutex<HashMap<String, EndpointHealth>>,
}

impl HealthMap {
    pub fn get(&self, name: &str) -> EndpointHealth {
        self.map
            .lock()
            .ok()
            .and_then(|m| m.get(name).cloned())
            .unwrap_or_default()
    }

    pub fn snapshot(&self) -> HashMap<String, EndpointHealth> {
        self.map.lock().map(|m| m.clone()).unwrap_or_default()
    }

    /// Record a failure. Returns true when the endpoint just entered a skip window.
    pub fn record_failure(&self, name: &str, now: i64, reason: &str, skip_after: u32, skip_secs: u64) -> bool {
        let mut m = match self.map.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        let e = m.entry(name.to_string()).or_default();
        e.streak = e.streak.saturating_add(1);
        e.total_errors += 1;
        e.last_reason = Some(reason.to_string());
        if skip_after > 0 && e.streak >= skip_after && now >= e.skip_until {
            e.skip_until = now + skip_secs as i64;
            return true;
        }
        false
    }

    pub fn record_success(&self, name: &str) {
        let mut m = match self.map.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        if let Some(e) = m.get_mut(name) {
            e.streak = 0;
            e.skip_until = 0;
            e.last_reason = None;
        }
    }

    /// Restore persisted health (startup) and produce a snapshot for persistence.
    pub fn restore(&self, data: HashMap<String, EndpointHealth>) {
        if let Ok(mut m) = self.map.lock() {
            *m = data;
        }
    }

    /// Forget every remembered failure and skip window ("reset statistics" in the console).
    pub fn clear(&self) {
        let mut m = match self.map.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        m.clear();
    }
}

/// conversation -> endpoint affinity (endpoint-level; keeps upstream session/cache semantics).
#[derive(Debug, Default)]
pub struct AffinityMap {
    map: Mutex<HashMap<String, (String, i64)>>,
}

impl AffinityMap {
    pub fn get(&self, session: &str, now: i64, ttl: i64) -> Option<String> {
        if ttl <= 0 {
            return None;
        }
        let m = self.map.lock().ok()?;
        m.get(session).and_then(|(acct, ts)| {
            if now - *ts <= ttl {
                Some(acct.clone())
            } else {
                None
            }
        })
    }

    pub fn put(&self, session: &str, endpoint: &str, now: i64) {
        if session.is_empty() {
            return;
        }
        let mut m = match self.map.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        if m.len() > 4096 {
            let cutoff = now - 86400;
            m.retain(|_, (_, ts)| *ts >= cutoff);
            if m.len() > 4096 {
                m.clear();
            }
        }
        m.insert(session.to_string(), (endpoint.to_string(), now));
    }
}

pub struct Router {
    pub health: HealthMap,
    pub affinity: AffinityMap,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl Router {
    pub fn new() -> Router {
        Router {
            health: HealthMap::default(),
            affinity: AffinityMap::default(),
        }
    }

    /// Is this endpoint temporarily skipped because it kept failing?
    pub fn is_skipped(&self, name: &str, now: i64) -> bool {
        let h = self.health.get(name);
        h.skip_until > now
    }

    /// Compute the ordered candidate list for one request.
    ///
    /// Endpoints are interchangeable nodes: each one serves exactly one hard-coded model, so the
    /// client's model string does not filter anything (it can only *pin* one endpoint).
    /// Order of preference:
    ///   1. buckets, per the peak/off-peak policy;
    ///   2. inside a bucket, ascending "order:" (lower first), then the intra-group strategy;
    ///   3. a second pass adds everything else as a last resort (cooling/skipped endpoints too,
    ///      unless they opted out with no_error_fallback).
    pub fn plan(
        &self,
        cfg: &Config,
        accounts: &[Arc<Account>],
        endpoint: Endpoint,
        forced: Option<&ForcedEndpoint>,
        session: Option<&str>,
        now: i64,
    ) -> RoutePlan {
        let mut skipped: Vec<(String, String)> = Vec::new();

        // ---- 1. protocol / forced-endpoint filters -------------------------------
        let required = endpoint.required_mode();
        let mut usable: Vec<Arc<Account>> = Vec::new();
        for acc in accounts {
            // A forced pin bypasses every routing policy and goes straight to that endpoint.
            if let Some(f) = forced {
                let hit = match f {
                    ForcedEndpoint::Name(name) => acc.name() == name,
                    ForcedEndpoint::ProviderModel(provider, model) => {
                        acc.cfg.provider == *provider && acc.cfg.model.eq_ignore_ascii_case(model)
                    }
                };
                if !hit {
                    skipped.push((acc.name().to_string(), "forced elsewhere".to_string()));
                    continue;
                }
            }
            if let Some(m) = required {
                if !acc.cfg.supports_mode(m) {
                    skipped.push((acc.name().to_string(), format!("no {} support", m.as_str())));
                    continue;
                }
            }
            usable.push(acc.clone());
        }

        // ---- 2. classify every prepaid plan -------------------------------------
        // surplus = this plan's quota would still be left over at the end of its window, so
        // spending it now costs nothing that we would have used later.
        let mut plans_surplus = false;
        let mut plans_available = false;
        let mut plans_by_name: std::collections::HashMap<String, PlanClass> =
            std::collections::HashMap::new();
        for acc in usable.iter().filter(|a| side_of(a) == Side::Plans) {
            let report = acc.rt.quota_report(now, cfg, &acc.cfg.quota);
            let class = classify_plan(acc, &report);
            if class != PlanClass::Exhausted {
                plans_available = true;
                if class == PlanClass::Surplus {
                    plans_surplus = true;
                }
            }
            plans_by_name.insert(acc.name().to_string(), class);
        }
        let class_of = |acc: &Arc<Account>| -> PlanClass {
            plans_by_name
                .get(acc.name())
                .copied()
                .unwrap_or(PlanClass::Tight)
        };

        // ---- 3. which buckets are tried, in which order? -------------------------
        let is_peak = cfg.is_peak(now);
        let buckets: Vec<Bucket> = match (cfg.router.mode, is_peak) {
            (crate::config::RouterMode::Plans, _) => vec![Bucket::Plans, Bucket::Cash],
            (crate::config::RouterMode::Fallback, _) => vec![Bucket::Cash, Bucket::Plans],
            (crate::config::RouterMode::Auto, true) => vec![Bucket::Plans, Bucket::Cash],
            (crate::config::RouterMode::Auto, false) => match cfg.router.idle_prefer {
                IdlePrefer::Plans => vec![Bucket::Plans, Bucket::Cash],
                IdlePrefer::Fallback => vec![Bucket::Cash, Bucket::Plans],
                // Off-peak: spend plans that would otherwise go unused, then the cheap(er) cash
                // accounts, and only afterwards the plans we want to keep for the busy hours.
                IdlePrefer::SurplusFirst => {
                    vec![Bucket::PlansSurplus, Bucket::Cash, Bucket::PlansTight]
                }
            },
        };
        let preferred = match buckets.first() {
            Some(Bucket::Cash) => Side::Cash,
            _ => Side::Plans,
        };
        let ctx = RuleCtx {
            is_peak,
            plans_surplus,
            plans_available,
        };

        // ---- 4. assemble candidates --------------------------------------------
        let mut candidates: Vec<Arc<Account>> = Vec::new();
        let mut added: std::collections::HashSet<String> = std::collections::HashSet::new();
        for pass in 0..2 {
            for bucket in buckets.iter() {
                for acc in
                    self.bucket_accounts(cfg, *bucket, &usable, now, pass, ctx, &class_of, &mut skipped, forced.is_some())
                {
                    if added.insert(acc.name().to_string()) {
                        candidates.push(acc);
                    }
                }
            }
        }

        // ---- 5. conversation affinity (only while that endpoint is healthy) -------------
        if cfg.router.session_affinity && forced.is_none() {
            if let Some(session) = session {
                if let Some(name) = self
                    .affinity
                    .get(session, now, cfg.router.session_affinity_ttl_secs as i64)
                {
                    let healthy = candidates.iter().any(|c| c.name() == name)
                        && !self.is_skipped(&name, now);
                    if healthy {
                        if let Some(pos) = candidates.iter().position(|c| c.name() == name) {
                            if pos > 0 {
                                let acc = candidates.remove(pos);
                                candidates.insert(0, acc);
                            }
                        }
                    }
                }
            }
        }

        // A forced endpoint needs no further ordering: it is the only candidate.
        if forced.is_some() {
            let reason = format!("forced={} candidates={}", "explicit", candidates.len());
            return RoutePlan {
                is_peak: cfg.is_peak(now),
                plans_surplus: false,
                plans_available: true,
                preferred: Side::Plans,
                reason,
                candidates,
                skipped,
            };
        }

        let reason = format!(
            "{} prefer={} plans_surplus={} plans_available={} buckets={}",
            if is_peak { "peak" } else { "offpeak" },
            preferred.as_str(),
            plans_surplus,
            plans_available,
            buckets
                .iter()
                .map(|b| b.as_str())
                .collect::<Vec<_>>()
                .join(">")
        );

        RoutePlan {
            candidates,
            is_peak,
            plans_surplus,
            plans_available,
            preferred,
            reason,
            skipped,
        }
    }

    /// One bucket of accounts for one pass, ordered by (order, intra-group strategy).
    #[allow(clippy::too_many_arguments)]
    fn bucket_accounts(
        &self,
        cfg: &Config,
        bucket: Bucket,
        usable: &[Arc<Account>],
        now: i64,
        pass: usize,
        ctx: RuleCtx,
        class_of: &dyn Fn(&Arc<Account>) -> PlanClass,
        skipped: &mut Vec<(String, String)>,
        pinned: bool,
    ) -> Vec<Arc<Account>> {
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<i32, Vec<Arc<Account>>> = BTreeMap::new();
        for acc in usable.iter().filter(|a| bucket_holds(bucket, a, class_of(a))) {
            groups.entry(acc.cfg.order).or_default().push(acc.clone());
        }
        let mut out: Vec<Arc<Account>> = Vec::new();
        for (_order, group) in groups.into_iter() {
            let mut keep: Vec<Arc<Account>> = Vec::new();
            for acc in group {
                let healthy = !acc.rt.in_cooldown(now);
                let report = acc.rt.quota_report(now, cfg, &acc.cfg.quota);
                let rules_ok = acc.cfg.rules.iter().any(|r| rule_matches(*r, ctx));
                let prefer_eligible = match acc.cfg.kind {
                    AccountKind::Plans => !report.exhausted && rules_ok,
                    AccountKind::Cash => rules_ok,
                };
                // "enabled: false" means the endpoint is not a candidate in any pass - otherwise
                // disabling it in the console would still leave it serving traffic as a fallback.
                // An explicit client pin overrides it: that is a deliberate, one-off request.
                if !acc.cfg.is_enabled() && !pinned {
                    skipped.push((acc.name().to_string(), "disabled".to_string()));
                    continue;
                }
                if pass == 0 {
                    if !healthy {
                        skipped.push((
                            acc.name().to_string(),
                            format!("cooldown {}s", acc.rt.cooldown_left(now)),
                        ));
                        continue;
                    }
                    if !prefer_eligible {
                        let why = if report.exhausted {
                            "quota exhausted"
                        } else {
                            "rule not matched"
                        };
                        skipped.push((acc.name().to_string(), why.to_string()));
                        continue;
                    }
                } else if !rules_ok && acc.cfg.no_error_fallback {
                    skipped.push((acc.name().to_string(), "no_error_fallback".to_string()));
                    continue;
                }
                keep.push(acc);
            }
            // Within one order group the list position is the sequence: the console writes
            // "order" from the row position, so position and order can never disagree.
            out.extend(keep);
        }
        out
    }

    /// Accounts of one side for one pass, ordered by (order, intra-group strategy).
    fn side_candidates(
        &self,
        cfg: &Config,
        side: Side,
        usable: &[Arc<Account>],
        now: i64,
        pass: usize,
        ctx: RuleCtx,
        skipped: &mut Vec<(String, String)>,
        pinned: bool,
    ) -> Vec<Arc<Account>> {
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<i32, Vec<Arc<Account>>> = BTreeMap::new();
        for acc in usable.iter().filter(|a| side_of(a) == side) {
            groups.entry(acc.cfg.order).or_default().push(acc.clone());
        }
        let mut out: Vec<Arc<Account>> = Vec::new();
        for (_order, group) in groups.into_iter() {
            let mut keep: Vec<Arc<Account>> = Vec::new();
            for acc in group {
                let healthy = !acc.rt.in_cooldown(now);
                let report = acc.rt.quota_report(now, cfg, &acc.cfg.quota);
                let rules_ok = acc.cfg.rules.iter().any(|r| rule_matches(*r, ctx));
                let prefer_eligible = match side {
                    Side::Plans => !report.exhausted && rules_ok,
                    Side::Cash => rules_ok,
                };
                if !acc.cfg.is_enabled() && !pinned {
                    skipped.push((acc.name().to_string(), "disabled".to_string()));
                    continue;
                }
                if pass == 0 {
                    if !healthy {
                        skipped.push((
                            acc.name().to_string(),
                            format!("cooldown {}s", acc.rt.cooldown_left(now)),
                        ));
                        continue;
                    }
                    if !prefer_eligible {
                        let why = if side == Side::Plans && report.exhausted {
                            "quota exhausted"
                        } else {
                            "rule not matched"
                        };
                        skipped.push((acc.name().to_string(), why.to_string()));
                        continue;
                    }
                } else if !rules_ok && acc.cfg.no_error_fallback {
                    skipped.push((acc.name().to_string(), "no_error_fallback".to_string()));
                    continue;
                }
                keep.push(acc);
            }
            // Within one order group the list position is the sequence: the console writes
            // "order" from the row position, so position and order can never disagree.
            out.extend(keep);
        }
        out
    }
}

/// Which side an account belongs to.
pub fn side_of(acc: &Account) -> Side {
    match acc.cfg.kind {
        crate::config::AccountKind::Plans => Side::Plans,
        crate::config::AccountKind::Cash => Side::Cash,
    }
}

/// Hint names accepted in the model field, e.g. "go/deepseek-flash", "cash/deepseek-v4-pro".
pub const HINT_PLANS: [&str; 3] = ["go", "plans", "prepaid"];

