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
  // Per-endpoint request/usage figures, read while editing a row: 5s is fresh enough.
  const { stats } = useStatsStream({ intervalMs: 5000 });
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
  // Array position IS the consumption order: the server renumbers "order" from it on every save.
  // No sorting here, and the order value is never shown - the list is the sequence.
  const rows = (doc?.endpoints ?? []).map((e, i) => ({ e, i }));
  const plans = rows.filter(({ e }) => e.kind === "plans");
  const cash = rows.filter(({ e }) => e.kind === "fallback");

  /// Move one row inside its own section. Only the section's own slots are permuted, so a plan
  /// can never jump across the cash endpoints that sit after it in the same array.
  const reorder = (section: { e: EndpointCfg; i: number }[], from: number, to: number) => {
    if (!doc || from === to || to < 0 || to >= section.length) return;
    const moved = [...section];
    const [item] = moved.splice(from, 1);
    moved.splice(to, 0, item);
    const next = [...doc.endpoints];
    section.forEach((slot, k) => {
      next[slot.i] = moved[k].e;
    });
    void persist(next, t.endpoints.moved);
  };

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
        onReorder={(from, to) => reorder(plans, from, to)}
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
        onReorder={(from, to) => reorder(cash, from, to)}
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
  onReorder,
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
  onReorder: (from: number, to: number) => void;
  liveByName: Map<string, { requests: number; saved: number; cost: number; available: boolean }>;
}) {
  const { t } = useI18n();
  const tone = kind === "plans" ? "var(--plans)" : "var(--cash)";
  // Row indices are positions inside THIS section; the page maps them back to document slots.
  const listRef = React.useRef<HTMLDivElement | null>(null);
  const [drag, setDrag] = React.useState<{ from: number; over: number } | null>(null);

  /// Index the pointer is over, or null once it has left this section. Returning null rather than
  /// clamping keeps a drag aimed past the section boundary from teleporting the row to the end.
  const dropIndexAt = (clientY: number): number | null => {
    const host = listRef.current;
    if (!host) return null;
    const box = host.getBoundingClientRect();
    if (clientY < box.top || clientY > box.bottom) return null;
    const items = Array.from(host.querySelectorAll<HTMLElement>("[data-row]"));
    for (let k = 0; k < items.length; k++) {
      const r = items[k].getBoundingClientRect();
      if (clientY < r.top + r.height / 2) return k;
    }
    return items.length - 1;
  };

  const startDrag = (index: number) => (ev: React.PointerEvent<HTMLElement>) => {
    ev.preventDefault();
    ev.currentTarget.setPointerCapture?.(ev.pointerId);
    setDrag({ from: index, over: index });
  };
  const moveDrag = (ev: React.PointerEvent<HTMLElement>) => {
    if (!drag) return;
    const over = dropIndexAt(ev.clientY);
    if (over === null || over === drag.over) return;
    setDrag({ from: drag.from, over });
  };
  const endDrag = () => {
    if (drag && drag.from !== drag.over) onReorder(drag.from, drag.over);
    setDrag(null);
  };

  return (
    <PlateBlock title={title} hint={hint}>
      {rows.length === 0 ? (
        <div className="px-4 py-6 text-center text-sm text-[var(--ink-faint)]">{t.common.empty}</div>
      ) : (
        <div ref={listRef} className="divide-y divide-[var(--line)]">
          {rows.map(({ e, i }, k) => {
            const live = liveByName.get(e.name);
            const enabled = e.enabled !== false;
            const dragging = drag?.from === k;
            const isDropTarget = drag !== null && drag.over === k && drag.from !== k;
            return (
              <div
                key={`${e.name}-${i}`}
                data-row={k}
                className={cn(
                  "flex flex-col gap-2 px-3 py-3 transition-colors sm:flex-row sm:items-center sm:gap-3 sm:px-4",
                  !enabled && "opacity-55",
                  dragging && "bg-[var(--panel-2)] opacity-70",
                  isDropTarget && (k < (drag?.from ?? 0) ? "shadow-[inset_0_2px_0_0_var(--signal)]" : "shadow-[inset_0_-2px_0_0_var(--signal)]"),
                )}
              >
                <span className="h-[3px] w-full shrink-0 rounded-full sm:hidden" style={{ background: tone }} />
                <div className="flex min-w-0 flex-1 items-center gap-2.5 sm:gap-3">
                  <button
                    type="button"
                    aria-label={t.endpoints.dragHandle}
                    title={t.endpoints.dragHandle}
                    className="mono flex h-7 w-6 shrink-0 cursor-grab touch-none select-none items-center justify-center rounded-[2px] text-[var(--ink-faint)] transition-colors hover:bg-[var(--panel-2)] hover:text-[var(--ink-dim)] active:cursor-grabbing"
                    onPointerDown={startDrag(k)}
                    onPointerMove={moveDrag}
                    onPointerUp={endDrag}
                    onPointerCancel={endDrag}
                    onKeyDown={(ev) => {
                      if (ev.key === "ArrowUp") {
                        ev.preventDefault();
                        onReorder(k, k - 1);
                      } else if (ev.key === "ArrowDown") {
                        ev.preventDefault();
                        onReorder(k, k + 1);
                      }
                    }}
                  >
                    ⋮⋮
                  </button>
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
                      [{e.kind}] {e.provider} · {e.model || "—"}
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


