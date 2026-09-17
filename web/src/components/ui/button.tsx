"use client";

import * as React from "react";
import { cn } from "@/lib/utils";

type Variant = "primary" | "ghost" | "outline" | "danger" | "selected";
type Size = "sm" | "md" | "icon";

// Every variant states BOTH its fill and its label colour explicitly, using dedicated
// "on-solid" tokens (see globals.css). Deriving the label from the page background token
// (--bg) previously produced identical foreground/background in some themes, i.e. an
// invisible label; these tokens are defined to be readable on their own fill in both themes.
const VARIANTS: Record<Variant, string> = {
  primary:
    "bg-[var(--solid)] text-[var(--on-solid)] hover:bg-[var(--solid-hover)] hover:text-[var(--on-solid)] border border-[var(--solid)] font-semibold",
  outline:
    "border border-[var(--line-strong)] text-[var(--ink)] hover:border-[var(--signal)] hover:text-[var(--signal)] bg-transparent",
  ghost: "border border-transparent text-[var(--ink-dim)] hover:text-[var(--ink)] hover:bg-[var(--panel-2)]",
  danger:
    "border border-[var(--danger)]/60 text-[var(--danger)] hover:bg-[var(--danger)]/12 bg-transparent",
  // A selected option in a group of mutually exclusive buttons. Deliberately distinct from
  // "primary" (an action like Save): this expresses state, e.g. "you are looking at chat".
  selected:
    "border border-[var(--signal)] bg-[var(--signal)]/15 text-[var(--signal)] font-semibold hover:bg-[var(--signal)]/25",
};

const SIZES: Record<Size, string> = {
  sm: "h-7 px-2.5 text-xs gap-1.5",
  md: "h-9 px-3.5 text-sm gap-2",
  // Same height as `sm` on purpose: icon and text buttons sit side by side in list rows, and a
  // taller icon button throws the row off its baseline.
  icon: "h-7 w-7 p-0 justify-center",
};

/** The size classes, exported so non-Button controls (ConfirmButton) can match exactly. */
export const BUTTON_SIZE = SIZES;

export type ButtonProps = React.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: Variant;
  size?: Size;
  loading?: boolean;
};

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { className, variant = "outline", size = "md", loading, disabled, children, ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      disabled={disabled || loading}
      className={cn(
        "mono inline-flex select-none items-center rounded-[2px] tracking-wide uppercase transition-colors duration-150",
        "disabled:cursor-not-allowed disabled:opacity-40",
        VARIANTS[variant],
        SIZES[size],
        className,
      )}
      {...rest}
    >
      {loading ? <Spinner /> : null}
      {children}
    </button>
  );
});

export function Spinner({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" className={cn("spin h-3 w-3 shrink-0", className)} aria-hidden>
      <circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" strokeOpacity="0.25" strokeWidth="3" />
      <path d="M21 12a9 9 0 0 0-9-9" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" />
    </svg>
  );
}
