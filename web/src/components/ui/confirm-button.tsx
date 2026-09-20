"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { BUTTON_SIZE } from "./button";

/**
 * A destructive action confirmed in place, anchored to the trigger.
 *
 * The alternative - a confirm() dialog or a bar at the top of the list - makes the user look
 * away from what they clicked and then hunt for the thing they were about to change. This keeps
 * the decision where the intent was: the button becomes its own confirmation, and clicking
 * elsewhere or pressing Escape backs out.
 *
 * The panel is positioned 'fixed' from the trigger's measured rect rather than as an absolutely
 * positioned child: the row it lives in scrolls horizontally, and an abs-positioned child of a
 * right-aligned button near the viewport edge would be pushed off screen.
 */
export function ConfirmButton({
  onConfirm,
  children,
  confirmLabel,
  title,
  className,
  disabled,
}: {
  onConfirm: () => void;
  children: React.ReactNode;
  /** Label of the confirming button, e.g. "Delete". */
  confirmLabel: string;
  /** Optional prompt shown above the buttons. */
  title?: string;
  className?: string;
  disabled?: boolean;
}) {
  const { t } = useI18n();
  const [open, setOpen] = React.useState(false);
  const [pos, setPos] = React.useState<{ top: number; left: number } | null>(null);
  const wrap = React.useRef<HTMLSpanElement>(null);
  const panel = React.useRef<HTMLSpanElement>(null);
  const cancelRef = React.useRef<HTMLButtonElement>(null);

  const PANEL_W = 232;
  const GAP = 6;

  // Measure and clamp on open, and keep it pinned while the page scrolls or resizes.
  const place = React.useCallback(() => {
    const t0 = wrap.current?.getBoundingClientRect();
    if (!t0) return;
    const h = panel.current?.offsetHeight ?? 88;
    const vw = window.innerWidth;
    // Prefer right-aligned to the trigger, but never past either viewport edge.
    let left = t0.right - PANEL_W;
    left = Math.min(Math.max(8, left), vw - PANEL_W - 8);
    // Above the trigger; flip below if there is no room above.
    let top = t0.top - h - GAP;
    if (top < 8) top = t0.bottom + GAP;
    setPos({ top, left });
  }, []);

  React.useLayoutEffect(() => {
    if (!open) return;
    place();
    const onScroll = () => place();
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onScroll);
    return () => {
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onScroll);
    };
  }, [open, place]);

  React.useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const target = e.target as Node;
      if (wrap.current?.contains(target) || panel.current?.contains(target)) return;
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  // A destructive confirmation should be hard to trigger by accident: focus moves to the
  // cancel side, so a stray Enter does not delete anything.
  React.useEffect(() => {
    if (open) cancelRef.current?.focus();
  }, [open]);

  return (
    <span ref={wrap} className="relative inline-flex">
      <button
        type="button"
        disabled={disabled}
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className={cn(
          BUTTON_SIZE.sm,
          "mono inline-flex shrink-0 items-center whitespace-nowrap rounded-[2px] border transition-colors",
          open
            ? "border-[var(--danger)] bg-[var(--danger)]/12 text-[var(--danger)]"
            // A destructive trigger: it hovers into its own colour (red label, faint red fill)
            // rather than the neutral accent. The row's other actions are neutral buttons, but
            // this one deletes something, and that meaning must not change under the pointer.
            : "border-transparent text-[var(--ink-dim)] hover:bg-[var(--danger)]/12 hover:text-[var(--danger)]",
          "disabled:cursor-not-allowed disabled:opacity-40",
          className,
        )}
      >
        {children}
      </button>

      {open ? (
        <span
          ref={panel}
          role="dialog"
          aria-label={title ?? confirmLabel}
          style={{
            position: "fixed",
            top: pos ? pos.top : -9999,
            left: pos ? pos.left : -9999,
            width: PANEL_W,
          }}
          className="rise z-50 block rounded-[2px] border border-[var(--line-strong)] bg-[var(--panel)] p-2.5 text-left shadow-xl"
        >
          {title ? (
            <span className="mb-2 block text-xs leading-snug text-[var(--ink-dim)]">{title}</span>
          ) : null}
          <span className="flex items-center justify-end gap-1.5">
            <button
              ref={cancelRef}
              type="button"
              onClick={() => setOpen(false)}
              className="mono h-7 rounded-[2px] px-2.5 text-xs uppercase tracking-wide text-[var(--ink-faint)] transition-colors hover:bg-[var(--signal)]/15 hover:text-[var(--signal)]"
            >
              {t.common.cancel}
            </button>
            <button
              type="button"
              onClick={() => {
                setOpen(false);
                onConfirm();
              }}
              className="mono h-7 rounded-[2px] border border-[var(--danger)] bg-[var(--danger)]/15 px-2.5 text-xs uppercase tracking-wide text-[var(--danger)] transition-colors hover:bg-[var(--danger)]/25"
            >
              {confirmLabel}
            </button>
          </span>
        </span>
      ) : null}
    </span>
  );
}
