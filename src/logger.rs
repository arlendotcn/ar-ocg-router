//! Minimal dependency-free logger (stdout + optional file), with level filtering.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use crate::timeutil;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
    Trace = 4,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" | "err" => Some(Level::Error),
            "warn" | "warning" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" => Some(Level::Debug),
            "trace" => Some(Level::Trace),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }
}

struct Sink {
    level: Level,
    file: Option<Mutex<std::fs::File>>,
    quiet: bool,
}

static SINK: OnceLock<Sink> = OnceLock::new();

pub fn init(level: Level, file: Option<&str>, quiet: bool) {
    let handle = file.and_then(|p| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .map_err(|e| {
                eprintln!("[logger] cannot open log file {}: {}", p, e);
                e
            })
            .ok()
    });
    let _ = SINK.set(Sink {
        level,
        file: handle.map(Mutex::new),
        quiet,
    });
}

pub fn level() -> Level {
    SINK.get().map(|s| s.level).unwrap_or(Level::Info)
}

pub fn enabled(lvl: Level) -> bool {
    lvl <= level()
}

pub fn log(level: Level, msg: &str) {
    let sink = match SINK.get() {
        Some(s) => s,
        None => {
            eprintln!("{}", msg);
            return;
        }
    };
    if level > sink.level {
        return;
    }
    let now = timeutil::iso8601(crate::util::now_secs());
    let line = format!("{} {} {}\n", now, level.as_str(), msg);
    if !sink.quiet {
        let _ = std::io::stdout().write_all(line.as_bytes());
        let _ = std::io::stdout().flush();
    }
    if let Some(f) = &sink.file {
        if let Ok(mut guard) = f.lock() {
            let _ = guard.write_all(line.as_bytes());
            let _ = guard.flush();
        }
    }
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => { $crate::logger::log($crate::logger::Level::Error, &format!($($arg)*)) };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::logger::log($crate::logger::Level::Warn, &format!($($arg)*)) };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::logger::log($crate::logger::Level::Info, &format!($($arg)*)) };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => { $crate::logger::log($crate::logger::Level::Debug, &format!($($arg)*)) };
}

#[macro_export]
macro_rules! log_trace {
    ($($arg:tt)*) => { $crate::logger::log($crate::logger::Level::Trace, &format!($($arg)*)) };
}
