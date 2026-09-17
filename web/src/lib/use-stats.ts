"use client";

import * as React from "react";
import { api } from "./api";
import type { Stats } from "@/types/api";

type Snapshot = {
  stats: Stats | null;
  error: string | null;
  loading: boolean;
  updatedAt: number;
  refresh: () => void;
  paused: boolean;
  setPaused: (v: boolean) => void;
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
const listeners = new Set<() => void>();

function emit() {
  listeners.forEach((l) => l());
}

async function tick(): Promise<void> {
  if (inFlight) return;
  if (typeof document !== "undefined" && document.hidden) return;
  inFlight = true;
  try {
    current = await api.stats();
    currentError = null;
    updatedAt = Date.now();
  } catch (e) {
    currentError = e instanceof Error ? e.message : String(e);
  } finally {
    inFlight = false;
    emit();
  }
}

let timer: ReturnType<typeof setInterval> | null = null;
function ensureTimer() {
  if (timer) return;
  timer = setInterval(() => {
    if (!paused) void tick();
  }, 2000);
  void tick();
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden) void tick();
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
    void tick();
  }, []);

  return {
    stats: enabled ? current : null,
    error: currentError,
    loading: enabled && current === null,
    updatedAt,
    refresh,
    paused: pausedState,
    setPaused: setPausedFn,
  };
}
