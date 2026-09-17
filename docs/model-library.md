# Model library

[**English**](model-library.md) · [中文](model-library-zh.md)

An endpoint is one merchant plus one model, and the same model is sold under different ids by
different merchants with different real limits. The model library records what is known about
each model so the console can show the limits next to an endpoint and offer a model picker
instead of a typing test.

It is stored as `models.library.json` **next to** `config.yaml`, not inside it. That matters:
the console regenerates `config.yaml` on every save, so anything embedded there would be
rewritten or lost. There is a test pinning this behaviour.

The library is advisory. The router never routes on it; the `model` field on an endpoint is
what actually gets sent.

## Fields

| Field | Meaning |
| --- | --- |
| `id` | Canonical name used inside the gateway. |
| `label` | Human-readable name; falls back to the id. |
| `aliases` | The ids different merchants use for the same model, e.g. `deepseek-flash` on DeepSeek official versus `deepseek-v4.1-flash` on a domestic plan. Used to fold a live catalog into one entry. |
| `context_tokens` | Maximum tokens accepted in one call (input plus output). |
| `max_output_tokens` | Most tokens a single reply can generate. |
| `reasoning_levels` | The reasoning/thinking levels the model accepts, weakest first. Include `none` when thinking can be turned off. |
| `input_modalities` / `output_modalities` | `text`, `image`, `audio`, `video`. |
| `notes` | Anything easy to forget: measured pitfalls, allowlist limits, billing quirks. |
| `tags` | For filtering, e.g. `deepseek`, `vision`, `long-context`, `plan`. |

A value of `0` means "unknown" and renders as an em dash rather than a fake number.

## The model picker

The Model ID field in the endpoint editor has a **Fetch list** button that calls
`GET /api/endpoints/<name>/models` on the real endpoint. Results are ordered by the library:
known models first, largest context window first, then unknown ids alphabetically. Each row
shows the context size and any non-text modalities. Selecting a row fills the field, and the
resolved spec is shown underneath.

The catalog is a suggestion source, never a constraint - upstreams list ids they cannot serve
and omit ones they can (see [Upstream notes](upstream.md)), so free text always remains
available.

## Defaults

Ten models ship compiled into the binary, covering the DeepSeek family and the common plan
models, so a fresh install shows useful limits immediately. Saving from the console writes
the file; delete it to fall back to the built-in set.
