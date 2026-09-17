//! Small utilities: time, randomness, formatting, JSON helpers.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// Current unix time in seconds.
///
/// AR_OCG_ROUTER_FAKE_NOW may pin "now" for deterministic peak/off-peak testing.
/// Accepted forms: unix seconds (1789551700) or YYYY-MM-DDTHH:MM:SSZ.
pub fn now_secs() -> i64 {
    static FAKE: OnceLock<Option<i64>> = OnceLock::new();
    let fake = FAKE.get_or_init(|| {
        std::env::var("AR_OCG_ROUTER_FAKE_NOW")
            .ok()
            .and_then(|s| parse_fake_now(s.trim()))
    });
    if let Some(v) = fake {
        return *v;
    }
    real_now_secs()
}

pub fn has_fake_now() -> bool {
    std::env::var("AR_OCG_ROUTER_FAKE_NOW")
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

pub fn real_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// YYYY-MM-DDTHH:MM:SSZ or bare unix seconds.
fn parse_fake_now(s: &str) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    if s.chars().all(|c| c.is_ascii_digit()) {
        return s.parse::<i64>().ok();
    }
    if s.len() < 19 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let min: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;
    Some(crate::timeutil::days_from_civil(year, month, day) * 86400 + hour * 3600 + min * 60 + sec)
}

// ---------------------------------------------------------------- randomness

static RNG_STATE: AtomicU64 = AtomicU64::new(0);

fn rng_next() -> u64 {
    let mut cur = RNG_STATE.load(Ordering::Relaxed);
    if cur == 0 {
        let seed = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15))
            ^ ((std::process::id() as u64) << 32)
            ^ 0x2545_F491_4F6C_DD1D;
        cur = seed | 1;
    }
    // xorshift64*
    let mut x = cur;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    RNG_STATE.store(x, Ordering::Relaxed);
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

pub fn rand_u64() -> u64 {
    rng_next()
}

/// Uniform-ish f64 in [0, 1).
pub fn rand_f64() -> f64 {
    (rng_next() >> 11) as f64 / (1u64 << 53) as f64
}

pub fn rand_hex(len: usize) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(len);
    while out.len() < len {
        let v = rng_next();
        for i in 0..8 {
            if out.len() >= len {
                break;
            }
            let byte = ((v >> (i * 8)) & 0xff) as usize;
            out.push(HEX[byte & 0x0f] as char);
        }
    }
    out
}

/// Session id used for OpenCode Go's x-opencode-session routing header.
pub fn gen_session_id() -> String {
    format!("ses_{}", rand_hex(24))
}

/// UUID v4 shaped identifier (internal request ids).
pub fn gen_uuid() -> String {
    let a = rand_hex(8);
    let b = rand_hex(4);
    let c = format!("4{}", rand_hex(3));
    let variant = "89ab".chars().nth((rand_u64() % 4) as usize).unwrap_or('8');
    let d = format!("{}{}", variant, rand_hex(3));
    let e = rand_hex(12);
    format!("{}-{}-{}-{}-{}", a, b, c, d, e)
}

// ---------------------------------------------------------------- formatting

pub fn fmt_usd(v: f64) -> String {
    if v == 0.0 {
        "0".to_string()
    } else if v.abs() < 0.01 {
        format!("{:.6}", v)
    } else {
        format!("{:.4}", v)
    }
}

pub fn fmt_secs(v: i64) -> String {
    if v <= 0 {
        return "-".to_string();
    }
    let d = v / 86400;
    let h = (v % 86400) / 3600;
    let m = (v % 3600) / 60;
    let s = v % 60;
    if d > 0 {
        format!("{}d{}h{}m", d, h, m)
    } else if h > 0 {
        format!("{}h{}m{}s", h, m, s)
    } else if m > 0 {
        format!("{}m{}s", m, s)
    } else {
        format!("{}s", s)
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut idx = max;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    format!("{}...(+{}B)", &s[..idx], s.len() - idx)
}

/// Show only a stable prefix/suffix of a secret.
pub fn redact(secret: &str) -> String {
    let n = secret.len();
    if n <= 10 {
        return "***".to_string();
    }
    format!("{}...{}***", &secret[..6], &secret[n - 4..])
}

// ---------------------------------------------------------------- json helpers

pub fn json_path<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    Some(cur)
}

pub fn json_u64(v: &Value, path: &[&str]) -> Option<u64> {
    json_path(v, path).and_then(|x| x.as_u64())
}

pub fn json_f64(v: &Value, path: &[&str]) -> Option<f64> {
    match json_path(v, path)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

pub fn json_str<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    json_path(v, path).and_then(|x| x.as_str())
}

pub fn json_compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string())
}

/// Short, non-reversible fingerprint of an api key, used to tell same-provider endpoints apart
/// in auto-generated names (e.g. generic-glm-5.3-flash-a1b2).
pub fn key_fingerprint(secret: &str) -> String {
    let h = hash64(secret);
    format!("{:04x}", (h & 0xffff) as u16)
}

/// Stable 64-bit hash (FNV-1a) used for deterministic jitter.
pub fn hash64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
