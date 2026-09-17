"use client";

import * as React from "react";
import { cn } from "@/lib/utils";
import { Button } from "./button";

/**
 * A right-side sheet on wide screens and a bottom sheet on phones. Used for
 * endpoint editing, the backup list and the raw config editor.
 */
export function Sheet({
  open,
  onClose,
  title,
  subtitle,
  children,
  footer,
  wide,
}: {
  open: boolean;
  onClose: () => void;
  title: React.ReactNode;
  subtitle?: React.ReactNode;
  children: React.ReactNode;
  footer?: React.ReactNode;
  wide?: boolean;
}) {
  React.useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.removeEventListener("keydown", onKey);
      document.body.style.overflow = prev;
    };
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 flex flex-col justify-end sm:flex-row sm:justify-end">
      <div
        className="absolute inset-0 bg-black/55 backdrop-blur-[1px]"
        onClick={onClose}
        aria-hidden
      />
      <div
        role="dialog"
        aria-modal="true"
        className={cn(
          "rise plate relative flex max-h-[92vh] w-full flex-col rounded-b-none sm:max-h-none sm:h-full sm:rounded-none",
          wide ? "sm:w-[min(760px,92vw)]" : "sm:w-[min(560px,92vw)]",
        )}
      >
        <div className="flex items-start justify-between gap-3 border-b border-[var(--line)] px-4 py-3">
          <div className="min-w-0">
            <div className="mono text-sm uppercase tracking-[0.16em]">{title}</div>
            {subtitle ? <div className="mt-1 text-xs text-[var(--ink-faint)]">{subtitle}</div> : null}
          </div>
          <Button variant="ghost" size="icon" onClick={onClose} aria-label="close">
            ×
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-4 py-4">{children}</div>
        {footer ? (
          <div className="flex items-center justify-end gap-2 border-t border-[var(--line)] bg-[var(--panel-2)] px-4 py-3">
            {footer}
          </div>
        ) : null}
      </div>
    </div>
  );
}
