//! Minimal, dependency-free HTTP/1.1 server (request parsing + streaming responses).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::{json, Value};

use crate::timeutil;

const MAX_HEADER_LINE: usize = 16 * 1024;
const MAX_HEADER_COUNT: usize = 200;
const MAX_HEADER_TOTAL: usize = 128 * 1024;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub path: String,
    pub query: String,
    pub version: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub keep_alive: bool,
    pub remote: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn bearer_token(&self) -> Option<String> {
        if let Some(v) = self.header("authorization") {
            if let Some(tok) = v.trim().strip_prefix("Bearer ") {
                return Some(tok.trim().to_string());
            }
            if let Some(tok) = v.trim().strip_prefix("bearer ") {
                return Some(tok.trim().to_string());
            }
        }
        if let Some(v) = self.header("x-api-key") {
            return Some(v.trim().to_string());
        }
        None
    }

    pub fn is_streaming_json(&self) -> bool {
        self.body
            .iter()
            .position(|b| *b == b'{')
            .map(|start| {
                let slice = &self.body[start..];
                let needle = b"\"stream\"";
                slice
                    .windows(needle.len())
                    .position(|w| w == needle)
                    .map(|idx| {
                        let rest = &slice[idx + needle.len()..];
                        let rest = rest
                            .iter()
                            .position(|b| *b == b':')
                            .map(|p| &rest[p + 1..])
                            .unwrap_or(&[]);
                        let rest: Vec<u8> =
                            rest.iter().cloned().skip_while(|b| b.is_ascii_whitespace()).collect();
                        rest.starts_with(b"true")
                    })
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }
}

#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status, self.message)
    }
}

pub const fn reason_phrase(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Status",
    }
}

pub struct Connection {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    pub remote: String,
    pub idle_timeout: Duration,
    pub request_timeout: Duration,
}

impl Connection {
    pub fn new(stream: TcpStream, idle_timeout_secs: u64, request_timeout_secs: u64) -> io::Result<Connection> {
        let _ = stream.set_nodelay(true);
        let peer: Option<SocketAddr> = stream.peer_addr().ok();
        let writer = stream.try_clone()?;
        let idle_timeout = Duration::from_secs(idle_timeout_secs.max(1));
        let _ = writer.set_write_timeout(Some(Duration::from_secs(600)));
        Ok(Connection {
            reader: BufReader::with_capacity(64 * 1024, stream),
            writer,
            remote: peer
                .map(|p| p.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            idle_timeout,
            request_timeout: Duration::from_secs(request_timeout_secs.max(1)),
        })
    }

    pub fn responder(&self) -> io::Result<Responder> {
        Ok(Responder {
            writer: self.writer.try_clone()?,
            head_sent: false,
            chunked: false,
            finished: false,
        })
    }

    /// Read one request. Ok(None) means the peer closed the connection or idled out.
    pub fn read_request(&mut self, max_body: usize) -> Result<Option<Request>, HttpError> {
        let _ = self
            .reader
            .get_ref()
            .set_read_timeout(Some(self.idle_timeout));
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => return Ok(None),
            Ok(_) => {}
            Err(e) => {
                if is_timeout(&e) {
                    return Ok(None);
                }
                return Err(HttpError {
                    status: 400,
                    message: format!("read error: {}", e),
                });
            }
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return Ok(None);
        }
        let mut parts = line.split(' ');
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("/").to_string();
        let version = parts.next().unwrap_or("HTTP/1.1").to_string();
        if method.is_empty() {
            return Err(HttpError {
                status: 400,
                message: "malformed request line".to_string(),
            });
        }

        // ---- headers
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut total = 0usize;
        loop {
            let mut hl = String::new();
            let n = self
                .reader
                .read_line(&mut hl)
                .map_err(|e| HttpError {
                    status: 400,
                    message: format!("header read error: {}", e),
                })?;
            if n == 0 {
                break;
            }
            total += n;
            if hl.len() > MAX_HEADER_LINE || total > MAX_HEADER_TOTAL || headers.len() > MAX_HEADER_COUNT {
                return Err(HttpError {
                    status: 431,
                    message: "request headers too large".to_string(),
                });
            }
            let hl = hl.trim_end_matches(['\r', '\n']);
            if hl.is_empty() {
                break;
            }
            if let Some((k, v)) = hl.split_once(':') {
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
        }

        let hdr = |name: &str| -> Option<String> {
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
        };

        // ---- body
        let expect_continue = hdr("expect")
            .map(|v| v.to_ascii_lowercase().contains("100-continue"))
            .unwrap_or(false);
        let _ = self
            .reader
            .get_ref()
            .set_read_timeout(Some(self.request_timeout));
        if expect_continue {
            let mut w = self.writer.try_clone().map_err(|e| HttpError {
                status: 500,
                message: e.to_string(),
            })?;
            let _ = w.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
            let _ = w.flush();
        }

        let mut body: Vec<u8> = Vec::new();
        if let Some(te) = hdr("transfer-encoding") {
            if te.to_ascii_lowercase().contains("chunked") {
                read_chunked(&mut self.reader, &mut body, max_body).map_err(|e| HttpError {
                    status: if e.kind() == io::ErrorKind::InvalidData {
                        400
                    } else {
                        408
                    },
                    message: format!("chunked body: {}", e),
                })?;
            }
        } else if let Some(cl) = hdr("content-length") {
            let len: usize = cl.trim().parse().map_err(|_| HttpError {
                status: 400,
                message: format!("bad content-length: {}", cl),
            })?;
            if len > max_body {
                return Err(HttpError {
                    status: 413,
                    message: format!("body too large: {} > {}", len, max_body),
                });
            }
            body.resize(len, 0);
            if len > 0 {
                self.reader.read_exact(&mut body).map_err(|e| HttpError {
                    status: 408,
                    message: format!("body read: {}", e),
                })?;
            }
        }

        let keep_alive = match hdr("connection").map(|v| v.to_ascii_lowercase()) {
            Some(c) if c.contains("close") => false,
            Some(c) if c.contains("keep-alive") => true,
            _ => version.eq_ignore_ascii_case("HTTP/1.1"),
        };

        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (target.clone(), String::new()),
        };

        Ok(Some(Request {
            method,
            target,
            path,
            query,
            version,
            headers,
            body,
            keep_alive,
            remote: self.remote.clone(),
        }))
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    ) || e.to_string().contains("timed out")
}

fn read_chunked<R: BufRead>(reader: &mut R, out: &mut Vec<u8>, max: usize) -> io::Result<()> {
    loop {
        let mut size_line = String::new();
        let n = reader.read_line(&mut size_line)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof in chunk size"));
        }
        let size_str = size_line.trim_end_matches(['\r', '\n']);
        let size_str = size_str.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_str, 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("bad chunk size {:?}", size_str)))?;
        if size == 0 {
            // trailers
            loop {
                let mut t = String::new();
                let n = reader.read_line(&mut t)?;
                if n == 0 || t.trim_end_matches(['\r', '\n']).is_empty() {
                    break;
                }
            }
            return Ok(());
        }
        if out.len() + size > max {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "chunked body too large"));
        }
        let start = out.len();
        out.resize(start + size, 0);
        reader.read_exact(&mut out[start..])?;
        let mut crlf = [0u8; 2];
        let _ = reader.read_exact(&mut crlf);
    }
}

/// Writes the response head/body. Chunked transfer is used when the length is unknown,
/// which is what streaming (SSE) responses need.
pub struct Responder {
    writer: TcpStream,
    head_sent: bool,
    chunked: bool,
    finished: bool,
}

impl Responder {
    pub fn head_sent(&self) -> bool {
        self.head_sent
    }

    pub fn send_head(
        &mut self,
        status: u16,
        headers: &[(String, String)],
        body_len: Option<usize>,
        keep_alive: bool,
        version: &str,
    ) -> io::Result<()> {
        let mut out = String::new();
        let version = if version.eq_ignore_ascii_case("HTTP/1.0") {
            "HTTP/1.0"
        } else {
            "HTTP/1.1"
        };
        out.push_str(&format!(
            "{} {} {}\r\n",
            version,
            status,
            reason_phrase(status)
        ));
        out.push_str(&format!("Date: {}\r\n", timeutil::http_date(crate::util::now_secs())));
        let mut has_ct = false;
        for (k, v) in headers {
            if k.eq_ignore_ascii_case("content-type") {
                has_ct = true;
            }
            out.push_str(&format!("{}: {}\r\n", k, v));
        }
        let _ = has_ct;
        match body_len {
            Some(len) => {
                out.push_str(&format!("Content-Length: {}\r\n", len));
                self.chunked = false;
            }
            None => {
                if version == "HTTP/1.1" {
                    out.push_str("Transfer-Encoding: chunked\r\n");
                    self.chunked = true;
                } else {
                    self.chunked = false;
                }
            }
        }
        out.push_str(if keep_alive && (version == "HTTP/1.1" || self.chunked) {
            "Connection: keep-alive\r\n"
        } else {
            "Connection: close\r\n"
        });
        out.push_str("\r\n");
        self.writer.write_all(out.as_bytes())?;
        self.writer.flush()?;
        self.head_sent = true;
        Ok(())
    }

    pub fn send_body(&mut self, data: &[u8]) -> io::Result<()> {
        if self.chunked {
            if data.is_empty() {
                return Ok(());
            }
            let mut head = format!("{:x}\r\n", data.len()).into_bytes();
            head.extend_from_slice(data);
            head.extend_from_slice(b"\r\n");
            self.writer.write_all(&head)?;
        } else {
            self.writer.write_all(data)?;
        }
        self.writer.flush()
    }

    pub fn finish(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        if self.chunked {
            self.writer.write_all(b"0\r\n\r\n")?;
        }
        self.writer.flush()
    }

    pub fn send_json(&mut self, status: u16, value: &serde_json::Value, keep_alive: bool, version: &str) -> io::Result<()> {
        let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
        self.send_head(
            status,
            &[("Content-Type".to_string(), "application/json".to_string())],
            Some(body.len()),
            keep_alive,
            version,
        )?;
        self.send_body(&body)?;
        self.finish()
    }

    /// Like send_json, plus extra headers. Used where a response carries a validator (ETag).
    pub fn send_json_with(
        &mut self,
        status: u16,
        value: &serde_json::Value,
        extra: &[(String, String)],
        keep_alive: bool,
        version: &str,
    ) -> io::Result<()> {
        let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
        let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        headers.extend_from_slice(extra);
        self.send_head(status, &headers, Some(body.len()), keep_alive, version)?;
        self.send_body(&body)?;
        self.finish()
    }

    pub fn send_text(&mut self, status: u16, text: &str, content_type: &str, keep_alive: bool, version: &str) -> io::Result<()> {
        self.send_head(
            status,
            &[("Content-Type".to_string(), content_type.to_string())],
            Some(text.len()),
            keep_alive,
            version,
        )?;
        self.send_body(text.as_bytes())?;
        self.finish()
    }
}

/// Global counters.
#[derive(Debug, Default)]
pub struct Statics {
    pub requests: std::sync::atomic::AtomicU64,
    pub proxied: std::sync::atomic::AtomicU64,
    pub errors: std::sync::atomic::AtomicU64,
    pub streams: std::sync::atomic::AtomicU64,
    pub body_bytes_out: std::sync::atomic::AtomicU64,
    pub plans_used: std::sync::atomic::AtomicU64,
    pub cash_used: std::sync::atomic::AtomicU64,
    /// Successful requests whose prompt was served without upstream cache hits.
    pub cold_starts: std::sync::atomic::AtomicU64,
    pub retries: std::sync::atomic::AtomicU64,
}

/// A frozen read of Statics, used by the state file, by "reset statistics" and as the basis of the
/// /router/stats validator.
///
/// The values are running totals, not per-interval deltas: a total can be checkpointed as often as
/// we like without the reader having to know when the last write happened, so a restart can neither
/// double-count nor lose a request.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StaticsSnapshot {
    pub requests: u64,
    pub proxied: u64,
    pub errors: u64,
    pub streams: u64,
    pub body_bytes_out: u64,
    pub plans_used: u64,
    pub cash_used: u64,
    pub cold_starts: u64,
    pub retries: u64,
}

impl StaticsSnapshot {
    pub fn to_json(self) -> Value {
        json!({
            "requests": self.requests,
            "proxied": self.proxied,
            "errors": self.errors,
            "streams": self.streams,
            "body_bytes_out": self.body_bytes_out,
            "plans_used": self.plans_used,
            "cash_used": self.cash_used,
            "cold_starts": self.cold_starts,
            "retries": self.retries,
        })
    }

    /// Absent or malformed fields read as zero; a non-object yields None.
    pub fn from_json(v: &Value) -> Option<StaticsSnapshot> {
        let o = v.as_object()?;
        let u = |k: &str| o.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        Some(StaticsSnapshot {
            requests: u("requests"),
            proxied: u("proxied"),
            errors: u("errors"),
            streams: u("streams"),
            body_bytes_out: u("body_bytes_out"),
            plans_used: u("plans_used"),
            cash_used: u("cash_used"),
            cold_starts: u("cold_starts"),
            retries: u("retries"),
        })
    }
}

impl Statics {
    pub fn snapshot(&self) -> StaticsSnapshot {
        StaticsSnapshot {
            requests: self.requests.load(Ordering::Relaxed),
            proxied: self.proxied.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            streams: self.streams.load(Ordering::Relaxed),
            body_bytes_out: self.body_bytes_out.load(Ordering::Relaxed),
            plans_used: self.plans_used.load(Ordering::Relaxed),
            cash_used: self.cash_used.load(Ordering::Relaxed),
            cold_starts: self.cold_starts.load(Ordering::Relaxed),
            retries: self.retries.load(Ordering::Relaxed),
        }
    }

    /// Adopt a persisted total: counters continue where the previous run stopped.
    pub fn restore(&self, s: StaticsSnapshot) {
        self.requests.store(s.requests, Ordering::Relaxed);
        self.proxied.store(s.proxied, Ordering::Relaxed);
        self.errors.store(s.errors, Ordering::Relaxed);
        self.streams.store(s.streams, Ordering::Relaxed);
        self.body_bytes_out.store(s.body_bytes_out, Ordering::Relaxed);
        self.plans_used.store(s.plans_used, Ordering::Relaxed);
        self.cash_used.store(s.cash_used, Ordering::Relaxed);
        self.cold_starts.store(s.cold_starts, Ordering::Relaxed);
        self.retries.store(s.retries, Ordering::Relaxed);
    }

    /// Zero every counter. Callers must checkpoint the baseline immediately, otherwise the next
    /// flush would write the difference against the old baseline and resurrect the totals.
    pub fn reset(&self) {
        self.restore(StaticsSnapshot::default());
    }
}
