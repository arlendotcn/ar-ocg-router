"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { Badge } from "@/components/ui/badge";
import { Button, Spinner } from "@/components/ui/button";
import { ChipInput, Field, Input, Select, Switch, Textarea } from "@/components/ui/field";
import { Sheet } from "@/components/ui/sheet";
import { copyText } from "@/lib/format";
import { ModelPicker } from "@/components/model-picker";
import type { EndpointCfg, ModelEntry, TestResult } from "@/types/api";

const RULES = ["always", "peak", "offpeak", "quota_low", "quota_exhausted", "primary_unavailable", "never"] as const;
const MODES = ["both", "openai-completion", "openai-responses", "anthropic-messages", "any"] as const;

export function EndpointSheet({
  open,
  endpoint,
  library,
  onClose,
  onCommit,
  onTest,
}: {
  open: boolean;
  endpoint: EndpointCfg | null;
  /** Model reference specs, used to annotate whatever id is chosen. */
  library: ModelEntry[];
  onClose: () => void;
  onCommit: (e: EndpointCfg) => void;
  onTest: (name: string) => Promise<TestResult | null>;
}) {
  const { t, lang } = useI18n();
  const [draft, setDraft] = React.useState<EndpointCfg | null>(endpoint);
  const [headersText, setHeadersText] = React.useState("");
  const [advanced, setAdvanced] = React.useState(false);
  const [testing, setTesting] = React.useState(false);
  const [result, setResult] = React.useState<TestResult | null>(null);
  const [copied, setCopied] = React.useState<string | null>(null);

  React.useEffect(() => {
    setDraft(endpoint);
    setResult(null);
    setAdvanced(false);
    setHeadersText(
      endpoint
        ? Object.entries(endpoint.headers ?? {})
            .map(([k, v]) => `${k}: ${v}`)
            .join("\n")
        : "",
    );
  }, [endpoint]);

  if (!open || !draft) return null;

  const set = <K extends keyof EndpointCfg>(k: K, v: EndpointCfg[K]) => setDraft((d) => (d ? { ...d, [k]: v } : d));
  const setQuota = <K extends keyof EndpointCfg["quota"]>(k: K, v: EndpointCfg["quota"][K]) =>
    setDraft((d) => (d ? { ...d, quota: { ...d.quota, [k]: v } } : d));

  const parseHeaders = (text: string) => {
    const out: Record<string, string> = {};
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith("#")) continue;
      const idx = trimmed.indexOf(":");
      if (idx <= 0) continue;
      out[trimmed.slice(0, idx).trim()] = trimmed.slice(idx + 1).trim();
    }
    return out;
  };

  const providerInferred = inferProvider(draft.url);
  const isGo = draft.provider === "opencodego";
  const errors = validate(draft);

  const curlFor = () => {
    const base = draft.url.replace(/\/+$/, "");
    const url = base.endsWith("/v1") ? `${base}/chat/completions` : `${base}/v1/chat/completions`;
    const lines = [
      `curl -sS ${url} \\`,
      `  -H 'Content-Type: application/json' \\`,
      `  -H 'Authorization: Bearer ${draft.key || "$KEY"}' \\`,
    ];
    if (draft.inject_session) lines.push(`  -H "x-opencode-session: $(uuidgen || echo probe)" \\`);
    for (const [k, v] of Object.entries(parseHeaders(headersText))) lines.push(`  -H '${k}: ${v}' \\`);
    lines.push(`  -d '{"model":"${draft.model || "MODEL"}","messages":[{"role":"user","content":"ping"}],"max_tokens":8}'`);
    return lines.join("\n");
  };

  const doCopy = async (what: string, text: string) => {
    if (await copyText(text)) {
      setCopied(what);
      setTimeout(() => setCopied(null), 1600);
    }
  };

  return (
    <Sheet
      open={open}
      onClose={onClose}
      title={endpoint && endpoint.name ? t.endpoints.editEndpoint : t.endpoints.newEndpoint}
      subtitle={`[${draft.kind}] ${draft.name || t.endpoints.nameHint.split(".")[0]}`}
      wide
      footer={
        <>
          <span className="mr-auto flex items-center gap-2">
            <Switch checked={draft.enabled} onChange={(v) => set("enabled", v)} label={t.common.enabled} />
            <span className="text-xs text-[var(--ink-faint)]">{draft.enabled ? t.common.enabledHint : t.common.disabledHint}</span>
          </span>
          <Button variant="ghost" onClick={onClose}>
            {t.common.cancel}
          </Button>
          <Button
            variant="primary"
            disabled={errors.length > 0}
            onClick={() => {
              set("headers", parseHeaders(headersText));
              setTimeout(() => onCommit({ ...draft, headers: parseHeaders(headersText) }), 0);
            }}
          >
            {t.common.confirm}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        {/* ---------- basics ---------- */}
        <Section title={t.group.basics}>
          <Field label={t.endpoints.name} help={t.endpoints.nameHint}>
            <Input value={draft.name} placeholder="go-dsf" onChange={(e) => set("name", e.target.value)} />
          </Field>

          {/* Full width: the picker expands to show a catalog, which needs the room. */}
          <Field label={t.endpoints.model} help={t.endpoints.modelHint} required>
            <ModelPicker
              value={draft.model}
              onChange={(v) => set("model", v)}
              endpointName={endpoint && endpoint.name ? endpoint.name : undefined}
              library={library}
              placeholder="deepseek-flash"
            />
          </Field>

          <Field label={t.endpoints.url} help={t.endpoints.urlHint} required>
            <div className="flex gap-2">
              <Input
                value={draft.url}
                placeholder="https://api.deepseek.com"
                onChange={(e) => set("url", e.target.value)}
                className="flex-1"
              />
              <Button size="sm" variant="outline" onClick={() => doCopy("url", draft.url)} className="h-9 shrink-0">
                {copied === "url" ? t.common.copied : t.common.copy}
              </Button>
            </div>
          </Field>

          <Field
            label={t.endpoints.key}
            help={t.endpoints.keyHint}
            hint={endpoint && endpoint.name ? t.endpoints.keyRedactedHint : undefined}
            required
          >
            <Input
              value={draft.key}
              placeholder="sk-…  /  env:MY_KEY  /  file:C:/keys/ds.txt"
              onChange={(e) => set("key", e.target.value)}
              type="password"
              autoComplete="off"
              spellCheck={false}
            />
          </Field>

          <div className="grid gap-3 sm:grid-cols-3">
            <Field
              label={t.endpoints.provider}
              help={t.endpoints.providerHint}
              hint={providerInferred !== draft.provider ? `auto: ${providerInferred}` : undefined}
            >
              <Select value={draft.provider} onChange={(e) => set("provider", e.target.value as EndpointCfg["provider"])}>
                <option value="generic">generic</option>
                <option value="opencodego">opencodego</option>
                <option value="deepseek">deepseek</option>
              </Select>
            </Field>
            <Field label={t.endpoints.mode} help={t.endpoints.modeHint}>
              <Select
                value={draft.modes.length === 1 ? draft.modes[0] : "both"}
                onChange={(e) => set("modes", e.target.value === "both" ? ["both"] : [e.target.value])}
              >
                {MODES.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </Select>
            </Field>
          </div>

          <div className="grid gap-3 sm:grid-cols-2">
            {draft.kind === "fallback" ? (
              <Field label={t.endpoints.rule} help={t.endpoints.ruleHint} hint={t.rules[(draft.rules[0] ?? "always") as keyof typeof t.rules]}>
                <Select
                  value={draft.rules[0] ?? "always"}
                  onChange={(e) => set("rules", [e.target.value])}
                >
                  {RULES.map((r) => (
                    <option key={r} value={r}>
                      {r}
                    </option>
                  ))}
                </Select>
              </Field>
            ) : null}
          </div>

          {isGo ? (
            <Field label={t.endpoints.injectSession} help={t.endpoints.injectSessionHint}>
              <SwitchRow checked={draft.inject_session} onChange={(v) => set("inject_session", v)} />
            </Field>
          ) : null}
        </Section>

        {/* ---------- quota ---------- */}
        <Section title={t.endpoints.quotaTitle}>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label={t.endpoints.unit} help={t.endpoints.unitHint}>
              <Select value={draft.quota.unit} onChange={(e) => setQuota("unit", e.target.value as EndpointCfg["quota"]["unit"])}>
                <option value="none">none</option>
                <option value="usd">usd</option>
                <option value="tokens">tokens</option>
              </Select>
            </Field>
            <Field label={t.endpoints.probe} help={t.endpoints.probeHint}>
              <Select value={draft.quota.probe} onChange={(e) => setQuota("probe", e.target.value as EndpointCfg["quota"]["probe"])}>
                <option value="none">none</option>
                <option value="usage">usage</option>
                <option value="balance">balance</option>
              </Select>
            </Field>
          </div>
          {draft.quota.unit !== "none" ? (
            <div className="grid gap-3 sm:grid-cols-3">
              <Field label={t.endpoints.rolling} help={t.endpoints.limitsHint}>
                <Input type="number" value={draft.quota.rolling} onChange={(e) => setQuota("rolling", Number(e.target.value) || 0)} />
              </Field>
              <Field label={t.endpoints.weekly}>
                <Input type="number" value={draft.quota.weekly} onChange={(e) => setQuota("weekly", Number(e.target.value) || 0)} />
              </Field>
              <Field label={t.endpoints.monthly}>
                <Input type="number" value={draft.quota.monthly} onChange={(e) => setQuota("monthly", Number(e.target.value) || 0)} />
              </Field>
            </div>
          ) : null}
        </Section>

        {/* ---------- advanced ---------- */}
        <button
          type="button"
          onClick={() => setAdvanced((v) => !v)}
          className="mono flex w-full items-center gap-2 py-1 text-xs uppercase tracking-[0.14em] text-[var(--ink-faint)] transition-colors hover:text-[var(--ink)]"
        >
          <span>{advanced ? t.common.hideAdvanced : t.common.showAdvanced}</span>
          <span className="text-2xs">{advanced ? "▴" : "▾"}</span>
        </button>

        {advanced ? (
          <div className="rise space-y-3">
            <Field label={t.endpoints.quotaTitle + " · " + t.endpoints.refreshSecs} help={t.endpoints.refreshSecsHint}>
              <Input
                type="number"
                value={draft.quota.refresh_secs}
                onChange={(e) => setQuota("refresh_secs", Number(e.target.value) || 300)}
              />
            </Field>
            <Field label={t.endpoints.headers} help={t.endpoints.headersHint}>
              <Textarea
                rows={3}
                value={headersText}
                spellCheck={false}
                placeholder={"user-agent: claude-cli/2.1.0 (external, cli)"}
                onChange={(e) => setHeadersText(e.target.value)}
              />
            </Field>
            <Field label={t.endpoints.dropParams} help={t.endpoints.dropParamsHint}>
              <ChipInput
                values={draft.drop_params}
                onChange={(v) => set("drop_params", v)}
                placeholder="store, service_tier"
              />
            </Field>
            <Field label={t.endpoints.noErrorFallback} help={t.endpoints.noErrorFallbackHint}>
              <SwitchRow checked={draft.no_error_fallback} onChange={(v) => set("no_error_fallback", v)} />
            </Field>
          </div>
        ) : null}

        {/* ---------- live test ---------- */}
        {endpoint && endpoint.name ? (
          <Section
            title={t.endpoints.testTitle}
            hint={t.endpoints.testHint}
            aside={
              <Button
                size="sm"
                variant="outline"
                loading={testing}
                onClick={async () => {
                  setTesting(true);
                  setResult(await onTest(endpoint.name));
                  setTesting(false);
                }}
              >
                {testing ? t.common.testing : t.common.test}
              </Button>
            }
          >
            {testing ? (
              <div className="flex items-center gap-2 text-sm text-[var(--ink-faint)]">
                <Spinner /> {t.endpoints.testRunning}
              </div>
            ) : result ? (
              <div className="space-y-2">
                <div className="flex flex-wrap gap-2">
                  <Badge tone={result.key_ok ? "on" : "danger"} dot>
                    key
                  </Badge>
                  <Badge tone={result.models.ok ? "on" : "danger"} dot>
                    {t.endpoints.models} {result.models.count}
                  </Badge>
                  {result.quota ? (
                    <Badge tone={result.quota.ok ? "on" : "warn"} dot>
                      {result.quota.kind}
                    </Badge>
                  ) : null}
                  <Badge tone={result.chat.ok ? "on" : "danger"} dot>
                    {t.endpoints.chatOk} {result.chat.status || ""}
                  </Badge>
                </div>
                <pre className="mono max-h-[220px] overflow-auto whitespace-pre-wrap break-all rounded-[2px] border border-[var(--line)] bg-[var(--panel-2)] p-2.5 text-xs leading-relaxed">
                  {[
                    result.models.ok
                      ? `${t.endpoints.models}: ${result.models.count}${result.models.ids.length ? "\n  " + result.models.ids.slice(0, 24).join(", ") : ""}`
                      : `${t.endpoints.models}: ${result.models.error ?? "failed"}`,
                    result.quota ? `${result.quota.kind}: ${result.quota.detail}` : null,
                    result.chat.ok
                      ? `${t.endpoints.chatOk}: 200 (${result.chat.latency_ms}ms, model=${result.chat.model})`
                      : `${t.endpoints.chatOk}: ${result.chat.status} ${result.chat.error ?? ""}`,
                    result.suggestions.length ? "\n" + result.suggestions.map((s) => "· " + s).join("\n") : null,
                  ]
                    .filter(Boolean)
                    .join("\n")}
                </pre>
              </div>
            ) : (
              <p className="text-xs text-[var(--ink-faint)]">{t.endpoints.testHint}</p>
            )}
          </Section>
        ) : null}

        <Section title={t.common.advanced}>
          <Button size="sm" variant="ghost" onClick={() => doCopy("curl", curlFor())}>
            {copied === "curl" ? t.endpoints.curlCopied : t.endpoints.copyCurl}
          </Button>
        </Section>

        {errors.length > 0 ? (
          <div className="rounded-[2px] border border-[var(--danger)]/50 px-3 py-2 text-xs text-[var(--danger)]">
            {errors.map((e) => (
              <div key={e}>· {e}</div>
            ))}
          </div>
        ) : null}
      </div>
    </Sheet>
  );
}

function Section({
  title,
  hint,
  aside,
  children,
}: {
  title: string;
  hint?: string;
  aside?: React.ReactNode;
  children: React.ReactNode;
}) {
  const { t } = useI18n();
  const [open, setOpen] = React.useState(false);
  return (
    <section className="plate">
      <div className="flex items-center justify-between gap-2 border-b border-[var(--line)] px-3 py-2">
        <div className="flex items-center gap-2">
          <h3 className="mono text-xs uppercase tracking-[0.16em]">{title}</h3>
          {hint ? (
            <button
              type="button"
              onClick={() => setOpen((v) => !v)}
              aria-label={t.field.help}
              className="mono flex h-4 w-4 items-center justify-center rounded-full border border-[var(--line-strong)] text-2xs leading-none text-[var(--ink-faint)] hover:border-[var(--signal)] hover:text-[var(--signal)]"
            >
              ?
            </button>
          ) : null}
        </div>
        {aside}
      </div>
      {open && hint ? (
        <p className="rise border-b border-[var(--line)] bg-[var(--panel-2)] px-3 py-2 text-xs leading-relaxed text-[var(--ink-dim)]">
          {hint}
        </p>
      ) : null}
      <div className="space-y-3 px-3 py-3">{children}</div>
    </section>
  );
}

function SwitchRow({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center gap-2">
      <Switch checked={checked} onChange={onChange} />
      <span className="mono text-xs text-[var(--ink-faint)]">{checked ? "on" : "off"}</span>
    </div>
  );
}

function inferProvider(url: string): EndpointCfg["provider"] {
  const u = url.toLowerCase();
  if (u.includes("api.deepseek.com")) return "deepseek";
  if (u.includes("opencode.ai")) return "opencodego";
  return "generic";
}

function validate(e: EndpointCfg): string[] {
  const out: string[] = [];
  if (!e.url || !/^https?:\/\//.test(e.url)) out.push("url must start with http:// or https://");
  if (!e.model.trim()) out.push("model is required: one endpoint serves exactly one model");
  if (!e.key.trim()) out.push("key is required (env:VAR and file:path are allowed)");
  if (e.url.endsWith("/")) out.push("url should not end with a slash");
  return out;
}
