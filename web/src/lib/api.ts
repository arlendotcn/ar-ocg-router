"use client";

import type { ConfigDoc, EndpointCfg, EndpointModels, ModelLibrary, PlanPreview, Stats, TestResult } from "@/types/api";

/** The console is served by the router itself, so every call is same-origin and relative. */
const BASE = "";

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

type RawResult = { status: number; json: unknown; text: string; etag: string | null };

/** One fetch, with the response headers the conditional-GET path needs. */
async function callRaw(path: string, etag?: string | null): Promise<RawResult> {
  const res = await fetch(BASE + path, {
    method: "GET",
    headers: etag ? { "If-None-Match": etag } : undefined,
    // The router answers this one conditionally; letting the browser cache it too would disable
    // revalidation and hand back a stale body instead.
    cache: "no-store",
  });
  if (res.status !== 304 && !res.ok) {
    throw new ApiError(res.status, res.statusText || `HTTP ${res.status}`);
  }
  const text = await res.text();
  let parsed: unknown = null;
  try {
    parsed = text ? JSON.parse(text) : null;
  } catch {
    parsed = null;
  }
  return { status: res.status, json: parsed, text, etag: res.headers.get("ETag") };
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(BASE + path, {
    method,
    headers: body === undefined ? undefined : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
    cache: "no-store",
  });
  const text = await res.text();
  let parsed: unknown = null;
  try {
    parsed = text ? JSON.parse(text) : null;
  } catch {
    parsed = null;
  }
  if (!res.ok) {
    const msg =
      (parsed as { error?: { message?: string }; message?: string } | null)?.error?.message ??
      (parsed as { message?: string } | null)?.message ??
      text ??
      res.statusText;
    throw new ApiError(res.status, String(msg).slice(0, 500));
  }
  return parsed as T;
}

export type StatsResult =
  | { changed: true; stats: Stats; etag: string | null }
  | { changed: false };

export const api = {
  /**
   * Conditional GET: send the validator of the snapshot we already hold and the router answers 304
   * when nothing moved, which is the common case for a 2-second poll. "changed: false" means the
   * caller should keep showing what it has.
   */
  stats: async (etag?: string | null): Promise<StatsResult> => {
    const res = await callRaw("/router/stats", etag);
    if (res.status === 304) return { changed: false };
    return { changed: true, stats: (res.json ?? {}) as Stats, etag: res.etag };
  },
  config: () => call<ConfigDoc>("GET", "/api/config"),
  rawConfig: () => call<{ path: string; text: string }>("GET", "/api/config/raw"),

  saveConfig: (doc: ConfigDoc) =>
    call<{ saved: boolean; reloaded: boolean; summary: string; warnings: string[] } & Record<string, unknown>>(
      "PUT",
      "/api/config",
      doc,
    ),
  rawSave: (text: string) => call<{ saved: boolean; reloaded: boolean; summary: string }>("PUT", "/api/config/raw", { text }),

  serialized: () => call<{ path: string; text: string }>("GET", "/api/config/export"),

  endpointModels: (name: string) =>
    call<EndpointModels>("GET", `/api/endpoints/${encodeURIComponent(name)}/models`),

  library: () => call<ModelLibrary>("GET", "/api/library"),
  saveLibrary: (lib: ModelLibrary) => call<ModelLibrary>("PUT", "/api/library", lib),

  planPreview: (endpoint: "chat" | "responses" | "anthropic", forced?: string) =>
    call<PlanPreview>(
      "POST",
      "/api/plan/preview",
      forced ? { endpoint, forced } : { endpoint },
    ),

  testEndpoint: (name: string) => call<TestResult>("POST", `/api/test/${encodeURIComponent(name)}`, {}),
  /** Probe the endpoint as the editor currently has it, without saving. */
  testDraft: (endpoint: EndpointCfg) =>
    call<TestResult>("POST", "/api/test", { endpoints: [endpoint] }),

  setEndpointState: (name: string, enabled: boolean) =>
    call<{ name: string; enabled: boolean; reloaded: boolean }>(
      "POST",
      `/api/endpoints/${encodeURIComponent(name)}/state`,
      { enabled },
    ),
  duplicateEndpoint: (name: string, newName?: string) =>
    call<{ name: string; source: string; reloaded: boolean }>(
      "POST",
      `/api/endpoints/${encodeURIComponent(name)}/duplicate`,
      newName ? { new_name: newName } : {},
    ),

  /**
   * Derive a no-usage-API plan's real allowance from two console readings.
   *
   * The provider only shows percentages and the router only sees its own traffic; two readings
   * around a known amount of forwarded consumption pin the ratio between them, which yields the
   * window totals and - anchored on the plan price - the true per-token value.
   */
  calibrate: (name: string, body: Record<string, unknown>) =>
    call<CalibrationResult>("POST", `/api/endpoints/${encodeURIComponent(name)}/calibrate`, body),

  backups: () => call<{ backups: BackupEntry[] }>("GET", "/api/backups"),
  createBackup: (tag?: string) => call<BackupEntry>("POST", "/api/backups", tag ? { tag } : {}),
  restoreBackup: (name: string) => call<{ restored: string; reloaded: boolean }>("POST", `/api/backups/${encodeURIComponent(name)}/restore`, {}),
  deleteBackup: (name: string) => call<{ deleted: string }>("DELETE", `/api/backups/${encodeURIComponent(name)}`),
  importConfig: (text: string, format: "auto" | "yaml" | "json", backup: boolean) =>
    call<{ saved: boolean; reloaded: boolean; summary: string; warnings: string[] }>("POST", "/api/config/import", {
      text,
      format,
      backup,
    }),
  reload: () => call<{ status: string; summary: string }>("POST", "/router/reload", {}),
  /** Zero the statistics and the endpoint health memory. The quota ledger is kept. */
  resetStats: () =>
    call<{ reset: boolean; note: string }>("POST", "/api/stats/reset", {}),
};

export type BackupEntry = {
  name: string;
  bytes: number;
  created_at: string;
  age_secs: number;
};

/** What a calibration stage answered. The shape varies by stage, so all fields are optional. */
export type CalibrationResult = {
  stage?: "start" | "finish" | "verify" | "cancel" | "sync" | "anchors";
  recorded?: boolean;
  cancelled?: boolean;
  /** sync: the level was re-anchored on the provider's current reading. */
  synced?: boolean;
  /** sync: whether a derivation exists. Without one the level is anchored but future
   *  consumption is still priced at whatever the config says. */
  has_derivation?: boolean;
  /** finish: the factor the configured rates were multiplied by. Reported only; the corrected
   *  rates already carry its effect, so nothing stores it. */
  rate_factor?: number;
  rolling_total?: number;
  weekly_total?: number;
  saved_prices?: boolean;
  reloaded?: boolean;
  note?: string;
  /** verify: predicted movement of the monthly percentage vs what the console actually showed. */
  predicted_pp?: number;
  residual_pp?: number;
  verified?: boolean;
};
