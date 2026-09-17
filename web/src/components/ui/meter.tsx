"use client";

import { cn } from "@/lib/utils";

/**
 * A quota window rendered as a segmented bar. The segments (rather than a smooth
 * gradient) are deliberate: they read like an instrument gauge and stay legible in
 * both themes. The thin cursor marks the "tight" threshold, so the operator can see
 * at a glance whether off-peak traffic would move to cash.
 */
export function Meter({
  pct,
  projected,
  limit,
  tone = "var(--signal)",
  segments = 30,
  className,
}: {
  pct: number;
  projected?: number;
  limit?: number;
  tone?: string;
  segments?: number;
  className?: string;
}) {
  const clamp = (v: number) => Math.max(0, Math.min(100, v));
  const filled = Math.round((clamp(pct) / 100) * segments);
  const projExtra =
    projected === undefined ? 0 : Math.max(0, Math.round((clamp(projected) / 100) * segments) - filled);
  const candleAt = limit === undefined ? -1 : Math.round((clamp(limit) / 100) * segments - 0.5);
  const over = limit !== undefined && pct > limit;

  return (
    <div
      className={cn("relative flex items-center gap-[2px]", className)}
      role="img"
      aria-label={`${pct.toFixed(0)}%${projected !== undefined ? ` (projected ${projected.toFixed(0)}%)` : ""}`}
    >
      {Array.from({ length: segments }, (_, i) => {
        const isFilled = i < filled;
        const isProjected = !isFilled && i < filled + projExtra;
        return (
          <span
            key={i}
            className="h-2.5 flex-1 rounded-[1px] transition-colors duration-300"
            style={{
              background: isFilled
                ? over
                  ? "var(--danger)"
                  : tone
                : isProjected
                  ? `color-mix(in oklab, ${tone} 30%, transparent)`
                  : "var(--panel-2)",
            }}
          />
        );
      })}
      {candleAt >= 0 && candleAt < segments ? (
        <span
          className="absolute -top-1 -bottom-1 w-[1px]"
          style={{ left: `calc(${((candleAt + 0.5) / segments) * 100}% - 0.5px)`, background: "var(--line-strong)" }}
        />
      ) : null}
    </div>
  );
}
