"use client";

import * as React from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { dictionaries, I18nContext, type I18nCtx, type Lang } from "@/lib/i18n";
import { ToastProvider } from "@/components/ui/toast";
import { cn } from "@/lib/utils";
import { useStatsStream } from "@/lib/use-stats";

type Theme = "light" | "dark";
const STORE_KEY = "arocr.ui";

type Stored = { theme: Theme; lang: Lang };

function loadStored(): Stored {
  if (typeof window === "undefined") return { theme: "light", lang: "zh" };
  try {
    const raw = window.localStorage.getItem(STORE_KEY);
    if (raw) {
      const v = JSON.parse(raw) as Partial<Stored>;
      return {
        // Only two themes exist. A stored "system" (from the earlier three-state toggle) or
        // anything unrecognised is resolved once, against the OS preference, and then kept -
        // so the toggle always means "switch to the other one".
        theme: v.theme === "light" || v.theme === "dark" ? v.theme : osPrefersDark() ? "dark" : "light",
        lang: v.lang === "en" ? "en" : "zh",
      };
    }
  } catch {
    /* ignore a corrupt preference blob */
  }
  const nav = typeof navigator !== "undefined" ? (navigator.language || "").toLowerCase() : "";
  return { theme: osPrefersDark() ? "dark" : "light", lang: nav.startsWith("zh") ? "zh" : "en" };
}

function osPrefersDark(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function applyTheme(theme: Theme) {
  document.documentElement.classList.toggle("dark", theme === "dark");
}

export function AppShell({ children }: { children: React.ReactNode }) {
  const [ui, setUi] = React.useState<Stored>({ theme: "light", lang: "zh" });
  const [ready, setReady] = React.useState(false);

  React.useEffect(() => {
    setUi(loadStored());
    setReady(true);
  }, []);

  // Re-apply on every preference change. Applying only on mount meant the toggle did nothing
  // until a reload.
  React.useEffect(() => {
    if (!ready) return;
    applyTheme(ui.theme);
  }, [ui.theme, ready]);

  React.useEffect(() => {
    if (!ready) return;
    document.documentElement.lang = ui.lang === "zh" ? "zh-CN" : "en";
    try {
      window.localStorage.setItem(STORE_KEY, JSON.stringify(ui));
    } catch {
      /* private mode: preferences simply do not persist */
    }
  }, [ui, ready]);

  const ctx = React.useMemo<I18nCtx>(
    () => ({
      lang: ui.lang,
      setLang: (lang: Lang) => setUi((p) => ({ ...p, lang })),
      t: dictionaries[ui.lang],
      pick: (v) => (ui.lang === "zh" ? v.zh : v.en),
    }),
    [ui.lang],
  );

  return (
    <I18nContext.Provider value={ctx}>
      <ToastProvider>
        {/* Fixed-height app shell: the document itself does not scroll, so the header and the
            mobile tab bar stay put and only the content pane moves. */}
        <div className="relative z-[1] flex h-dvh flex-col overflow-hidden">
          <TopBar theme={ui.theme} setTheme={(theme) => setUi((p) => ({ ...p, theme }))} />
          <main className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
            <div className="mx-auto w-full max-w-[1180px] px-3 pb-8 pt-4 sm:px-5 sm:pb-10 sm:pt-5">
              {children}
            </div>
          </main>
          <TabBar />
        </div>
      </ToastProvider>
    </I18nContext.Provider>
  );
}

function TopBar({ theme, setTheme }: { theme: Theme; setTheme: (t: Theme) => void }) {
  const { t, lang, setLang } = useI18nSafe();
  const { stats } = useStatsStream();
  const peak = stats?.router.peak;
  return (
    <header className="relative z-40 shrink-0 border-b border-[var(--line)] bg-[var(--bg)]">
      <div className="mx-auto flex w-full max-w-[1180px] items-center gap-3 px-3 py-2.5 sm:px-5">
        <Link href="/" className="group flex min-w-0 items-center gap-2">
          <Sigil />
          <span className="mono truncate text-sm uppercase leading-none tracking-[0.2em]">{t.app.name}</span>
        </Link>

        <div className="ml-auto flex items-center gap-2">
          {stats ? (
            <span className="mono hidden items-center gap-1.5 rounded-[2px] border px-2 py-1 text-2xs uppercase tracking-[0.12em] sm:inline-flex"
              style={{
                borderColor: peak ? "color-mix(in oklab, var(--signal) 45%, transparent)" : "var(--line-strong)",
                color: peak ? "var(--signal)" : "var(--ink-faint)",
              }}
            >
              <span className={cn("h-[6px] w-[6px] rounded-full bg-current", peak && "live-dot")} />
              {peak ? t.dash.peak : t.dash.offpeak}
            </span>
          ) : null}

          <div className="mono flex items-center rounded-[2px] border border-[var(--line-strong)]">
            <button
              type="button"
              onClick={() => setLang("zh")}
              aria-pressed={lang === "zh"}
              className={cn("px-1.5 py-1 text-2xs uppercase tracking-wider transition-colors", lang === "zh" ? "text-[var(--signal)]" : "text-[var(--ink-faint)] hover:text-[var(--ink)]")}
            >
              中
            </button>
            <span className="h-3 w-px bg-[var(--line-strong)]" />
            <button
              type="button"
              onClick={() => setLang("en")}
              aria-pressed={lang === "en"}
              className={cn("px-1.5 py-1 text-2xs uppercase tracking-wider transition-colors", lang === "en" ? "text-[var(--signal)]" : "text-[var(--ink-faint)] hover:text-[var(--ink)]")}
            >
              EN
            </button>
          </div>

          <button
            type="button"
            onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
            title={theme === "dark" ? t.topbar.toLight : t.topbar.toDark}
            aria-label={theme === "dark" ? t.topbar.toLight : t.topbar.toDark}
            className="mono flex h-7 w-7 items-center justify-center rounded-[2px] border border-[var(--line-strong)] text-[var(--ink-dim)] transition-colors hover:border-[var(--signal)] hover:text-[var(--signal)]"
          >
            {theme === "dark" ? <Moon /> : <Sun />}
          </button>
        </div>
      </div>
      {/* The desktop navigation lives inside the sticky header so it can never scroll out of
          reach; on phones the fixed bottom tab bar takes over. */}
      <nav className="hidden border-t border-[var(--line)] sm:block">
        <div className="mx-auto flex w-full max-w-[1180px] gap-1 px-3 sm:px-5">
          <NavLinks />
        </div>
      </nav>

      <div className="h-px w-full overflow-hidden">
        <div className="sweep h-px w-full" style={{ background: "linear-gradient(90deg, transparent, var(--signal), transparent)" }} />
      </div>
    </header>
  );
}

function NavLinks() {
  const { t } = useI18nSafe();
  const path = usePathname();
  const items = [
    { href: "/", label: t.nav.dashboard },
    { href: "/endpoints", label: t.nav.endpoints },
    { href: "/policy", label: t.nav.policy },
    { href: "/settings", label: t.nav.settings },
    { href: "/models", label: t.nav.models },
  ];
  const active = (href: string) => (href === "/" ? path === "/" : path.startsWith(href));
  return (
    <>
      {items.map((it) => (
        <Link
          key={it.href}
          href={it.href}
          className={cn(
            "mono relative px-2.5 py-2 text-2xs uppercase tracking-[0.14em] transition-colors sm:px-3",
            active(it.href) ? "text-[var(--ink)]" : "text-[var(--ink-faint)] hover:text-[var(--ink-dim)]",
          )}
        >
          {it.label}
          {active(it.href) ? (
            <span className="absolute inset-x-1.5 bottom-0 h-[2px]" style={{ background: "var(--signal)" }} />
          ) : null}
        </Link>
      ))}
    </>
  );
}

function TabBar() {
  const { t } = useI18nSafe();
  const path = usePathname();
  const items = [
    { href: "/", label: t.nav.dashboard, icon: <Gauge /> },
    { href: "/endpoints", label: t.nav.endpoints, icon: <Nodes /> },
    { href: "/policy", label: t.nav.policy, icon: <SwitchIcon /> },
    { href: "/settings", label: t.nav.settings, icon: <Sliders /> },
    { href: "/models", label: t.nav.models, icon: <Library /> },
  ];
  const active = (href: string) => (href === "/" ? path === "/" : path.startsWith(href));
  return (
    <>
      {/* phone: a bottom row of the shell rather than a fixed overlay, so it never covers the
          end of the content and the content pane does not need to reserve space for it */}
      <nav className="z-40 shrink-0 border-t border-[var(--line)] bg-[var(--panel)] sm:hidden">
        <div className="grid grid-cols-5">
          {items.map((it) => (
            <Link
              key={it.href}
              href={it.href}
              className={cn(
                "flex flex-col items-center gap-1 py-2.5 text-2xs transition-colors",
                active(it.href) ? "text-[var(--signal)]" : "text-[var(--ink-faint)]",
              )}
            >
              {it.icon}
              <span className="mono uppercase tracking-wider">{it.label}</span>
            </Link>
          ))}
        </div>
        <div className="h-[env(safe-area-inset-bottom)]" />
      </nav>
    </>
  );
}

function useI18nSafe() {
  const { t, lang, setLang } = React.useContext(I18nContext)!;
  return { t, lang, setLang };
}

/* --- inline marks ---------------------------------------------------------- */

function Sigil() {
  return (
    <svg width="22" height="22" viewBox="0 0 22 22" aria-hidden className="shrink-0">
      <rect x="0.5" y="0.5" width="21" height="21" rx="1.5" fill="none" stroke="var(--line-strong)" />
      <path d="M5 15 L11 5 L17 15" fill="none" stroke="var(--signal)" strokeWidth="1.6" strokeLinecap="square" />
      <circle cx="11" cy="11" r="1.6" fill="var(--signal)" />
    </svg>
  );
}

const stroke = { fill: "none", stroke: "currentColor", strokeWidth: 1.5, strokeLinecap: "square" as const };

function Gauge() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden>
      <path d="M2.5 11.5a6 6 0 1 1 11 0" {...stroke} />
      <path d="M8 11.5 L11 7" {...stroke} />
    </svg>
  );
}
function Nodes() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden>
      <rect x="2.5" y="2.5" width="4.5" height="4.5" {...stroke} />
      <rect x="9" y="9" width="4.5" height="4.5" {...stroke} />
      <path d="M7 5 H12.5 V9" {...stroke} />
    </svg>
  );
}
function SwitchIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden>
      <path d="M2.5 5.5 H11 M13.5 5.5 A1.5 1.5 0 1 1 13.5 8.5 A1.5 1.5 0 1 1 13.5 5.5" {...stroke} />
      <path d="M13.5 10.5 H5 M2.5 10.5 A1.5 1.5 0 1 1 2.5 13.5 A1.5 1.5 0 1 1 2.5 10.5" {...stroke} />
    </svg>
  );
}
function Sliders() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden>
      <path d="M3 3 V13 M8 3 V13 M13 3 V13" {...stroke} />
      <path d="M1.5 6 H4.5 M6.5 10 H9.5 M11.5 5 H14.5" {...stroke} />
    </svg>
  );
}
function Sun() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden>
      <circle cx="8" cy="8" r="3" {...stroke} />
      <path d="M8 1v2M8 13v2M1 8h2M13 8h2M3 3l1.4 1.4M11.6 11.6L13 13M13 3l-1.4 1.4M4.4 11.6L3 13" {...stroke} />
    </svg>
  );
}
function Moon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden>
      <path d="M13 10.2A5.6 5.6 0 0 1 5.8 3 5.8 5.8 0 1 0 13 10.2Z" {...stroke} />
    </svg>
  );
}


function Library() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden>
      <path d="M2.5 2.5h3v11h-3z" {...stroke} />
      <path d="M6.5 2.5h3v11h-3z" {...stroke} />
      <path d="M11 3.2l2.4-.7 2.1 10.4-2.4.7z" {...stroke} />
    </svg>
  );
}
