"use client";

import * as React from "react";
import { cn } from "@/lib/utils";

export type Toast = { id: number; kind: "ok" | "err" | "info"; text: string };

const Ctx = React.createContext<{ push: (kind: Toast["kind"], text: string) => void } | null>(null);

export function useToast() {
  const c = React.useContext(Ctx);
  if (!c) throw new Error("useToast outside provider");
  return c;
}

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const [items, setItems] = React.useState<Toast[]>([]);
  const push = React.useCallback((kind: Toast["kind"], text: string) => {
    const id = Date.now() + Math.random();
    setItems((prev) => [...prev.slice(-3), { id, kind, text }]);
    setTimeout(() => setItems((prev) => prev.filter((t) => t.id !== id)), kind === "err" ? 7000 : 3200);
  }, []);
  const value = React.useMemo(() => ({ push }), [push]);
  return (
    <Ctx.Provider value={value}>
      {children}
      {/* The shell keeps the tab bar in normal flow, so nothing has to be cleared here beyond
          the phone's own safe area. */}
      <div className="pointer-events-none fixed inset-x-0 bottom-0 z-[60] flex flex-col items-center gap-2 p-3 pb-[calc(0.75rem+env(safe-area-inset-bottom))] sm:inset-x-auto sm:bottom-4 sm:right-4 sm:items-end sm:pb-3">
        {items.map((t) => (
          <div
            key={t.id}
            className={cn(
              "rise plate pointer-events-auto max-w-[92vw] px-3 py-2 text-sm shadow-lg sm:max-w-[420px]",
            )}
            style={{
              borderColor:
                t.kind === "err" ? "var(--danger)" : t.kind === "ok" ? "var(--up)" : "var(--line-strong)",
            }}
          >
            <span className="mono mr-2 text-2xs uppercase tracking-[0.14em] opacity-60">
              {t.kind === "err" ? "ERR" : t.kind === "ok" ? "OK" : "INFO"}
            </span>
            {t.text}
          </div>
        ))}
      </div>
    </Ctx.Provider>
  );
}
