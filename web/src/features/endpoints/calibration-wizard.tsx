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
  // Each part is its own labelled cell: an inline "3 boxes + unit text" row squeezed the inputs
  // into 52px, which is narrower than a two-digit number plus its spin button.
  const box = (part: "d" | "h" | "m", label: string) => (
    <label className="block">
      <span className="mono mb-0.5 block text-2xs text-[var(--ink-faint)]">{label}</span>
      <Input
        type="number"
        min={0}
        aria-label={label}
        value={parts[part] || ""}
        onChange={(e) => setPart(part, Number(e.target.value))}
      />
    </label>
  );
  return (
    <div className={cn("grid gap-1.5", withDays ? "grid-cols-3" : "grid-cols-2")}>
      {withDays ? box("d", t.endpoints.calUnitDayShort) : null}
      {box("h", t.endpoints.calUnitHourShort)}
      {box("m", t.endpoints.calUnitMinuteShort)}
    </div>
  );
}

/**
 * Remaining seconds until an absolute anchor.
 *
 * The stored anchors are immediate instants (the console's countdown resolved at the moment it was
 * entered), not durations. Feeding one straight into the boxes showed days-since-1970: the field
 * must display how long is left, and send the re-entered duration back.
 */
function remainingSeconds(anchor: number): number {
  if (anchor <= 0) return 0;
  return Math.max(0, anchor - Math.floor(Date.now() / 1000));
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
  onCycleDay,
  onDone,
}: {
  endpoint: EndpointCfg;
  /** Edits the draft's `quota.cycle_day`; the endpoint is saved as usual, since it is config. */
  onCycleDay: (day: number) => void;
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
  // Anchor edits go through the API and come back on the statistics poll, so for up to five
  // seconds the panel would show the value from before the edit and appear to reject it. These
  // hold the user's own numbers until the server reports them back.
  const [localAnchors, setLocalAnchors] = React.useState<{ bucket_5h: number; week_reset: number } | null>(null);

  const cal = useCalibration(endpoint);
  React.useEffect(() => {
    if (!localAnchors) return;
    const fresh = cal.anchors;
    // Once the server's value agrees with what was sent, the local override is no longer needed.
    if (Math.abs(fresh.bucket_5h - localAnchors.bucket_5h) <= 2 && Math.abs(fresh.week_reset - localAnchors.week_reset) <= 2) {
      setLocalAnchors(null);
    }
  }, [cal.anchors, localAnchors]);
  const anchors = localAnchors ?? cal.anchors;

  const sendAnchors = (bucket_5h: number, week_reset: number) => {
    const sent = {
      bucket_5h: bucket_5h > 0 ? Math.floor(Date.now() / 1000) + bucket_5h : 0,
      week_reset: week_reset > 0 ? Math.floor(Date.now() / 1000) + week_reset : 0,
    };
    setLocalAnchors(sent);
    void run("anchors", { cd_5h: bucket_5h, cd_week: week_reset });
  };
  const pending = cal.pending;
  const acc = pending?.accumulated;
  // The reveal gate is advisory: enough traffic that a derivation has a chance. The server's
  // own check on the percentage movement is what actually decides.
  const ready = (acc?.total ?? 0) >= REVEAL_TOKENS;
  const monthly = endpoint.quota.monthly;
  const hintReady = monthly > 0 && (acc?.cost ?? 0) >= monthly * 0.003;

  /**
   * `close` is only for the stage that rewrites the endpoint's own config (`finish` writes the
   * derived prices back, which makes the open draft stale). Everything else - anchors above all -
   * must leave the panel open: the wizard reads its live state from the statistics stream, so it
   * refreshes itself, and closing on every countdown keystroke made the section impossible to use.
   */
  const run = async (stage: string, body: Record<string, unknown>, close = false) => {
    setBusy(true);
    setError(null);
    try {
      const res = await api.calibrate(endpoint.name, { stage, ...body });
      setResult(res);
      if (close) onDone();
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
                <CountdownInput
                  seconds={remainingSeconds(anchors.bucket_5h)}
                  withDays={false}
                  onChange={(cd_5h) => sendAnchors(cd_5h, remainingSeconds(anchors.week_reset))}
                />
              </Field>
              <Field label={t.endpoints.calResetWeek}>
                <CountdownInput
                  seconds={remainingSeconds(anchors.week_reset)}
                  withDays
                  onChange={(cd_week) => sendAnchors(remainingSeconds(anchors.bucket_5h), cd_week)}
                />
              </Field>
            </div>
            {/* The monthly window's anchor is a config field rather than a countdown: the provider
                resets on a day of the month, and the router can derive the instant from it. Kept here
                so all three window anchors are edited in one place. */}
            <Field label={t.endpoints.cycleDay} help={t.endpoints.cycleDayHint}>
              <Input
                type="number"
                min={0}
                max={31}
                value={endpoint.quota.cycle_day}
                onChange={(e) => onCycleDay(Math.min(31, Math.max(0, Number(e.target.value) || 0)))}
              />
            </Field>
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
                  onClick={() => void run("finish", current, true)}>
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
