"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { api } from "@/lib/api";
import { useToast } from "@/components/ui/toast";
import { Button } from "@/components/ui/button";
import { fmtMoney, fmtNum } from "@/lib/format";
import type { History, HistoryBucket } from "@/types/api";
import { cn } from "@/lib/utils";

/**
 * Daily history, in two layers.
 *
 * The overview is a fixed layout someone watches: adding a block to it pushes everything down and
 * makes the page re-learn itself. So the numbers that fit on the existing rows go *into* them - a
 * line under the request counter, a figure in the clock cell - and the rest (a month of bars, the
 * per-endpoint split, the CSV) waits behind `HistoryPanel`, which is collapsed until asked for.
 *
 * Money is never summed across endpoints: several plans can be denominated in different currencies,
 * and their sum would be a number in no currency at all. Each endpoint's money is shown in its own
 * unit, and the single compact figure only appears when there is exactly one currency to speak of.
 */

export type Metric = "requests" | "tokens" | "cost";

export const num = (b: HistoryBucket | undefined, k: keyof HistoryBucket) => Number(b?.[k] ?? 0);
export const tokens = (b: HistoryBucket | undefined) =>
  num(b, "prompt_tokens") + num(b, "completion_tokens");
const metricOf = (b: HistoryBucket | undefined, m: Metric) =>
  m === "requests" ? num(b, "requests") : m === "tokens" ? tokens(b) : num(b, "cost");

/** Fetch the history on its own cadence: a day bucket changes when a request is served. */
export function useHistory(span: number) {
  const [data, setData] = React.useState<History | null>(null);
  React.useEffect(() => {
    let alive = true;
    const load = () => {
      api.history(span)
        .then((h) => { if (alive) setData(h); })
        .catch(() => { /* keep whatever was last shown */ });
    };
    load();
    const id = window.setInterval(load, 60_000);
    return () => { alive = false; window.clearInterval(id); };
  }, [span]);
  return { data, span };
}

/** The one number a single-currency install can put on an existing row; null when it is a mix. */
export function onlyCurrency(
  data: History | null,
  accounts: { name: string; currency: string }[],
): string | null {
  const seen = new Set(accounts.map((a) => a.currency).filter((c) => c && c.trim() !== ""));
  if (seen.size !== 1) return null;
  // A day rebuilt from the ledger has money that belongs to no endpoint, so a single-currency figure
  // would be missing exactly that part. It is shown only when the day was counted as it happened.
  if (data?.days?.[data.days.length - 1]?.backfilled) return null;
  return [...seen][0] ?? null;
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

  return (
    <div className="space-y-3">
        {offset === null ? (
          <div className="flex flex-wrap items-center justify-between gap-2 border-l-2 border-[var(--warn)]/50 bg-[var(--panel-2)] px-2.5 py-2 text-xs text-[var(--ink-dim)]">
            <span>{t.history.tzUnset}</span>
            <Button size="sm" variant="outline" disabled={busy} onClick={() => void useBrowserTime()}>
              {t.history.tzUse.replace("{n}", String(-new Date().getTimezoneOffset()))}
            </Button>
          </div>
        ) : null}

        <div>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex gap-1">
              {(["requests", "tokens", ...(single ? (["cost"] as Metric[]) : [])] as Metric[]).map((m) => (
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
  );
}
