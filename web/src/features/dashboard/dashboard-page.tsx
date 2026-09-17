"use client";

import * as React from "react";
import Link from "next/link";
import { useI18n } from "@/lib/i18n";
import { useStatsStream } from "@/lib/use-stats";
import { api } from "@/lib/api";
import { useToast } from "@/components/ui/toast";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Meter } from "@/components/ui/meter";
import { Plate, PlateBlock, Readout } from "@/components/ui/plate";
import { PageHeader } from "@/components/page-header";
import { fmtBytes, fmtAgo, fmtDuration, fmtNum, fmtPct, fmtUsd, fmtWhen } from "@/lib/format";
import type { Account } from "@/types/api";
import { cn } from "@/lib/utils";

export function DashboardPage() {
  const { t, lang } = useI18n();
  const toast = useToast();
  const { stats, error, loading, paused, setPaused, refresh, updatedAt } = useStatsStream();
  const [tick, setTick] = React.useState(0);
  // Which endpoint is being toggled right now, so the button cannot be double-fired.
  const [busy, setBusy] = React.useState<string | null>(null);

  const setEndpointEnabled = async (name: string, enabled: boolean) => {
    setBusy(name);
    try {
      await api.setEndpointState(name, enabled);
      toast.push("ok", `${name}: ${enabled ? t.common.enabled : t.common.disabled}`);
      refresh();
    } catch (e) {
      toast.push("err", e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };

  // the countdown needs its own 1s heartbeat; the data itself arrives every 2s
  React.useEffect(() => {
    const id = setInterval(() => setTick((v) => v + 1), 1000);
    return () => clearInterval(id);
  }, []);

  const secondsLeft = React.useMemo(() => {
    if (!stats) return 0;
    const drift = paused ? 0 : Math.floor((Date.now() - updatedAt) / 1000);
    return Math.max(0, stats.router.seconds_to_change - drift);
  }, [stats, updatedAt, paused, tick]);

  if (loading && !stats) return <Skeleton t={t} />;
  if (error && !stats) {
    return (
      <Plate className="p-4">
        <p className="mono text-sm text-[var(--danger)]">{error}</p>
        <p className="mt-2 text-sm text-[var(--ink-faint)]">{t.err.offline}</p>
        <Button className="mt-3" onClick={refresh}>
          {t.common.retry}
        </Button>
      </Plate>
    );
  }
  if (!stats) return null;

  const r = stats.router;
  const c = stats.counters;
  const plans = stats.accounts.filter((a) => a.kind === "plans");
  const cash = stats.accounts.filter((a) => a.kind === "fallback");
  // The roster is the routing order, which is what the order field means. Ties keep the
  // configured order (sort is stable), so equal orders stay side by side.
  const roster = [...stats.accounts].sort((x, y) => x.order - y.order);

  return (
    <div className="space-y-4">
      <PageHeader
        title={t.dash.title}
        meta={
          <Badge tone={r.peak ? "signal" : "plain"} dot>
            {r.peak ? t.dash.peak : t.dash.offpeak}
          </Badge>
        }
        actions={
          <>
            <Button size="sm" variant="ghost" onClick={() => setPaused(!paused)}>
              {paused ? t.dash.resume : t.dash.pause}
            </Button>
            <Button size="sm" variant="outline" onClick={refresh}>
              {t.common.refresh}
            </Button>
          </>
        }
      />

      {/* ---- the clock plate: the single most important thing on the page ----
           Six columns so the clock can take half the width while the three supporting readouts
           share the other half exactly: 3 + 1 + 1 + 1 = 6. A 4-column grid with a col-span-2
           clock and three 1-wide cells sums to 5 and wraps the last one onto its own row. */}
      <Plate className="rise overflow-hidden">
        <div className="grid grid-cols-1 sm:grid-cols-6">
          <div className="border-b border-[var(--line)] px-3 py-3 sm:col-span-3 sm:border-b-0 sm:border-r">
            <div className="flex items-center gap-2">
              <span
                className={cn("h-[7px] w-[7px] rounded-full", r.peak && "live-dot")}
                style={{ background: r.peak ? "var(--signal)" : "var(--ink-faint)" }}
              />
              <span className="label">{t.dash.now}</span>
              {r.fake_now ? <Badge tone="warn">fake now</Badge> : null}
            </div>
            <div className="tabular mono mt-2 text-3xl leading-none sm:text-3xl">
              {r.peak ? t.dash.peak : t.dash.offpeak}
            </div>
            <div className="mt-2 text-xs text-[var(--ink-faint)]">{fmtWhen(r.now, lang)}</div>
          </div>
          <Readout
            label={t.dash.nextChange}
            value={fmtDuration(secondsLeft, lang)}
            sub={fmtWhen(r.next_change, lang)}
            className="border-b border-[var(--line)] sm:border-b-0 sm:border-r"
          />
          <Readout
            label={t.dash.windows}
            value={<span className="mono text-sm leading-tight">{r.peak_windows}</span>}
            sub={`${t.policy.mode}: ${r.mode} · ${r.idle_prefer}`}
            className="border-b border-[var(--line)] sm:border-b-0 sm:border-r"
          />
          <Readout
            label={t.app.uptime}
            value={fmtDuration(r.uptime_secs, lang)}
            sub={`v${r.version} · pid ${r.pid}`}
            className="border-[var(--line)]"
          />
        </div>
      </Plate>

      {/* ---- counters + money ---- */}
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Plate className="rise">
          <Readout
            label={t.dash.requests}
            value={fmtNum(c.requests)}
            sub={`${t.dash.proxied} ${fmtNum(c.proxied)} · ${t.dash.errors} ${fmtNum(c.errors)}`}
          />
        </Plate>
        <Plate className="rise">
          <Readout
            label={t.dash.streams}
            value={fmtNum(c.streams)}
            sub={`${t.dash.retries} ${fmtNum(c.retries)} · ${fmtBytes(c.bytes_out)}`}
          />
        </Plate>
        <Plate className="rise">
          <Readout
            label={t.dash.saved}
            value={<span style={{ color: "var(--up)" }}>{fmtUsd(stats.savings.opencodego_saved_usd)}</span>}
            help={t.dash.savedHint}
          />
        </Plate>
        <Plate className="rise">
          <Readout
            label={t.dash.spent}
            value={<span style={{ color: "var(--cash)" }}>{fmtUsd(stats.savings.fallback_spent_usd)}</span>}
            help={t.dash.spentHint}
          />
        </Plate>
      </div>

      {/* ---- per side summary ---- */}
      <div className="grid gap-3 sm:grid-cols-2">
        <SideSummary kind="plans" accounts={plans} used={c.plans_used} />
        <SideSummary kind="fallback" accounts={cash} used={c.cash_used} />
      </div>

      {/* ---- endpoint roster ---- */}
      <PlateBlock
        title={t.dash.accounts}
        hint={t.dash.recentHint}
        actions={
          <>
            <span className="mono text-2xs uppercase tracking-[0.12em] text-[var(--ink-faint)]">
              {paused ? t.dash.paused : t.dash.live}
            </span>
            <span className={cn("h-[6px] w-[6px] rounded-full", !paused && "live-dot")} style={{ background: paused ? "var(--ink-faint)" : "var(--up)" }} />
          </>
        }
      >
        {stats.accounts.length === 0 ? (
          <div className="px-4 py-6 text-center text-sm text-[var(--ink-faint)]">
            {t.dash.noAccounts}{" "}
            <Link href="/endpoints" className="underline" style={{ color: "var(--signal)" }}>
              {t.nav.endpoints}
            </Link>
          </div>
        ) : (
          <div className="divide-y divide-[var(--line)]">
            {roster.map((a, i) => (
              <AccountRow
                key={a.name}
                a={a}
                index={i}
                busy={busy === a.name}
                onToggle={setEndpointEnabled}
              />
            ))}
          </div>
        )}
      </PlateBlock>

      {stats.warnings.length > 0 ? (
        <Plate className="rise border-[var(--warn)]/40">
          <div className="px-3 py-2.5 sm:px-4">
            <div className="label" style={{ color: "var(--warn)" }}>
              config warnings
            </div>
            <ul className="mt-2 space-y-1.5">
              {stats.warnings.map((w, i) => (
                <li key={i} className="text-sm leading-relaxed text-[var(--ink-dim)]">
                  · {w}
                </li>
              ))}
            </ul>
          </div>
        </Plate>
      ) : null}

      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 px-1 text-2xs text-[var(--ink-faint)]">
        <span className="mono">
          {t.app.configPath}: <span className="text-[var(--ink-dim)]">{r.config_path}</span>
        </span>
        <span className="mono">
          {t.nav.policy}: {r.surplus_max_pct}% · {r.surplus_projection ? "projection on" : "projection off"}
        </span>
      </div>
    </div>
  );
}

function SideSummary({ kind, accounts, used }: { kind: "plans" | "fallback"; accounts: Account[]; used: number }) {
  const { t } = useI18n();
  const tone = kind === "plans" ? "var(--plans)" : "var(--cash)";
  const healthy = accounts.filter((a) => a.available).length;
  const tokens = accounts.reduce((n, a) => n + a.stats.prompt_tokens + a.stats.completion_tokens, 0);
  const cost = accounts.reduce((n, a) => n + (kind === "plans" ? a.stats.saved_usd : a.stats.cost_usd), 0);
  return (
    <Plate className="rise">
      <div className="flex items-center justify-between border-b border-[var(--line)] px-3 py-2">
        <span className="mono text-xs uppercase tracking-[0.16em]" style={{ color: tone }}>
          {kind === "plans" ? t.endpoints.plans : t.endpoints.fallback}
        </span>
        <span className="mono text-2xs text-[var(--ink-faint)]">
          {healthy}/{accounts.length} · {fmtNum(used)} req
        </span>
      </div>
      <div className="grid grid-cols-3 divide-x divide-[var(--line)]">
        <Readout label={t.dash.tokens} value={fmtNum(tokens)} sub={kind === "plans" ? t.dash.saved : t.dash.cost} />
        <Readout label={kind === "plans" ? t.dash.saved : t.dash.cost} value={fmtUsd(cost)} />
        <Readout
          label={t.common.enabled}
          value={`${accounts.filter((a) => a.enabled !== false).length}/${accounts.length}`}
          sub={`${healthy} ${t.dash.healthy}`}
        />
      </div>
    </Plate>
  );
}

function AccountRow({
  a,
  index,
  busy,
  onToggle,
}: {
  a: Account;
  index: number;
  busy: boolean;
  onToggle: (name: string, enabled: boolean) => void;
}) {
  const { t, lang } = useI18n();
  const [open, setOpen] = React.useState(false);
  const tone = a.kind === "plans" ? "var(--plans)" : "var(--cash)";
  const q = a.quota;
  const worst = Math.max(q.rolling.percent || 0, q.weekly.percent || 0, q.monthly.percent || 0);
  const hasQuota = q.unit !== "none" && (q.rolling.limit > 0 || q.weekly.limit > 0 || q.monthly.limit > 0);

  // A disabled endpoint is out of the pool on purpose, so it must not read as unhealthy.
  const enabled = a.enabled !== false;
  const status = !enabled
    ? { label: t.common.disabled, tone: "plain" as const }
    : !a.available
      ? { label: a.cooldown_secs_left > 0 ? `${t.dash.cooldown} ${a.cooldown_secs_left}s` : t.dash.skipped, tone: "danger" as const }
      : a.health?.skipped
        ? { label: `${t.dash.skipped} ${a.health.skip_secs_left}s`, tone: "warn" as const }
        : { label: t.dash.healthy, tone: "on" as const };

  return (
    <div className="rise" style={{ animationDelay: `${Math.min(index * 28, 240)}ms` }}>
      <div className={cn("flex items-center", !enabled && "opacity-60")}>
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          className="flex min-w-0 flex-1 items-center gap-3 px-3 py-2.5 text-left transition-colors hover:bg-[var(--panel-2)] sm:px-4"
          aria-expanded={open}
        >
          <span className="mono w-[18px] shrink-0 text-2xs text-[var(--ink-faint)]">{a.order}</span>
          <span className="h-8 w-[2px] shrink-0 rounded-full" style={{ background: tone }} />
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-2">
              <span className="mono truncate text-sm">{a.name}</span>
              <Badge tone={status.tone} dot className="shrink-0">
                {status.label}
              </Badge>
            </span>
            <span className="mono mt-0.5 block truncate text-2xs text-[var(--ink-faint)]">
              {a.provider} · {a.model ?? "—"} · {fmtNum(a.stats.requests)} req · {fmtAgo(a.stats.last_used, lang)}
            </span>
          </span>
          <span className="hidden w-[150px] shrink-0 sm:block">
            {hasQuota ? (
              <>
                <Meter
                  pct={worst}
                  projected={Math.max(q.rolling.projected_percent, q.weekly.projected_percent, q.monthly.projected_percent)}
                  tone={q.exhausted ? "var(--danger)" : tone}
                />
                <span className="mono mt-1 block text-right text-2xs text-[var(--ink-faint)]">
                  {fmtPct(worst)} · {q.source === "remote" ? t.dash.sourceRemote : q.source === "local" ? t.dash.sourceLocal : t.dash.sourceUnknown}
                </span>
              </>
            ) : a.balance ? (
              <span className="mono block text-right text-sm">
                <span style={{ color: "var(--cash)" }}>{a.balance.total}</span>{" "}
                <span className="text-2xs text-[var(--ink-faint)]">{a.balance.currency}</span>
              </span>
            ) : (
              <span className="mono block text-right text-2xs text-[var(--ink-faint)]">{t.common.none}</span>
            )}
          </span>
          <span className="mono hidden w-[86px] shrink-0 text-right text-sm md:block">
            {a.kind === "plans" ? (
              <span style={{ color: "var(--up)" }}>{fmtUsd(a.stats.saved_usd)}</span>
            ) : (
              <span style={{ color: "var(--cash)" }}>{fmtUsd(a.stats.cost_usd)}</span>
            )}
          </span>
            <span className="shrink-0 text-2xs text-[var(--ink-faint)]">{open ? "▴" : "▾"}</span>
        </button>
        <div className="shrink-0 pr-3 sm:pr-4">
          <Button
            size="sm"
            variant={enabled ? "ghost" : "outline"}
            loading={busy}
            onClick={() => onToggle(a.name, !enabled)}
            title={enabled ? t.common.disabledHint : t.common.enabledHint}
          >
            {enabled ? t.common.disable : t.common.enable}
          </Button>
        </div>
      </div>

      {open ? <AccountDetail a={a} /> : null}
    </div>
  );
}

function AccountDetail({ a }: { a: Account }) {
  const { t, lang } = useI18n();
  const q = a.quota;
  const windows = [
    { key: "rolling", label: t.dash.rolling, v: q.rolling, limit: q.rolling.limit, surplus: q.surplus },
    { key: "weekly", label: t.dash.weekly, v: q.weekly, limit: q.weekly.limit, surplus: q.surplus },
    { key: "monthly", label: t.dash.monthly, v: q.monthly, limit: q.monthly.limit, surplus: q.surplus },
  ];
  const unit = q.unit === "usd" ? "$" : q.unit === "tokens" ? "" : "";

  return (
    <div className="rise border-t border-[var(--line)] bg-[var(--panel-2)] px-3 py-3 sm:px-4">
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="space-y-2.5">
          {windows.map((w) => (
            <div key={w.key}>
              <div className="mb-1 flex items-baseline justify-between">
                <span className="label">{w.label}</span>
                <span className="mono tabular text-xs">
                  {w.limit > 0 ? (
                    <>
                      <span style={{ color: w.v.percent >= 100 ? "var(--danger)" : "var(--ink)" }}>{fmtPct(w.v.percent, 0)}</span>
                      <span className="text-[var(--ink-faint)]">
                        {"  "}
                        {unit}
                        {fmtNum(w.v.used, unit ? 2 : 0)}/{unit}
                        {fmtNum(w.limit, unit ? 0 : 0)}
                        {w.v.projected_percent > w.v.percent + 1 ? ` · ${t.dash.projected} ${fmtPct(w.v.projected_percent, 0)}` : ""}
                      </span>
                    </>
                  ) : (
                    <span className="text-[var(--ink-faint)]">— / {t.common.none}</span>
                  )}
                </span>
              </div>
              <Meter
                pct={w.v.percent}
                projected={w.v.projected_percent}
                tone={a.kind === "plans" ? "var(--plans)" : "var(--cash)"}
              />
              {w.v.resets_at ? (
                <div className="mono mt-1 text-2xs text-[var(--ink-faint)]">
                  {t.dash.resetsAt} {fmtWhen(w.v.resets_at, lang)}
                </div>
              ) : null}
            </div>
          ))}
        </div>

        <dl className="mono grid grid-cols-2 gap-x-3 gap-y-1.5 text-xs sm:content-start">
          <Row k={t.dash.identity} v={a.identity ?? `${a.provider}/${a.model}`} />
          <Row k="URL" v={a.url} mono />
          <Row k={t.dash.key} v={a.key} mono />
          <Row
            k={t.dash.dataSource}
            v={q.source === "remote" ? t.dash.sourceRemote : q.source === "local" ? t.dash.sourceLocal : t.dash.sourceUnknown}
          />
          <Row k={t.dash.todayTokens} v={`${fmtNum(a.stats.prompt_tokens)} / ${fmtNum(a.stats.completion_tokens)}`} />
          <Row k={t.dash.cached} v={fmtNum(a.stats.cached_tokens)} />
          <Row k={t.dash.streams} v={fmtNum(a.stats.stream_requests)} />
          <Row k="latency" v={`${fmtNum(a.stats.avg_latency_ms)} ms`} />
          <Row k={t.dash.lastUsed} v={fmtAgo(a.stats.last_used, lang)} />
          {a.balance ? (
            <Row k={t.dash.balance} v={`${a.balance.total} ${a.balance.currency}`} />
          ) : null}
          {a.cooldown_reason ? <Row k="cooldown" v={a.cooldown_reason} tone="var(--danger)" /> : null}
          {a.stats.last_error ? <Row k={t.dash.lastError} v={a.stats.last_error} tone="var(--danger)" /> : null}
          {a.health?.last_reason ? <Row k="health" v={a.health.last_reason} tone="var(--warn)" /> : null}
        </dl>
      </div>
    </div>
  );
}

function Row({ k, v, mono, tone }: { k: string; v: React.ReactNode; mono?: boolean; tone?: string }) {
  return (
    <>
      <dt className="truncate text-[var(--ink-faint)]">{k}</dt>
      <dd className={cn("truncate text-right", mono && "mono")} style={tone ? { color: tone } : undefined} title={typeof v === "string" ? v : undefined}>
        {v}
      </dd>
    </>
  );
}

function Skeleton({ t }: { t: ReturnType<typeof useI18n>["t"] }) {
  return (
    <div className="space-y-3">
      <div className="h-6 w-40 animate-pulse rounded-[2px] bg-[var(--panel-2)]" />
      <Plate className="h-[104px] animate-pulse" />
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        {[0, 1, 2, 3].map((i) => (
          <Plate key={i} className="h-[76px] animate-pulse" />
        ))}
      </div>
      <span className="sr-only">{t.common.loading}</span>
    </div>
  );
}
