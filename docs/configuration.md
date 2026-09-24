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
| `quota.unit` | `usd` / `rmb` / `tokens` / `none`. Token-metered plans need no price table. |
| `quota.probe` | `usage` (provider usage API), `balance` (account balance), `none` (local ledger only). |
| `quota.rolling` / `weekly` / `monthly` | Window limits in the configured unit. OpenCode Go's built-in default is the **non-promotional** grant: 3 / 7.5 / 15 USD (the 20% / 50% / 100% shape of a $15 month). A promotion raises the grant, not its shape, so write the promotional numbers in the config next to the endpoint — a compiled-in default would silently misreport quota the day a promotion starts or ends. |
| `quota.cycle_day` | The **reset day** (1–31) of a subscription plan, i.e. the day it was bought. Unset or `0` = no reset, and the monthly window is a plain 30-day sliding sum (what pay-as-you-go endpoints want). See below. |
| `quota.refresh_secs` | Probe interval. Default 60s for `usage`, 300s for `balance`. |

When no probe is available the router falls back to a local ledger (accumulated cost or tokens
as a share of the plan limit). That only affects the precision of the off-peak
"is this quota going to waste" test, never availability.

The ledger stores **token counts, never money**. Money is a function of the rates in force at the
moment of reading, so folding rates into the samples would freeze whatever price was configured
when each request happened: correcting a price could then never fix the history that price
produced. Keeping the counts means corrected rates apply everywhere at once, backwards included,
and two models billed differently can be added up by pricing each with its own rates.

### Endpoints that share a plan

Two endpoints configured with the same provider **and the same key** are two models on one plan,
so they consume one allowance. They share the quota state - readings, calibration, and the token
ledger - while keeping their own statistics, cooldowns and in-flight marks, because those describe
the endpoint rather than the plan. Rates stay per endpoint: the provider charges each model
differently, and the ledger prices each model's tokens with its own.

This means a plan is **calibrated once** for all of its models, and the console reports the same
allowed percentage for each of them rather than showing the same consumption under two names.

The key is part of the identity on purpose: the same provider reached with two different keys is
two plans, and merging them would invent an allowance that neither key has. An endpoint with no key
has nothing to group on and keeps its own allowance.

### Subscription cycles, and why a reset day is needed

A pay-as-you-go account has no cycle: what it spends keeps accumulating. A subscription is
different - the provider resets the allowance to full at the start of each billing cycle, and the
reset day is the **day of purchase**. The provider's own rule is "effective the day you buy,
expiring the next month on the same day at 23:59:59": buy on the 4th and it expires on the 4th;
buy on Jan 31 and it expires Feb 28 (a month without that day uses its last day).

A 30-day sliding window **cannot express this**. On the day the provider resets, the sliding window
still holds the entire previous month, so the local percentage starts high and only becomes
self-consistent a month later.

```yaml
quota: { unit: rmb, monthly: 200, probe: none, cycle_day: 26 }
```

With `cycle_day` set, "used this cycle" means everything accumulated since the cycle began. The
boundary is derived from the anchor day, so the reset instant is **knowable locally** - something a
sliding window never provided. That is why the console can now show a reset countdown and a
projection that agree with the provider's own screen.

The ledger views report that same window: the amount beside a percentage is computed from the cycle
start rather than from a trailing 30 days. A trailing sum would still hold the previous cycle on
the day the provider resets, so the amount and the percentage would disagree at exactly the moment
an operator looks - a 0% window beside the whole of last cycle's spend. The lifetime total is
unaffected by the cycle, so nothing is forgotten.

### The 5-hour and weekly windows open on use, not on a clock

The monthly window follows the calendar; the two shorter ones do not. A window ends when its period
elapses, and the next one opens at the **first request that arrives after that** - so an idle stretch
belongs to no window at all, and a window's span is not knowable in advance.

A fixed grid rolled forward from a countdown read off the console cannot express this: it drifts the
moment you pause and come back. The provider states the rule itself whenever it refuses a request
over quota - "it will reset at ..." - and that instant is only reproducible when the window is
anchored on use.

Two consequences:

- The amount beside a window is the ledger's sum since that window opened, so it lines up with the
  console's percentage with no correction - provided the traffic inside the window really did go
  through the router.
- Between a window's end and the next request, no period is running: the window reads zero and shows
  no reset, rather than a countdown to a boundary that has already passed.

`CD 5h` and `CD week` (the console's reset countdowns) are still applied the moment they arrive, but
they now correct the window's *start* - the countdown minus one period - instead of feeding a grid.
Sending one leaves the other window alone; zero clears it.

### Calibration: deriving the real allowance from console readings

The router only sees traffic **it forwarded itself**, and a plan without a usage API starts its
local ledger from zero while the provider's console already shows a consumed percentage. The
console's calibration wizard closes that gap with **two readings**:

1. Copy the three window percentages from the provider's console;
2. use the endpoint normally for a while, so the router accumulates consumption;
3. copy the percentages again.

The percentage-point movement between the two readings corresponds to the consumption the router
actually forwarded. That ratio, anchored on the plan price (the monthly limit), yields each
window's real total allowance and the true per-token value of input, cache and output - which is
written back into `prices`, so the local ledger reads true money instead of ratios.

The derivation is reused across billing cycles until the plan or the coefficients change. A third
reading verifies it: the gap between the prediction and the console is non-router traffic or a
coefficient error. Both readings must fall inside one subscription cycle, and the monthly window
must move at least 0.3 percentage points (below that the console's rounding would swamp the
derivation, and it is refused).

The progress shown between the two readings ("consumed since the baseline") is read from the
usage ledger, so a statistics reset cannot distort it; conversely, a reset is refused while a
calibration is recording, and when it does run it drops an unfinished baseline. A 5-hour or
weekly window whose readings cross its own reset, or move less than 0.3 points, is skipped
rather than derived from broken numbers, and the response says so. The end-of-cycle projection
is only shown when the ledger has observed the cycle from its start - a ledger with half a
cycle of history cannot extrapolate and reports the measured percentage alone.

### The level a window is measured from

A reading gives each window a level, and the window is then read as `level + what the ledger
recorded since`. The level is stored as **money** - computed once, when the reading is taken
(`total x percentage / 100`) - rather than as the percentage it came from. Money is what the
allowance is measured in, so no read converts anything; a stored percentage would instead be
re-multiplied by the plan total on every read, which lets a later correction to the plan silently
rescale a level that describes an instant already past.

The same reading has a second, independent use: **Sync remaining quota** re-anchors the level on
what the console shows now. It moves the level and nothing else, because the corrected rates and the
window totals are properties of the plan rather than of the current level - a resync never costs you
a derivation. Reach for it when the allowance moved for reasons the router never saw, such as the
same credential being used elsewhere.

A third action derives a window total from a **single** reading. Two readings are normally needed
because the money level at the first one is unknown; with the window tracked from the request that
opened it, the level is known, so `total = money forwarded inside the window / reading` pins the total
outright. Anything sent for a window that is closed, or whose reading is zero, is refused rather than
guessed. It is accurate only when everything inside that window went through the router - traffic that
bypassed it makes the total read low, which is the safe direction, since the window will then appear
more used than it is.

Editing `quota.monthly` is the one change that does retire a derivation, because both window totals
are linear in it: after the change they are wrong by that ratio while the stored level stays where it
was spent, and reconciling the two would mean rewriting the past. The derivation is **set aside**
rather than destroyed - the ledger, which needs no totals, speaks until a fresh calibration replaces
it, the console says so, and putting the plan value back restores it. Editing the rates or the
promotion multiplier does **not** touch the totals (they cancel out of the derivation) and only
changes the price of requests from that moment on.

### Per-token prices, and why there are no "deduction coefficients"

An endpoint may declare what it charges:

```yaml
prices: { currency: CNY, input: 1, output: 4, cached_input: 0.02, peak_multiplier: 2, promo_multiplier: 1 }
```

Priority is **the upstream's own reported cost > these rates > nothing** (no money is recorded
rather than a guessed one). `currency` is a label: the router never converts, because two
merchants' prices are not a currency pair and no exchange rate was ever quoted between them.

`peak_multiplier` applies during the peak window. `promo_multiplier` is a whole-plan factor over all
three rates, for a promotion or a plan-wide adjustment. They are separate because they answer
different questions: peak pricing is a property of the clock, a promotion is a property of the plan
and applies whenever a request happens. Absent or unusable means 1.0 for both.

Changing any rate affects **later** requests only. Every ledger sample records what that request cost
at the rates in force when it was forwarded, and that figure is frozen - a corrected rate or a new
promotion cannot rewrite a period that has already been paid for. That is what makes it safe to apply
a promotion mid-cycle.

Providers bill different token classes at different rates, and that is expressed **here**, in the
three rates — a cached input token costs 0.02 while a fresh one costs 1. The same three rates feed
the local ledger, so a plan's remaining budget is computed in the same money the provider charges.
That is why there is no separate set of "deduction coefficients": one price list per endpoint
already covers cache-hit/cache-miss/output differences, and a plan whose grant is not money is
expressed with `quota.unit: tokens` instead. A coefficient table would be a second way to say the
same thing, and the two could disagree.

Reference rates (provider / model → rates), verified on the dates shown:

| Provider | Model | Currency | input | cached_input | output | peak |
| --- | --- | --- | --- | --- | --- | --- |
| DeepSeek official | deepseek-flash | CNY | 1 | 0.02 | 4 | ×2 |
| DeepSeek official | deepseek-v4-pro | CNY | 4.5 | 0.15 | 13.5 | ×2 |
| OpenCode Go | deepseek-flash | USD | 0.15 | 0.003 | 0.60 | — |
| OpenCode Go | deepseek-v4-pro | USD | 0.66 | 0.022 | 1.98 | — |
| OpenCode Go | glm-5.3-flash | USD | 0.15 | 0.015 | 0.50 | — |
| OpenCode Go | glm-5.3 | USD | 1.40 | 0.26 | 4.40 | — |
| OpenCode Go | kimi-k3 | USD | 3.00 | 0.30 | 15.00 | — |
| OpenCode Go | qwen3.8-max | USD | 2.00 | 0.25 | 6.00 | — |
| Volcengine Ark | (any) | CNY | measure | yours | yourself | — |

DeepSeek's peak windows are Beijing time Mon–Fri 09:00–12:00 and 14:00–18:00, which is exactly the
UTC `01:00-04:00, 06:00-10:00` in the default `peak_windows`. Ark publishes per-region rates that
change often and does not report a cost per request, so fill those in from your console.

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
| `rate_limit_retries` / `rate_retry_delay_ms` | 1 / 700 | How many times a *transient* rate limit (a 429 that is not a quota exhaustion) is retried on the **same** endpoint, and how long to wait first. That answer reads "requests are too frequent, wait a short moment", and hopping providers instead hands the conversation to another one, which may refuse what the first was willing to take. `0` restores failover on the first 429. |
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
| `log.dump_error_request` (alias `dump_request_on_4xx`) | false | When an upstream answers 4xx, log a **redacted shape** of the request that was sent: field names, value kinds, numbers verbatim, and - for `input` / `messages` - a tally of the items plus the last eight of them (`reasoning(text,summary,encrypted)`, `message(role)`, ...). No prompt text ever appears. This is how "the upstream rejected the conversation" is answered without guessing whether a reasoning item carried the text the upstream demands. |
