//! UTC civil-time helpers and peak/off-peak window evaluation.
//!
//! DeepSeek official **and** OpenCode Go both bill DeepSeek models with the same
//! schedule: peak = 01:00-04:00 and 06:00-10:00 **UTC**, Monday-Friday. Everything
//! else (including the whole weekend) is off-peak (half price).
//! Source: <https://api-docs.deepseek.com/quick_start/pricing/> and
//! <https://opencode.ai/docs/go/> (verified 2026-09-16).

use std::fmt;

/// Default peak specification used when config.yaml does not override it.
pub const DEFAULT_PEAK_SPEC: &str = "Mon-Fri 01:00-04:00, 06:00-10:00 UTC";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub min: u32,
    pub sec: u32,
    /// 0 = Monday .. 6 = Sunday
    pub weekday: u32,
}

impl fmt::Display for Civil {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const WD: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z ({})",
            self.year,
            self.month,
            self.day,
            self.hour,
            self.min,
            self.sec,
            WD[self.weekday as usize]
        )
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant algorithm).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = ((m + 9) % 12) as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Inverse of days_from_civil.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn civil_from_unix(secs: i64) -> Civil {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (year, month, day) = civil_from_days(days);
    Civil {
        year,
        month,
        day,
        hour: (rem / 3600) as u32,
        min: ((rem % 3600) / 60) as u32,
        sec: (rem % 60) as u32,
        weekday: (days + 3).rem_euclid(7) as u32,
    }
}

pub fn iso8601(secs: i64) -> String {
    let c = civil_from_unix(secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        c.year, c.month, c.day, c.hour, c.min, c.sec
    )
}

/// RFC 1123 date for the HTTP Date header.
pub fn http_date(secs: i64) -> String {
    const WD: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MO: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let c = civil_from_unix(secs);
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        WD[c.weekday as usize],
        c.day,
        MO[(c.month - 1) as usize],
        c.year,
        c.hour,
        c.min,
        c.sec
    )
}

/// Parse an ISO-8601 / RFC-3339 timestamp into unix seconds (fraction ignored).
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let min: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;
    let mut base = days_from_civil(year, month, day) * 86400 + hour * 3600 + min * 60 + sec;
    let rest = s[19..].trim_start_matches('.');
    let rest: String = rest.chars().skip_while(|c| c.is_ascii_digit()).collect();
    let rest = rest.trim();
    match rest.chars().next() {
        None | Some('Z') | Some('z') => {}
        Some(sign @ ('+' | '-')) => {
            let body = &rest[1..];
            let (hh, mm) = match body.split_once(':') {
                Some((h, m)) => (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?),
                None => {
                    let h = body.get(0..2)?.parse::<i64>().ok()?;
                    let m = body.get(2..4).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
                    (h, m)
                }
            };
            let off = hh * 3600 + mm * 60;
            if sign == '+' {
                base -= off;
            } else {
                base += off;
            }
        }
        _ => {}
    }
    Some(base)
}

// ------------------------------------------------------------- peak windows

#[derive(Debug, Clone)]
pub struct PeakWindow {
    /// Index 0 = Monday.
    pub days: [bool; 7],
    /// (start_minute, end_minute) in the window timezone; start > end wraps midnight.
    pub ranges: Vec<(u32, u32)>,
    /// Offset from UTC in seconds.
    pub offset_secs: i32,
}

impl PeakWindow {
    fn matches(&self, unix: i64) -> bool {
        let local = unix + self.offset_secs as i64;
        let days = local.div_euclid(86400);
        let secs = local.rem_euclid(86400);
        let minutes = (secs / 60) as u32;
        let weekday = (days + 3).rem_euclid(7) as usize;
        if !self.days[weekday] {
            return false;
        }
        self.ranges.iter().any(|(s, e)| {
            if s == e {
                false
            } else if s < e {
                minutes >= *s && minutes < *e
            } else {
                minutes >= *s || minutes < *e
            }
        })
    }
}

/// Parse a compact peak specification, e.g.
/// "Mon-Fri 01:00-04:00, 06:00-10:00 UTC" or "Sat,Sun 09:00-18:00 +08:00".
///
/// Grammar: window ( ';' window )*  where  window := days time-range (',' time-range)* [tz].
pub fn parse_peak_spec(spec: &str) -> Result<Vec<PeakWindow>, String> {
    let mut windows = Vec::new();
    for chunk in spec.split(';') {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        windows.push(parse_window(chunk).map_err(|e| format!("peak window {:?}: {}", chunk, e))?);
    }
    if windows.is_empty() {
        return Err("peak spec is empty".to_string());
    }
    Ok(windows)
}

fn parse_weekday(tok: &str) -> Result<usize, String> {
    let t = tok.trim().to_ascii_lowercase();
    let idx = match t.as_str() {
        "mon" | "monday" | "1" => 0,
        "tue" | "tues" | "tuesday" | "2" => 1,
        "wed" | "wednesday" | "3" => 2,
        "thu" | "thur" | "thurs" | "thursday" | "4" => 3,
        "fri" | "friday" | "5" => 4,
        "sat" | "saturday" | "6" => 5,
        "sun" | "sunday" | "0" | "7" => 6,
        _ => return Err(format!("unknown weekday {:?}", tok)),
    };
    Ok(idx)
}

fn parse_hhmm(tok: &str) -> Result<u32, String> {
    let t = tok.trim();
    let (h, m) = t
        .split_once(':')
        .ok_or_else(|| format!("bad time {:?} (expected HH:MM)", tok))?;
    let h: u32 = h.trim().parse().map_err(|_| format!("bad hour in {:?}", tok))?;
    let m: u32 = m.trim().parse().map_err(|_| format!("bad minute in {:?}", tok))?;
    if h > 24 || m > 59 {
        return Err(format!("time out of range {:?}", tok));
    }
    Ok(h * 60 + m)
}

fn parse_tz(tok: &str) -> Result<i32, String> {
    let t = tok.trim();
    let up = t.to_ascii_uppercase();
    if up == "UTC" || up == "Z" || up == "GMT" {
        return Ok(0);
    }
    let (sign, body) = match t.chars().next() {
        Some('+') => (1, &t[1..]),
        Some('-') => (-1, &t[1..]),
        _ => return Err(format!("unknown timezone {:?}", tok)),
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (
            h.parse::<i32>().map_err(|_| format!("bad tz {:?}", tok))?,
            m.parse::<i32>().map_err(|_| format!("bad tz {:?}", tok))?,
        ),
        None => (body.parse::<i32>().map_err(|_| format!("bad tz {:?}", tok))?, 0),
    };
    Ok(sign * (h * 3600 + m * 60))
}

fn looks_like_tz(tok: &str) -> bool {
    let up = tok.to_ascii_uppercase();
    up == "UTC"
        || up == "GMT"
        || up == "Z"
        || ((tok.starts_with('+') || tok.starts_with('-'))
            && tok.len() >= 3
            && tok[1..].chars().all(|c| c.is_ascii_digit() || c == ':'))
}

fn parse_window(chunk: &str) -> Result<PeakWindow, String> {
    let chunk = chunk.trim();
    // <days> <ranges...> [tz]   -- ranges may contain spaces after commas.
    let (day_spec, rest) = chunk
        .split_once(char::is_whitespace)
        .ok_or_else(|| format!("expected '<days> <HH:MM-HH:MM> [tz]', got {:?}", chunk))?;
    let mut time_spec = rest.trim();
    let offset = match time_spec.rsplit_once(char::is_whitespace) {
        Some((head, last)) if looks_like_tz(last) => {
            time_spec = head.trim();
            parse_tz(last)?
        }
        _ => {
            if looks_like_tz(time_spec) {
                let off = parse_tz(time_spec)?;
                time_spec = "";
                off
            } else {
                0
            }
        }
    };
    if time_spec.is_empty() {
        return Err(format!("missing time range in {:?}", chunk));
    }
    let day_spec = day_spec.replace(' ', "");
    let time_spec = time_spec.to_string();

    let mut days = [false; 7];
    if day_spec == "*" || day_spec == "all" || day_spec.eq_ignore_ascii_case("daily") {
        days = [true; 7];
    } else {
        for part in day_spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((a, b)) = part.split_once('-') {
                let a = parse_weekday(a)?;
                let b = parse_weekday(b)?;
                let mut i = a;
                loop {
                    days[i] = true;
                    if i == b {
                        break;
                    }
                    i = (i + 1) % 7;
                }
            } else {
                days[parse_weekday(part)?] = true;
            }
        }
    }

    let mut ranges = Vec::new();
    for r in time_spec.split(',') {
        let r = r.trim();
        if r.is_empty() {
            continue;
        }
        let (a, b) = r
            .split_once('-')
            .ok_or_else(|| format!("bad range {:?} (expected HH:MM-HH:MM)", r))?;
        let start = parse_hhmm(a)?;
        let end = if b.trim() == "24:00" {
            24 * 60
        } else {
            parse_hhmm(b)?
        };
        ranges.push((start, end));
    }
    if ranges.is_empty() {
        return Err("no time ranges".to_string());
    }
    Ok(PeakWindow {
        days,
        ranges,
        offset_secs: offset,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleState {
    pub is_peak: bool,
    /// Unix seconds of the next peak<->off-peak transition.
    pub next_change: i64,
}

/// Evaluate the schedule at now (unix seconds) and find the next transition.
pub fn eval_schedule(now: i64, windows: &[PeakWindow]) -> ScheduleState {
    let is_peak = is_peak_at(now, windows);
    // Minute-granular schedule: scan forward at most 8 days.
    let mut t = now - now.rem_euclid(60) + 60;
    let limit = now + 8 * 86400;
    let mut next = limit;
    while t <= limit {
        if is_peak_at(t, windows) != is_peak {
            next = t;
            break;
        }
        t += 60;
    }
    ScheduleState {
        is_peak,
        next_change: next,
    }
}

pub fn is_peak_at(unix: i64, windows: &[PeakWindow]) -> bool {
    windows.iter().any(|w| w.matches(unix))
}

/// Human readable summary of a peak spec.
pub fn describe_spec(spec: &str) -> String {
    match parse_peak_spec(spec) {
        Ok(w) => {
            let mut parts = Vec::new();
            for win in w {
                let days: Vec<&str> = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| win.days[*i])
                    .map(|(_, d)| *d)
                    .collect();
                let ranges: Vec<String> = win
                    .ranges
                    .iter()
                    .map(|(s, e)| format!("{:02}:{:02}-{:02}:{:02}", s / 60, s % 60, e / 60, e % 60))
                    .collect();
                let tz = if win.offset_secs == 0 {
                    "UTC".to_string()
                } else {
                    let sign = if win.offset_secs >= 0 { '+' } else { '-' };
                    let a = win.offset_secs.abs();
                    format!("{}{:02}:{:02}", sign, a / 3600, (a % 3600) / 60)
                };
                parts.push(format!("{} {} {}", days.join(","), ranges.join(","), tz));
            }
            parts.join(" ; ")
        }
        Err(e) => format!("invalid: {}", e),
    }
}
