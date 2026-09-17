# Upstream notes

[**English**](upstream.md) · [中文](upstream-zh.md)

Provider behaviour below was verified against the live APIs. Merchants control model lists,
quota endpoints and terms, and change them without notice - treat this as a starting point,
not a contract.

## Peak and off-peak

| Item | Finding |
| --- | --- |
| Peak | Mon-Fri **01:00-04:00** and **06:00-10:00 UTC** |
| Off-peak | Everything else, including the whole weekend, at 50% of the peak price |
| Models | DeepSeek V4.1 Flash, V4 Pro, V4 Flash, V4 Flash Vision (exp) |

Both DeepSeek official and OpenCode Go share this window.

## Endpoints and protocol

| Provider | Base URL | Protocols |
| --- | --- | --- |
| DeepSeek official | `https://api.deepseek.com` | `/chat/completions`, `/responses` |
| OpenCode Go | `https://opencode.ai/zen/go/v1` | `/chat/completions`, `/responses` |

The Go prefix is `/zen/go/v1`, **not** `/zen/v1` (the latter is the metered Zen product).

## OpenCode Go requirements

1. Every request must carry **`x-opencode-session`**, otherwise the API answers
   `400 {"type":"error","error":{"type":"MissingSessionID",...}}`. The router finds a session id
   in the client's headers or body and otherwise uses a process-level fallback, so requests
   are always serviceable and session routing stays as stable as possible.
2. The client must send its own **User-Agent**; Go classifies "coding agent traffic" by it.

## Quota windows

OpenCode Go exposes three independent windows (fractions of a monthly cap):

| Window | Share | On a $60 plan |
| --- | --- | --- |
| Rolling 5 hours | 20% | $12 |
| Weekly | 50% | $30 |
| Monthly | 100% | $60 |

`GET https://opencode.ai/zen/go/v1/usage` returns them. This endpoint is **undocumented** and
carries no compatibility promise. `GET .../quota` is a 404 and does not exist.
DeepSeek official has no quota API, only `GET https://api.deepseek.com/user/balance`.

## The economics, which drive the whole policy

DeepSeek models cost the same on OpenCode Go as on the official API, and both double during
peak hours:

| Model | Off-peak (in / out / cached) | Peak |
| --- | --- | --- |
| DeepSeek V4.1 Flash | $0.15 / $0.60 / $0.003 | $0.30 / $1.20 / $0.006 |
| DeepSeek V4 Pro | $0.66 / $1.98 / $0.022 | $1.32 / $3.96 / $0.044 |

So one token of quota equals one token's cash price at all hours: "use Go at peak, official
off-peak" is cash-neutral. The only thing that saves money is **not leaving quota unused**,
which is exactly what `idle_prefer: surplus_first` implements.

## Model catalogs are advisory

`GET <base>/models` cannot be used for admission control:

| Provider | Result | Nature |
| --- | --- | --- |
| OpenCode Go | 200, ~37 ids | Usable catalog |
| DeepSeek official | 200, 2 ids | Trustworthy |
| Some domestic plans | 200, hundreds | The whole account catalog; entries that 404, and usable ids that are absent |

The same model also has different ids per merchant, which is why the router sends each
endpoint's `model` verbatim and why the [model library](model-library.md) exists to record the
mapping.

## Compliance

Some plans (notably Tencent's Token Plan) restrict use to supported coding tools and forbid
automation scripts, custom application backends and non-interactive batch calls. This router
is a local proxy; whether that counts as a backend is your call. Do not run batch workloads
against those plans.
