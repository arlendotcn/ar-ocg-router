"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { api, type CalibrationResult } from "@/lib/api";
import { useStatsStream } from "@/lib/use-stats";
import { Button } from "@/components/ui/button";
import { Field, Input } from "@/components/ui/field";
import { cn } from "@/lib/utils";
import type { EndpointCfg, QuotaAnchors, QuotaCalibration, QuotaReading } from "@/types/api";

/** Reveal the final-reading fields once this many tokens have flowed since the baseline.
 *  A necessary hint, not a guarantee: what actually matters is the percentage movement on the
 *  provider's console, which the server validates when the derivation is attempted. */
const REVEAL_TOKENS = 1_000_000;

/** A "resets in ..." countdown as three small boxes: days, hours, minutes.
 *  One box holding raw seconds was unusable - the value displayed a unit the user was not typing
 *  in, so every keystroke re-scaled the number under the cursor. Each box shows the unit it
 *  collects, and the stored value stays in seconds. `withDays` is off for the 5-hour bucket,
 *  which never spans a day. */
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

/** The wizard's live state comes from the statistics stream, not the config draft: it lives in
 *  the running registry, and the console refreshes it on the same poll as everything else. */
function useCalibration(endpoint: EndpointCfg) {
  const { stats } = useStatsStream({ intervalMs: 5000 });
  const live = stats?.accounts.find((a) => a.name === endpoint.name)?.quota.calibration;
  return live ?? { anchors: { bucket_5h: 0, week_reset: 0 }, pending: null, derived: null };
}

const PctRow = ({
  label5h,
  labelWeek,
  labelMonth,
  v,
  set,
}: {
  label5h: string;
  labelWeek: string;
  labelMonth: string;
  v: { pct_5h: number; pct_week: number; pct_month: number };
  set: (x: { pct_5h: number; pct_week: number; pct_month: number }) => void;
}) => (
  <div className="grid grid-cols-3 gap-2">
    <Field label={label5h}>
      <Input type="number" step="0.01" min={0} max={100} value={v.pct_5h}
        onChange={(e) => set({ ...v, pct_5h: Number(e.target.value) || 0 })} />
    </Field>
    <Field label={labelWeek}>
      <Input type="number" step="0.01" min={0} max={100} value={v.pct_week}
        onChange={(e) => set({ ...v, pct_week: Number(e.target.value) || 0 })} />
    </Field>
    <Field label={labelMonth}>
      <Input type="number" step="0.01" min={0} max={100} value={v.pct_month}
        onChange={(e) => set({ ...v, pct_month: Number(e.target.value) || 0 })} />
    </Field>
  </div>
);

/** Derive a plan's real allowance when the provider exposes no usage API.
 *
 *  Three phases, in this order: record the baseline, let the endpoint accumulate a measurable
 *  amount of traffic, then record the current reading. Between baseline and current the percentage
 *  movement corresponds to the forwarded consumption, which pins down the window totals and the
 *  true per-token prices. The bucket anchors live outside the readings entirely - they are
 *  editable at any time and apply the moment they arrive. */
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
  const blank = { pct_5h: 0, pct_week: 0, pct_month: 0 };
  const [base, setBase] = React.useState(blank);
  const [current, setCurrent] = React.useState(blank);
  const [cd, setCd] = React.useState({ cd_5h: 0, cd_week: 0 });

  const cal = useCalibration(endpoint);
  const pending = cal.pending;
  const acc = pending?.accumulated;
  // The reveal gate is advisory: enough traffic that a derivation has a chance. The server's
  // own check on the percentage movement is what actually decides.
  const ready = (acc?.total ?? 0) >= REVEAL_TOKENS;
  const monthly = endpoint.quota.monthly;
  const hintReady = monthly > 0 && (acc?.cost ?? 0) >= monthly * 0.003;

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
            {cal.derived
              ? cal.derived.verified_at
                ? t.endpoints.calStateVerified
                : t.endpoints.calStateDerived
              : cal.pending
                ? t.endpoints.calStatePending
                : t.endpoints.calStateNone}
          </span>
          <span className="text-2xs text-[var(--ink-faint)]">{open ? "▴" : "▾"}</span>
        </span>
      </button>

      {open ? (
        <div className="space-y-3 px-3 py-3">
          <p className="text-xs leading-relaxed text-[var(--ink-dim)]">{t.endpoints.calIntro}</p>

          {/* ---- bucket anchors: editable at any time, applied on arrival ---- */}
          <div className="space-y-2 rounded-[2px] border border-[var(--line)] bg-[var(--panel)] p-2.5">
            <div className="label">{t.endpoints.calAnchorsTitle}</div>
            <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
              <Field label={t.endpoints.calReset5h}>
                <CountdownInput seconds={cal.anchors.bucket_5h} withDays={false}
                  onChange={(cd_5h) => void run("anchors", { cd_5h, cd_week: cal.anchors.week_reset })} />
              </Field>
              <Field label={t.endpoints.calResetWeek}>
                <CountdownInput seconds={cal.anchors.week_reset} withDays
                  onChange={(cd_week) => void run("anchors", { cd_5h: cal.anchors.bucket_5h, cd_week })} />
              </Field>
            </div>
            <div className="text-2xs text-[var(--ink-faint)]">{t.endpoints.calAnchorsHint}</div>
          </div>

          {cal.derived ? (
            <div className="space-y-1.5 rounded-[2px] border border-[var(--line)] bg-[var(--panel)] p-2.5">
              <div className="label">{t.endpoints.calDerived}</div>
              <div className="mono text-xs text-[var(--ink)]">
                {t.endpoints.calScale.replace("{n}", cal.derived.scale.toFixed(4))}
              </div>
              {cal.derived.rolling_total > 0 ? (
                <div className="mono text-xs">
                  {t.endpoints.calTotal5h.replace("{n}", cal.derived.rolling_total.toFixed(2))}
                </div>
              ) : null}
              {cal.derived.weekly_total > 0 ? (
                <div className="mono text-xs">
                  {t.endpoints.calTotalWeek.replace("{n}", cal.derived.weekly_total.toFixed(2))}
                </div>
              ) : null}
              {cal.derived.verified_at ? (
                <div className="mono text-2xs text-[var(--up)]">
                  {t.endpoints.calVerified.replace("{n}", (cal.derived.residual_pp ?? 0).toFixed(3))}
                </div>
              ) : (
                <div className="text-2xs text-[var(--ink-faint)]">{t.endpoints.calUnverified}</div>
              )}
            </div>
          ) : null}

          {pending ? (
            <div className="space-y-2 rounded-[2px] border border-[var(--line)] bg-[var(--panel)] p-2.5">
              <div className="label">{t.endpoints.calRecording}</div>
              <div className="mono text-xs">
                {t.endpoints.calProgress
                  .replace("{in}", (acc?.prompt ?? 0) - (acc?.cached ?? 0) > 0
                    ? ((acc?.prompt ?? 0) - (acc?.cached ?? 0)).toLocaleString() : "0")
                  .replace("{cached}", (acc?.cached ?? 0).toLocaleString())
                  .replace("{out}", (acc?.completion ?? 0).toLocaleString())}
              </div>
              <div className="mono text-2xs text-[var(--ink-faint)]">
                {t.endpoints.calProgressTotal
                  .replace("{n}", (acc?.total ?? 0).toLocaleString())
                  .replace("{goal}", REVEAL_TOKENS.toLocaleString())}
              </div>
              {!ready ? (
                <div className="text-2xs text-[var(--warn)]">{t.endpoints.calKeepGoing}</div>
              ) : null}
              <div className="text-2xs text-[var(--danger)]">{t.endpoints.calExclusive}</div>
            </div>
          ) : null}

          {/* ---- phase 1: the baseline ---- */}
          {!pending ? (
            <div className="space-y-2">
              <div className="label">{t.endpoints.calStep1}</div>
              <PctRow label5h={t.endpoints.cal5h} labelWeek={t.endpoints.calWeek}
                labelMonth={t.endpoints.calMonth} v={base} set={setBase} />
              <Button size="sm" variant="outline" disabled={busy}
                onClick={() => void run("start", base)}>
                {t.endpoints.calRecord1}
              </Button>
            </div>
          ) : null}

          {/* ---- phase 2: the final reading, only once enough traffic has accrued ---- */}
          {pending && ready ? (
            <div className="space-y-2 border-t border-[var(--line)] pt-3">
              <div className="label">{t.endpoints.calStep2}</div>
              <PctRow label5h={t.endpoints.cal5h} labelWeek={t.endpoints.calWeek}
                labelMonth={t.endpoints.calMonth} v={current} set={setCurrent} />
              <div className="flex gap-2">
                <Button size="sm" variant="primary" disabled={busy}
                  onClick={() => void run("finish", current)}>
                  {t.endpoints.calDerive}
                </Button>
                <Button size="sm" variant="ghost" disabled={busy}
                  onClick={() => void run("cancel", {})}>
                  {t.common.cancel}
                </Button>
              </div>
            </div>
          ) : null}

          {/* ---- verification of an existing derivation ---- */}
          {cal.derived ? (
            <div className="space-y-2 border-t border-[var(--line)] pt-3">
              <div className="label">{t.endpoints.calVerifyTitle}</div>
              <Field label={t.endpoints.calMonth}>
                <Input type="number" step="0.01" min={0} max={100} value={current.pct_month}
                  onChange={(e) => setCurrent({ ...current, pct_month: Number(e.target.value) || 0 })} />
              </Field>
              <Button size="sm" variant="outline" disabled={busy}
                onClick={() => void run("verify", { pct_month: current.pct_month })}>
                {t.endpoints.calVerify}
              </Button>
            </div>
          ) : null}

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
