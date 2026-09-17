import type { Metadata, Viewport } from "next";
import { IBM_Plex_Mono, IBM_Plex_Sans } from "next/font/google";
import "./globals.css";
import { AppShell } from "@/components/app-shell";

// Only the latin subset is shipped: the CJK glyphs come from the OS font stack
// (pingfang / yahei / noto sans sc), which keeps the embedded payload small.
// The variable names must match what globals.css consumes (--font-plex-sans / --font-plex-mono).
// A mismatch does not fail the build: var() simply resolves to nothing and the browser falls
// back silently, which is exactly how the webfont went missing.
const plexSans = IBM_Plex_Sans({
  subsets: ["latin"],
  weight: ["400", "500", "600"],
  variable: "--font-plex-sans",
  display: "swap",
  preload: false,
});

const plexMono = IBM_Plex_Mono({
  subsets: ["latin"],
  weight: ["400", "500"],
  variable: "--font-plex-mono",
  display: "swap",
  preload: false,
});

export const metadata: Metadata = {
  title: "ar-OCG-Router",
  description: "Multi-account routing console for OpenCode Go + DeepSeek official",
};

export const viewport: Viewport = {
  width: "device-width",
  initialScale: 1,
  themeColor: [
    { media: "(prefers-color-scheme: light)", color: "#f4f5f2" },
    { media: "(prefers-color-scheme: dark)", color: "#0c0d0e" },
  ],
};

/** Runs before paint so the stored theme is applied without a flash. */
const THEME_BOOT = `(function(){try{
  var raw = localStorage.getItem('arocr.ui');
  var ui = raw ? JSON.parse(raw) : null;
  // Two themes only. A legacy "system" value is resolved once, against the OS preference.
  var theme = ui && ui.theme ? ui.theme : null;
  if (theme !== 'light' && theme !== 'dark') {
    theme = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  }
  var dark = theme === 'dark';
  document.documentElement.classList.toggle('dark', dark);
  var lang = ui && ui.lang ? ui.lang : (navigator.language || '').toLowerCase().indexOf('zh') === 0 ? 'zh' : 'en';
  document.documentElement.lang = lang === 'zh' ? 'zh-CN' : 'en';
}catch(e){}})();`;

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="zh-CN" className={`${plexSans.variable} ${plexMono.variable}`} suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: THEME_BOOT }} />
      </head>
      <body>
        <AppShell>{children}</AppShell>
      </body>
    </html>
  );
}
