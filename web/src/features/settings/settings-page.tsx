"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { useConfig } from "@/lib/use-config";
import { api, type BackupEntry } from "@/lib/api";
import { useToast } from "@/components/ui/toast";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Plate, PlateBlock } from "@/components/ui/plate";
import { PageHeader } from "@/components/page-header";
import { Field, Input, Select, Textarea } from "@/components/ui/field";
import { Sheet } from "@/components/ui/sheet";
import { copyText, download, fmtAgo } from "@/lib/format";
import { useStatsStream } from "@/lib/use-stats";
import type { LogCfg, ServerCfg } from "@/types/api";

export function SettingsPage() {
  const { t, lang } = useI18n();
  const toast = useToast();
  const cfg = useConfig();
  const { stats, refresh } = useStatsStream();
  const [backups, setBackups] = React.useState<BackupEntry[]>([]);
  const [rawOpen, setRawOpen] = React.useState(false);
  const [rawText, setRawText] = React.useState("");
  const [rawBusy, setRawBusy] = React.useState(false);

  const loadBackups = React.useCallback(async () => {
    try {
      const res = await api.backups();
      setBackups(res.backups ?? []);
    } catch {
      setBackups([]);
    }
  }, []);

  React.useEffect(() => {
    void loadBackups();
  }, [loadBackups]);

  const d = cfg.draft;
  if (cfg.loading || !d) return <Plate className="h-40 animate-pulse" />;
  const s = d.server;
  const setServer = <K extends keyof ServerCfg>(k: K, v: ServerCfg[K]) =>
    cfg.setDraft((doc) => ({ ...doc, server: { ...doc.server, [k]: v } }));
  const setLog = <K extends keyof LogCfg>(k: K, v: LogCfg[K]) =>
    cfg.setDraft((doc) => ({ ...doc, log: { ...doc.log, [k]: v } }));

  const endpointHint = stats
    ? `http://${stats.router.peak_windows ? s.host : s.host}:${s.port}/v1`
    : `http://127.0.0.1:${s.port}/v1`;

  return (
    <div className="space-y-4">
      <PageHeader title={t.settings.title} hint={t.settings.dataHint} actions={<Save cfg={cfg} />} />

      <PlateBlock title={t.settings.server}>
        <div className="grid gap-3 px-3 py-3 sm:grid-cols-2 sm:px-4">
          <Field label={t.settings.host} help={t.settings.hostHint}>
            <Input value={s.host} onChange={(e) => setServer("host", e.target.value)} />
          </Field>
          <Field label={t.settings.port} help={t.settings.portHint}>
            <Input type="number" value={s.port} onChange={(e) => setServer("port", Number(e.target.value) || 0)} />
          </Field>
          <Field label={t.settings.maxConns} help={t.settings.maxConnsHint}>
            <Input
              type="number"
              value={s.max_connections}
              onChange={(e) => setServer("max_connections", Number(e.target.value) || 0)}
            />
          </Field>
          <Field label={t.settings.idleTimeout} help={t.settings.idleTimeoutHint}>
            <Input
              type="number"
              value={s.idle_timeout_secs}
              onChange={(e) => setServer("idle_timeout_secs", Number(e.target.value) || 0)}
            />
          </Field>
          <Field label={t.settings.readTimeout} help={t.settings.readTimeoutHint}>
            <Input
              type="number"
              value={s.read_timeout_secs}
              onChange={(e) => setServer("read_timeout_secs", Number(e.target.value) || 0)}
            />
          </Field>
          <Field label={t.settings.maxBody} help={t.settings.maxBodyHint}>
            <Input
              type="number"
              value={s.max_body_bytes}
              onChange={(e) => setServer("max_body_bytes", Number(e.target.value) || 0)}
            />
          </Field>
        </div>
        <div className="border-t border-[var(--line)] px-3 py-3 sm:px-4">
          <Field
            label={t.settings.clientKeys}
            help={t.settings.clientKeysHint}
            hint={s.client_keys.length ? `${s.client_keys.length} key(s)` : t.common.none}
          >
            <Textarea
              rows={2}
              spellCheck={false}
              value={s.client_keys.join("\n")}
              placeholder="sk-router-secret"
              onChange={(e) =>
                setServer(
                  "client_keys",
                  e.target.value
                    .split("\n")
                    .map((l) => l.trim())
                    .filter(Boolean),
                )
              }
            />
          </Field>
        </div>
        <div className="grid gap-3 border-t border-[var(--line)] px-3 py-3 sm:grid-cols-2 sm:px-4">
          <Field label={t.settings.logLevel} help={t.settings.logLevelHint}>
            <Select value={d.log.level} onChange={(e) => setLog("level", e.target.value)}>
              {["error", "warn", "info", "debug", "trace"].map((l) => (
                <option key={l} value={l}>
                  {l}
                </option>
              ))}
            </Select>
          </Field>
          <Field label={t.settings.logFile} help={t.settings.logFileHint}>
            <Input value={d.log.file ?? ""} onChange={(e) => setLog("file", e.target.value)} placeholder="ar-ocg-router.log" />
          </Field>
        </div>
      </PlateBlock>

      <PlateBlock title={t.settings.clientSetup} hint={t.settings.clientSetupHint}>
        <div className="space-y-2 px-3 py-3 sm:px-4">
          {[
            { k: "OPENAI_BASE_URL", v: endpointHint, env: true },
            { k: "OPENAI_API_KEY", v: s.client_keys[0] ?? "anything", env: true },
            { k: "ANTHROPIC_BASE_URL", v: endpointHint.replace(/\/v1$/, ""), env: true },
            { k: "model", v: "ar-ocg-router", env: false },
          ].map((row) => (
            <div key={row.k} className="flex items-center gap-2">
              <span className="mono w-[130px] shrink-0 text-2xs uppercase tracking-wider text-[var(--ink-faint)]">
                {row.env ? "export " : ""}
                {row.k}
              </span>
              <code className="mono min-w-0 flex-1 truncate rounded-[2px] border border-[var(--line)] bg-[var(--panel-2)] px-2 py-1 text-xs">
                {row.v}
              </code>
              <Button
                size="sm"
                variant="ghost"
                onClick={async () => {
                  const ok = await copyText(row.env ? `export ${row.k}=${row.v}` : row.v);
                  toast.push(ok ? "ok" : "err", ok ? t.common.copied : t.common.error);
                }}
              >
                {t.common.copy}
              </Button>
            </div>
          ))}
        </div>
      </PlateBlock>

      <PlateBlock
        title={t.settings.dataTitle}
        hint={t.settings.dataHint}
        actions={
          <>
            <Button
              size="sm"
              variant="outline"
              onClick={async () => {
                try {
                  await api.createBackup("manual");
                  await loadBackups();
                  toast.push("ok", t.common.backupDone);
                } catch (e) {
                  toast.push("err", e instanceof Error ? e.message : String(e));
                }
              }}
            >
              {t.settings.backupNow}
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={async () => {
                try {
                  const res = await api.serialized();
                  download("config.yaml", res.text);
                } catch (e) {
                  toast.push("err", e instanceof Error ? e.message : String(e));
                }
              }}
            >
              {t.settings.exportFile}
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={async () => {
                try {
                  const res = await api.rawConfig();
                  setRawText(res.text);
                  setRawOpen(true);
                } catch (e) {
                  toast.push("err", e instanceof Error ? e.message : String(e));
                }
              }}
            >
              {t.common.edit} YAML
            </Button>
          </>
        }
      >
        <div className="divide-y divide-[var(--line)]">
          {backups.length === 0 ? (
            <div className="px-4 py-5 text-center text-sm text-[var(--ink-faint)]">{t.common.empty}</div>
          ) : (
            backups.slice(0, 12).map((b) => (
              <div key={b.name} className="flex items-center gap-3 px-3 py-2 sm:px-4">
                <span className="mono min-w-0 flex-1 truncate text-xs">{b.name}</span>
                <span className="mono hidden text-2xs text-[var(--ink-faint)] sm:inline">
                  {fmtAgo(b.created_at, lang)}
                </span>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    download(b.name, "");
                    window.open(`/api/backups/${encodeURIComponent(b.name)}`, "_blank");
                  }}
                >
                  {t.common.download}
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={async () => {
                    if (!window.confirm(t.common.restoreConfirm)) return;
                    try {
                      await api.restoreBackup(b.name);
                      await cfg.reload();
                      await loadBackups();
                      refresh();
                      toast.push("ok", t.common.restoreDone);
                    } catch (e) {
                      toast.push("err", e instanceof Error ? e.message : String(e));
                    }
                  }}
                >
                  {t.common.restore}
                </Button>
              </div>
            ))
          )}
        </div>
        <div className="border-t border-[var(--line)] px-3 py-3 sm:px-4">
          <Field label={t.settings.importFile} help={t.settings.importHint} htmlFor="config-import">
            <input
              id="config-import"
              name="config-import"
              type="file"
              accept=".yaml,.yml,.json,text/yaml,application/json"
              onChange={async (e) => {
                const file = e.target.files?.[0];
                if (!file) return;
                const text = await file.text();
                try {
                  const res = await api.importConfig(text, "auto", true);
                  await cfg.reload();
                  await loadBackups();
                  refresh();
                  toast.push("ok", `${t.settings.importDone} — ${res.summary ?? ""}`);
                } catch (err) {
                  toast.push("err", err instanceof Error ? err.message : String(err));
                } finally {
                  e.target.value = "";
                }
              }}
              className="mono w-full rounded-[2px] border border-dashed border-[var(--line-strong)] bg-[var(--panel-2)] px-2 py-2 text-xs file:mr-3 file:rounded-[2px] file:border-0 file:bg-[var(--ink)] file:px-2 file:py-1 file:text-xs file:text-[var(--bg)]"
            />
          </Field>
        </div>
      </PlateBlock>

      <Plate className="p-3 sm:p-4">
        <div className="label">{t.settings.about}</div>
        <p className="mt-2 text-sm leading-relaxed text-[var(--ink-dim)]">{t.settings.license}</p>
        <div className="mono mt-3 flex flex-wrap gap-x-4 gap-y-1 text-2xs text-[var(--ink-faint)]">
          <span>version {stats?.router.version ?? "—"}</span>
          <span>pid {stats?.router.pid ?? "—"}</span>
          <span>config {stats?.router.config_path ?? "—"}</span>
          {stats?.ui ? <Badge tone="plain">{stats.ui.managed ? "managed" : "read-only"}</Badge> : null}
        </div>
      </Plate>

      <Save cfg={cfg} />

      <Sheet
        open={rawOpen}
        onClose={() => setRawOpen(false)}
        title="config.yaml"
        subtitle={stats?.router.config_path}
        wide
        footer={
          <>
            <Button variant="ghost" onClick={() => setRawOpen(false)}>
              {t.common.cancel}
            </Button>
            <Button
              variant="primary"
              loading={rawBusy}
              onClick={async () => {
                setRawBusy(true);
                try {
                  const res = await api.rawSave(rawText);
                  await cfg.reload();
                  refresh();
                  toast.push("ok", `${t.common.saved} — ${res.summary ?? ""}`);
                  setRawOpen(false);
                } catch (e) {
                  toast.push("err", e instanceof Error ? e.message : String(e));
                } finally {
                  setRawBusy(false);
                }
              }}
            >
              {t.common.save}
            </Button>
          </>
        }
      >
        <p className="mb-2 text-xs leading-relaxed text-[var(--ink-faint)]">{t.raw.warn}</p>
        <Textarea
          value={rawText}
          onChange={(e) => setRawText(e.target.value)}
          rows={32}
          spellCheck={false}
          className="min-h-[60vh] text-xs leading-relaxed"
        />
      </Sheet>
    </div>
  );
}

function Save({ cfg, block }: { cfg: ReturnType<typeof useConfig>; block?: boolean }) {
  const { t } = useI18n();
  const toast = useToast();
  if (!cfg.dirty) return null;
  // Bottom placement of the same action. Right-aligned rather than full width so it reads as
  // the same control as the header button, not as a page-level banner.
  return (
    <div className="flex justify-end">
      <Button
        variant="primary"
        loading={cfg.saving}
      onClick={async () => {
        const ok = await cfg.save();
        toast.push(ok ? "ok" : "err", ok ? t.common.saved : (cfg.error ?? t.common.error));
      }}
    >
        {cfg.saving ? t.common.saving : t.common.save}
      </Button>
    </div>
  );
}
