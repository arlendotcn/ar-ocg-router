"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { api, type CalibrationResult } from "@/lib/api";
import { useStatsStream } from "@/lib/use-stats";
import { Button } from "@/components/ui/button";
import { Field, Input } from "@/components/ui/field";
import { cn } from "@/lib/utils";
import type { EndpointCfg, QuotaCalibration, QuotaReading } from "@/types/api";

/**
 * A "resets in ..." countdown as three small boxes: days, hours, minutes.
 *
 * One box holding raw seconds was unusable - the value displayed a unit the user was not typing in,
 * so every keystroke re-scaled the number under the cursor. Three boxes each show the unit they
 * collect, and the stored value stays in seconds. `withDays` is off for the 5-hour bucket, which
 * never spans a day.
 */
function CountdownInput({
  seconds,
  withDays,
  onChange,
}: {
  seconds: number;
  withDays: boolean;
  onChange: (seconds: number) => void;
}) {
  const { t } = useI18n();
  const s = Math.max(0, Math.round(seconds));
  const parts = {
    d: withDays ? Math.floor(s / 86400) : 0,
    h: Math.floor((s % 86400) / 3600),
    m: Math.floor((s % 3600) / 60),
  };
  const setPart = (part: "d" | "h" | "m", value: number) => {
    const v = Math.max(0, Math.floor(value) || 0);
    const next = { ...parts, [part]: v };
    onChange(next.d * 86400 + next.h * 3600 + next.m * 60);
  };
  const box = (part: "d" | "h" | "m", label: string, max?: number) => (
    <Input
      type="number"
      min={0}
      max={max}
      aria-label={label}
      value={parts[part] || ""}
      onChange={(e) => setPart(part, Number(e.target.value))}
    />
  );
  return (
    <div className="flex items-center gap-1">
      {withDays ? (
        <>
          <div className="w-[52px]">{box("d", t.endpoints.calUnitDays)}</div>
          <span className="text-2xs text-[var(--ink-faint)]">{t.endpoints.calUnitDayShort}</span>
        </>
      ) : null}
      <div className="w-[52px]">{box("h", t.endpoints.calUnitHours)}</div>
      <span className="text-2xs text-[var(--ink-faint)]">{t.endpoints.calUnitHourShort}</span>
      <div className="w-[52px]">{box("m", t.endpoints.calUnitMinutes)}</div>
      <span className="text-2xs text-[var(--ink-faint)]">{t.endpoints.calUnitMinuteShort}</span>
    </div>
  );
}

/**
 * Derive a plan's real allowance when the provider exposes no usage API.
 *
 * The console shows only percentages, and the router only sees the traffic it forwarded. Two
 * percentage readings taken around a known amount of forwarded consumption fix the ratio between
 * "percentage points moved" and "weighted units spent", so the window totals fall out - and,
 * anchored on the plan price, so does the true per-token value. That is why the wizard is a
 * before/after pair rather than a single number: one reading says nothing about the total, only the
 * movement between two does.
 *
 * The countdown fields are optional but worth copying: they are the only way to learn when each
 * bucket restarts, which is what lets the local percentages line up with the provider's.
 */
export function CalibrationWizard({
  endpoint,
  onDone,
}: {
  endpoint: EndpointCfg;
  onDone: () => void;
}) {
  const { t } = useI18n();
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const [result, setResult] = React.useState<CalibrationResult | null>(null);
  const [open, setOpen] = React.useState(false);
  // The reading being entered: percentages plus the console's countdowns (in minutes, for typing).
  const blank = { pct_5h: 0, pct_week: 0, pct_month: 0, cd_5h: 0, cd_week: 0, cd_month: 0 };
  const [r1, setR1] = React.useState(blank);
  const [r2, setR2] = React.useState(blank);

  const run = async (stage: string, body: Record<string, unknown>) => {
    setBusy(true);
    setError(null);
    try {
      const res = await api.calibrate(endpoint.name, { stage, ...body });
      setResult(res);
      onDone();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const readFields = (
    label: string,
    v: typeof blank,
    set: (x: typeof blank) => void,
    withCountdown: boolean,
  ) => (
    <div className="space-y-2">
      <div className="label">{label}</div>
      <div className="grid grid-cols-3 gap-2">
        <Field label={t.endpoints.cal5h}>
          <Input type="number" step="0.01" min={0} max={100} value={v.pct_5h}
            onChange={(e) => set({ ...v, pct_5h: Number(e.target.value) || 0 })} />
        </Field>
        <Field label={t.endpoints.calWeek}>
          <Input type="number" step="0.01" min={0} max={100} value={v.pct_week}
            onChange={(e) => set({ ...v, pct_week: Number(e.target.value) || 0 })} />
        </Field>
        <Field label={t.endpoints.calMonth}>
          <Input type="number" step="0.01" min={0} max={100} value={v.pct_month}
            onChange={(e) => set({ ...v, pct_month: Number(e.target.value) || 0 })} />
        </Field>
      </div>
      {withCountdown ? (
        <>
          <div className="text-2xs text-[var(--ink-faint)]">{t.endpoints.calCountdownHint}</div>
          <div className="grid grid-cols-3 gap-2">
            <Field label={t.endpoints.calReset5h}>
              <CountdownInput seconds={v.cd_5h} withDays={false}
                onChange={(secs) => set({ ...v, cd_5h: secs })} />
            </Field>
            <Field label={t.endpoints.calResetWeek}>
              <CountdownInput seconds={v.cd_week} withDays
                onChange={(secs) => set({ ...v, cd_week: secs })} />
            </Field>
            <Field label={t.endpoints.calResetMonth}>
              <CountdownInput seconds={v.cd_month} withDays
                onChange={(secs) => set({ ...v, cd_month: secs })} />
            </Field>
          </div>
        </>
      ) : null}
    </div>
  );

  // The wizard's live state comes from the statistics stream, not the config draft: it lives in
  // the running registry, and the console refreshes it on the same poll as everything else.
  const { stats } = useStatsStream({ intervalMs: 5000 });
  const live = stats?.accounts.find((a) => a.name === endpoint.name)?.quota.calibration;
  const state: { pending: QuotaReading | null; derived: QuotaCalibration | null } =
    live ?? { pending: null, derived: null };

  return (
    <section className="plate">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className={cn(
          "flex w-full items-center justify-between gap-2 px-3 py-2 text-left transition-colors hover:bg-[var(--signal)]/15",
          open && "border-b border-[var(--line)]",
        )}
      >
        <span className="mono text-xs uppercase tracking-[0.16em]">{t.endpoints.calTitle}</span>
        <span className="flex items-center gap-2">
          <span className="mono text-2xs text-[var(--ink-faint)]">
            {state.derived
              ? state.derived.verified_at
                ? t.endpoints.calStateVerified
                : t.endpoints.calStateDerived
              : state.pending
                ? t.endpoints.calStatePending
                : t.endpoints.calStateNone}
          </span>
          <span className="text-2xs text-[var(--ink-faint)]">{open ? "▴" : "▾"}</span>
        </span>
      </button>

      {open ? (
        <div className="space-y-3 px-3 py-3">
          <p className="text-xs leading-relaxed text-[var(--ink-dim)]">{t.endpoints.calIntro}</p>

          {state.derived ? (
            <div className="space-y-1.5 rounded-[2px] border border-[var(--line)] bg-[var(--panel)] p-2.5">
              <div className="label">{t.endpoints.calDerived}</div>
              <div className="mono text-xs text-[var(--ink)]">
                {t.endpoints.calScale.replace("{n}", state.derived.scale.toFixed(4))}
              </div>
              {state.derived.rolling_total > 0 ? (
                <div className="mono text-xs">
                  {t.endpoints.calTotal5h.replace("{n}", state.derived.rolling_total.toFixed(2))}
                </div>
              ) : null}
              {state.derived.weekly_total > 0 ? (
                <div className="mono text-xs">
                  {t.endpoints.calTotalWeek.replace("{n}", state.derived.weekly_total.toFixed(2))}
                </div>
              ) : null}
              {state.derived.verified_at ? (
                <div className="mono text-2xs text-[var(--up)]">
                  {t.endpoints.calVerified.replace("{n}", (state.derived.residual_pp ?? 0).toFixed(3))}
                </div>
              ) : (
                <div className="text-2xs text-[var(--ink-faint)]">{t.endpoints.calUnverified}</div>
              )}
            </div>
          ) : null}

          {readFields(t.endpoints.calStep1, r1, setR1, true)}
          <div className="flex gap-2">
            <Button size="sm" variant="outline" disabled={busy}
              onClick={() => void run("start", r1)}>
              {t.endpoints.calRecord1}
            </Button>
            {state.pending ? (
              <Button size="sm" variant="ghost" disabled={busy}
                onClick={() => void run("cancel", {})}>
                {t.common.cancel}
              </Button>
            ) : null}
          </div>

          {state.pending ? (
            <div className="mono text-2xs text-[var(--ink-faint)]">
              {t.endpoints.calPendingAt.replace("{n}", new Date(state.pending.at * 1000).toLocaleString())}
            </div>
          ) : null}

          <div className="border-t border-[var(--line)] pt-3">
            {readFields(t.endpoints.calStep2, r2, setR2, true)}
            <div className="mt-2 flex flex-wrap gap-2">
              <Button size="sm" variant="primary" disabled={busy || !state.pending}
                onClick={() => void run("finish", r2)}>
                {t.endpoints.calDerive}
              </Button>
              <Button size="sm" variant="outline" disabled={busy || !state.derived}
                onClick={() => void run("verify", { pct_month: r2.pct_month })}>
                {t.endpoints.calVerify}
              </Button>
            </div>
          </div>

          {result?.note ? <div className="text-2xs text-[var(--warn)]">{result.note}</div> : null}
          {result?.residual_pp !== undefined ? (
            <div className="text-2xs text-[var(--ink-dim)]">
              {t.endpoints.calResidual.replace("{n}", result.residual_pp.toFixed(3))}
            </div>
          ) : null}
          {error ? <div className="text-2xs text-[var(--danger)]">{error}</div> : null}
        </div>
      ) : null}
    </section>
  );
}
