"use client";

import * as React from "react";
import { cn } from "@/lib/utils";
import { useI18n } from "@/lib/i18n";

export function Plate({ className, children, ...rest }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={cn("plate", className)} {...rest}>
      {children}
    </div>
  );
}

export function PlateBlock({
  title,
  hint,
  actions,
  children,
  className,
}: {
  title: React.ReactNode;
  hint?: React.ReactNode;
  actions?: React.ReactNode;
  children: React.ReactNode;
  className?: string;
}) {
  const { t } = useI18n();
  const [showHint, setShowHint] = React.useState(false);
  return (
    <section className={cn("plate", className)}>
      {/* Stacked on phones: a long title and a wide action group do not fit one row at 440px,
          and letting them overlap is worse than using a second line. */}
      <div className="flex flex-col gap-2 border-b border-[var(--line)] px-3 py-2.5 sm:flex-row sm:items-start sm:justify-between sm:gap-3 sm:px-4">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <h2 className="mono text-xs uppercase tracking-[0.18em]">{title}</h2>
            {hint ? (
              <button
                type="button"
                onClick={() => setShowHint((v) => !v)}
                aria-expanded={showHint}
                aria-label={t.field.help}
                className={cn(
                  "mono flex h-4 w-4 items-center justify-center rounded-full border text-2xs leading-none transition-colors",
                  showHint
                    ? "border-[var(--signal)] text-[var(--signal)]"
                    : "border-[var(--line-strong)] text-[var(--ink-faint)] hover:border-[var(--signal)] hover:text-[var(--signal)]",
                )}
              >
                ?
              </button>
            ) : null}
          </div>
          {showHint && hint ? (
            <p className="rise mt-2 border-l-2 border-[var(--signal)]/50 bg-[var(--panel-2)] px-2.5 py-2 text-xs leading-relaxed text-[var(--ink-dim)]">
              {hint}
            </p>
          ) : null}
        </div>
        {actions ? (
          <div className="flex flex-wrap items-center gap-2 sm:shrink-0">{actions}</div>
        ) : null}
      </div>
      {children}
    </section>
  );
}

export function Readout({
  label,
  value,
  sub,
  tone,
  className,
  help,
}: {
  label: React.ReactNode;
  value: React.ReactNode;
  sub?: React.ReactNode;
  tone?: string;
  className?: string;
  help?: React.ReactNode;
}) {
  const { t } = useI18n();
  const [open, setOpen] = React.useState(false);
  return (
    <div className={cn("px-3 py-2.5", className)}>
      <div className="flex items-center gap-1.5">
        <span className="label">{label}</span>
        {help ? (
          <button
            type="button"
            onClick={() => setOpen((v) => !v)}
            aria-label={t.field.help}
            className={cn(
              "mono flex h-[15px] w-[15px] items-center justify-center rounded-full border text-2xs leading-none transition-colors",
              open ? "border-[var(--signal)] text-[var(--signal)]" : "border-[var(--line-strong)] text-[var(--ink-faint)]",
            )}
          >
            ?
          </button>
        ) : null}
      </div>
      <div className="tabular mt-1.5 text-xl leading-none sm:text-2xl" style={tone ? { color: tone } : undefined}>
        {value}
      </div>
      {open && help ? (
        <div className="rise mt-1.5 text-xs leading-relaxed text-[var(--ink-faint)]">{help}</div>
      ) : sub ? (
        <div className="mt-1 text-xs leading-snug text-[var(--ink-faint)]">{sub}</div>
      ) : null}
    </div>
  );
}
