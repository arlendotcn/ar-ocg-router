"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { api } from "@/lib/api";
import { Badge } from "@/components/ui/badge";
import { Button, Spinner } from "@/components/ui/button";
import { Input } from "@/components/ui/field";
import { fmtNum } from "@/lib/format";
import { cn } from "@/lib/utils";
import type { ModelEntry } from "@/types/api";

/**
 * A model id field that can also be *browsed*. The upstream catalog is a suggestion source,
 * never a constraint, so free text always stays available: upstreams list ids they cannot serve
 * and omit ones they can (README 10.1). The library annotates whatever is shown with the specs
 * we actually know.
 */
export function ModelPicker({
  value,
  onChange,
  endpointName,
  library,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  endpointName?: string;
  library: ModelEntry[];
  placeholder?: string;
}) {
  const { t } = useI18n();
  const [open, setOpen] = React.useState(false);
  const [loading, setLoading] = React.useState(false);
  const [models, setModels] = React.useState<string[] | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [filter, setFilter] = React.useState("");

  const byId = React.useMemo(() => {
    const m = new Map<string, ModelEntry>();
    for (const e of library) {
      m.set(e.id.toLowerCase(), e);
      for (const a of e.aliases) m.set(a.toLowerCase(), e);
    }
    return m;
  }, [library]);

  const spec = byId.get(value.trim().toLowerCase());

  // A library-only list is the fallback when no endpoint is chosen yet (a brand new endpoint has
  // no upstream to ask), so the picker is still useful before the first save.
  const fetchModels = React.useCallback(async () => {
    setOpen(true);
    if (!endpointName) {
      setModels(library.map((e) => e.id));
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const res = await api.endpointModels(endpointName);
      setModels(res.models);
      if (!res.ok) setError(res.error ?? t.picker.failed);
    } catch (e) {
      setModels([]);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [endpointName, library, t.picker.failed]);

  const shown = React.useMemo(() => {
    const q = filter.trim().toLowerCase();
    const list = models ?? (endpointName ? [] : library.map((e) => e.id));
    if (!q) return list;
    return list.filter((m) => {
      if (m.toLowerCase().includes(q)) return true;
      const e = byId.get(m.toLowerCase());
      return Boolean(e && (e.label.toLowerCase().includes(q) || e.aliases.some((a) => a.toLowerCase().includes(q))));
    });
  }, [models, filter, byId, endpointName, library]);

  return (
    <div className="space-y-2">
      <div className="flex gap-2">
        <Input
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholder}
          spellCheck={false}
          className="flex-1"
        />
        <Button size="sm" variant="outline" className="h-9 shrink-0" onClick={fetchModels}>
          {loading ? <Spinner /> : null}
          {loading ? t.picker.fetching : t.picker.fetch}
        </Button>
      </div>

      {spec ? (
        <ModelSpecLine entry={spec} />
      ) : value.trim() ? (
        <p className="text-2xs leading-snug text-[var(--ink-faint)]">{t.picker.unknownLimits}</p>
      ) : null}

      {open ? (
        <div className="rise overflow-hidden rounded-[2px] border border-[var(--line-strong)] bg-[var(--panel)] shadow-lg">
          <div className="flex items-center justify-between gap-2 border-b border-[var(--line)] px-2.5 py-2">
            <span className="label">{t.picker.title}</span>
            <span className="mono text-2xs text-[var(--ink-faint)]">
              {models ? t.picker.count.replace("{n}", String(shown.length)) : "—"}
            </span>
          </div>

          <div className="px-2.5 pt-2 text-2xs leading-relaxed text-[var(--ink-faint)]">{t.picker.hint}</div>
          <div className="h-px bg-[var(--line)]" />

          {error ? (
            <div className="mx-2.5 mt-2 rounded-[2px] border border-[var(--warn)]/50 px-2 py-1.5 text-2xs" style={{ color: "var(--warn)" }}>
              {t.picker.failed}: {error}
            </div>
          ) : null}

          <div className="p-2.5">
            <Input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder={t.picker.search}
              aria-label={t.picker.search}
              className="h-8"
            />
          </div>

          <div className="max-h-[min(46vh,320px)] overflow-y-auto border-t border-[var(--line)]">
            {loading ? (
              <div className="px-3 py-4 text-sm text-[var(--ink-faint)]">{t.picker.fetching}</div>
            ) : shown.length === 0 ? (
              <div className="px-3 py-4 text-sm text-[var(--ink-faint)]">
                {models && models.length === 0 ? t.picker.empty : t.common.empty}
              </div>
            ) : (
              shown.map((m) => {
                const e = byId.get(m.toLowerCase());
                const isCurrent = m.toLowerCase() === value.trim().toLowerCase();
                return (
                  <button
                    key={m}
                    type="button"
                    onClick={() => {
                      onChange(m);
                      setOpen(false);
                    }}
                    className={cn(
                      "flex w-full items-center gap-2 border-b border-[var(--line)] px-3 py-2 text-left transition-colors last:border-b-0 hover:bg-[var(--panel)]",
                      isCurrent && "bg-[var(--signal)]/10",
                    )}
                  >
                    <span className="mono min-w-0 flex-1 truncate text-xs" title={m}>{m}</span>
                    {isCurrent ? <Badge tone="signal">{t.picker.already}</Badge> : null}
                    {e ? (
                      <>
                        <span className="mono shrink-0 text-2xs text-[var(--ink-faint)]">
                          {e.context_tokens > 0 ? `${fmtNum(Math.round(e.context_tokens / 1000))}k` : "—"}
                        </span>
                        {e.input_modalities.some((x) => x !== "text") ? (
                          <Badge tone="cash">{e.input_modalities.filter((x) => x !== "text").join("/")}</Badge>
                        ) : null}
                      </>
                    ) : (
                      <span className="span shrink-0 text-2xs text-[var(--ink-faint)]">—</span>
                    )}
                  </button>
                );
              })
            )}
          </div>
        </div>
      ) : null}
    </div>
  );
}

/** One-line spec summary shown under the field, so the limits are visible while choosing. */
export function ModelSpecLine({ entry }: { entry: ModelEntry }) {
  const parts: string[] = [];
  if (entry.context_tokens > 0) parts.push(`${fmtNum(Math.round(entry.context_tokens / 1000))}k ctx`);
  if (entry.max_output_tokens > 0) parts.push(`${fmtNum(Math.round(entry.max_output_tokens / 1000))}k out`);
  if (entry.reasoning_levels.length) parts.push(entry.reasoning_levels.join("/"));
  if (entry.input_modalities.length) parts.push(entry.input_modalities.join("+"));
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <Badge tone="plain">{entry.label}</Badge>
      {parts.map((p) => (
        <span key={p} className="mono text-2xs text-[var(--ink-faint)]">
          {p}
        </span>
      ))}
    </div>
  );
}
