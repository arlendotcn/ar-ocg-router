"use client";

import * as React from "react";
import { api } from "./api";
import type { ConfigDoc } from "@/types/api";

type ConfigState = {
  doc: ConfigDoc | null;
  draft: ConfigDoc | null;
  dirty: boolean;
  loading: boolean;
  saving: boolean;
  error: string | null;
  stale: boolean;
  setDraft: (updater: (d: ConfigDoc) => ConfigDoc) => void;
  save: () => Promise<boolean>;
  revert: () => void;
  reload: () => Promise<void>;
};

const DRAFT_KEY = "arocr.draft";

/**
 * Config editing model: the server copy is the source of truth, `draft` is what the
 * form writes to. The draft survives a reload in sessionStorage so a stray refresh
 * on a phone does not throw away a half-finished endpoint.
 */
export function useConfig(): ConfigState {
  const [doc, setDoc] = React.useState<ConfigDoc | null>(null);
  const [draft, setDraftState] = React.useState<ConfigDoc | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [saving, setSaving] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const [stale, setStale] = React.useState(false);

  const load = React.useCallback(async () => {
    try {
      const next = await api.config();
      setDoc(next);
      setDraftState((prev) => {
        if (prev) {
          try {
            const raw = sessionStorage.getItem(DRAFT_KEY);
            if (raw) return JSON.parse(raw) as ConfigDoc;
          } catch {
            /* ignore */
          }
          return prev;
        }
        return next;
      });
      setError(null);
      setStale(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    void load();
  }, [load]);

  const setDraft = React.useCallback((updater: (d: ConfigDoc) => ConfigDoc) => {
    setDraftState((prev) => {
      if (!prev) return prev;
      const next = updater(prev);
      try {
        sessionStorage.setItem(DRAFT_KEY, JSON.stringify(next));
      } catch {
        /* over quota: the draft simply stays in memory */
      }
      return next;
    });
  }, []);

  const save = React.useCallback(async () => {
    if (!draft) return false;
    setSaving(true);
    try {
      const res = await api.saveConfig(draft);
      const fresh = await api.config();
      setDoc(fresh);
      setDraftState(fresh);
      try {
        sessionStorage.removeItem(DRAFT_KEY);
      } catch {
        /* ignore */
      }
      setError(null);
      setStale(false);
      return Boolean(res?.saved ?? true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return false;
    } finally {
      setSaving(false);
    }
  }, [draft]);

  const revert = React.useCallback(() => {
    setDraftState(doc);
    try {
      sessionStorage.removeItem(DRAFT_KEY);
    } catch {
      /* ignore */
    }
  }, [doc]);

  const reload = React.useCallback(async () => {
    await load();
  }, [load]);

  const dirty = Boolean(doc && draft && JSON.stringify(doc) !== JSON.stringify(draft));

  return { doc, draft, dirty, loading, saving, error, stale, setDraft, save, revert, reload };
}
