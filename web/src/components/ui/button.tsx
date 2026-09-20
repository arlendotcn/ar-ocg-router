"use client";

import * as React from "react";
import { cn } from "@/lib/utils";

type Variant = "primary" | "ghost" | "outline" | "danger" | "selected";
type Size = "sm" | "md" | "icon";

// Every variant states BOTH its fill and its label colour explicitly, using dedicated
// "on-solid" tokens (see globals.css). Deriving the label from the page background token
// (--bg) previously produced identical foreground/background in some themes, i.e. an
// invisible label; these tokens are defined to be readable on their own fill in both themes.
/**
 * Hover means "the pointer is on this control", expressed in the *colour the button already has*.
 *
 * Neutral buttons go to the accent (`--signal`, orange): the colour itself is the feedback, and it
 * is the same accent the app already uses for the focused/selected state, so hovering reads as
 * "this is the one you are about to act on".
 *
 * Semantic buttons keep their own colour and never trade it for the accent. A destructive control
 * stays red on hover, because red is what it is telling the user; turning it orange - or, as an
 * earlier attempt did, near-white via `--ink` - deletes that information at the exact moment the
 * user is deciding whether to press it. `primary` keeps its own solid fill and label pair.
 *
 * The earlier rule sent every neutral label to `--ink`. That token is body text, not feedback: in
 * the dark theme it is near-white, so every button hovered to white and the variants stopped being
 * distinguishable at all.
 */
const HOVER_ACCENT = "hover:text-[var(--signal)]";

/**
 * Fill for a neutral button: the accent at 15%, which the `selected` variant already uses at rest.
 * It replaced `--panel-2`, a *surface* token that differs from `--panel` by a contrast ratio of
 * 1.068 (dark) / 1.115 (light) - and 1.0 means "identical", so the fill was invisible. This one
 * separates by 1.321 / 1.212.
 */
const HOVER_FILL = "hover:bg-[var(--signal)]/15";

/** Destructive controls hover deeper into their own colour instead of taking the accent. */
const HOVER_DANGER = "hover:text-[var(--danger)] hover:bg-[var(--danger)]/12";

const VARIANTS: Record<Variant, string> = {
  primary:
    "bg-[var(--solid)] text-[var(--on-solid)] hover:bg-[var(--solid-hover)] hover:text-[var(--on-solid)] border border-[var(--solid)] font-semibold",
  outline: `border border-[var(--line-strong)] text-[var(--ink)] ${HOVER_ACCENT} ${HOVER_FILL} hover:border-[var(--signal)] bg-transparent`,
  ghost: `border border-transparent text-[var(--ink-dim)] ${HOVER_ACCENT} ${HOVER_FILL}`,
  danger: `border border-[var(--danger)]/60 text-[var(--danger)] ${HOVER_DANGER} bg-transparent`,
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
