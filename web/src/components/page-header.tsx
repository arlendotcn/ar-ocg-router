"use client";

import * as React from "react";
import { Badge } from "@/components/ui/badge";

export function PageHeader({
  title,
  hint,
  actions,
  meta,
}: {
  title: string;
  hint?: React.ReactNode;
  actions?: React.ReactNode;
  meta?: React.ReactNode;
}) {
  return (
    <div className="mb-4 flex flex-col gap-3 sm:mb-5 sm:flex-row sm:items-end sm:justify-between">
      <div className="min-w-0">
        <div className="flex items-center gap-2.5">
          <h1 className="mono text-lg uppercase tracking-[0.2em] sm:text-xl">{title}</h1>
          {meta}
        </div>
        {hint ? <p className="mt-1.5 text-sm leading-relaxed text-[var(--ink-faint)]">{hint}</p> : null}
      </div>
      {actions ? <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div> : null}
    </div>
  );
}
