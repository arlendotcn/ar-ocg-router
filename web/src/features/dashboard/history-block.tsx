"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { api } from "@/lib/api";
import { useToast } from "@/components/ui/toast";
import { Button } from "@/components/ui/button";
import { fmtCompact, fmtMoney, fmtNum } from "@/lib/format";
import type { History, HistoryBucket } from "@/types/api";
import { cn } from "@/lib/utils";

/**
 * The complete history, laid out for the sheet it lives in rather than for the overview row it is
 * opened from: a summary across the top, a chart with room to read, the per-endpoint split under it,
 * and the settings and export at the end where they do not compete with the numbers.
 *
 * Money is never summed across endpoints: several plans can be denominated in different currencies,
 * and their sum would be a number in no currency at all. When every endpoint shares one currency the
 * summary leads with money; otherwise it leads with requests and the money stays in the table, in
 * each endpoint's own unit.
 */

export type Metric = "requests" | "tokens" | "cost";

export const num = (b: HistoryBucket | undefined, k: keyof HistoryBucket) => Number(b?.[k] ?? 0);
export const tokens = (b: HistoryBucket | undefined) =>
  num(b, "prompt_tokens") + num(b, "completion_tokens");
/** Share of the input that was served from cache, as the endpoint rows already report it. */
const hitRate = (b: HistoryBucket | undefined) => {
  const input = num(b, "prompt_tokens");
  return input > 0 ? ((num(b, "cached_tokens") / input) * 100).toFixed(1) + "%" : "—";
};

const metricOf = (b: HistoryBucket | undefined, m: Metric) =>
  m === "requests" ? num(b, "requests") : m === "tokens" ? tokens(b) : num(b, "cost");

/** Fetch the history on its own cadence: a day bucket changes when a request is served. */
/**
 * Fetch the history, at a cadence the caller chooses.
 *
 * It is not the two-second statistics poll and should not be: this answer is a few kilobytes of
 * calendar, and the day buckets only move when a request finishes. But a minute is long enough that a
 * figure beside a counter updating every two seconds looks frozen, so the panel asks for a fast
 * cadence while it is open. Changing the cadence re-runs the effect, which means opening the panel
 * fetches immediately rather than waiting for the next tick.
 */
export function useHistory(span: number, intervalMs = 10_000) {
  const [data, setData] = React.useState<History | null>(null);
  React.useEffect(() => {
    let alive = true;
    const load = () => {
      api.history(span)
        .then((h) => { if (alive) setData(h); })
        .catch(() => { /* keep whatever was last shown */ });
    };
    load();
    const id = window.setInterval(load, intervalMs);
    return () => { alive = false; window.clearInterval(id); };
  }, [span, intervalMs]);
  return { data, span };
}

/**
 * The single currency every endpoint shares, or null when they do not.
 *
 * Also null when the newest day was rebuilt from the ledger: such a day carries money that belongs
 * to no endpoint, so a single-currency figure would silently omit it.
 */
export function onlyCurrency(
  data: History | null,
  accounts: { name: string; currency: string }[],
): string | null {
  const seen = new Set(accounts.map((a) => a.currency).filter((c) => c && c.trim() !== ""));
  if (seen.size !== 1) return null;
  if (data?.days?.[data.days.length - 1]?.backfilled) return null;
  return [...seen][0] ?? null;
}

/**
 * What the plans saved, grouped by the currency each one is priced in.
 *
 * A saved amount belongs to the endpoint that did the saving - it is the cash price it avoided - so
 * several currencies must be listed rather than added, exactly like the money they are compared to.
 */
function savedByCurrency(
  data: History | null,
  accounts: { name: string; currency: string }[],
  period: "today" | "week" | "month",
): string[] {
  const totals = new Map<string, number>();
  for (const a of accounts) {
    const v = num(data?.accounts?.[a.name]?.[period], "saved");
    if (v <= 0) continue;
    const cur = (a.currency || "").trim();
    totals.set(cur, (totals.get(cur) ?? 0) + v);
  }
  return [...totals.entries()].map(([cur, v]) => fmtMoney(v, cur));
}

export function HistoryPanel({
  data,
  span,
  setSpan,
  metric,
  setMetric,
  accounts,
}: {
  data: History | null;
  span: number;
  setSpan: (n: number) => void;
  metric: Metric;
  setMetric: (m: Metric) => void;
  accounts: { name: string; currency: string }[];
}) {
  const { t } = useI18n();
  const toast = useToast();
  const [offset, setOffset] = React.useState<number | null | undefined>(undefined);
  const [busy, setBusy] = React.useState(false);

  React.useEffect(() => {
    api.config()
      .then((c) => setOffset(c.stats?.utc_offset_minutes ?? null))
      .catch(() => setOffset(null));
  }, []);

  const useBrowserTime = async () => {
    const minutes = -new Date().getTimezoneOffset();
    setBusy(true);
    try {
      const cfg = await api.config();
      cfg.stats = { retention_days: cfg.stats?.retention_days ?? 400, utc_offset_minutes: minutes };
      await api.saveConfig(cfg);
      setOffset(minutes);
      toast.push("ok", t.history.tzDone.replace("{n}", String(minutes)));
    } catch (e) {
      toast.push("err", e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const single = onlyCurrency(data, accounts);
  const days = data?.days ?? [];
  const accountNames = Object.keys(data?.accounts ?? {}).sort();
  const peak = Math.max(1, ...days.map((d) => metricOf(d.bucket, metric)));
  const periodTotal = days.reduce((a, d) => a + metricOf(d.bucket, metric), 0);
  const anyBackfilled = days.some((d) => d.backfilled);

  const fmt = (v: number, m: Metric, currency?: string | null) =>
    m === "cost" ? (currency ? fmtMoney(v, currency) : v.toFixed(2)) : fmtNum(v);
  const compact = (v: number) => (metric === "cost" ? fmtMoney(v, single) : fmtCompact(v));

  const exportCsv = () => {
    const header = ["date", "requests", "successes", "errors", "prompt_tokens", "completion_tokens", "cached_tokens", "cost", "saved", "backfilled"];
    const rows = days.map((d) =>
      [
        d.date,
        ...header.slice(1, 9).map((k) => String((d.bucket as Record<string, number | undefined>)[k] ?? 0)),
        d.backfilled ? "yes" : "no",
      ].join(","),
    );
    const blob = new Blob([header.join(",") + "\n" + rows.join("\n") + "\n"], { type: "text/csv" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `ar-ocg-router-history-${span}d.csv`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const cards = [
    { key: "today", label: t.history.today, now: data?.totals?.today, before: data?.totals?.yesterday },
    { key: "week", label: t.history.week, now: data?.totals?.week, before: data?.totals?.week_prev },
    { key: "month", label: t.history.month, now: data?.totals?.month, before: data?.totals?.month_prev },
  ];

  return (
    <div className="space-y-4">
      {offset === null ? (
        <div className="flex flex-wrap items-center justify-between gap-2 border-l-2 border-[var(--warn)]/50 bg-[var(--panel-2)] px-2.5 py-2 text-xs text-[var(--ink-dim)]">
          <span>{t.history.tzUnset}</span>
          <Button size="sm" variant="outline" disabled={busy} onClick={() => void useBrowserTime()}>
            {t.history.tzUse.replace("{n}", String(-new Date().getTimezoneOffset()))}
          </Button>
        </div>
      ) : null}

      {/* ---- the three periods, side by side: the question the panel exists to answer ---- */}
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        {cards.map((c) => {
          const now = c.now;
          const a = metricOf(now, metric);
          const b = metricOf(c.before, metric);
          const pct = b > 0 ? ((a - b) / b) * 100 : null;
          const headline = single ? fmtMoney(num(now, "cost"), single) : fmtNum(num(now, "requests"));
          const saved = savedByCurrency(data, accounts, c.key as "today" | "week" | "month");
          return (
            <div key={c.key} className="rounded-[2px] border border-[var(--line)] bg-[var(--panel-2)] px-3 py-2.5">
              {/* The comparison sits with the label that names it: on its own line at the bottom it
                  reads as a stray caption belonging to whatever is above it. */}
              <div className="flex items-baseline justify-between gap-2">
                <span className="label">{c.label}</span>
                {pct === null ? null : (
                  <span
                    title={t.history.vsPrevHint}
                    className={cn("mono shrink-0 text-2xs", pct >= 0 ? "text-[var(--up)]" : "text-[var(--ink-faint)]")}
                  >
                    {pct >= 0 ? "+" : ""}{pct.toFixed(0)}% {t.history.vsPrev}
                  </span>
                )}
              </div>
              <div className="mono mt-1.5 truncate text-2xl leading-none">{headline}</div>
              <div className="mono mt-1.5 text-2xs text-[var(--ink-faint)]">
                {fmtNum(num(now, "requests"))} {t.history.requests} · {fmtCompact(tokens(now))} {t.history.tokenUnit}
              </div>
              {/* The two token kinds and the cache rate: the totals above say how much, these say
                  what it was made of - and cached input is the cheap kind. */}
              <div className="mono mt-0.5 text-2xs text-[var(--ink-faint)]">
                {t.history.inTokens} {fmtCompact(num(now, "prompt_tokens"))} · {t.history.outTokens} {fmtCompact(num(now, "completion_tokens"))}
              </div>
              <div className="mono mt-0.5 text-2xs text-[var(--ink-dim)]" title={t.history.cacheHint}>
                {t.history.cacheHit} {fmtCompact(num(now, "cached_tokens"))} · {hitRate(now)}
              </div>
              {saved.length === 0 ? null : (
                <div className="mono mt-0.5 text-2xs text-[var(--up)]">
                  {t.history.saved} {saved.join(" · ")}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* ---- the chart: taller here than it could ever be on the overview ---- */}
      <div className="rounded-[2px] border border-[var(--line)]">
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-[var(--line)] px-3 py-2">
          <div className="flex gap-1">
            {(["tokens", "requests", ...(single ? (["cost"] as Metric[]) : [])] as Metric[]).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => setMetric(m)}
                className={cn(
                  "mono rounded-[2px] border px-2 py-[3px] text-2xs transition-colors",
                  metric === m
                    ? "border-[var(--signal)] text-[var(--signal)]"
                    : "border-[var(--line-strong)] text-[var(--ink-faint)] hover:text-[var(--ink)]",
                )}
              >
                {m === "cost" ? t.history.money : m === "tokens" ? t.history.tokens : t.history.requests}
              </button>
            ))}
          </div>
          <div className="flex items-center gap-1">
            {[7, 30, 90].map((s) => (
              <button
                key={s}
                type="button"
                onClick={() => setSpan(s)}
                className={cn(
                  "mono rounded-[2px] border px-2 py-[3px] text-2xs transition-colors",
                  span === s
                    ? "border-[var(--signal)] text-[var(--signal)]"
                    : "border-[var(--line-strong)] text-[var(--ink-faint)] hover:text-[var(--ink)]",
                )}
              >
                {s}d
              </button>
            ))}
          </div>
        </div>
        <div className="px-3 py-3">
          <div className="mono flex items-baseline justify-between gap-2 text-2xs text-[var(--ink-faint)]">
            <span>{t.history.peakDay} {compact(peak)}</span>
            <span>{t.history.periodTotal} {compact(periodTotal)}</span>
          </div>
          <div className="mt-2 flex h-40 items-end gap-[3px] border-b border-[var(--line)] pb-[1px]">
            {days.length === 0 ? (
              <div className="mono text-2xs text-[var(--ink-faint)]">{t.history.empty}</div>
            ) : (
              days.map((d) => {
                const v = metricOf(d.bucket, metric);
                const pctOf = (x: number) => (x <= 0 ? 0 : Math.max(1, Math.round((x / peak) * 100)));
                const tip = `${d.date} · ${fmt(v, metric, single)} · ${num(d.bucket, "requests")} ${t.history.requests}${d.backfilled ? ` · ${t.history.backfilled}` : ""}`;
                // On the token metric the bar is split into what the tokens actually were: input
                // that missed the cache, input that hit it, and output. Cached input is a subset of
                // the input, so the three parts add up to the day without counting anything twice.
                if (metric === "tokens") {
                  const hit = num(d.bucket, "cached_tokens");
                  const miss = Math.max(0, num(d.bucket, "prompt_tokens") - hit);
                  const out = num(d.bucket, "completion_tokens");
                  return (
                    // h-full: the segments are percentages, and a percentage of an auto-height box is
                    // zero - which is a chart with no bars in it.
                    <div key={d.date} title={tip} className="flex h-full min-w-0 flex-1 flex-col justify-end gap-[1px]">
                      <div style={{ height: `${pctOf(out)}%` }} className="rounded-t-[1px] bg-[var(--ink-faint)]/60" />
                      <div style={{ height: `${pctOf(hit)}%` }} className="bg-[var(--signal)]/35" />
                      <div style={{ height: `${pctOf(miss)}%` }} className="bg-[var(--signal)]/75" />
                    </div>
                  );
                }
                return (
                  <div
                    key={d.date}
                    title={tip}
                    style={{ height: `${pctOf(v)}%` }}
                    className={cn(
                      "min-w-0 flex-1 rounded-t-[1px] transition-colors hover:brightness-125",
                      d.backfilled ? "bg-[var(--ink-faint)]/45" : "bg-[var(--signal)]/70",
                    )}
                  />
                );
              })
            )}
          </div>
          <div className="mono mt-1 flex justify-between text-2xs text-[var(--ink-faint)]">
            <span>{days[0]?.date ?? ""}</span>
            <span>{days[days.length - 1]?.date ?? ""}</span>
          </div>
          <div className="mono mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-2xs text-[var(--ink-faint)]">
            {metric === "tokens" ? (
              <>
                <span className="flex items-center gap-1.5">
                  <span className="inline-block h-2 w-2 rounded-[1px] bg-[var(--signal)]/75" />
                  {t.history.inTokens} ({t.history.cacheMiss})
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="inline-block h-2 w-2 rounded-[1px] bg-[var(--signal)]/35" />
                  {t.history.cacheHit}
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="inline-block h-2 w-2 rounded-[1px] bg-[var(--ink-faint)]/60" />
                  {t.history.outTokens}
                </span>
              </>
            ) : null}
            {anyBackfilled ? (
              <span className="flex items-center gap-1.5" title={t.history.backfilled}>
                <span className="inline-block h-2 w-2 rounded-[1px] bg-[var(--ink-faint)]/45" />
                {t.history.backfilledShort}
              </span>
            ) : null}
          </div>
        </div>
      </div>

      {/* ---- per endpoint, in each endpoint's own currency ---- */}
      {accountNames.length === 0 ? null : (
        <div className="overflow-hidden rounded-[2px] border border-[var(--line)]">
          <table className="mono w-full text-xs">
            <thead className="bg-[var(--panel-2)]">
              <tr className="text-2xs uppercase tracking-[0.12em] text-[var(--ink-dim)]">
                <th className="px-3 py-2 text-left font-normal">{t.endpoints.title}</th>
                <th className="px-3 py-2 text-right font-normal">{t.history.today}</th>
                <th className="px-3 py-2 text-right font-normal">{t.history.week}</th>
                <th className="px-3 py-2 text-right font-normal">{t.history.month}</th>
                <th className="px-3 py-2 text-right font-normal">{t.history.saved}</th>
              </tr>
            </thead>
            <tbody>
              {accountNames.map((name) => {
                const cur = accounts.find((a) => a.name === name)?.currency ?? null;
                const row = data?.accounts?.[name] ?? {};
                const saved = num(row["month"], "saved");
                return (
                  <tr key={name} className="border-t border-[var(--line)]">
                    <td className="max-w-[11rem] truncate px-3 py-2">{name}</td>
                    {(["today", "week", "month"] as const).map((p) => {
                      const b = row[p];
                      return (
                        <td key={p} className="px-3 py-2 text-right">
                          <div title={`${t.history.inTokens} ${fmtNum(num(b, "prompt_tokens"))} · ${t.history.outTokens} ${fmtNum(num(b, "completion_tokens"))}`}>
                            {fmtMoney(num(b, "cost"), cur ?? "")}
                          </div>
                          <div className="text-2xs text-[var(--ink-faint)]">
                            {fmtNum(num(b, "requests"))} · {fmtCompact(tokens(b))} · {hitRate(b)}
                          </div>
                        </td>
                      );
                    })}
                    <td className="px-3 py-2 text-right text-[var(--up)]">
                      {saved > 0 ? fmtMoney(saved, cur ?? "") : "—"}
                    </td>
                  </tr>
                );
              })}
              <tr className="border-t border-[var(--line-strong)] bg-[var(--panel-2)]">
                <td className="px-3 py-2">{t.history.totalRow}</td>
                {(["today", "week", "month"] as const).map((p) => {
                  const b = data?.totals?.[p];
                  return (
                    <td key={p} className="px-3 py-2 text-right">
                      <div title={`${t.history.inTokens} ${fmtNum(num(b, "prompt_tokens"))} · ${t.history.outTokens} ${fmtNum(num(b, "completion_tokens"))}`}>
                        {single ? fmtMoney(num(b, "cost"), single) : "—"}
                      </div>
                      <div className="text-2xs text-[var(--ink-faint)]">
                        {fmtNum(num(b, "requests"))} · {fmtCompact(tokens(b))} · {hitRate(b)}
                      </div>
                    </td>
                  );
                })}
                <td className="px-3 py-2 text-right text-[var(--up)]">
                  {single && num(data?.totals?.["month"], "saved") > 0
                    ? fmtMoney(num(data?.totals?.["month"], "saved"), single)
                    : "—"}
                </td>
              </tr>
            </tbody>
          </table>
          {single ? null : (
            <div className="mono border-t border-[var(--line)] px-3 py-2 text-2xs text-[var(--ink-faint)]">
              {t.history.mixedCurrency}
            </div>
          )}
        </div>
      )}

      {/* ---- the settings and the way out of here ---- */}
      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-[var(--line)] pt-3">
        <div className="mono text-2xs text-[var(--ink-faint)]">
          {t.history.dayLine
            .replace("{off}", offset == null ? "UTC" : `UTC${offset >= 0 ? "+" : ""}${(offset / 60).toFixed(offset % 60 === 0 ? 0 : 1)}`)
            .replace("{n}", String(data?.retention_days ?? 400))}
        </div>
        <div className="flex items-center gap-2">
          {offset === null ? (
            <Button size="sm" variant="outline" disabled={busy} onClick={() => void useBrowserTime()}>
              {t.history.tzUse.replace("{n}", String(-new Date().getTimezoneOffset()))}
            </Button>
          ) : null}
          <Button size="sm" variant="outline" onClick={exportCsv} disabled={days.length === 0}>
            {t.common.export}
          </Button>
        </div>
      </div>
    </div>
  );
}
