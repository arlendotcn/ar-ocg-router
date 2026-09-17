"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { api } from "@/lib/api";
import { useToast } from "@/components/ui/toast";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmButton } from "@/components/ui/confirm-button";
import { Plate, PlateBlock } from "@/components/ui/plate";
import { PageHeader } from "@/components/page-header";
import { ChipInput, Field, Input, Switch, Textarea } from "@/components/ui/field";
import { Sheet } from "@/components/ui/sheet";
import { fmtNum } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useStatsStream } from "@/lib/use-stats";
import type { ModelEntry, ModelLibrary } from "@/types/api";

const BLANK: ModelEntry = {
  id: "",
  label: "",
  aliases: [],
  context_tokens: 0,
  max_output_tokens: 0,
  reasoning_levels: [],
  input_modalities: ["text"],
  output_modalities: ["text"],
  notes: "",
  tags: [],
};

export function ModelsPage() {
  const { t } = useI18n();
  const toast = useToast();
  const { stats } = useStatsStream();
  const [lib, setLib] = React.useState<ModelLibrary | null>(null);
  const [draft, setDraft] = React.useState<ModelLibrary | null>(null);
  const [editing, setEditing] = React.useState<{ index: number; entry: ModelEntry } | null>(null);
  const [saving, setSaving] = React.useState(false);
  const [tagFilter, setTagFilter] = React.useState<string | null>(null);
  const [query, setQuery] = React.useState("");

  const load = React.useCallback(async () => {
    try {
      const res = await api.library();
      setLib(res);
      setDraft(res);
    } catch (e) {
      toast.push("err", e instanceof Error ? e.message : String(e));
    }
  }, [toast]);

  React.useEffect(() => {
    void load();
  }, [load]);

  // Which endpoints use which library entry (by canonical id or alias).
  const usage = React.useMemo(() => {
    const m = new Map<string, string[]>();
    for (const acc of stats?.accounts ?? []) {
      const model = (acc.model ?? "").toLowerCase();
      if (!model) continue;
      const hit = (draft?.models ?? []).find(
        (e) => e.id.toLowerCase() === model || e.aliases.some((a) => a.toLowerCase() === model),
      );
      const key = (hit?.id ?? model).toLowerCase();
      m.set(key, [...(m.get(key) ?? []), acc.name]);
    }
    return m;
  }, [stats, draft]);

  const tags = React.useMemo(() => {
    const s = new Set<string>();
    for (const e of draft?.models ?? []) for (const x of e.tags) s.add(x);
    return [...s].sort();
  }, [draft]);

  const shown = React.useMemo(() => {
    const q = query.trim().toLowerCase();
    return (draft?.models ?? []).filter((e) => {
      if (tagFilter && !e.tags.includes(tagFilter)) return false;
      if (!q) return true;
      return (
        e.id.toLowerCase().includes(q) ||
        e.label.toLowerCase().includes(q) ||
        e.aliases.some((a) => a.toLowerCase().includes(q)) ||
        e.notes.toLowerCase().includes(q)
      );
    });
  }, [draft, tagFilter, query]);

  const dirty = Boolean(lib && draft && JSON.stringify(lib) !== JSON.stringify(draft));

  const commit = (entry: ModelEntry) => {
    setDraft((d) => {
      if (!d) return d;
      const models = [...d.models];
      if (editing && editing.index >= 0) models[editing.index] = entry;
      else models.push(entry);
      return { ...d, models };
    });
    setEditing(null);
  };

  const save = async () => {
    if (!draft) return;
    setSaving(true);
    try {
      const res = await api.saveLibrary(draft);
      setLib(res);
      setDraft(res);
      toast.push("ok", t.library.saved);
    } catch (e) {
      toast.push("err", e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  if (!draft) return <Plate className="h-40 animate-pulse" />;

  return (
    <div className="space-y-4">
      <PageHeader
        title={t.library.title}
        hint={t.library.hint}
        meta=<Badge tone="plain">{t.library.count.replace("{n}", String(draft.models.length))}</Badge>
        actions={
          <>
            <Button size="sm" onClick={() => setEditing({ index: -1, entry: { ...BLANK } })}>
              + {t.library.add}
            </Button>
          </>
        }
      />

      {dirty ? (
        <div className="plate sticky top-[84px] z-20 flex items-center justify-between gap-3 px-3 py-2">
          <Badge tone="signal" dot>
            {t.common.unsaved}
          </Badge>
          <div className="flex items-center gap-2">
            <Button size="sm" variant="ghost" onClick={() => setDraft(lib)}>
              {t.common.cancel}
            </Button>
            <Button size="sm" variant="primary" loading={saving} onClick={save}>
              {t.common.save}
            </Button>
          </div>
        </div>
      ) : null}

      {/* items-start, not items-center: the tag chips wrap to several rows and centring the
          input against them misaligned the two top edges. */}
      <div className="flex flex-col gap-2 sm:flex-row sm:items-start">
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t.picker.search}
          aria-label={t.picker.search}
          className="h-8 shrink-0 sm:w-[260px]"
        />
        <div className="flex flex-wrap gap-1.5">
          <button
            type="button"
            onClick={() => setTagFilter(null)}
            className={cn(
              "mono h-8 rounded-[2px] border px-2 text-2xs uppercase leading-none tracking-wider transition-colors",
              tagFilter === null
                ? "border-[var(--signal)] bg-[var(--signal)]/15 text-[var(--signal)]"
                : "border-[var(--line-strong)] text-[var(--ink-faint)] hover:text-[var(--ink)]",
            )}
          >
            {t.library.all}
          </button>
          {tags.map((tag) => (
            <button
              key={tag}
              type="button"
              onClick={() => setTagFilter(tag === tagFilter ? null : tag)}
              className={cn(
                "mono h-8 rounded-[2px] border px-2 text-2xs uppercase leading-none tracking-wider transition-colors",
                tagFilter === tag
                  ? "border-[var(--signal)] bg-[var(--signal)]/15 text-[var(--signal)]"
                  : "border-[var(--line-strong)] text-[var(--ink-faint)] hover:text-[var(--ink)]",
              )}
            >
              {tag}
            </button>
          ))}
        </div>
      </div>

      <PlateBlock title={t.library.title} hint={t.library.hint}>
        {shown.length === 0 ? (
          <div className="px-4 py-6 text-center text-sm text-[var(--ink-faint)]">{t.common.empty}</div>
        ) : (
          <div className="divide-y divide-[var(--line)]">
            {shown.map((e) => {
              const index = draft.models.findIndex((x) => x.id === e.id);
              const used = usage.get(e.id.toLowerCase()) ?? [];
              return (
                <div key={e.id} className="flex flex-col gap-2 px-3 py-3 sm:px-4">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="mono text-sm">{e.id}</span>
                    {e.label && e.label !== e.id ? <Badge tone="plain">{e.label}</Badge> : null}
                    {e.tags.map((tag) => (
                      <Badge key={tag} tone="cash">
                        {tag}
                      </Badge>
                    ))}
                  </div>
                  <div className="mono flex flex-wrap gap-x-3 gap-y-1 text-2xs text-[var(--ink-faint)]">
                    {e.context_tokens > 0 ? (
                      <span>
                        {t.library.context} {fmtNum(e.context_tokens)}
                      </span>
                    ) : null}
                    {e.max_output_tokens > 0 ? (
                      <span>
                        {t.library.maxOutput} {fmtNum(e.max_output_tokens)}
                      </span>
                    ) : null}
                    {e.reasoning_levels.length ? <span>{e.reasoning_levels.join(" / ")}</span> : null}
                    {e.input_modalities.length ? <span>in: {e.input_modalities.join("+")}</span> : null}
                    {e.output_modalities.length ? <span>out: {e.output_modalities.join("+")}</span> : null}
                  </div>
                  {e.aliases.length ? (
                    <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                      <span className="label shrink-0">{t.library.aliases}</span>
                      {e.aliases.map((a) => (
                        <span
                          key={a}
                          className="mono rounded-[1px] border border-[var(--line)] bg-[var(--panel-2)] px-1.5 py-[1px] text-2xs text-[var(--ink-dim)]"
                        >
                          {a}
                        </span>
                      ))}
                    </div>
                  ) : null}
                  {e.notes ? <p className="text-2xs leading-relaxed text-[var(--ink-dim)]">{e.notes}</p> : null}
                  <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <span className="label shrink-0">{t.library.usedBy}</span>
                    {used.length ? (
                      used.map((n) => (
                        <Badge key={n} tone="plans">
                          {n}
                        </Badge>
                      ))
                    ) : (
                      <span className="text-2xs text-[var(--ink-faint)]">{t.library.unused}</span>
                    )}
                  </div>
                  <div className="flex flex-wrap items-center gap-1.5">
                    <Button size="sm" variant="ghost" onClick={() => setEditing({ index, entry: e })}>
                      {t.common.edit}
                    </Button>
                    <ConfirmButton
                      onConfirm={() =>
                        setDraft((d) =>
                          d ? { ...d, models: d.models.filter((x) => x.id !== e.id) } : d,
                        )
                      }
                      confirmLabel={t.common.delete}
                      title={t.library.deleteConfirm.replace("{name}", e.id)}
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

      <ModelSheet
        open={editing !== null}
        entry={editing?.entry ?? null}
        existingIds={draft.models.map((m) => m.id.toLowerCase())}
        editingIndex={editing?.index ?? -1}
        onClose={() => setEditing(null)}
        onCommit={commit}
      />
    </div>
  );
}

function ModelSheet({
  open,
  entry,
  existingIds,
  editingIndex,
  onClose,
  onCommit,
}: {
  open: boolean;
  entry: ModelEntry | null;
  existingIds: string[];
  editingIndex: number;
  onClose: () => void;
  onCommit: (e: ModelEntry) => void;
}) {
  const { t, lang } = useI18n();
  const [draft, setDraft] = React.useState<ModelEntry | null>(entry);

  React.useEffect(() => setDraft(entry), [entry]);
  if (!open || !draft) return null;

  const set = <K extends keyof ModelEntry>(k: K, v: ModelEntry[K]) =>
    setDraft((d) => (d ? { ...d, [k]: v } : d));

  const idTaken =
    draft.id.trim().length > 0 &&
    existingIds.some((x, i) => x === draft.id.trim().toLowerCase() && i !== editingIndex);

  return (
    <Sheet
      open={open}
      onClose={onClose}
      title={editingIndex >= 0 ? t.library.editModel : t.library.newModel}
      subtitle={draft.id || undefined}
      wide
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {t.common.cancel}
          </Button>
          <Button
            variant="primary"
            disabled={!draft.id.trim() || idTaken}
            onClick={() => onCommit({ ...draft, id: draft.id.trim() })}
          >
            {t.common.confirm}
          </Button>
        </>
      }
    >
      <div className="space-y-3">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field
            label={t.library.id}
            help={t.library.idHint}
            required
            error={idTaken ? t.common.dupId : null}
          >
            <Input value={draft.id} onChange={(e) => set("id", e.target.value)} placeholder="deepseek-flash" spellCheck={false} />
          </Field>
          <Field label={t.library.label} help={t.library.labelHint}>
            <Input value={draft.label} onChange={(e) => set("label", e.target.value)} placeholder="DeepSeek V4.1 Flash" />
          </Field>
        </div>

        <Field label={t.library.aliases} help={t.library.aliasesHint}>
          <ChipInput values={draft.aliases} onChange={(v) => set("aliases", v)} placeholder="deepseek-v4.1-flash" />
        </Field>

        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={t.library.context} help={t.library.contextHint}>
            <Input
              type="number"
              value={draft.context_tokens}
              onChange={(e) => set("context_tokens", Number(e.target.value) || 0)}
            />
          </Field>
          <Field label={t.library.maxOutput} help={t.library.maxOutputHint}>
            <Input
              type="number"
              value={draft.max_output_tokens}
              onChange={(e) => set("max_output_tokens", Number(e.target.value) || 0)}
            />
          </Field>
        </div>

        <Field label={t.library.reasoning} help={t.library.reasoningHint}>
          <ChipInput values={draft.reasoning_levels} onChange={(v) => set("reasoning_levels", v)} placeholder="low, medium, high" />
        </Field>

        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={t.library.inputModalities} help={t.library.inputModalitiesHint}>
            <ChipInput values={draft.input_modalities} onChange={(v) => set("input_modalities", v)} placeholder="text, image" />
          </Field>
          <Field label={t.library.outputModalities} help={t.library.outputModalitiesHint}>
            <ChipInput values={draft.output_modalities} onChange={(v) => set("output_modalities", v)} placeholder="text" />
          </Field>
        </div>

        <Field label={t.library.tags} help={t.library.tagsHint}>
          <ChipInput values={draft.tags} onChange={(v) => set("tags", v)} placeholder="deepseek, plan" />
        </Field>

        <Field label={t.library.notes} help={t.library.notesHint}>
          <Textarea rows={3} value={draft.notes} onChange={(e) => set("notes", e.target.value)} />
        </Field>

        <div className="flex items-center gap-3 rounded-[2px] border border-[var(--line)] bg-[var(--panel-2)] px-3 py-2 text-2xs text-[var(--ink-faint)]">
          <Switch checked onChange={() => {}} disabled label="preview" />
          <span>{t.library.zeroHint}</span>
        </div>
      </div>
    </Sheet>
  );
}
