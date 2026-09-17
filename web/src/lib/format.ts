"use client";

import type { Lang } from "./i18n";

export function fmtNum(n: number | null | undefined, digits = 0): string {
  if (n === null || n === undefined || Number.isNaN(n)) return "—";
  return n.toLocaleString(undefined, { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

export function fmtUsd(n: number | null | undefined, digits = 4): string {
  if (n === null || n === undefined || Number.isNaN(n)) return "—";
  if (n === 0) return "$0";
  if (Math.abs(n) < 0.0001) return "$" + n.toExponential(2);
  return "$" + n.toFixed(digits).replace(/0+$/, "").replace(/\.$/, "");
}

export function fmtBytes(n: number): string {
  if (!n) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v < 10 && i > 0 ? 1 : 0)} ${units[i]}`;
}

/** 88200 -> "1d 0h 30m" (compact, three units max) */
export function fmtDuration(secs: number, lang: Lang): string {
  const s = Math.max(0, Math.floor(secs));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const u = lang === "zh" ? { d: "天", h: "时", m: "分", s: "秒" } : { d: "d", h: "h", m: "m", s: "s" };
  const sep = lang === "zh" ? "" : " ";
  if (d > 0) return `${d}${u.d}${sep}${h}${u.h}`;
  if (h > 0) return `${h}${u.h}${sep}${m}${u.m}`;
  if (m > 0) return `${m}${u.m}${sep}${sec}${u.s}`;
  return `${sec}${u.s}`;
}

export function fmtPct(n: number | null | undefined, digits = 0): string {
  if (n === null || n === undefined || Number.isNaN(n)) return "—";
  return `${n.toFixed(digits)}%`;
}

/** ISO strings from the router; falls back to the raw value when unparsable. */
export function fmtWhen(iso: string | null | undefined, lang: Lang): string {
  if (!iso) return "—";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return iso;
  const d = new Date(t);
  return d.toLocaleString(lang === "zh" ? "zh-CN" : "en-US", {
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

export function fmtAgo(iso: string | null | undefined, lang: Lang): string {
  if (!iso) return "—";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return iso;
  const secs = (Date.now() - t) / 1000;
  if (secs < 5) return lang === "zh" ? "刚刚" : "just now";
  return fmtDuration(secs, lang) + (lang === "zh" ? "前" : " ago");
}

export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* fall through to the legacy path */
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}

export function download(filename: string, text: string, mime = "text/yaml"): void {
  const blob = new Blob([text], { type: `${mime};charset=utf-8` });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
