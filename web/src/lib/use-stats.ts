"use client";

import * as React from "react";
import { api } from "./api";
import type { Stats } from "@/types/api";

type Snapshot = {
  stats: Stats | null;
  error: string | null;
  loading: boolean;
  updatedAt: number;
  /** Force a fetch now. Never suppressed, not even by a hidden tab. */
  refresh: () => void;
  paused: boolean;
  setPaused: (v: boolean) => void;
  /** True while polling is suspended because the tab is in the background. */
  background: boolean;
};

/**
 * One shared poller for /router/stats. Every widget subscribes to the same
 * module-level store, so a page issues exactly one request per tick no matter how
 * many readouts are on screen.
 */
let current: Stats | null = null;
let currentError: string | null = null;
let updatedAt = 0;
let paused = false;
let inFlight = false;
/// Set when a fetch is requested while one is already running: the request is honoured instead of
/// being dropped, otherwise a manual refresh (or the one after a config change) can vanish.
let rerun = false;
let background = false;
/// Validator of the snapshot we hold, echoed back so the server can answer 304.
let etag: string | null = null;
const listeners = new Set<() => void>();

/**
 * How often the shared poller runs.
 *
 * This single poller feeds every widget on the page, but the widgets do not need the same freshness:
 * the dashboard is watched live, while the top bar only wants "is it peak yet?". So each widget
 * declares what it needs and the poller runs at the fastest request currently mounted - it can only
 * ever get faster than the default, never slower, so a page cannot starve a faster one.
 */
const DEFAULT_INTERVAL_MS = 5000;
const wanted = new Set<number>();
let intervalMs = DEFAULT_INTERVAL_MS;

function recomputeInterval() {
  let next = DEFAULT_INTERVAL_MS;
  for (const v of wanted) next = Math.min(next, v);
  if (next === intervalMs) return;
  intervalMs = next;
  reschedule();
}

function emit() {
  listeners.forEach((l) => l());
}

const hidden = () => typeof document !== "undefined" && document.hidden;

/// `force` skips the background-tab suppression: a poll may be skipped while nobody is looking,
/// but a fetch the user asked for must never be.
async function tick(force = false): Promise<void> {
  if (inFlight) {
    if (force) rerun = true;
    return;
  }
  if (!force && hidden()) {
    if (!background) {
      background = true;
      emit();
    }
    return;
  }
  inFlight = true;
  try {
    const res = await api.stats(etag);
    if (res.changed) {
      current = res.stats;
      etag = res.etag;
    }
    currentError = null;
    // Even a 304 counts as "the page just talked to the router": the countdown on the dashboard
    // measures drift from this timestamp, so freezing it would make the clock walk away.
    updatedAt = Date.now();
    if (background) {
      background = false;
    }
  } catch (e) {
    currentError = e instanceof Error ? e.message : String(e);
  } finally {
    inFlight = false;
    emit();
    if (rerun) {
      rerun = false;
      void tick(true);
    }
  }
}

let timer: ReturnType<typeof setInterval> | null = null;

/// Restart the timer at the current interval, keeping the accumulated phase out of it: a changed
/// interval means "from now on", and one stray early tick is not worth the bookkeeping.
function reschedule() {
  if (!timer) return;
  clearInterval(timer);
  timer = setInterval(() => {
    if (!paused) void tick();
  }, intervalMs);
}

function ensureTimer() {
  if (timer) return;
  timer = setInterval(() => {
    if (!paused) void tick();
  }, intervalMs);
  void tick(true);
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", () => {
      // Coming back into view always refetches, so the page never shows a stale snapshot that
      // looks like live data.
      if (!document.hidden) void tick(true);
    });
  }
}

/**
 * Register one subscriber's freshness requirement. Returns the release function for that
 * subscriber's effect cleanup.
 */
export function requestStatsInterval(ms: number): () => void {
  wanted.add(ms);
  recomputeInterval();
  return () => {
    wanted.delete(ms);
    recomputeInterval();
  };
}

export function useStatsStream(opts?: { enabled?: boolean; intervalMs?: number }): Snapshot {
  const enabled = opts?.enabled ?? true;
  const interval = opts?.intervalMs;
  const [, force] = React.useReducer((n) => n + 1, 0);
  const [pausedState, setPausedState] = React.useState(paused);

  React.useEffect(() => {
    if (!enabled) return;
    const cb = () => force();
    listeners.add(cb);
    ensureTimer();
    return () => {
      listeners.delete(cb);
    };
  }, [enabled]);

  // A page states how fresh it needs this to be; the fastest one currently mounted wins.
  React.useEffect(() => {
    if (!enabled) return;
    return requestStatsInterval(interval ?? DEFAULT_INTERVAL_MS);
  }, [enabled, interval]);

  const setPausedFn = React.useCallback((v: boolean) => {
    paused = v;
    setPausedState(v);
    if (!v) void tick();
  }, []);

  const refresh = React.useCallback(() => {
    void tick(true);
  }, []);

  return {
    stats: enabled ? current : null,
    error: currentError,
    loading: enabled && current === null,
    updatedAt,
    refresh,
    paused: pausedState,
    setPaused: setPausedFn,
    background,
  };
}
