"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { api } from "@/lib/api";
import { useToast } from "@/components/ui/toast";
import { Button } from "@/components/ui/button";
import { Plate, PlateBlock } from "@/components/ui/plate";
import { fmtMoney, fmtNum, moneySymbol } from "@/lib/format";
import type { History, HistoryBucket } from "@/types/api";
import { cn } from "@/lib/utils";

/**
 * What the counters beside each endpoint cannot say: how much of it happened *when*.
 *
 * The numbers come from /router/history, which is fetched on its own cadence rather than polled with
 * the two-second statistics - a day bucket changes when a request is served, not while you watch.
 *
 * Money is deliberately not summed across endpoints here. Several plans with different currencies
 * would add up to a number in no currency at all, so the aggregate shows requests and tokens, and
 * each endpoint's money is shown in its own unit beside its own name.
 */

type Metric = "requests" | "tokens" | "cost";

const num = (b: HistoryBucket | undefined, k: keyof HistoryBucket) => Number(b?.[k] ?? 0);
const tokens = (b: HistoryBucket | undefined) => num(b, "prompt_tokens") + num(b, "completion_tokens");
const metricOf = (b: HistoryBucket | undefined, m: Metric) =>
  m === "requests" ? num(b, "requests") : m === "tokens" ? tokens(b) : num(b, "cost");

export function HistoryBlock({ accounts }: { accounts: { name: string; currency: string }[] }) {
  const { t, lang } = useI18n();
  const toast = useToast();
  const [span, setSpan] = React.useState(30);
  const [metric, setMetric] = React.useState<Metric>("requests");
  const [data, setData] = React.useState<History | null>(null);
  // undefined = not asked yet, null = asked and unset (filed by UTC).
  const [offset, setOffset] = React.useState<number | null | undefined>(undefined);
  const [busy, setBusy] = React.useState(false);

  React.useEffect(() => {
    let alive = true;
    const load = () => {
      api.history(span)
        .then((h) => { if (alive) setData(h); })
        .catch(() => { /* the block keeps whatever it last showed */ });
    };
    load();
    const id = window.setInterval(load, 60_000);
    return () => { alive = false; window.clearInterval(id); };
  }, [span]);

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

  const currencies = React.useMemo(
    () => Array.from(new Set(accounts.map((a) => a.currency).filter((c) => c && c.trim() !== ""))),
    [accounts],
  );
  const single = currencies.length === 1 ? currencies[0] : null;

  const cards: { key: string; label: string; now?: HistoryBucket; before?: HistoryBucket }[] = [
    { key: "today", label: t.history.today, now: data?.totals?.today, before: data?.totals?.yesterday },
    { key: "week", label: t.history.week, now: data?.totals?.week, before: data?.totals?.week_prev },
    { key: "month", label: t.history.month, now: data?.totals?.month, before: data?.totals?.month_prev },
  ];

  const days = data?.days ?? [];
  const peak = Math.max(1, ...days.map((d) => metricOf(d.bucket, metric)));
  const accountNames = Object.keys(data?.accounts ?? {}).sort();

  const exportCsv = () => {
    const header = ["date", "requests", "successes", "errors", "prompt_tokens", "completion_tokens", "cached_tokens", "cost", "saved"];
    const rows = days.map((d) =>
      [d.date, ...header.slice(1).map((k) => String((d.bucket as Record<string, number | undefined>)[k] ?? 0))].join(","),
    );
    const blob = new Blob([header.join(",") + "\n" + rows.join("\n") + "\n"], { type: "text/csv" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `ar-ocg-router-history-${span}d.csv`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const fmt = (v: number, m: Metric, currency?: string | null) =>
    m === "cost" ? (currency ? fmtMoney(v, currency) : v.toFixed(2)) : fmtNum(v);
  const delta = (now?: HistoryBucket, before?: HistoryBucket) => {
    const a = metricOf(now, metric);
    const b = metricOf(before, metric);
    if (b <= 0) return null;
    const pct = ((a - b) / b) * 100;
    return pct;
  };

  return (
    <PlateBlock title={t.history.title} hint={t.history.help}>
      <div className="space-y-3 px-3 py-3 sm:px-4">
        {offset === null ? (
          <div className="flex flex-wrap items-center justify-between gap-2 border-l-2 border-[var(--warn)]/50 bg-[var(--panel-2)] px-2.5 py-2 text-xs text-[var(--ink-dim)]">
            <span>{t.history.tzUnset}</span>
            <Button size="sm" variant="outline" disabled={busy} onClick={() => void useBrowserTime()}>
              {t.history.tzUse.replace("{n}", String(-new Date().getTimezoneOffset()))}
            </Button>
          </div>
        ) : null}

        {/* Aggregates. Each card compares against the same span of the period before it. */}
        <div className="grid grid-cols-3 gap-3">
          {cards.map((c) => {
            const d = delta(c.now, c.before);
            const primary = metric === "cost" && single ? fmt(num(c.now, "cost"), "cost", single) : fmtNum(num(c.now, "requests"));
            const secondary = metric === "cost" && single ? `${fmtNum(num(c.now, "requests"))} ${t.history.requests}` : `${fmtNum(tokens(c.now))} tok`;
            return (
              <div key={c.key} className="min-w-0">
                <div className="label">{c.label}</div>
                <div className="mono mt-0.5 truncate text-lg leading-tight">{primary}</div>
                <div className="mono mt-0.5 truncate text-2xs text-[var(--ink-faint)]">{secondary}</div>
                {d === null ? null : (
                  <div className={cn("mono mt-0.5 text-2xs", d >= 0 ? "text-[var(--up)]" : "text-[var(--ink-faint)]")}>
                    {d >= 0 ? "+" : ""}{d.toFixed(0)}% {t.history.vsPrev}
                  </div>
                )}
              </div>
            );
          })}
        </div>

        {/* The bars. Height is relative to the busiest day in the window, so a quiet month still shows
            its shape rather than a flat line at the top. */}
        <div>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex gap-1">
              {(["requests", "tokens", ...(single ? (["cost"] as const) : [])] as Metric[]).map((m) => (
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
              <Button size="sm" variant="ghost" onClick={exportCsv} disabled={days.length === 0}>
                {t.common.export}
              </Button>
            </div>
          </div>

          <div className="mt-2 flex h-24 items-end gap-[2px]">
            {days.length === 0 ? (
              <div className="mono text-2xs text-[var(--ink-faint)]">{t.history.empty}</div>
            ) : (
              days.map((d) => {
                const v = metricOf(d.bucket, metric);
                const h = v <= 0 ? 0 : Math.max(3, Math.round((v / peak) * 100));
                return (
                  <div
                    key={d.date}
                    title={`${d.date} · ${fmt(v, metric, single)} ${num(d.bucket, "requests")} ${t.history.requests}${d.backfilled ? ` · ${t.history.backfilled}` : ""}`}
                    style={{ height: `${h}%` }}
                    className={cn(
                      "min-w-0 flex-1 rounded-[1px]",
                      d.backfilled ? "bg-[var(--ink-faint)]/40" : "bg-[var(--signal)]/70",
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
        </div>

        {/* Per endpoint: money in the endpoint's own currency, which is why this is a table and the
            aggregate above is not. */}
        {accountNames.length === 0 ? null : (
          <table className="mono w-full text-xs">
            <thead>
              <tr className="text-2xs uppercase tracking-[0.12em] text-[var(--ink-dim)]">
                <th className="py-1 text-left font-normal">{t.endpoints.title}</th>
                <th className="py-1 text-right font-normal">{t.history.today}</th>
                <th className="py-1 text-right font-normal">{t.history.week}</th>
                <th className="py-1 text-right font-normal">{t.history.month}</th>
              </tr>
            </thead>
            <tbody>
              {accountNames.map((name) => {
                const cur = accounts.find((a) => a.name === name)?.currency ?? null;
                const row = data?.accounts?.[name] ?? {};
                return (
                  <tr key={name} className="border-t border-[var(--line)]">
                    <td className="max-w-[9rem] truncate py-1">{name}</td>
                    {(["today", "week", "month"] as const).map((p) => {
                      const b = row[p];
                      return (
                        <td key={p} className="py-1 text-right">
                          <div>{fmtMoney(num(b, "cost"), cur ?? "")}</div>
                          <div className="text-2xs text-[var(--ink-faint)]">
                            {fmtNum(num(b, "requests"))} · {fmtNum(tokens(b))}
                          </div>
                        </td>
                      );
                    })}
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}

        <div className="mono text-2xs text-[var(--ink-faint)]">
          {t.history.dayLine
            .replace("{off}", offset == null ? "UTC" : `UTC${offset >= 0 ? "+" : ""}${(offset / 60).toFixed(offset % 60 === 0 ? 0 : 1)}`)
            .replace("{n}", String(data?.retention_days ?? 400))}
        </div>
      </div>
    </PlateBlock>
  );
}
