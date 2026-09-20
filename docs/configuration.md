# Configuration

[**English**](configuration.md) · [中文](configuration-zh.md)

The router reads a single YAML file. Every field has a working default, so the minimum viable
config is one endpoint. A fully commented template ships as `config.example.yaml`.

## File lookup order

1. `--config <path>` when given (relative or absolute);
2. otherwise `config.yaml`, then `config.yml`, **next to the binary**;
3. otherwise the same two names in the **current working directory**.

If none exists the process exits 1 and prints every path it searched.

## Endpoints

An endpoint is one merchant plus one model. There are two lists:

| List | Meaning |
| --- | --- |
| `plans` | Prepaid subscriptions and quota buckets. Consuming them costs no cash until the quota runs out, so they are preferred by default. |
| `fallback` | Pay-as-you-go accounts billed per token. Their price follows the peak/off-peak window. |

```yaml
plans:
  - name: go-dsf
    url: https://opencode.ai/zen/go/v1
    provider: opencodego
    key: sk-...
    model: deepseek-flash
    mode: both
    order: 10
    inject_session: true
    quota: { unit: usd, rolling: 12, weekly: 30, monthly: 60, probe: usage }

fallback:
  - name: ds-official-flash
    url: https://api.deepseek.com
    provider: deepseek
    key: sk-...
    model: deepseek-flash
    mode: both
    order: 10
    rule: [offpeak, quota_low]
    quota: { probe: balance }
```

### Endpoint fields

| Field | Description |
| --- | --- |
| `name` | Used in logs, stats and client pinning. Explicit wins; duplicates get `-2`, `-3`. When omitted it is generated as `<provider>-<model>-<key fingerprint>`. |
| `provider` | `opencodego` / `deepseek` / `generic`. Defaults to a URL sniff. **Set it explicitly when the endpoint is a bridge or proxy**, otherwise model mapping, quota probing and balance probing fall back to `generic`. |
| `url` | Upstream prefix, no trailing slash. |
| `key` | Literal, `env:VAR`, or `file:/path/key.txt`. |
| `model` | **Required.** The only model id this endpoint ever sends, verbatim. A wrong id fails upstream and the router moves on. |
| `mode` | `openai-completion` / `openai-responses` / `anthropic-messages` / `both` / `any`, or a list. |
| `order` | Consumption order inside its list; smaller goes first. Derived from the position of the entry, and rewritten that way on every save — arrange it in the console instead of editing it here. |
| `rule` | `fallback` only: conditions under which it may be *preferred*. See below. |
| `inject_session` | Send `x-opencode-session`. Required by OpenCode Go. |
| `headers` | Extra request headers (some bridges require a client fingerprint). |
| `drop_params` | Body fields this endpoint rejects. |
| `max_output_tokens_limit` | **A ceiling, not a fixed value.** When the client asks for more than this, the request is sent with this value instead; anything at or below it is passed through untouched. Omitted or `0` means the value is never touched. Set it from a measured upstream limit, not a guess: set too low, it silently shortens every long answer. See below. |
| `enabled` | `false` removes the endpoint from every candidate list, including failover. See [Routing](routing.md). |

#### `max_output_tokens_limit`

Some upstreams reject the whole request when `max_output_tokens` exceeds their own ceiling, and
they say so with a generic "a parameter specified in the request is not valid" that **names no
parameter** — so the failure looks like "this endpoint cannot serve anything" rather than "one
number is too big". Coding agents make it worse by asking for a multiple of the context window
(384000, i.e. 3 × 128k), which no model can return.

The ceiling exists for exactly that case, per endpoint because the ceilings differ per upstream
(DeepSeek accepts 200000; Ark rejects it). Two properties are deliberate:

- **Clamping, not forcing.** A request that asks for 4096 is sent as 4096.
- **Your fact, not the router's guess.** Fill it in after measuring the upstream once. The router
  never derives it from the model library or from anywhere else.

Every clamp is logged at INFO as
`<endpoint>: max_output_tokens 384000 -> 131072 (endpoint limit)`, so the record shows what was
actually sent.

### `rule` values

Multiple values are OR-ed. `always` (the default), `peak`, `offpeak`, `quota_low`,
`quota_exhausted`, `primary_unavailable`, `never`. `no_error_fallback` additionally bars the
endpoint from being used as a failure fallback.

## Quota

| Field | Description |
| --- | --- |
| `quota.unit` | `usd` / `tokens` / `none`. Token-metered plans need no price table. |
| `quota.probe` | `usage` (provider usage API), `balance` (account balance), `none` (local ledger only). |
| `quota.rolling` / `weekly` / `monthly` | Window limits in the configured unit. OpenCode Go defaults to 20% / 50% / 100% of a monthly plan. |
| `quota.refresh_secs` | Probe interval. Default 60s for `usage`, 300s for `balance`. |

When no probe is available the router falls back to a local ledger (cost accumulated from
token counts divided by the plan limit). That only affects the precision of the off-peak
"is this quota going to waste" test, never availability.

## Router policy

| Field | Default | Description |
| --- | --- | --- |
| `mode` | `auto` | `auto` decides from the clock and quota; `plans` always drains prepaid first; `fallback` always spends cash first. |
| `peak_windows` | `Mon-Fri 01:00-04:00, 06:00-10:00 UTC` | Syntax `<day> <HH:MM-HH:MM>[, ...] [timezone]`, groups separated by `;`. |
| `idle_prefer` | `surplus_first` | `surplus_first` keeps using prepaid quota that would otherwise expire, deferring to half-price cash only when it is tight. `plans` and `fallback` are hard policies. |
| `surplus_max_pct` | 80 | Any window at or above this percentage counts as tight. |
| `surplus_projection` | true | Extrapolate linearly along the window; a plan that will not be used up still counts as surplus. |
| `exhaust_at_pct` | 99 | At this percentage the plan stops being preferred. |
| `session_affinity` | true | Keep one conversation on one endpoint to preserve upstream session and cache semantics. |
| `session_affinity_ttl_secs` | 1800 | Binding expiry. |
| `session_fallback` | `process` | Session id when the client sends none: `process` (steady caching) or `per-request`. |
| `session_headers` | see template | Headers the session id is read from, in order. |
| `user_agent` | `ar-ocg-router/<version> (coding-agent)` | OpenCode Go classifies traffic by user agent; do not water it down. |
| `skip_after_failures` / `skip_secs` | 3 / 300 | Skip an endpoint entirely after N consecutive failures, for this long. |
| `attempt_budget_secs` | 120 | Total time allowed for hopping between endpoints. |
| `cooldown_secs` / `auth_cooldown_secs` / `server_error_cooldown_secs` | 30 / 600 / 20 | Rest after 429 / 401-403 / 5xx. |
| `retry_on_model_error` | true | Treat "model not found" as a failover trigger. Model ids differ between providers, so this matters. |
| `inject_stream_usage` | false | Add `stream_options.include_usage` to streaming chat requests. |
| `quota_refresh_secs` | 60 | Background quota poll interval (an endpoint's own value overrides it). |

## Compatibility layer

Zero rewriting by default: apart from `model`, the request body is forwarded verbatim and
protocol headers are passed through. Enable only what an upstream actually needs.

| Field | Description |
| --- | --- |
| `developer_role_to_system` | Rewrite the `developer` role. DeepSeek-style APIs reject it. |
| `max_completion_tokens_to_max_tokens` | Chat-only rename. |
| `drop_params` | Body fields dropped for every endpoint, e.g. `store`, `service_tier`. |
| `forward_headers` | Extra client headers forwarded verbatim. |

Everything else (`tools`, `tool_choice`, `response_format`, `temperature`, `thinking`,
`reasoning_effort`, beta headers, ...) is passed through untouched rather than allowlisted,
so new parameters are never silently swallowed.

## Server and log

| Field | Default | Description |
| --- | --- | --- |
| `server.host` / `port` | `127.0.0.1` / 8787 | Listening address. Keep it on loopback unless you also configure `client_keys`. |
| `server.client_keys` | empty | When set, clients must send `Authorization: Bearer <key>`. The admin API enforces it too. `/health` stays open. |
| `server.max_connections` | 64 | Excess connections get 503. |
| `server.max_body_bytes` | 64 MiB | Larger bodies get 413. |
| `log.level` | `info` | `error` < `warn` < `info` < `debug` < `trace`. Use `debug` when diagnosing routing. |
| `log.file` | none | Also append to a file. Relative paths resolve next to the binary, because a service starts with the SCM working directory. |
