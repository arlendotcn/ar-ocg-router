"use client";

import type { ConfigDoc, EndpointModels, ModelLibrary, PlanPreview, Stats, TestResult } from "@/types/api";

/** The console is served by the router itself, so every call is same-origin and relative. */
const BASE = "";

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
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

export const api = {
  stats: () => call<Stats>("GET", "/router/stats"),
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
