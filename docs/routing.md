# Routing

[**English**](routing.md) · [中文](routing-zh.md)

Each request is resolved to an ordered list of candidate endpoints; the router tries them in
order until one answers successfully.

## Candidate order

Endpoints are grouped into **buckets** whose order depends on the clock and the quota state:

| Window | Plan quota | Bucket order |
| --- | --- | --- |
| Peak (cash is expensive) | not exhausted | `plans` → `fallback` |
| Peak | all exhausted | `fallback` → (`plans` still last resort) |
| Off-peak (cash is half price) | `surplus_first` | `plans-surplus` → `fallback` → `plans-tight` |
| Off-peak | `plans` / `fallback` | that side first |

Within a bucket: ascending `order`. That value is derived from the position of the entry, so
the sequence you arrange by dragging rows in the console is the sequence used here.

The second pass adds everything still usable as a last resort, including cooling-down
endpoints, unless they opted out.

## What "surplus" means

A plan's quota is *surplus* when it would still be left over at the end of its window, so
spending it now costs nothing that would have been used later. Two tests apply:

1. any window at or above `surplus_max_pct` makes it **tight**;
2. with `surplus_projection`, usage is extrapolated linearly along the window - a plan
   heading for less than 100% also counts as surplus.

## Failover

| Upstream response | Action |
| --- | --- |
| 401 / 403 | Cool the endpoint for `auth_cooldown_secs`, try the next |
| 429, or a quota-shaped error | Cool it, and mark plan quota exhausted, try the next |
| 400 / 404 / 422 whose message looks like a model problem | Try the next (`retry_on_model_error`) |
| 400 / 404 / 422 for anything else | Return to the client unchanged - a parameter error must not be masked |
| 5xx, timeout, connection failure | Short cooldown, try the next |
| Every candidate failed | `502` with the per-endpoint failure detail |

A single request tries at most `max_attempts` endpoints and stops after
`attempt_budget_secs`.

## Pinning an endpoint

The client's model string carries no routing meaning, with one exception: it can pin a single
endpoint.

| Client sends | Effect |
| --- | --- |
| `ar-ocg-router` | Normal routing |
| `deepseek-flash` (anything else) | Also normal routing; the value is only logged |
| `go-dsf/x` | Pins the endpoint named `go-dsf` |
| `generic/model-c` | Pins by provider + model |

A pin bypasses policy, skip state and the disable switch: it is a deliberate, one-off request.

## Disabling an endpoint

`enabled: false` removes the endpoint from every candidate list, including the failover pass.
It is a real off switch, not a lower priority. To keep an endpoint but use it only sometimes,
change its `rule` instead.

Re-enabling restores the natural default for its side: plan endpoints become eligible again,
cash endpoints return to `[offpeak, quota_low]`.

## Streaming

Streaming (SSE) responses are forwarded chunk by chunk, unmodified and unbuffered, while a
side-channel parser reads the trailing usage block for accounting. The retry chain only
applies before the first byte is written; once the head is on the wire the response is
committed.
