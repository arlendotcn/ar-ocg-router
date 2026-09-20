"use client";

import * as React from "react";
import Link from "next/link";
import { useI18n } from "@/lib/i18n";
import { useStatsStream } from "@/lib/use-stats";
import { api } from "@/lib/api";
import { useToast } from "@/components/ui/toast";
import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/field";
import { Button } from "@/components/ui/button";
import { Meter } from "@/components/ui/meter";
import { Plate, PlateBlock, Readout } from "@/components/ui/plate";
import { PageHeader } from "@/components/page-header";

import { fmtBytes, fmtAgo, fmtDuration, fmtMoney, fmtNum, fmtPct, fmtWhen } from "@/lib/format";
import type { Account } from "@/types/api";
import { cn } from "@/lib/utils";

export function DashboardPage() {
  const { t, lang } = useI18n();
  const toast = useToast();
  // The dashboard is the one page someone watches live: 2s.
  const { stats, error, loading, paused, setPaused, refresh, updatedAt, background } = useStatsStream({ intervalMs: 2000 });
  const [tick, setTick] = React.useState(0);
  // Which endpoint is being toggled right now, so the button cannot be double-fired.
  const [busy, setBusy] = React.useState<string | null>(null);
  // Reset is destructive and irreversible, so it confirms in place (same idea as deleting an
  // endpoint): the first click arms it, the second one runs it. A floating panel would have to
  // hang over the plates below on a phone, and window.confirm would take the decision off screen.
  const [resetArmed, setResetArmed] = React.useState(false);
  const [resetting, setResetting] = React.useState(false);

  const setEndpointEnabled = async (name: string, enabled: boolean) => {
    setBusy(name);
    try {
      await api.setEndpointState(name, enabled);
      toast.push("ok", `${name}: ${enabled ? t.common.enabled : t.common.disabled}`);
      refresh();
    } catch (e) {
      toast.push("err", e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };

  /// Zero the counters server-side, then pull the fresh snapshot instead of waiting for the next
  /// poll, so the page cannot keep showing numbers that no longer exist.
  const resetStats = async () => {
    setResetting(true);
    try {
      await api.resetStats();
      refresh();
      toast.push("ok", t.dash.resetStatsDone);
    } catch (e) {
      toast.push("err", e instanceof Error ? e.message : String(e));
    } finally {
      setResetArmed(false);
      setResetting(false);
    }
  };

  // the countdown needs its own 1s heartbeat; the data itself arrives every 2s
  React.useEffect(() => {
    const id = setInterval(() => setTick((v) => v + 1), 1000);
    return () => clearInterval(id);
  }, []);

  const secondsLeft = React.useMemo(() => {
    if (!stats) return 0;
    const drift = paused ? 0 : Math.floor((Date.now() - updatedAt) / 1000);
    return Math.max(0, stats.router.seconds_to_change - drift);
  }, [stats, updatedAt, paused, tick]);

  if (loading && !stats) return <Skeleton t={t} />;
  if (error && !stats) {
    return (
      <Plate className="p-4">
        <p className="mono text-sm text-[var(--danger)]">{error}</p>
        <p className="mt-2 text-sm text-[var(--ink-faint)]">{t.err.offline}</p>
        <Button className="mt-3" onClick={refresh}>
          {t.common.retry}
        </Button>
      </Plate>
    );
  }
  if (!stats) return null;

  const r = stats.router;
  const c = stats.counters;
  const plans = stats.accounts.filter((a) => a.kind === "plans");
  const cash = stats.accounts.filter((a) => a.kind === "fallback");
  // The roster is the routing order, shown per side. "order" restarts at 10 inside each side, so
  // a single global sort would interleave plans and cash; and dragging is only ever allowed
  // within one side, since the two sides are separate queues.
  const byOrder = (x: Account, y: Account) => x.order - y.order;
  const roster = {
    plans: stats.accounts.filter((a) => a.kind === "plans").sort(byOrder),
    fallback: stats.accounts.filter((a) => a.kind === "fallback").sort(byOrder),
  };

  /// Move one endpoint inside its own side and save. The server renumbers "order" from the array
  /// position, so this is the only thing the console has to get right.
  const reorder = async (kind: "plans" | "fallback", from: number, to: number) => {
    const list = roster[kind];
    if (from === to || to < 0 || to >= list.length) return;
    const movedNames = list.map((a) => a.name);
    const [name] = movedNames.splice(from, 1);
    movedNames.splice(to, 0, name);
    try {
      const doc = await api.config();
      // Rebuild the endpoint array. The side being dragged keeps its own slots - only which
      // endpoint sits in each slot changes - so the other side is left exactly where it was.
      const slots = doc.endpoints.map((e, i) => ({ e, i })).filter(({ e }) => e.kind === kind);
      const byName = new Map(slots.map(({ e }) => [e.name, e]));
      const next = [...doc.endpoints];
      movedNames.forEach((n, k) => {
        const slot = slots[k];
        const moved = byName.get(n);
        if (slot && moved) next[slot.i] = moved;
      });
      const res = await api.saveConfig({ ...doc, endpoints: next });
      if (res.reloaded === false) toast.push("err", t.common.notApplied);
      else toast.push("ok", t.endpoints.moved);
      refresh();
    } catch (e) {
      toast.push("err", e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="space-y-4">
      <PageHeader
        title={t.dash.title}
        meta={
          <Badge tone={r.peak ? "signal" : "plain"} dot>
            {r.peak ? t.dash.peak : t.dash.offpeak}
          </Badge>
        }
        actions={
          <>
            {/* Reset sits next to the switch but is not another way to refresh: it destroys data.
                Armed state carries the warning colour so it cannot be mistaken for the normal button. */}
            {resetArmed ? (
              <span className="flex items-center gap-1.5">
                <span className="max-w-[13rem] text-2xs leading-snug text-[var(--warn)] sm:max-w-none">
                  {t.dash.resetStatsTitle}
                </span>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={resetting}
                  onClick={() => void resetStats()}
                  className="border-[var(--warn)] text-[var(--warn)] hover:bg-[var(--warn)]/15"
                >
                  {t.common.confirm}
                </Button>
                <Button size="sm" variant="ghost" disabled={resetting} onClick={() => setResetArmed(false)}>
                  {t.common.cancel}
                </Button>
              </span>
            ) : (
              <Button
                size="sm"
                variant="outline"
                onClick={() => setResetArmed(true)}
                title={t.dash.resetStatsHint}
                aria-label={t.dash.resetStatsHint}
              >
                {t.dash.resetStats}
              </Button>
            )}
            <label className="mono flex cursor-pointer items-center gap-2 text-2xs uppercase tracking-[0.12em] text-[var(--ink-faint)]">
              {t.dash.autoRefresh}
              {/* checked means "the page keeps refreshing itself"; the manual button below only
                  exists while that is off, so it is never a second way to do the same thing. */}
              <Switch checked={!paused} onChange={(v) => setPaused(!v)} label={t.dash.autoRefresh} />
            </label>
            {paused ? (
              <Button size="sm" variant="outline" onClick={refresh}>
                {t.common.refresh}
              </Button>
            ) : null}
          </>
        }
      />

      {/* ---- the clock plate: the single most important thing on the page ----
           Six columns so the clock can take half the width while the three supporting readouts
           share the other half exactly: 3 + 1 + 1 + 1 = 6. A 4-column grid with a col-span-2
           clock and three 1-wide cells sums to 5 and wraps the last one onto its own row. */}
      <Plate className="rise overflow-hidden">
        <div className="grid grid-cols-1 sm:grid-cols-6">
          <div className="border-b border-[var(--line)] px-3 py-3 sm:col-span-3 sm:border-b-0 sm:border-r">
            <div className="flex items-center gap-2">
              <span
                className={cn("h-[7px] w-[7px] rounded-full", r.peak && "live-dot")}
                style={{ background: r.peak ? "var(--signal)" : "var(--ink-faint)" }}
              />
              <span className="label">{t.dash.now}</span>
              {r.fake_now ? <Badge tone="warn">fake now</Badge> : null}
            </div>
            <div className="tabular mono mt-2 text-3xl leading-none sm:text-3xl">
              {r.peak ? t.dash.peak : t.dash.offpeak}
            </div>
            <div className="mt-2 text-xs text-[var(--ink-faint)]">{fmtWhen(r.now, lang)}</div>
          </div>
          <Readout
            label={t.dash.nextChange}
            value={fmtDuration(secondsLeft, lang)}
            sub={fmtWhen(r.next_change, lang)}
            className="border-b border-[var(--line)] sm:border-b-0 sm:border-r"
          />
          <Readout
            label={t.dash.windows}
            value={<span className="mono text-sm leading-tight">{r.peak_windows}</span>}
            sub={`${t.policy.mode}: ${r.mode} · ${r.idle_prefer}`}
            className="border-b border-[var(--line)] sm:border-b-0 sm:border-r"
          />
          <Readout
            label={t.app.uptime}
            value={fmtDuration(r.uptime_secs, lang)}
            sub={`v${r.version} · pid ${r.pid}`}
            className="border-[var(--line)]"
          />
        </div>
      </Plate>

      {/* ---- counters + money ---- */}
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Plate className="rise">
          <Readout
            label={t.dash.requests}
            value={fmtNum(c.requests)}
            sub={`${t.dash.proxied} ${fmtNum(c.proxied)} · ${t.dash.errors} ${fmtNum(c.errors)}`}
            help={t.dash.lifetimeHint}
          />
        </Plate>
        <Plate className="rise">
          <Readout
            label={t.dash.streams}
            value={fmtNum(c.streams)}
            sub={`${t.dash.retries} ${fmtNum(c.retries)} · ${fmtBytes(c.bytes_out)}`}
          />
        </Plate>
        {/* One card per currency, because these are not one sum: see the note on savings_json.
            A currency with no endpoints of its own simply has no card. */}
        {Object.entries(stats.savings).map(([cur, v]) => (
          <Plate className="rise" key={cur}>
            <Readout
              label={cur === "UNSPECIFIED" ? t.dash.saved : `${t.dash.saved} · ${cur}`}
              value={
                <span className="mono text-sm leading-tight">
                  <span style={{ color: "var(--up)" }}>{fmtMoney(v.saved, cur)}</span>
                  <span className="mx-1.5 text-[var(--ink-faint)]">/</span>
                  <span style={{ color: "var(--cash)" }}>{fmtMoney(v.spent, cur)}</span>
                </span>
              }
              sub={`${t.dash.saved} / ${t.dash.spent}`}
              help={t.dash.savedHint}
            />
          </Plate>
        ))}
      </div>

      {/* ---- per side summary ---- */}
      <div className="grid gap-3 sm:grid-cols-2">
        <SideSummary kind="plans" accounts={plans} used={c.plans_used} />
        <SideSummary kind="fallback" accounts={cash} used={c.cash_used} />
      </div>

      {/* ---- endpoint roster ---- */}
      <PlateBlock
        title={t.dash.accounts}
        hint={t.dash.recentHint}
        actions={
          <>
            {/* A tab in the background stops polling; without this the last snapshot would sit
                there looking live. */}
            <span className="mono text-2xs uppercase tracking-[0.12em] text-[var(--ink-faint)]">
              {paused ? t.dash.paused : background ? t.dash.background : t.dash.live}
            </span>
            <span
              className={cn("h-[6px] w-[6px] rounded-full", !paused && !background && "live-dot")}
              style={{ background: paused || background ? "var(--ink-faint)" : "var(--up)" }}
            />
          </>
        }
      >
        {stats.accounts.length === 0 ? (
          <div className="px-4 py-6 text-center text-sm text-[var(--ink-faint)]">
            {t.dash.noAccounts}{" "}
            <Link href="/endpoints" className="underline" style={{ color: "var(--signal)" }}>
              {t.nav.endpoints}
            </Link>
          </div>
        ) : (
          <div className="divide-y divide-[var(--line)]">
            {/* One list per side: rows can be dragged inside their own side and never across,
                because plans and cash are separate queues with independent order sequences. */}
            {(["plans", "fallback"] as const).map((kind) =>
              roster[kind].length === 0 ? null : (
                <RosterGroup
                  key={kind}
                  kind={kind}
                  title={kind === "plans" ? t.endpoints.plans : t.endpoints.fallback}
                  accounts={roster[kind]}
                  busy={busy}
                  onToggle={setEndpointEnabled}
                  onReorder={(from, to) => void reorder(kind, from, to)}
                />
              ),
            )}
          </div>
        )}
      </PlateBlock>

      {stats.warnings.length > 0 ? (
        <Plate className="rise border-[var(--warn)]/40">
          <div className="px-3 py-2.5 sm:px-4">
            <div className="label" style={{ color: "var(--warn)" }}>
              config warnings
            </div>
            <ul className="mt-2 space-y-1.5">
              {stats.warnings.map((w, i) => (
                <li key={i} className="text-sm leading-relaxed text-[var(--ink-dim)]">
                  · {w}
                </li>
              ))}
            </ul>
          </div>
        </Plate>
      ) : null}

      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 px-1 text-2xs text-[var(--ink-faint)]">
        <span className="mono">
          {t.app.configPath}: <span className="text-[var(--ink-dim)]">{r.config_path}</span>
        </span>
        <span className="mono">
          {t.nav.policy}: {r.surplus_max_pct}% · {r.surplus_projection ? "projection on" : "projection off"}
        </span>
      </div>
    </div>
  );
}

function SideSummary({ kind, accounts, used }: { kind: "plans" | "fallback"; accounts: Account[]; used: number }) {
  const { t } = useI18n();
  const tone = kind === "plans" ? "var(--plans)" : "var(--cash)";
  const healthy = accounts.filter((a) => a.available).length;
  const tokens = accounts.reduce((n, a) => n + a.stats.prompt_tokens + a.stats.completion_tokens, 0);
  // Summed per currency: a plan billed in yuan and one billed in dollars cannot be added, and the
  // side list can hold both.
  const costs = accounts.reduce<Record<string, number>>((m, a) => {
    const cur = a.stats.currency || "UNSPECIFIED";
    m[cur] = (m[cur] ?? 0) + (kind === "plans" ? a.stats.saved : a.stats.cost);
    return m;
  }, {});
  return (
    <Plate className="rise">
      <div className="flex items-center justify-between border-b border-[var(--line)] px-3 py-2">
        <span className="mono text-xs uppercase tracking-[0.16em]" style={{ color: tone }}>
          {kind === "plans" ? t.endpoints.plans : t.endpoints.fallback}
        </span>
        <span className="mono text-2xs text-[var(--ink-faint)]">
          {healthy}/{accounts.length} · {fmtNum(used)} req
        </span>
      </div>
      <div className="grid grid-cols-3 divide-x divide-[var(--line)]">
        <Readout label={t.dash.tokens} value={fmtNum(tokens)} sub={kind === "plans" ? t.dash.saved : t.dash.cost} />
        <Readout
          label={kind === "plans" ? t.dash.saved : t.dash.cost}
          value={
            <span className="mono text-sm leading-tight">
              {Object.entries(costs).map(([cur, v]) => (
                <span key={cur} className="mr-2 inline-block">
                  {fmtMoney(v, cur)}
                </span>
              ))}
            </span>
          }
        />
        <Readout
          label={t.common.enabled}
          value={`${accounts.filter((a) => a.enabled !== false).length}/${accounts.length}`}
          sub={`${healthy} ${t.dash.healthy}`}
        />
      </div>
    </Plate>
  );
}

/// One draggable side of the roster. The drag state lives here so a plan row can never be
/// dropped among the cash rows: this list only ever contains one kind.
function RosterGroup({
  kind,
  title,
  accounts,
  busy,
  onToggle,
  onReorder,
}: {
  kind: "plans" | "fallback";
  title: string;
  accounts: Account[];
  busy: string | null;
  onToggle: (name: string, enabled: boolean) => void;
  onReorder: (from: number, to: number) => void;
}) {
  const { t } = useI18n();
  const listRef = React.useRef<HTMLDivElement | null>(null);
  const [drag, setDrag] = React.useState<{ from: number; over: number } | null>(null);

  /// Index the pointer is over, or null when it has left this group. Returning null instead of
  /// clamping is what stops a drag aimed past the section from teleporting the row to the end.
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
    return Math.max(0, items.length - 1);
  };

  const start = (index: number) => (ev: React.PointerEvent<HTMLElement>) => {
    ev.preventDefault();
    ev.currentTarget.setPointerCapture?.(ev.pointerId);
    setDrag({ from: index, over: index });
  };
  const move = (ev: React.PointerEvent<HTMLElement>) => {
    if (!drag) return;
    const over = dropIndexAt(ev.clientY);
    if (over === null || over === drag.over) return;
    setDrag({ from: drag.from, over });
  };
  const end = () => {
    // A drag that never entered this group leaves the order alone: dragging across the section
    // boundary is not a reorder, it is a miss.
    if (drag && drag.from !== drag.over) onReorder(drag.from, drag.over);
    setDrag(null);
  };

  return (
    <div ref={listRef} data-group={kind}>
      <div className="bg-[var(--panel-2)] px-3 py-1.5 sm:px-4">
        <span className="label">{title}</span>
      </div>
      {accounts.map((a, i) => {
        const dragging = drag?.from === i;
        const isDropTarget = drag !== null && drag.over === i && drag.from !== i;
        return (
          <div
            key={a.name}
            data-row={i}
            className={cn(
              "relative",
              dragging && "opacity-70",
              isDropTarget && (i < (drag?.from ?? 0) ? "shadow-[inset_0_2px_0_0_var(--signal)]" : "shadow-[inset_0_-2px_0_0_var(--signal)]"),
            )}
          >
            <AccountRow
              a={a}
              index={i}
              busy={busy === a.name}
              onToggle={onToggle}
              dragging={dragging}
              onDragStart={start(i)}
              onDragMove={move}
              onDragEnd={end}
              onStep={(delta) => onReorder(i, i + delta)}
              dragLabel={t.endpoints.dragHandle}
            />
          </div>
        );
      })}
    </div>
  );
}
function AccountRow({
  a,
  index,
  busy,
  onToggle,
  dragging,
  onDragStart,
  onDragMove,
  onDragEnd,
  onStep,
  dragLabel,
}: {
  a: Account;
  index: number;
  busy: boolean;
  onToggle: (name: string, enabled: boolean) => void;
  dragging: boolean;
  onDragStart: (ev: React.PointerEvent<HTMLElement>) => void;
  onDragMove: (ev: React.PointerEvent<HTMLElement>) => void;
  onDragEnd: () => void;
  onStep: (delta: number) => void;
  dragLabel: string;
}) {
  const { t, lang } = useI18n();
  const [open, setOpen] = React.useState(false);
  const tone = a.kind === "plans" ? "var(--plans)" : "var(--cash)";
  const q = a.quota;
  const worst = Math.max(q.rolling.percent || 0, q.weekly.percent || 0, q.monthly.percent || 0);
  const hasQuota = q.unit !== "none" && (q.rolling.limit > 0 || q.weekly.limit > 0 || q.monthly.limit > 0);

  // A disabled endpoint is out of the pool on purpose, so it must not read as unhealthy.
  const enabled = a.enabled !== false;
  const status = !enabled
    ? { label: t.common.disabled, tone: "plain" as const }
    : !a.available
      ? { label: a.cooldown_secs_left > 0 ? `${t.dash.cooldown} ${a.cooldown_secs_left}s` : t.dash.skipped, tone: "danger" as const }
      : a.health?.skipped
        ? { label: `${t.dash.skipped} ${a.health.skip_secs_left}s`, tone: "warn" as const }
        : { label: t.dash.healthy, tone: "on" as const };

  return (
    <div className="rise" style={{ animationDelay: `${Math.min(index * 28, 240)}ms` }}>
      {/* The highlight belongs to the whole row, not to the middle of it. It used to be painted by
          the name button alone, which left a 4px notch beside the drag handle and a 12px one before
          the enable switch - the row looked like three separate hit boxes stacked end to end. The
          button still owns the click; it no longer owns the background. */}
      <div
        className={cn(
          "group/row flex items-center transition-colors",
          !enabled && "opacity-60",
          dragging ? "bg-[var(--panel-2)]" : "hover:bg-[var(--panel-2)]",
        )}
      >
        {/* Drag handle: pointer-draggable on touch and mouse, and arrow keys on the keyboard. */}
        <button
          type="button"
          aria-label={dragLabel}
          title={dragLabel}
          className="mono ml-1 flex h-7 w-6 shrink-0 cursor-grab touch-none select-none items-center justify-center rounded-[2px] text-[var(--ink-faint)] transition-colors hover:bg-[var(--panel-2)] hover:text-[var(--ink-dim)] active:cursor-grabbing"
          onPointerDown={onDragStart}
          onPointerMove={onDragMove}
          onPointerUp={onDragEnd}
          onPointerCancel={onDragEnd}
          onKeyDown={(ev) => {
            if (ev.key === "ArrowUp") {
              ev.preventDefault();
              onStep(-1);
            } else if (ev.key === "ArrowDown") {
              ev.preventDefault();
              onStep(1);
            }
          }}
        >
          ⋮⋮
        </button>
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          className="flex min-w-0 flex-1 items-center gap-3 px-1 py-2.5 text-left sm:px-2"
          aria-expanded={open}
        >
          <span className="h-8 w-[2px] shrink-0 rounded-full" style={{ background: tone }} />
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-2">
              <span className="mono truncate text-sm">{a.name}</span>
              <Badge tone={status.tone} dot className="shrink-0">
                {status.label}
              </Badge>
            </span>
            <span className="mono mt-0.5 block truncate text-2xs text-[var(--ink-faint)]">
              {a.provider} · {a.model ?? "—"} · {fmtNum(a.stats.requests)} req · {fmtAgo(a.stats.last_used, lang)}
            </span>
          </span>
          <span className="hidden w-[150px] shrink-0 sm:block">
            {hasQuota ? (
              <>
                <Meter
                  pct={worst}
                  projected={Math.max(q.rolling.projected_percent, q.weekly.projected_percent, q.monthly.projected_percent)}
                  tone={q.exhausted ? "var(--danger)" : tone}
                />
                <span className="mono mt-1 block text-right text-2xs text-[var(--ink-faint)]">
                  {fmtPct(worst)} · {q.source === "remote" ? t.dash.sourceRemote : q.source === "local" ? t.dash.sourceLocal : t.dash.sourceUnknown}
                </span>
              </>
            ) : a.balance ? (
              <span className="mono block text-right text-sm">
                <span style={{ color: "var(--cash)" }}>{a.balance.total}</span>{" "}
                <span className="text-2xs text-[var(--ink-faint)]">{a.balance.currency}</span>
              </span>
            ) : (
              <span className="mono block text-right text-2xs text-[var(--ink-faint)]">{t.common.none}</span>
            )}
          </span>
          <span className="mono hidden w-[86px] shrink-0 text-right text-sm md:block">
            {a.kind === "plans" ? (
              <span style={{ color: "var(--up)" }}>{fmtMoney(a.stats.saved, a.stats.currency)}</span>
            ) : (
              <span style={{ color: "var(--cash)" }}>{fmtMoney(a.stats.cost, a.stats.currency)}</span>
            )}
          </span>
            <span className="shrink-0 text-2xs text-[var(--ink-faint)]">{open ? "▴" : "▾"}</span>
        </button>
        <div className="shrink-0 pr-3 sm:pr-4">
          <Button
            size="sm"
            variant={enabled ? "ghost" : "outline"}
            loading={busy}
            onClick={() => onToggle(a.name, !enabled)}
            title={enabled ? t.common.disabledHint : t.common.enabledHint}
          >
            {enabled ? t.common.disable : t.common.enable}
          </Button>
        </div>
      </div>

      {open ? <AccountDetail a={a} /> : null}
    </div>
  );
}

function AccountDetail({ a }: { a: Account }) {
  const { t, lang } = useI18n();
  const q = a.quota;
  const windows = [
    { key: "rolling", label: t.dash.rolling, v: q.rolling, limit: q.rolling.limit, surplus: q.surplus },
    { key: "weekly", label: t.dash.weekly, v: q.weekly, limit: q.weekly.limit, surplus: q.surplus },
    { key: "monthly", label: t.dash.monthly, v: q.monthly, limit: q.monthly.limit, surplus: q.surplus },
  ];
  const unit = q.unit === "usd" ? "$" : q.unit === "tokens" ? "" : "";

  return (
    <div className="rise border-t border-[var(--line)] bg-[var(--panel-2)] px-3 py-3 sm:px-4">
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="space-y-2.5">
          {windows.map((w) => (
            <div key={w.key}>
              <div className="mb-1 flex items-baseline justify-between">
                <span className="label">{w.label}</span>
                <span className="mono tabular text-xs">
                  {w.limit > 0 ? (
                    <>
                      <span style={{ color: w.v.percent >= 100 ? "var(--danger)" : "var(--ink)" }}>{fmtPct(w.v.percent, 0)}</span>
                      <span className="text-[var(--ink-faint)]">
                        {"  "}
                        {unit}
                        {fmtNum(w.v.used, unit ? 2 : 0)}/{unit}
                        {fmtNum(w.limit, unit ? 0 : 0)}
                        {w.v.projected_percent > w.v.percent + 1 ? ` · ${t.dash.projected} ${fmtPct(w.v.projected_percent, 0)}` : ""}
                      </span>
                    </>
                  ) : (
                    <span className="text-[var(--ink-faint)]">— / {t.common.none}</span>
                  )}
                </span>
              </div>
              <Meter
                pct={w.v.percent}
                projected={w.v.projected_percent}
                tone={a.kind === "plans" ? "var(--plans)" : "var(--cash)"}
              />
              {w.v.resets_at ? (
                <div className="mono mt-1 text-2xs text-[var(--ink-faint)]">
                  {t.dash.resetsAt} {fmtWhen(w.v.resets_at, lang)}
                </div>
              ) : null}
            </div>
          ))}
        </div>

        <dl className="mono grid grid-cols-2 gap-x-3 gap-y-1.5 text-xs sm:content-start">
          <Row k={t.dash.identity} v={a.identity ?? `${a.provider}/${a.model}`} />
          <Row k="URL" v={a.url} mono />
          <Row k={t.dash.key} v={a.key} mono />
          <Row
            k={t.dash.dataSource}
            v={q.source === "remote" ? t.dash.sourceRemote : q.source === "local" ? t.dash.sourceLocal : t.dash.sourceUnknown}
          />
          <Row k={t.dash.todayTokens} v={`${fmtNum(a.stats.prompt_tokens)} / ${fmtNum(a.stats.completion_tokens)}`} />
          {/* Cache hits as a share of the prompt: the raw count alone cannot tell 3k hits out of
              300k input from 3k out of 4k. */}
          <Row
            k={t.dash.cached}
            v={
              a.stats.prompt_tokens > 0
                ? `${fmtNum(a.stats.cached_tokens)} (${fmtPct((a.stats.cached_tokens / a.stats.prompt_tokens) * 100)})`
                : fmtNum(a.stats.cached_tokens)
            }
          />
          <Row k={t.dash.streams} v={fmtNum(a.stats.stream_requests)} />
          <Row k="latency" v={`${fmtNum(a.stats.avg_latency_ms)} ms`} />
          <Row k={t.dash.lastUsed} v={fmtAgo(a.stats.last_used, lang)} />
          {a.balance ? (
            <Row k={t.dash.balance} v={`${a.balance.total} ${a.balance.currency}`} />
          ) : null}
          {a.cooldown_reason ? <Row k="cooldown" v={a.cooldown_reason} tone="var(--danger)" /> : null}
          {a.stats.last_error ? <Row k={t.dash.lastError} v={a.stats.last_error} tone="var(--danger)" /> : null}
          {a.health?.last_reason ? <Row k="health" v={a.health.last_reason} tone="var(--warn)" /> : null}
        </dl>
      </div>
    </div>
  );
}

function Row({ k, v, mono, tone }: { k: string; v: React.ReactNode; mono?: boolean; tone?: string }) {
  return (
    <>
      <dt className="truncate text-[var(--ink-faint)]">{k}</dt>
      <dd className={cn("truncate text-right", mono && "mono")} style={tone ? { color: tone } : undefined} title={typeof v === "string" ? v : undefined}>
        {v}
      </dd>
    </>
  );
}

function Skeleton({ t }: { t: ReturnType<typeof useI18n>["t"] }) {
  return (
    <div className="space-y-3">
      <div className="h-6 w-40 animate-pulse rounded-[2px] bg-[var(--panel-2)]" />
      <Plate className="h-[104px] animate-pulse" />
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        {[0, 1, 2, 3].map((i) => (
          <Plate key={i} className="h-[76px] animate-pulse" />
        ))}
      </div>
      <span className="sr-only">{t.common.loading}</span>
    </div>
  );
}
