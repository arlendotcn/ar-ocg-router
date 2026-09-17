"use client";

import * as React from "react";
import { cn } from "@/lib/utils";

export type Tone = "plain" | "plans" | "cash" | "signal" | "warn" | "danger" | "on";

const TONES: Record<Tone, string> = {
  plain: "border-[var(--line-strong)] text-[var(--ink-dim)]",
  plans: "border-[var(--plans)]/45 text-[var(--plans)]",
  cash: "border-[var(--cash)]/45 text-[var(--cash)]",
  signal: "border-[var(--signal)]/50 text-[var(--signal)]",
  warn: "border-[var(--warn)]/50 text-[var(--warn)]",
  danger: "border-[var(--danger)]/50 text-[var(--danger)]",
  on: "border-[var(--up)]/50 text-[var(--up)]",
};

export function Badge({
  tone = "plain",
  children,
  className,
  dot,
}: {
  tone?: Tone;
  children: React.ReactNode;
  className?: string;
  dot?: boolean;
}) {
  return (
    <span
      className={cn(
        "mono inline-flex items-center gap-1.5 rounded-[2px] border px-1.5 py-[2px] text-2xs uppercase tracking-[0.1em] leading-none",
        TONES[tone],
        className,
      )}
    >
      {dot ? <span className="h-[5px] w-[5px] rounded-full bg-current" /> : null}
      {children}
    </span>
  );
}
