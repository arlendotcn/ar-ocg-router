"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { Badge } from "@/components/ui/badge";
import { Button, Spinner } from "@/components/ui/button";
import { ChipInput, Field, Input, Select, Switch, Textarea } from "@/components/ui/field";
import { Sheet } from "@/components/ui/sheet";
import { copyText } from "@/lib/format";
import { cn } from "@/lib/utils";
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
  /** Probes the endpoint as the form currently has it, without saving. */
  onTest: (endpoint: EndpointCfg) => Promise<TestResult | null>;
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
  const setPrices = (patch: Partial<EndpointCfg["prices"]>) =>
    setDraft((d) => (d ? { ...d, prices: { ...d.prices, ...patch } } : d));

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
                <option value="rmb">rmb</option>
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
          {/* The three windows keep a plain input on purpose: 0 is a real value here ("this window
              is not measured"), and measures_something() treats it exactly that way - unlike the
              numeric fields on the policy and settings pages, which the parser clamps and which
              therefore refuse out-of-range input. */}
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
        {/* Same plate as every other section, just collapsible: the fields inside are part of the
            saved endpoint, so they belong in a container that looks like the rest, not in a bare
            box hanging under a text link. */}
        <section className="plate">
          <button
            type="button"
            onClick={() => setAdvanced((v) => !v)}
            aria-expanded={advanced}
            // The divider belongs to the header only while there is something under it. Collapsed,
            // it sat on top of the plate's own bottom border and read as a doubled edge.
            className={cn(
              "flex w-full items-center justify-between gap-2 px-3 py-2 text-left transition-colors hover:bg-[var(--panel-2)]/50",
              advanced && "border-b border-[var(--line)]",
            )}
          >
            <span className="mono text-xs uppercase tracking-[0.16em]">{t.endpoints.advancedTitle}</span>
            <span className="text-2xs text-[var(--ink-faint)]">{advanced ? "▴" : "▾"}</span>
          </button>
          {advanced ? (
            <div className="rise space-y-3 px-3 py-3">
            <Field label={t.endpoints.quotaTitle + " · " + t.endpoints.refreshSecs} help={t.endpoints.refreshSecsHint}>
              <Input
                type="number"
                value={draft.quota.refresh_secs}
                onChange={(e) => setQuota("refresh_secs", Number(e.target.value) || 300)}
              />
            </Field>
            {/* A ceiling, not a fixed value: only a request that asks for more is rewritten down
                to it. Empty means "not set", which is written as the key being absent rather than
                as 0 - two spellings of "nothing" in one file is how the wrong one gets read. Zero,
                empty and unparseable input are all ignored, matching the server side. */}
            <Field label={t.endpoints.maxOutputTokens} help={t.endpoints.maxOutputTokensHint}>
              {/* No placeholder on purpose. Any number here would read as "the normal value" and
                  get copied onto endpoints that have no ceiling at all, silently shortening every
                  long answer. This field is a measured fact about one upstream, not a default. */}
              <Input
                type="number"
                min={0}
                value={draft.max_output_tokens_limit > 0 ? draft.max_output_tokens_limit : ""}
                onChange={(e) => {
                  const n = Math.floor(Number(e.target.value));
                  set("max_output_tokens_limit", Number.isFinite(n) && n > 0 ? n : 0);
                }}
              />
            </Field>
            {/* Rates are per endpoint and per provider, so they belong here rather than in a table
                inside the binary. A currency alone is a label: the router never converts. */}
            <Field
              label={t.endpoints.prices}
              help={t.endpoints.pricesHint}
              hint={priceHint(draft, t)}
            >
              <div className="flex flex-wrap items-center gap-2">
                <Select
                  value={draft.prices.currency}
                  onChange={(e) => setPrices({ currency: e.target.value })}
                  className="w-[110px] shrink-0"
                >
                  <option value="">—</option>
                  <option value="USD">USD</option>
                  <option value="CNY">CNY</option>
                </Select>
                <span className="mono text-2xs text-[var(--ink-faint)]">{t.endpoints.priceIn}</span>
                <Input
                  type="number"
                  step="0.001"
                  min={0}
                  className="w-[92px] shrink-0"
                  value={draft.prices.input || ""}
                  onChange={(e) => setPrices({ input: numOr0(e.target.value) })}
                />
                <span className="mono text-2xs text-[var(--ink-faint)]">{t.endpoints.priceOut}</span>
                <Input
                  type="number"
                  step="0.001"
                  min={0}
                  className="w-[92px] shrink-0"
                  value={draft.prices.output || ""}
                  onChange={(e) => setPrices({ output: numOr0(e.target.value) })}
                />
                <span className="mono text-2xs text-[var(--ink-faint)]">{t.endpoints.priceCached}</span>
                <Input
                  type="number"
                  step="0.001"
                  min={0}
                  className="w-[92px] shrink-0"
                  value={draft.prices.cached_input || ""}
                  onChange={(e) => setPrices({ cached_input: numOr0(e.target.value) })}
                />
                <span className="mono text-2xs text-[var(--ink-faint)]">{t.endpoints.pricePeak}</span>
                <Input
                  type="number"
                  step="0.1"
                  min={1}
                  className="w-[72px] shrink-0"
                  value={draft.prices.peak_multiplier}
                  onChange={(e) => setPrices({ peak_multiplier: Number(e.target.value) || 1 })}
                />
              </div>
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
            {/* Moved in here: a section of its own that held one button was a heading pretending to
                be a feature, and calling it "Advanced" next to the real advanced options read as
                two different things with the same name. */}
            <Field label={t.endpoints.tools} help={t.endpoints.toolsHint}>
              <Button size="sm" variant="ghost" onClick={() => doCopy("curl", curlFor())}>
                {copied === "curl" ? t.endpoints.curlCopied : t.endpoints.copyCurl}
              </Button>
            </Field>
            </div>
          ) : null}
        </section>

        {/* ---------- live test ---------- */}
        {endpoint && endpoint.name ? (
          <Section
            title={t.endpoints.testTitle}
            hint={t.endpoints.testDraftHint}
            aside={
              <Button
                size="sm"
                variant="outline"
                loading={testing}
                onClick={async () => {
                  setTesting(true);
                  // Probe the form, not the file: a test button sitting next to Save would
                  // otherwise report on the configuration as it was before the edits above.
                  set("headers", parseHeaders(headersText));
                  setResult(await onTest({ ...draft, headers: parseHeaders(headersText) }));
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

const numOr0 = (raw: string) => {
  const n = Number(raw);
  return Number.isFinite(n) && n > 0 ? n : 0;
};

/** What the operator needs to know while filling the rates: whose prices win, and whether these
 *  numbers are used at all. */
function priceHint(e: EndpointCfg, t: ReturnType<typeof useI18n>["t"]): string | undefined {
  const set = e.prices.input > 0 || e.prices.output > 0 || e.prices.cached_input > 0;
  if (!set) return t.endpoints.pricesUnset;
  return e.provider === "opencodego" ? t.endpoints.pricesUpstreamWins : undefined;
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
