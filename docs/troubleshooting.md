# Troubleshooting

[**English**](troubleshooting.md) · [中文](troubleshooting-zh.md)

## Diagnosing routing

Start with the console's **Candidate order preview** on the Policy page, or
`ar-ocg-router --plan`, which prints the decision for the current moment without sending
traffic. `GET /router/stats` shows why each endpoint was skipped.

Run the router with `--log-level debug` to see the full candidate list and the skip reasons
for every request.

## The client gets 503 `no_account_available`

No endpoint could serve the request. Check `plan.reason` and `plan.skipped` in the debug log:

| Reason | Fix |
| --- | --- |
| `no <mode> support` | The endpoint's `mode` does not cover the protocol being used (`/v1/messages` needs `anthropic-messages`) |
| `cooldown Ns` | A recent failure; wait or fix the underlying error |
| `quota exhausted` | The plan hit `exhaust_at_pct`; raise the limit or wait for the window to reset |
| `rule not matched` | A `fallback` endpoint whose `rule` does not currently apply; add `always` |
| `disabled` | `enabled: false` on that endpoint |

## The client gets 502 `all_accounts_failed`

Every candidate failed; the message lists each one with its status. Common causes:

- **401 / 403** - wrong key, or the wrong product prefix. OpenCode Go must be
  `https://opencode.ai/zen/go/v1`, not `/zen/v1`.
- **400 MissingSessionID** - an OpenCode Go endpoint without `inject_session: true`.
- **429 or quota wording** - the plan is exhausted; the endpoint is cooled down.
- **Model errors** - the endpoint's `model` is not served by that merchant. Model ids differ
  per merchant; check the [model library](model-library.md).

## `--selftest` reports transport errors

`os error 10013` / `WSAEACCES` / "forbidden by its access permissions" means outbound
connections are blocked by policy, not by the upstream. Allow the executable through the
firewall (see [Deployment](deployment.md)). A service running in session 0 never raises the
interactive prompt, so this must be allowed explicitly. Some machines key the policy to the
exact binary, so every rebuild may need the rule again.

## Requests are slower than expected

The retry chain hides misconfiguration: if the first endpoint cannot serve a request, the
client only sees a slower response. Look for `endpoint <name> disabled for Ns after M
consecutive failures` in the log. The router skips such endpoints entirely rather than paying a
doomed attempt on every request, and un-skips them after `skip_secs`.

## Quota shows `source=local` or a stale reading

The provider usage endpoint is unavailable or undocumented and changed. The local ledger
(cost accumulated from token counts) is used instead, which only affects the precision of the
off-peak surplus test. `quota.last_error` records the last probe failure.

## Config reloads are not picked up

The file is watched every 3 s, so a reload should be automatic. If it is not, the parse
failed: `--check` prints the error and `GET /router/stats` carries the warnings array. A
typo in a top-level section name (for example `opencodego:` instead of `plans:`) is reported
as `unknown top-level section` and otherwise ignored.

## The dashboard numbers look wrong

- `savings.opencodego_saved_usd` is what the traffic routed through prepaid endpoints *would*
  have cost in cash at that moment's peak/off-peak rate - an estimate, not an invoice.
- `fallback_spent_usd` is what was actually charged to cash accounts.
- `cold_starts` counts successful requests whose prompt was not served from upstream cache,
  which is the number to watch when a session is hopping between endpoints.

## Uninstalling

`service.ps1 uninstall` (Windows) or `install.sh uninstall` (Linux) removes the service and
the binary but keeps the configuration and logs. The state file
`ar-ocg-router.state.json` is advisory and safe to delete; an unreadable or wrong-version
file is moved aside as `.corrupt` and rebuilt.
