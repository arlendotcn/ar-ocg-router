/** Wire types for the router's admin API. Mirrors src/api.rs. */

export type QuotaView = {
  percent: number;
  projected_percent: number;
  used: number;
  limit: number;
  resets_at: string | null;
  has_data: boolean;
};

export type Quota = {
  source: "remote" | "local" | "unknown";
  /**
   * What the windows below are counted in. The server reports the currency it actually accounted
   * in, so this is usually `"usd"`/`"rmb"`/`"tokens"`, or `"none"` when nothing is metered. It is a
   * currency-or-measure label, not a closed set: render it, do not branch on a fixed list.
   */
  unit: string;
  stale: boolean;
  fetched_at: string | null;
  exhausted: boolean;
  surplus: boolean;
  exhausted_until: string | null;
  rolling: QuotaView;
  weekly: QuotaView;
  monthly: QuotaView;
  local_ledger: {
    total: number;
    rolling: number;
    weekly: number;
    monthly: number;
  };
  last_error: string | null;
};

export type AccountStats = {
  requests: number;
  successes: number;
  errors: number;
  stream_requests: number;
  prompt_tokens: number;
  completion_tokens: number;
  cached_tokens: number;
  /** Unit of this endpoint's money. Never converted; a label only. */
  currency: string;
  cost: number;
  saved: number;
  avg_latency_ms: number;
  last_used: string | null;
  last_error: string | null;
};

export type Account = {
  name: string;
  kind: "plans" | "fallback";
  order: number;
  provider: "opencode-go" | "deepseek" | "generic";
  identity?: string;
  model?: string;
  url: string;
  modes: string[];
  rules: string[];
  key: string;
  available: boolean;
  cooldown_secs_left: number;
  cooldown_reason: string | null;
  balance: { total: number; currency: string; is_available: boolean; fetched_at: string } | null;
  quota: Quota;
  stats: AccountStats;
  health?: {
    failure_streak: number;
    total_errors: number;
    skipped: boolean;
    skip_secs_left: number;
    last_reason: string | null;
  };
  enabled?: boolean;
  managed?: boolean;
  extra_headers?: Record<string, string>;
  drop_params?: string[];
  inject_session?: boolean;
  no_error_fallback?: boolean;
  quota_cfg?: QuotaCfg;
};

export type Stats = {
  router: {
    version: string;
    pid: number;
    uptime_secs: number;
    now: string;
    fake_now: boolean;
    peak: boolean;
    next_change: string;
    seconds_to_change: number;
    peak_windows: string;
    mode: "auto" | "plans" | "fallback";
    idle_prefer: "surplus_first" | "plans" | "fallback";
    surplus_max_pct: number;
    surplus_projection: boolean;
    config_path: string;
  };
  counters: {
    requests: number;
    proxied: number;
    errors: number;
    streams: number;
    retries: number;
    plans_used: number;
    cold_starts: number;
    cash_used: number;
    bytes_out: number;
  };
  /** Grouped by currency: adding a dollar figure to a yuan one yields money in no currency. */
  savings: Record<string, { saved: number; spent: number }>;
  accounts: Account[];
  warnings: string[];
  ui?: { managed: boolean; path_locked: boolean };
};

export type QuotaCfg = {
  unit: "usd" | "rmb" | "tokens" | "none";
  rolling: number;
  weekly: number;
  monthly: number;
  probe: "usage" | "balance" | "none";
  refresh_secs: number;
  /**
   * Day of the month the subscription's allowance restarts (1-31, a short month uses its last day).
   * 0 = no cycle: the monthly window is a plain 30-day sliding sum. A plan bought on the 26th has
   * its whole allowance reset on the 26th, which a sliding sum cannot express.
   */
  cycle_day: number;
  /** Percentage the provider's console showed as of `used_at`; 0 = no calibration. */
  used_percent: number;
  /** Unix seconds `used_percent` was read at. 0 = unset (the calibration is ignored). */
  used_at: number;
};

export type EndpointCfg = {
  name: string;
  kind: "plans" | "fallback";
  /** Derived from the row position in its section; written by the server, never edited here. */
  order: number;
  provider: "opencodego" | "deepseek" | "generic";
  url: string;
  key: string;
  model: string;
  modes: string[];
  rules: string[];
  no_error_fallback: boolean;
  /** Ceiling for the client's `max_output_tokens`: only requests asking for MORE than this are
   *  rewritten down to it. 0 (or absent) means the value is never touched. */
  max_output_tokens_limit: number;
  /** What this endpoint charges, in the provider's own currency. All zero = no money recorded. */
  prices: { currency: string; input: number; output: number; cached_input: number; peak_multiplier: number };
  inject_session: boolean;
  headers: Record<string, string>;
  drop_params: string[];
  quota: QuotaCfg;
  enabled: boolean;
  /** Set only on a save that renames an endpoint: the name its credential is stored under.
   *  The server uses it to resolve the "********" placeholder; it is never sent back. */
  renamed_from?: string;
};

export type ServerCfg = {
  host: string;
  port: number;
  max_connections: number;
  client_keys: string[];
  max_body_bytes: number;
  idle_timeout_secs: number;
  read_timeout_secs: number;
  stream: boolean;
};

export type LogCfg = { level: string; file: string; quiet: boolean };

export type RouterCfg = {
  mode: "auto" | "plans" | "fallback";
  peak_windows: string;
  idle_prefer: "surplus_first" | "plans" | "fallback";
  surplus_max_pct: number;
  surplus_projection: boolean;
  exhaust_at_pct: number;
  quota_refresh_secs: number;
  cooldown_secs: number;
  auth_cooldown_secs: number;
  server_error_cooldown_secs: number;
  retry_on_model_error: boolean;
  skip_after_failures: number;
  skip_secs: number;
  attempt_budget_secs: number;
  session_affinity: boolean;
  session_affinity_ttl_secs: number;
  session_fallback: "process" | "per-request";
  inject_stream_usage: boolean;
  session_headers: string[];
  user_agent: string;
  request_timeout_secs: number;
  stream_idle_timeout_secs: number;
};

export type CompatCfg = {
  developer_role_to_system: boolean;
  max_completion_tokens_to_max_tokens: boolean;
  drop_params: string[];
  forward_headers: string[];
};

export type ConfigDoc = {
  server: ServerCfg;
  log: LogCfg;
  router: RouterCfg;
  compat: CompatCfg;
  endpoints: EndpointCfg[];
  warnings: string[];
};

export type PlanPreview = {
  now: string;
  /** Always "live": the preview reflects the *running* config, not an unsaved draft. */
  source: string;
  peak: boolean;
  candidates: { name: string; kind: string; model: string; url: string }[];
  skipped: { name: string; reason: string }[];
  preferred: string;
  reason: string;
  plans_surplus: boolean;
  plans_available: boolean;
};

export type TestResult = {
  name: string;
  key_ok: boolean;
  models: { ok: boolean; count: number; ids: string[]; error: string | null };
  quota: { kind: string; ok: boolean; detail: string } | null;
  chat: { ok: boolean; status: number; model: string; latency_ms: number; error: string | null };
  suggestions: string[];
};

export type EndpointModels = {
  name: string;
  ok: boolean;
  count: number;
  models: string[];
  current: string;
  error?: string;
};

export type ModelEntry = {
  id: string;
  label: string;
  aliases: string[];
  context_tokens: number;
  max_output_tokens: number;
  reasoning_levels: string[];
  input_modalities: string[];
  output_modalities: string[];
  notes: string;
  tags: string[];
};

export type ModelLibrary = {
  version: number;
  models: ModelEntry[];
};
