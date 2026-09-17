# Web console

[**English**](web-console.md) · [中文](web-console-zh.md)

Open <http://127.0.0.1:8787/>. The entire frontend - a static Next.js export - is compiled
into the binary, so deployment is still one executable plus `config.yaml`. There is no
`node_modules` at runtime and no asset directory to lose.

Stack: Next.js 15 (`output: "export"`), React 18, TypeScript, Tailwind CSS v4, and a small
in-house component set. Mobile-first (bottom tab bar on phones, sticky top navigation on
desktop), fully bilingual (English and Chinese), light and dark themes that follow the system
and can be overridden. Preferences live in `localStorage`; nothing is stored server-side.

## Pages

| Page | What it does |
| --- | --- |
| **Overview** | Peak/off-peak state with a countdown to the next change, request/error/stream/retry counters, plan savings versus cash spend, per-endpoint health and cooldown, quota meters for all three windows (with projection and threshold marks), balance, tokens, cache hits, latency, last error. Refreshes every 2 s and can be paused. |
| **Endpoints** | Add, edit and remove endpoints; **drag rows to set the consumption order**; **enable/disable with immediate effect**; **duplicate an endpoint**; copy a ready-made `curl`; run a **connectivity self-test** (key, `/models`, quota or balance, one minimal request) that returns evidence-based suggestions. |
| **Policy** | Routing mode, off-peak preference, surplus threshold and projection, exhaustion threshold, session affinity, every cooldown and skip value, and the compatibility switches. Includes a **candidate order preview** that simulates a request against the current clock and quota. |
| **Models** | The [model library](model-library.md): reference specs, aliases, and which endpoints use each model. |
| **Settings** | Listen address, port, connections, timeouts, body limit, gateway keys, log level and file, copy-paste client snippets, **backup / export / import**, and raw YAML editing. |

Every configurable field has a `?` button next to it that explains, inline and in the current
language, what that field does - the goal is that changing a setting never requires leaving
the page to read documentation.

## Safety boundary

- The console is an unauthenticated local admin surface, exactly like `/router/stats`. It
  binds to `127.0.0.1` by default; keep it there, or set `server.client_keys` first, which
  the `/api/*` routes then enforce as well.
- **Keys are write-only.** `GET /api/config` never returns a literal credential - it returns a
  placeholder. Sending that placeholder back on save means "keep the stored value".
  `env:VAR` and `file:path` references are shown as-is.
- Static assets reject path traversal; backup reads, deletes and restores only accept
  `config-*.yaml` names.

## How a save reaches disk

Every write follows the same path, which is what keeps the console and a hand-edited file
from disagreeing:

1. the edit is rendered to YAML;
2. it is validated with `config::parse`, **the same parser the router uses at runtime**, so
   anything the console can store is something the router can load;
3. on failure the request returns 400 and the file on disk is **not touched**;
4. on success a timestamped copy goes to `backups/`, then the file is written atomically
   (temp file in the same directory, fsync, rename);
5. the running configuration is reloaded, so the page immediately reflects what is live.

> **Note.** Saving rewrites `config.yaml` and **does not preserve hand-written comments**.
> A timestamped backup is taken before every write. For long-lived comments, keep them in
> `config.example.yaml` or use Export.

## Building the frontend

The build scripts rebuild the console on every run and then embed it:

```powershell
.\build.ps1            # npm run build (web/ -> web/out), then cargo, then package dist
```

```bash
./build.sh
```

For Rust-only iteration call cargo directly instead: it re-embeds the existing `web/out`.
Only a changed frontend needs a web build.
