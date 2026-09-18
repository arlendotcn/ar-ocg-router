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
const listeners = new Set<() => void>();

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
    current = await api.stats();
    currentError = null;
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
function ensureTimer() {
  if (timer) return;
  timer = setInterval(() => {
    if (!paused) void tick();
  }, 2000);
  void tick(true);
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", () => {
      // Coming back into view always refetches, so the page never shows a stale snapshot that
      // looks like live data.
      if (!document.hidden) void tick(true);
    });
  }
}

export function useStatsStream(opts?: { enabled?: boolean }): Snapshot {
  const enabled = opts?.enabled ?? true;
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
