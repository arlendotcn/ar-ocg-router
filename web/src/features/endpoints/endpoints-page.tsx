"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { useConfig } from "@/lib/use-config";
import { api } from "@/lib/api";
import { useToast } from "@/components/ui/toast";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmButton } from "@/components/ui/confirm-button";
import { Plate, PlateBlock } from "@/components/ui/plate";
import { PageHeader } from "@/components/page-header";
import { EndpointSheet } from "./endpoint-sheet";
import { fmtUsd } from "@/lib/format";
import type { EndpointCfg, ModelEntry } from "@/types/api";
import { useStatsStream } from "@/lib/use-stats";
import { cn } from "@/lib/utils";

const BLANK: EndpointCfg = {
  name: "",
  kind: "plans",
  order: 10,
  provider: "generic",
  url: "",
  key: "",
  model: "",
  modes: ["both"],
  weight: 50,
  rules: ["always"],
  no_error_fallback: false,
  inject_session: false,
  headers: {},
  drop_params: [],
  quota: { unit: "none", rolling: 0, weekly: 0, monthly: 0, probe: "none", refresh_secs: 300 },
  enabled: true,
};

export function EndpointsPage() {
  const { t } = useI18n();
  const toast = useToast();
  const cfg = useConfig();
  const { stats } = useStatsStream();
  const [editing, setEditing] = React.useState<{ index: number; endpoint: EndpointCfg } | null>(null);
  // Reference specs from the model library, so the editor can annotate a chosen model id.
  const [library, setLibrary] = React.useState<ModelEntry[]>([]);

  React.useEffect(() => {
    api
      .library()
      .then((l) => setLibrary(l.models ?? []))
      .catch(() => setLibrary([]));
  }, []);

  // The list renders the server's copy. Every row action applies immediately, so there is no
  // page-level draft to go stale: the endpoint editor keeps its own form state and hands back a
  // finished endpoint, which is written straight away.
  const doc = cfg.doc;
  const liveByName = React.useMemo(() => {
    const m = new Map<string, { requests: number; saved: number; cost: number; available: boolean }>();
    stats?.accounts.forEach((a) =>
      m.set(a.name, { requests: a.stats.requests, saved: a.stats.saved_usd, cost: a.stats.cost_usd, available: a.available }),
    );
    return m;
  }, [stats]);

  // Rows are shown in routing order, which is what "order" means. The index is kept from the
  // original array so edit/remove still address the right endpoint after sorting.
  const rows = (doc?.endpoints ?? []).map((e, i) => ({ e, i }));
  const byOrder = (x: { e: EndpointCfg }, y: { e: EndpointCfg }) => x.e.order - y.e.order;
  const plans = rows.filter(({ e }) => e.kind === "plans").sort(byOrder);
  const cash = rows.filter(({ e }) => e.kind === "fallback").sort(byOrder);

  const add = (kind: "plans" | "fallback") => {
    const maxOrder = Math.max(0, ...(doc?.endpoints ?? []).filter((e) => e.kind === kind).map((e) => e.order));
    setEditing({
      index: -1,
      endpoint: { ...BLANK, kind, order: maxOrder + 10, name: "" },
    });
  };

  /// Write the endpoint list and reload, so the page never shows a value the router is not using.
  /// The console's write path is config.yaml itself: there is nothing to "apply" afterwards.
  const persist = async (endpoints: EndpointCfg[], ok: string) => {
    if (!doc) return;
    try {
      const res = await api.saveConfig({ ...doc, endpoints });
      if (res.reloaded === false) {
        // Written to disk but not taken up by the running router - say so instead of "saved".
        const detail = (res as { warning?: string }).warning ?? "";
        toast.push("err", `${ok} (${t.common.notApplied})${detail ? `: ${detail}` : ""}`);
      } else {
        toast.push("ok", ok);
      }
      await cfg.reload();
    } catch (e) {
      toast.push("err", e instanceof Error ? e.message : String(e));
    }
  };

  const commit = (endpoint: EndpointCfg) => {
    const endpoints = [...(doc?.endpoints ?? [])];
    const previous = editing && editing.index >= 0 ? doc?.endpoints[editing.index]?.name : undefined;
    // A rename moves the endpoint away from the name its credential is stored under, so the save
    // carries the previous name: the key field still holds the placeholder and only the server
    // can turn that back into the real key.
    const entry: EndpointCfg =
      previous && previous !== endpoint.name ? { ...endpoint, renamed_from: previous } : endpoint;
    if (editing && editing.index >= 0) endpoints[editing.index] = entry;
    else endpoints.push(entry);
    setEditing(null);
    void persist(endpoints, `${endpoint.name || t.endpoints.newEndpoint}: ${t.common.saved}`);
  };

  const remove = (index: number) => {
    const endpoints = (doc?.endpoints ?? []).filter((_, i) => i !== index);
    const name = doc?.endpoints[index]?.name || t.endpoints.unnamed;
    void persist(endpoints, `${name}: ${t.common.deleted}`);
  };

  const liveToggle = async (name: string, enabled: boolean) => {
    try {
      await api.setEndpointState(name, enabled);
      toast.push("ok", enabled ? `${name}: ${t.common.enabled}` : `${name}: ${t.common.disabled}`);
      cfg.reload();
    } catch (e) {
      toast.push("err", e instanceof Error ? e.message : String(e));
    }
  };

  const duplicate = async (name: string) => {
    try {
      await api.duplicateEndpoint(name);
      toast.push("ok", `${name} → ${t.endpoints.duplicate}`);
      cfg.reload();
    } catch (e) {
      toast.push("err", e instanceof Error ? e.message : String(e));
    }
  };

  if (cfg.loading || !doc) {
    return <Plate className="h-40 animate-pulse" />;
  }

  return (
    <div className="space-y-4 pb-2">
      <PageHeader
        title={t.endpoints.title}
        hint={t.endpoints.hint}
        meta={<Badge tone="plain">{t.endpoints.count.replace("{n}", String(doc.endpoints.length))}</Badge>}
        actions={
          <>
            <Button size="sm" onClick={() => add("plans")}>
              + {t.endpoints.plans}
            </Button>
            <Button size="sm" onClick={() => add("fallback")}>
              + {t.endpoints.fallback}
            </Button>
          </>
        }
      />

      <SideList
        kind="plans"
        title={t.endpoints.plans}
        hint={t.endpoints.plansHint}
        rows={plans}
        onEdit={(index, e) => setEditing({ index, endpoint: e })}
        onRemove={remove}
        onLiveToggle={liveToggle}
        onDuplicate={duplicate}
        liveByName={liveByName}
      />
      <SideList
        kind="fallback"
        title={t.endpoints.fallback}
        hint={t.endpoints.fallbackHint}
        rows={cash}
        onEdit={(index, e) => setEditing({ index, endpoint: e })}
        onRemove={remove}
        onLiveToggle={liveToggle}
        onDuplicate={duplicate}
        liveByName={liveByName}
      />

      <EndpointSheet
        open={editing !== null}
        endpoint={editing?.endpoint ?? null}
        library={library}
        onClose={() => setEditing(null)}
        onCommit={commit}
        onTest={async (name) => {
          try {
            return await api.testEndpoint(name);
          } catch (e) {
            toast.push("err", e instanceof Error ? e.message : String(e));
            return null;
          }
        }}
      />
    </div>
  );
}

function SideList({
  kind,
  title,
  hint,
  rows,
  onEdit,
  onRemove,
  onLiveToggle,
  onDuplicate,
  liveByName,
}: {
  kind: "plans" | "fallback";
  title: string;
  hint: string;
  rows: { e: EndpointCfg; i: number }[];
  onEdit: (index: number, e: EndpointCfg) => void;
  onRemove: (index: number) => void;
  onLiveToggle: (name: string, enabled: boolean) => void;
  onDuplicate: (name: string) => void;
  liveByName: Map<string, { requests: number; saved: number; cost: number; available: boolean }>;
}) {
  const { t } = useI18n();
  const tone = kind === "plans" ? "var(--plans)" : "var(--cash)";
  return (
    <PlateBlock title={title} hint={hint}>
      {rows.length === 0 ? (
        <div className="px-4 py-6 text-center text-sm text-[var(--ink-faint)]">{t.common.empty}</div>
      ) : (
        <div className="divide-y divide-[var(--line)]">
          {rows.map(({ e, i }) => {
            const live = liveByName.get(e.name);
            const enabled = e.enabled !== false;
            return (
              <div
                key={`${e.name}-${i}`}
                className={cn("flex flex-col gap-2 px-3 py-3 transition-colors sm:flex-row sm:items-center sm:gap-3 sm:px-4", !enabled && "opacity-55")}
              >
                <span className="h-[3px] w-full shrink-0 rounded-full sm:hidden" style={{ background: tone }} />
                <div className="flex min-w-0 flex-1 items-center gap-2.5 sm:gap-3">
                  <span className="mono w-7 shrink-0 tabular text-2xs text-[var(--ink-faint)]">{e.order}</span>
                  <span className="hidden h-10 w-[2px] shrink-0 rounded-full sm:block" style={{ background: tone }} />
                  <button type="button" onClick={() => onEdit(i, e)} className="min-w-0 flex-1 text-left">
                    <span className="flex flex-wrap items-center gap-2">
                      <span className="mono text-sm">{e.name || <em className="text-[var(--ink-faint)]">auto</em>}</span>
                      {enabled ? (
                        <Badge tone="on">{t.common.enabled}</Badge>
                      ) : (
                        <Badge tone="plain">{t.common.disabled}</Badge>
                      )}
                      {e.inject_session ? <Badge tone="signal">session</Badge> : null}
                      {live && !live.available ? <Badge tone="danger">cooldown</Badge> : null}
                    </span>
                    <span className="mono mt-1 block truncate text-2xs text-[var(--ink-faint)]">
                      [{e.kind}] weight={e.weight} · {e.provider} · {e.model || "—"}
                    </span>
                    <span className="mono block truncate text-2xs text-[var(--ink-faint)]">{e.url || "—"}</span>
                  </button>
                </div>
                <div className="flex shrink-0 items-center gap-1.5 overflow-x-auto">
                  {live ? (
                    <span className="mono hidden text-2xs text-[var(--ink-faint)] md:inline">
                      {live.requests} req · {kind === "plans" ? fmtUsd(live.saved) : fmtUsd(live.cost)}
                    </span>
                  ) : null}
                  <Button size="sm" variant={enabled ? "ghost" : "outline"} onClick={() => onEdit(i, e)}>
                    {t.common.edit}
                  </Button>
                  {e.name ? (
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => onLiveToggle(e.name, !enabled)}
                      title={enabled ? t.common.disabledHint : t.common.enabledHint}
                    >
                      {enabled ? t.common.disable : t.common.enable}
                    </Button>
                  ) : null}
                  {e.name ? (
                    <Button size="sm" variant="ghost" onClick={() => onDuplicate(e.name)}>
                      {t.common.copy}
                    </Button>
                  ) : null}
                  <ConfirmButton
                    onConfirm={() => onRemove(i)}
                    confirmLabel={t.common.delete}
                    title={e.name ? t.endpoints.deleteConfirmTitle.replace("{name}", e.name) : t.endpoints.deleteConfirmTitleNew}
                  >
                    {t.common.delete}
                  </ConfirmButton>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </PlateBlock>
  );
}


