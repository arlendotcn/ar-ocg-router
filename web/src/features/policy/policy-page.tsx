"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { useConfig } from "@/lib/use-config";
import { api } from "@/lib/api";
import { useToast } from "@/components/ui/toast";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Plate, PlateBlock, Readout } from "@/components/ui/plate";
import { PageHeader } from "@/components/page-header";
import { ChipInput, Field, Input, Select, Switch } from "@/components/ui/field";
import type { PlanPreview, RouterCfg } from "@/types/api";
import { fmtDuration } from "@/lib/format";
import { useStatsStream } from "@/lib/use-stats";
import { cn } from "@/lib/utils";

/** The three client entry points the router distinguishes when picking endpoints. */
type Protocol = "chat" | "responses" | "anthropic";

export function PolicyPage() {
  const { t, lang } = useI18n();
  const toast = useToast();
  const cfg = useConfig();
  const { stats } = useStatsStream();
  // The three protocols are separate entry points with separate eligible-endpoint sets: an
  // endpoint only takes part when its "modes" covers the protocol the client asked for. So the
  // preview has to be asked per protocol, and "no candidate" is a real answer, not an error.
  const [preview, setPreview] = React.useState<PlanPreview | null>(null);
  const [previewKind, setPreviewKind] = React.useState<Protocol>("chat");
  const [previewLoading, setPreviewLoading] = React.useState(false);

  const d = cfg.draft;

  const set = React.useCallback(
    <K extends keyof RouterCfg>(k: K, v: RouterCfg[K]) => {
      cfg.setDraft((doc) => ({ ...doc, router: { ...doc.router, [k]: v } }));
    },
    [cfg],
  );

  const runPreview = React.useCallback(
    async (kind: Protocol) => {
      setPreviewKind(kind);
      setPreviewLoading(true);
      try {
        setPreview(await api.planPreview(kind));
      } catch (e) {
        toast.push("err", e instanceof Error ? e.message : String(e));
      } finally {
        setPreviewLoading(false);
      }
    },
    [toast],
  );

  // Run once so the panel shows the current answer instead of an empty placeholder - this is the
  // first thing anyone opening the policy page wants to check.
  React.useEffect(() => {
    void runPreview("chat");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (cfg.loading || !d) return <Plate className="h-40 animate-pulse" />;
  const r = d.router;

  return (
    <div className="space-y-4">
      <PageHeader title={t.policy.title} hint={t.endpoints.hint} actions={<SaveButton cfg={cfg} />} />

      <Plate className="rise">
        <div className="grid grid-cols-2 sm:grid-cols-4">
          <Readout
            label={t.policy.peakNow}
            value={stats ? (stats.router.peak ? t.dash.peak : t.dash.offpeak) : "—"}
            tone={stats?.router.peak ? "var(--signal)" : undefined}
          />
          <Readout label={t.dash.nextChange} value={stats ? fmtDuration(stats.router.seconds_to_change, lang) : "—"} />
          <Readout label={t.policy.mode} value={r.mode} sub={r.idle_prefer} />
          <Readout
            label={t.policy.surplusMaxPct}
            value={`${r.surplus_max_pct}%`}
            sub={r.surplus_projection ? "projection on" : "projection off"}
          />
        </div>
      </Plate>

      <PlateBlock
        title={t.policy.preview}
        hint={t.policy.previewHint}
        actions={
          <>
            <Button
              size="sm"
              variant={previewKind === "chat" ? "selected" : "outline"}
              aria-pressed={previewKind === "chat"}
              onClick={() => void runPreview("chat")}
            >
              {t.policy.previewChat}
            </Button>
            <Button
              size="sm"
              variant={previewKind === "responses" ? "selected" : "outline"}
              aria-pressed={previewKind === "responses"}
              onClick={() => void runPreview("responses")}
            >
              {t.policy.previewResponses}
            </Button>
            <Button
              size="sm"
              variant={previewKind === "anthropic" ? "selected" : "outline"}
              aria-pressed={previewKind === "anthropic"}
              onClick={() => void runPreview("anthropic")}
            >
              {t.policy.previewAnthropic}
            </Button>
          </>
        }
      >
        {cfg.dirty ? (
          // Without this the panel is actively misleading: it answers for the running router
          // while the page in front of the user shows edited values.
          <div className="border-b border-[var(--line)] bg-[var(--warn)]/10 px-3 py-2 text-xs leading-relaxed text-[var(--warn)] sm:px-4">
            {t.policy.previewStale}
          </div>
        ) : null}
        {preview ? (
          <div className={cn("px-3 py-3 sm:px-4", previewLoading && "opacity-60")}>
            <div className="flex flex-wrap items-center gap-2">
              <Badge tone={preview.peak ? "signal" : "plain"} dot>
                {preview.peak ? t.dash.peak : t.dash.offpeak}
              </Badge>
              <Badge tone="plain">
                surplus={String(preview.plans_surplus)} · available={String(preview.plans_available)}
              </Badge>
            </div>
            <div className="mt-3 flex flex-wrap items-center gap-1.5">
              {preview.candidates.length === 0 ? (
                <span className="text-sm text-[var(--danger)]">{t.policy.previewEmpty}</span>
              ) : (
                preview.candidates.map((c, i) => (
                  <React.Fragment key={c.name}>
                    {i > 0 ? <span className="text-[var(--ink-faint)]">›</span> : null}
                    <span
                      className="mono rounded-[2px] border px-2 py-1 text-xs"
                      style={{
                        borderColor: c.kind === "plans" ? "var(--plans)" : "var(--cash)",
                        background:
                          c.kind === "plans"
                            ? "color-mix(in oklab, var(--plans) 12%, transparent)"
                            : "color-mix(in oklab, var(--cash) 12%, transparent)",
                        color: c.kind === "plans" ? "var(--plans)" : "var(--cash)",
                      }}
                    >
                      {c.name}
                      <span className="ml-1.5 text-2xs text-[var(--ink-faint)]">{c.model}</span>
                    </span>
                  </React.Fragment>
                ))
              )}
            </div>
            <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1">
              <p className="mono text-2xs text-[var(--ink-faint)]">
                {t.policy.previewReason}: {preview.reason}
              </p>
              {/* The preview only re-runs when asked: after saving, this is how you see the new
                  answer without walking back to the button row. */}
              <button
                type="button"
                onClick={() => void runPreview(previewKind)}
                className="mono text-2xs uppercase tracking-[0.12em] text-[var(--ink-faint)] underline transition-colors hover:text-[var(--ink)]"
              >
                {previewLoading ? t.policy.previewLoading : t.policy.previewRun}
              </button>
            </div>
            {preview.skipped.length ? (
              <p className="mono mt-1 text-2xs text-[var(--ink-faint)]">
                {t.policy.stoppedAt}: {preview.skipped.map((s) => `${s.name}(${s.reason})`).join(", ")}
              </p>
            ) : null}
          </div>
        ) : (
          <div className="px-4 py-5 text-sm text-[var(--ink-faint)]">{t.policy.previewHint}</div>
        )}
      </PlateBlock>

      <PlateBlock title={t.group.routing}>
        <div className="grid gap-3 px-3 py-3 sm:grid-cols-2 sm:px-4">
          <Field label={t.policy.mode} help={t.policy.modeHint}>
            <Select value={r.mode} onChange={(e) => set("mode", e.target.value as RouterCfg["mode"])}>
              <option value="auto">auto</option>
              <option value="plans">plans</option>
              <option value="fallback">fallback</option>
            </Select>
          </Field>
          <Field label={t.policy.idlePrefer} help={t.policy.idlePreferHint}>
            <Select value={r.idle_prefer} onChange={(e) => set("idle_prefer", e.target.value as RouterCfg["idle_prefer"])}>
              <option value="surplus_first">surplus_first</option>
              <option value="plans">plans</option>
              <option value="fallback">fallback</option>
            </Select>
          </Field>
          <Field label={t.policy.surplusMaxPct} help={t.policy.surplusMaxPctHint}>
            <Input
              type="number"
              min={0}
              max={100}
              value={r.surplus_max_pct}
              onChange={(e) => set("surplus_max_pct", Number(e.target.value) || 0)}
            />
          </Field>
          <Field label={t.policy.peakWindows} help={t.policy.peakWindowsHint}>
            <Input value={r.peak_windows} onChange={(e) => set("peak_windows", e.target.value)} />
          </Field>
          <Field label={t.policy.exhaustAtPct} help={t.policy.exhaustAtPctHint}>
            <Input
              type="number"
              min={0}
              max={100}
              value={r.exhaust_at_pct}
              onChange={(e) => set("exhaust_at_pct", Number(e.target.value) || 0)}
            />
          </Field>
        </div>
        <div className="border-t border-[var(--line)] px-3 py-1 sm:px-4">
          <ToggleLine
            label={t.policy.surplusProjection}
            help={t.policy.surplusProjectionHint}
            checked={r.surplus_projection}
            onChange={(v) => set("surplus_projection", v)}
          />
          <ToggleLine
            label={t.policy.sessionAffinity}
            help={t.policy.sessionAffinityHint}
            checked={r.session_affinity}
            onChange={(v) => set("session_affinity", v)}
          />
          <ToggleLine
            label={t.policy.retryOnModelError}
            help={t.policy.retryOnModelErrorHint}
            checked={r.retry_on_model_error}
            onChange={(v) => set("retry_on_model_error", v)}
          />
          <ToggleLine
            label={t.policy.injectStreamUsage}
            help={t.policy.injectStreamUsageHint}
            checked={r.inject_stream_usage}
            onChange={(v) => set("inject_stream_usage", v)}
          />
        </div>
      </PlateBlock>

      <PlateBlock title={t.policy.retryTitle}>
        <div className="grid gap-3 px-3 py-3 sm:grid-cols-3 sm:px-4">
          <Field label={t.policy.cooldown} help={t.policy.cooldownHint}>
            <Input type="number" value={r.cooldown_secs} onChange={(e) => set("cooldown_secs", Number(e.target.value) || 0)} />
          </Field>
          <Field label={t.policy.authCooldown} help={t.policy.authCooldownHint}>
            <Input
              type="number"
              value={r.auth_cooldown_secs}
              onChange={(e) => set("auth_cooldown_secs", Number(e.target.value) || 0)}
            />
          </Field>
          <Field label={t.policy.serverErrorCooldown} help={t.policy.serverErrorCooldownHint}>
            <Input
              type="number"
              value={r.server_error_cooldown_secs}
              onChange={(e) => set("server_error_cooldown_secs", Number(e.target.value) || 0)}
            />
          </Field>
          <Field label={t.policy.skipAfterFailures} help={t.policy.skipAfterFailuresHint}>
            <Input
              type="number"
              value={r.skip_after_failures}
              onChange={(e) => set("skip_after_failures", Number(e.target.value) || 0)}
            />
          </Field>
          <Field label={t.policy.skipSecs} help={t.policy.skipSecsHint}>
            <Input type="number" value={r.skip_secs} onChange={(e) => set("skip_secs", Number(e.target.value) || 0)} />
          </Field>
          <Field label={t.policy.attemptBudget} help={t.policy.attemptBudgetHint}>
            <Input
              type="number"
              value={r.attempt_budget_secs}
              onChange={(e) => set("attempt_budget_secs", Number(e.target.value) || 0)}
            />
          </Field>
          <Field label={t.policy.quotaRefreshSecs} help={t.policy.quotaRefreshSecsHint}>
            <Input
              type="number"
              value={r.quota_refresh_secs}
              onChange={(e) => set("quota_refresh_secs", Number(e.target.value) || 0)}
            />
          </Field>
        </div>
      </PlateBlock>

      <PlateBlock title="session">
        <div className="grid gap-3 px-3 py-3 sm:grid-cols-2 sm:px-4">
          <Field label={t.policy.sessionFallback} help={t.policy.sessionFallbackHint}>
            <Select
              value={r.session_fallback}
              onChange={(e) => set("session_fallback", e.target.value as RouterCfg["session_fallback"])}
            >
              <option value="process">process</option>
              <option value="per-request">per-request</option>
            </Select>
          </Field>
          <Field label={t.policy.sessionAffinityTtl} help={t.policy.sessionAffinityTtlHint}>
            <Input
              type="number"
              value={r.session_affinity_ttl_secs}
              onChange={(e) => set("session_affinity_ttl_secs", Number(e.target.value) || 0)}
            />
          </Field>
        </div>
        <div className="space-y-3 border-t border-[var(--line)] px-3 py-3 sm:px-4">
          <Field label={t.policy.sessionHeaders} help={t.policy.sessionHeadersHint}>
            <ChipInput values={r.session_headers} onChange={(v) => set("session_headers", v)} placeholder="x-session-id" />
          </Field>
          <Field label={t.policy.userAgent} help={t.policy.userAgentHint}>
            <Input value={r.user_agent} onChange={(e) => set("user_agent", e.target.value)} />
          </Field>
        </div>
      </PlateBlock>

      <PlateBlock title={t.policy.compatTitle} hint={t.policy.compatHint}>
        <div className="px-3 py-1 sm:px-4">
          <ToggleLine
            label={t.policy.developerRole}
            help={t.policy.developerRoleHint}
            checked={d.compat.developer_role_to_system}
            onChange={(v) => cfg.setDraft((doc) => ({ ...doc, compat: { ...doc.compat, developer_role_to_system: v } }))}
          />
          <ToggleLine
            label={t.policy.maxCompletionTokens}
            help={t.policy.maxCompletionTokensHint}
            checked={d.compat.max_completion_tokens_to_max_tokens}
            onChange={(v) =>
              cfg.setDraft((doc) => ({ ...doc, compat: { ...doc.compat, max_completion_tokens_to_max_tokens: v } }))
            }
          />
        </div>
        <div className="space-y-3 border-t border-[var(--line)] px-3 py-3 sm:px-4">
          <Field label={t.policy.dropParams} help={t.policy.dropParamsHint}>
            <ChipInput
              values={d.compat.drop_params}
              onChange={(v) => cfg.setDraft((doc) => ({ ...doc, compat: { ...doc.compat, drop_params: v } }))}
              placeholder="store"
            />
          </Field>
          <Field label={t.policy.forwardHeaders} help={t.policy.forwardHeadersHint}>
            <ChipInput
              values={d.compat.forward_headers}
              onChange={(v) => cfg.setDraft((doc) => ({ ...doc, compat: { ...doc.compat, forward_headers: v } }))}
              placeholder="anthropic-beta"
            />
          </Field>
        </div>
      </PlateBlock>

      {d.warnings?.length ? (
        <Plate className="border-[var(--warn)]/40 p-3">
          {d.warnings.map((w, i) => (
            <p key={i} className="text-xs leading-relaxed" style={{ color: "var(--warn)" }}>
              · {w}
            </p>
          ))}
        </Plate>
      ) : null}

      <SaveButton cfg={cfg} />
    </div>
  );
}

function ToggleLine({
  label,
  help,
  checked,
  onChange,
}: {
  label: string;
  help?: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  const { t } = useI18n();
  const [open, setOpen] = React.useState(false);
  const switchId = React.useId();
  return (
    <div className="flex items-center justify-between gap-3 border-b border-[var(--line)] py-2.5 last:border-b-0">
      <div className="flex min-w-0 items-center gap-2">
        <label htmlFor={switchId} className="cursor-pointer text-sm">
          {label}
        </label>
        {help ? (
          <button
            type="button"
            onClick={() => setOpen((v) => !v)}
            aria-label={t.field.help}
            className="mono flex h-4 w-4 shrink-0 items-center justify-center rounded-full border border-[var(--line-strong)] text-2xs leading-none text-[var(--ink-faint)] hover:border-[var(--signal)] hover:text-[var(--signal)]"
          >
            ?
          </button>
        ) : null}
      </div>
      <div className="flex shrink-0 items-center gap-2">
        {open && help ? (
          <span className="hidden min-w-0 flex-1 text-right text-xs leading-snug text-[var(--ink-faint)] sm:inline">{help}</span>
        ) : null}
        <Switch id={switchId} checked={checked} onChange={onChange} label={label} />
      </div>
    </div>
  );
}

function SaveButton({ cfg, block }: { cfg: ReturnType<typeof useConfig>; block?: boolean }) {
  const { t } = useI18n();
  const toast = useToast();
  if (!cfg.dirty) return null;
  // Bottom placement of the same action. Right-aligned rather than full width so it reads as
  // the same control as the header button, not as a page-level banner.
  return (
    <div className="flex justify-end">
      <Button
        variant="primary"
        loading={cfg.saving}
      onClick={async () => {
        const ok = await cfg.save();
        toast.push(ok ? "ok" : "err", ok ? t.common.saved : (cfg.error ?? t.common.error));
      }}
    >
        {cfg.saving ? t.common.saving : t.common.save}
      </Button>
    </div>
  );
}
