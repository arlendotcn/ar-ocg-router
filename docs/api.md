# HTTP API

[**English**](api.md) · [中文](api-zh.md)

Everything is served by one process on one port. The proxy and the admin API are separate
namespaces: `/v1/*` is proxied, everything under `/api/*` is the console, and the console
static files own the remaining paths.

## Proxy

| Endpoint | Notes |
| --- | --- |
| `POST /v1/chat/completions` | OpenAI chat |
| `POST /v1/responses` | OpenAI Responses |
| `POST /v1/messages` | Anthropic Messages |
| `GET /v1/models` | Returns exactly one id, `ar-ocg-router` |

A bare `http://host:port` also works, and `/openai/v1` and `/api/v1` prefixes are accepted.

Every proxied response carries:

| Header | Value |
| --- | --- |
| `x-router-account` | Endpoint name that served the request |
| `x-router-endpoint` | Same value, explicit alias |
| `x-router-kind` | `plans` or `fallback` |
| `x-router-model` | The model id actually sent upstream |
| `x-router-peak` | `peak` or `offpeak` |
| `x-router-cache` | `hit`, `cold` or `unknown` (non-streaming only; a streamed head is already on the wire) |
| `x-router-request-id` | Correlation id used in the log |

## Admin

| Endpoint | Purpose |
| --- | --- |
| `GET /` | Web console (falls back to a text banner when not embedded) |
| `GET /router/banner` | Text banner, for scripts |
| `GET /health` | Liveness probe; never requires a key |
| `GET /router/stats` | Full JSON snapshot: clock state, counters, per-endpoint health, quota windows, balance, tokens and cost |
| `GET /router/schedule` | Upcoming peak/off-peak transitions |
| `POST /router/reload` | Re-read the config now (it is also watched every 3 s) |
| `GET /api/config` | Config as the console edits it (keys redacted) |
| `PUT /api/config` | Validate, back up, write, reload |
| `GET /api/config/raw` · `PUT /api/config/raw` | Raw YAML text |
| `GET /api/config/export` | Serialised config for download |
| `POST /api/config/import` | Import YAML or JSON |
| `POST /api/plan/preview` | Simulate candidate order right now |
| `POST /api/test/<name>` | Live endpoint probe (key, /models, quota, one request) |
| `POST /api/endpoints/<name>/state` | Enable or disable |
| `POST /api/endpoints/<name>/duplicate` | Copy an endpoint under a free name |
| `GET /api/endpoints/<name>/models` | That endpoint's live model catalog |
| `GET /api/library` · `PUT /api/library` | Model library |
| `GET /api/backups` · `POST /api/backups` | List and create config backups |
| `GET` / `DELETE` `/api/backups/<name>` · `POST .../restore` | Download, delete, restore |

When `server.client_keys` is set, every admin route except `/health` requires the same bearer
token as the proxy. The console remains unauthenticated only on the loopback default, which is
why that default should be kept.

## Error shape

Errors are JSON in the OpenAI shape:

```json
{"error": {"message": "...", "type": "all_accounts_failed", "code": 502,
           "router": "ar-ocg-router 0.0.1"}}
```

Types include `unauthorized`, `no_account_available`, `all_accounts_failed`,
`reload_failed`, `not_found` and `admin_error`.
