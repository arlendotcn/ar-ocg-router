//! End-to-end tests: the real router binary is started against an in-process mock upstream.
//!
//! The mock speaks the same wire shapes verified live against OpenCode Go
//! (https://opencode.ai/zen/go/v1) and DeepSeek official (https://api.deepseek.com):
//! /chat/completions, /responses, /models, plus OpenCode Go /usage and DeepSeek /user/balance.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// --------------------------------------------------------------------- mock upstream

#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

#[derive(Clone, Debug)]
struct MockResponse {
    status: u16,
    content_type: String,
    body: String,
    chunks: Option<Vec<(u64, String)>>,
}

impl MockResponse {
    fn json(status: u16, body: &str) -> MockResponse {
        MockResponse {
            status,
            content_type: "application/json".to_string(),
            body: body.to_string(),
            chunks: None,
        }
    }
    fn sse(chunks: Vec<(u64, String)>) -> MockResponse {
        MockResponse {
            status: 200,
            content_type: "text/event-stream".to_string(),
            body: String::new(),
            chunks: Some(chunks),
        }
    }
}

struct MockState {
    port: u16,
    requests: Mutex<Vec<Recorded>>,
    routes: Mutex<HashMap<String, MockResponse>>,
    quota_pct: Mutex<f64>,
}

#[derive(Clone)]
struct Mock {
    inner: Arc<MockState>,
}

impl Mock {
    fn start() -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let port = listener.local_addr().unwrap().port();
        let inner = Arc::new(MockState {
            port,
            requests: Mutex::new(Vec::new()),
            routes: Mutex::new(HashMap::new()),
            quota_pct: Mutex::new(10.0),
        });
        let state = inner.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(s) = stream {
                    let st = state.clone();
                    std::thread::spawn(move || handle_conn(s, st));
                }
            }
        });
        Mock { inner }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.inner.port)
    }

    fn set(&self, path: &str, resp: MockResponse) {
        self.inner.routes.lock().unwrap().insert(path.to_string(), resp);
    }

    fn set_quota_pct(&self, pct: f64) {
        *self.inner.quota_pct.lock().unwrap() = pct;
    }

    fn requests(&self) -> Vec<Recorded> {
        self.inner.requests.lock().unwrap().clone()
    }

    fn hits(&self, path: &str) -> usize {
        self.requests().iter().filter(|r| r.path == path).count()
    }

    fn posts(&self) -> Vec<Recorded> {
        self.requests().into_iter().filter(|r| r.method == "POST").collect()
    }

    fn last(&self, path: &str) -> Option<Recorded> {
        self.requests().into_iter().filter(|r| r.path == path).last()
    }
}

fn read_headers(reader: &mut BufReader<TcpStream>) -> Option<(String, Vec<(String, String)>)> {
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let start = line.trim().to_string();
    let mut headers = Vec::new();
    loop {
        let mut hl = String::new();
        if reader.read_line(&mut hl).ok()? == 0 {
            break;
        }
        let hl = hl.trim_end().to_string();
        if hl.is_empty() {
            break;
        }
        if let Some((k, v)) = hl.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Some((start, headers))
}

fn handle_conn(stream: TcpStream, state: Arc<MockState>) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    loop {
        let (start, headers) = match read_headers(&mut reader) {
            Some(v) => v,
            None => return,
        };
        let mut parts = start.split(' ');
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("/").to_string();
        let path = target.split('?').next().unwrap_or("/").to_string();
        let len: usize = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; len];
        if len > 0 && reader.read_exact(&mut body).is_err() {
            return;
        }
        state.requests.lock().unwrap().push(Recorded {
            method,
            path: path.clone(),
            headers,
            body: String::from_utf8_lossy(&body).to_string(),
        });

        let route = state.routes.lock().unwrap().get(&path).cloned();
        let resp = match route {
            Some(r) => r,
            None => default_route(&path, &state),
        };
        if let Some(chunks) = &resp.chunks {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
                resp.content_type
            );
            let _ = writer.write_all(head.as_bytes());
            let _ = writer.flush();
            for (delay, payload) in chunks {
                if *delay > 0 {
                    std::thread::sleep(Duration::from_millis(*delay));
                }
                let frame = format!("{:x}\r\n{}\r\n", payload.len(), payload);
                if writer.write_all(frame.as_bytes()).is_err() {
                    return;
                }
                let _ = writer.flush();
            }
            let _ = writer.write_all(b"0\r\n\r\n");
            let _ = writer.flush();
        } else {
            let reason = match resp.status {
                200 => "OK",
                400 => "Bad Request",
                401 => "Unauthorized",
                404 => "Not Found",
                429 => "Too Many Requests",
                _ => "Server Error",
            };
            let head = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                resp.status,
                reason,
                resp.content_type,
                resp.body.len()
            );
            let _ = writer.write_all(head.as_bytes());
            let _ = writer.write_all(resp.body.as_bytes());
            let _ = writer.flush();
        }
    }
}

fn default_route(path: &str, state: &Arc<MockState>) -> MockResponse {
    let pct = *state.quota_pct.lock().unwrap();
    if path.ends_with("/usage") {
        let body = format!(
            "{{\"usage\":{{\"rolling\":{{\"status\":\"ok\",\"percent\":{},\"resetsAt\":\"2026-12-31T23:00:00Z\"}},\"weekly\":{{\"status\":\"ok\",\"percent\":{},\"resetsAt\":\"2026-12-31T23:00:00Z\"}},\"monthly\":{{\"status\":\"ok\",\"percent\":{},\"resetsAt\":\"2026-12-31T23:00:00Z\"}}}}}}",
            pct, pct, pct
        );
        return MockResponse::json(200, &body);
    }
    if path.ends_with("/models") {
        return MockResponse::json(
            200,
            "{\"object\":\"list\",\"data\":[{\"id\":\"deepseek-v4-flash\",\"object\":\"model\"},{\"id\":\"deepseek-v4-pro\",\"object\":\"model\"}]}",
        );
    }
    if path.ends_with("/user/balance") {
        return MockResponse::json(
            200,
            "{\"is_available\":true,\"balance_infos\":[{\"currency\":\"CNY\",\"total_balance\":\"668.95\",\"granted_balance\":\"0.00\",\"topped_up_balance\":\"668.95\"}]}",
        );
    }
    if path.ends_with("/chat/completions") || path.ends_with("/responses") {
        return MockResponse::json(
            200,
            "{\"id\":\"mock\",\"object\":\"chat.completion\",\"model\":\"mock-model\",\"choices\":[{\"index\":0,\"message\":{\"role\":\"assistant\",\"content\":\"pong\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":200,\"prompt_tokens_details\":{\"cached_tokens\":400}}}",
        );
    }
    MockResponse::json(404, "{\"error\":{\"message\":\"no route\"}}")
}

// --------------------------------------------------------------------- router process

struct Router {
    child: std::process::Child,
    port: u16,
    log: std::path::PathBuf,
}

impl Drop for Router {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind");
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

fn start_router(tag: &str, fake_now: &str, build: &dyn Fn(u16) -> String) -> Router {
    let port = free_port();
    let config = build(port);
    let dir = std::env::temp_dir().join(format!("ar-ocg-router-test-{}-{}", tag, port));
    let _ = std::fs::create_dir_all(&dir);
    let cfg_path = dir.join("config.yaml");
    std::fs::write(&cfg_path, config).expect("write config");
    let log_path = dir.join("router.log");
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_ar-ocg-router"))
        .arg("--config")
        .arg(&cfg_path)
        .arg("--log-level")
        .arg("debug")
        .arg("--log-file")
        .arg(&log_path)
        .env("AR_OCG_ROUTER_FAKE_NOW", fake_now)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn router");
    let r = Router {
        child,
        port,
        log: log_path,
    };
    wait_ready(r.port);
    r
}

fn wait_ready(port: u16) {
    for _ in 0..100 {
        if let Ok(resp) = try_request(port, "GET", "/health", None, &[]) {
            if resp.status == 200 {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("router on port {} never became ready", port);
}

/// Wait until the OpenCode Go account has live quota data (usage endpoint polled).
fn wait_quota(port: u16, account: &str) {
    for _ in 0..80 {
        if let Ok(resp) = try_request(port, "GET", "/router/stats", None, &[]) {
            if resp.body.contains(&format!("\"name\":\"{}\"", account))
                && resp.body.contains("\"source\":\"remote\"")
            {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("quota data for {} never arrived", account);
}

// --------------------------------------------------------------------- test http client

struct HttpResp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    chunk_times: Vec<Instant>,
}

impl HttpResp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

fn try_request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> std::io::Result<HttpResp> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    let mut req = format!("{} {} HTTP/1.1\r\nHost: 127.0.0.1\r\n", method, path);
    let mut has_ct = false;
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("content-type") {
            has_ct = true;
        }
        req.push_str(&format!("{}: {}\r\n", k, v));
    }
    if body.is_some() && !has_ct {
        req.push_str("Content-Type: application/json\r\n");
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    stream.write_all(req.as_bytes())?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let status: u16 = line
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut hdrs: Vec<(String, String)> = Vec::new();
    loop {
        let mut hl = String::new();
        if reader.read_line(&mut hl)? == 0 {
            break;
        }
        let hl = hl.trim_end().to_string();
        if hl.is_empty() {
            break;
        }
        if let Some((k, v)) = hl.split_once(':') {
            hdrs.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let get = |n: &str| -> Option<String> {
        hdrs.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(n))
            .map(|(_, v)| v.clone())
    };
    let mut body_out = String::new();
    let mut chunk_times = Vec::new();
    if let Some(cl) = get("content-length") {
        let n: usize = cl.parse().unwrap_or(0);
        let mut buf = vec![0u8; n];
        if n > 0 {
            reader.read_exact(&mut buf)?;
        }
        body_out = String::from_utf8_lossy(&buf).to_string();
    } else if get("transfer-encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false)
    {
        loop {
            let mut sz = String::new();
            reader.read_line(&mut sz)?;
            let sz = sz.trim();
            let n = usize::from_str_radix(sz.split(';').next().unwrap_or("0"), 16).unwrap_or(0);
            if n == 0 {
                let mut trailer = String::new();
                let _ = reader.read_line(&mut trailer);
                break;
            }
            let mut buf = vec![0u8; n];
            reader.read_exact(&mut buf)?;
            body_out.push_str(&String::from_utf8_lossy(&buf));
            chunk_times.push(Instant::now());
            let mut crlf = [0u8; 2];
            let _ = reader.read_exact(&mut crlf);
        }
    }
    Ok(HttpResp {
        status,
        headers: hdrs,
        body: body_out,
        chunk_times,
    })
}

fn request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> HttpResp {
    try_request(port, method, path, body, headers).expect("http request")
}

fn dump_log(r: &Router) {
    if let Ok(text) = std::fs::read_to_string(&r.log) {
        eprintln!("---- router log ({}) ----\n{}", r.log.display(), text);
    }
}

fn config_peak_offpeak(port: u16, go: &str, fb: &str, idle_prefer: &str, surplus_max_pct: u32) -> String {
    format!(
        r#"server:
  host: 127.0.0.1
  port: {port}
  max_connections: 64
log:
  level: debug
  file: ""
router:
  mode: auto
  peak_windows: "Mon-Fri 01:00-04:00, 06:00-10:00 UTC"
  idle_prefer: {idle}
  surplus_max_pct: {surplus}
  surplus_projection: false
  quota_refresh_secs: 30
  selection: weighted
plans:
  - name: go-1
    url: {go}
    key: sk-go-test
    model: deepseek-flash
    inject_session: true
    mode: both
    weight: 60
    provider: opencodego
fallback:
  - name: deepseek-official
    url: {fb}
    key: sk-fb-test
    model: deepseek-flash
    mode: both
    rule: [offpeak, quota_low]
    weight: 50
    provider: deepseek
"#,
        port = port,
        go = go,
        fb = fb,
        idle = idle_prefer,
        surplus = surplus_max_pct
    )
}

const CHAT_BODY: &str = r#"{"model":"deepseek-flash","messages":[{"role":"user","content":"hi"}],"temperature":0.3,"max_tokens":64}"#;

// --------------------------------------------------------------------------- tests

#[test]
fn peak_prefers_opencodego_and_maps_request() {
    let go = Mock::start();
    let fb = Mock::start();
    let r = start_router("peak", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "surplus_first", 80)
    });
    let resp = request(
        r.port,
        "POST",
        "/v1/chat/completions",
        Some(CHAT_BODY),
        &[("x-session-id", "sess-abc")],
    );
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200, "body={}", resp.body);
    assert_eq!(resp.header("x-router-account"), Some("go-1"));
    assert_eq!(resp.header("x-router-peak"), Some("peak"));
    assert_eq!(go.posts().len(), 1);
    assert_eq!(fb.posts().len(), 0);
    let rec = go.last("/v1/chat/completions").unwrap();
    assert_eq!(rec.header("authorization"), Some("Bearer sk-go-test"));
    assert_eq!(rec.header("x-opencode-session"), Some("sess-abc"));
    assert!(rec.header("user-agent").unwrap().contains("ar-ocg-router"));
    let body = rec.json();
    assert_eq!(rec.method, "POST");
    assert_eq!(body["model"], "deepseek-flash");
    assert_eq!(body["temperature"], 0.3);
    assert_eq!(body["max_tokens"], 64);
    assert_eq!(body["messages"][0]["content"], "hi");
}

#[test]
fn offpeak_with_surplus_uses_go_quota() {
    let go = Mock::start();
    let fb = Mock::start();
    go.set_quota_pct(12.0);
    let r = start_router("offpeak-surplus", "2026-09-16T05:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "surplus_first", 80)
    });
    wait_quota(r.port, "go-1");
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.header("x-router-account"),
        Some("go-1"),
        "off-peak with spare quota should still use OpenCode Go"
    );
    assert_eq!(go.hits("/v1/chat/completions"), 1);
    assert_eq!(fb.hits("/chat/completions"), 0);
}

#[test]
fn offpeak_with_tight_quota_uses_fallback_and_maps_model() {
    let go = Mock::start();
    let fb = Mock::start();
    go.set_quota_pct(93.0);
    let r = start_router("offpeak-tight", "2026-09-16T05:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "surplus_first", 80)
    });
    wait_quota(r.port, "go-1");
    let body = r#"{"model":"deepseek-v4.1-flash","messages":[{"role":"user","content":"hi"}],"max_tokens":32}"#;
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(body), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("x-router-account"), Some("deepseek-official"));
    assert_eq!(resp.header("x-router-peak"), Some("offpeak"));
    assert_eq!(go.hits("/v1/chat/completions"), 0, "Go must be skipped when quota is tight off-peak");
    let rec = fb.last("/chat/completions").expect("fallback called");
    assert_eq!(rec.json()["model"], "deepseek-flash");
    assert_eq!(rec.header("authorization"), Some("Bearer sk-fb-test"));
    assert_eq!(rec.header("x-opencode-session"), None);
}

#[test]
fn failover_on_quota_429_then_fallback() {
    let go = Mock::start();
    let fb = Mock::start();
    go.set(
        "/v1/chat/completions",
        MockResponse::json(
            429,
            r#"{"error":{"message":"usage limit reached for this window","type":"rate_limit"}}"#,
        ),
    );
    let r = start_router("failover429", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "opencodego", 80)
    });
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200, "fallback should have served the request");
    assert_eq!(resp.header("x-router-account"), Some("deepseek-official"));
    assert_eq!(go.hits("/v1/chat/completions"), 1);
    assert_eq!(fb.hits("/chat/completions"), 1);

    let stats = request(r.port, "GET", "/router/stats", None, &[]);
    let v = stats.json();
    let go_acc = v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "go-1")
        .unwrap()
        .clone();
    assert_eq!(go_acc["available"], false, "go-1 should be cooling down: {}", go_acc);
    assert!(go_acc["stats"]["errors"].as_u64().unwrap_or(0) >= 1);
}

#[test]
fn failover_on_model_error() {
    let go = Mock::start();
    let fb = Mock::start();
    go.set(
        "/v1/chat/completions",
        MockResponse::json(
            400,
            r#"{"error":{"message":"The model deepseek-flash does not exist","type":"invalid_request_error"}}"#,
        ),
    );
    let r = start_router("failover-model", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "opencodego", 80)
    });
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("x-router-account"), Some("deepseek-official"));
    assert_eq!(fb.hits("/chat/completions"), 1);
}

#[test]
fn request_errors_are_not_retried_elsewhere() {
    let go = Mock::start();
    let fb = Mock::start();
    go.set(
        "/v1/chat/completions",
        MockResponse::json(
            400,
            r#"{"error":{"message":"temperature must be between 0 and 2","type":"invalid_request_error"}}"#,
        ),
    );
    let r = start_router("client-error", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "opencodego", 80)
    });
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    assert_eq!(resp.status, 400);
    assert!(resp.body.contains("temperature"), "body={}", resp.body);
    assert_eq!(fb.hits("/chat/completions"), 0, "client errors must not fail over");
}

#[test]
fn streaming_passthrough_is_incremental_and_accounted() {
    let go = Mock::start();
    let fb = Mock::start();
    let frame1 = "data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"po\"}}]}\n\n";
    let frame2 = "data: {\"id\":\"c1\",\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":500,\"prompt_tokens_details\":{\"cached_tokens\":250}}}\n\n";
    let frame3 = "data: [DONE]\n\n";
    go.set(
        "/v1/chat/completions",
        MockResponse::sse(vec![
            (0, frame1.to_string()),
            (400, frame2.to_string()),
            (0, frame3.to_string()),
        ]),
    );
    let r = start_router("stream", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "opencodego", 80)
    });
    let body = r#"{"model":"deepseek-flash","messages":[{"role":"user","content":"hi"}],"stream":true}"#;
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(body), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("x-router-account"), Some("go-1"));
    assert!(resp.header("content-type").unwrap().contains("text/event-stream"));
    assert_eq!(resp.body, format!("{}{}{}", frame1, frame2, frame3));
    assert!(resp.chunk_times.len() >= 3, "expected chunked streaming, got {}", resp.chunk_times.len());
    let spread = resp
        .chunk_times
        .last()
        .unwrap()
        .duration_since(resp.chunk_times[0]);
    assert!(
        spread >= Duration::from_millis(300),
        "SSE frames must be forwarded while the upstream is still streaming (spread {:?})",
        spread
    );

    let stats = request(r.port, "GET", "/router/stats", None, &[]);
    let v = stats.json();
    let acc = v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "go-1")
        .unwrap()
        .clone();
    assert_eq!(acc["stats"]["prompt_tokens"], 1000, "stats={}", acc["stats"]);
    assert_eq!(acc["stats"]["completion_tokens"], 500);
    assert_eq!(acc["stats"]["cached_tokens"], 250);
    // stats are rounded to 6 decimals
    let cost = acc["stats"]["cost_usd"].as_f64().unwrap();
    assert!((cost - 0.0008265).abs() < 1e-6, "cost={}", cost);
    let saved = v["savings"]["opencodego_saved_usd"].as_f64().unwrap();
    assert!((saved - 0.0008265).abs() < 1e-6, "saved={}", saved);
}

#[test]
fn responses_endpoint_is_proxied_with_usage() {
    let go = Mock::start();
    let fb = Mock::start();
    go.set(
        "/v1/responses",
        MockResponse::json(
            200,
            r#"{"id":"resp_1","object":"response","status":"completed","output":[],"usage":{"input_tokens":1000,"output_tokens":200,"input_tokens_details":{"cached_tokens":0}}}"#,
        ),
    );
    let r = start_router("responses", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "opencodego", 80)
    });
    let body = r#"{"model":"deepseek-flash","input":"hi","max_output_tokens":32}"#;
    let resp = request(r.port, "POST", "/v1/responses", Some(body), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("x-router-account"), Some("go-1"));
    assert_eq!(resp.json()["status"], "completed");
    let rec = go.last("/v1/responses").expect("upstream responses call");
    assert_eq!(rec.json()["input"], "hi");
    assert_eq!(rec.json()["model"], "deepseek-flash");
    let stats = request(r.port, "GET", "/router/stats", None, &[]);
    let acc = stats.json()["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "go-1")
        .unwrap()
        .clone();
    assert_eq!(acc["stats"]["prompt_tokens"], 1000);
    assert_eq!(acc["stats"]["completion_tokens"], 200);
}

#[test]
fn client_api_key_is_enforced_when_configured() {
    let go = Mock::start();
    let fb = Mock::start();
    let r = start_router("auth", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "opencodego", 80).replace(
            "  max_connections: 64",
            "  max_connections: 64\n  client_keys:\n    - sk-router-secret",
        )
    });
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    assert_eq!(resp.status, 401);
    assert_eq!(go.hits("/v1/chat/completions"), 0);
    let resp = request(
        r.port,
        "POST",
        "/v1/chat/completions",
        Some(CHAT_BODY),
        &[("authorization", "Bearer sk-router-secret")],
    );
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200);
    let resp = request(r.port, "GET", "/health", None, &[]);
    assert_eq!(resp.status, 200);
}

#[test]
fn all_accounts_failing_returns_502_with_details() {
    let go = Mock::start();
    let fb = Mock::start();
    go.set("/v1/chat/completions", MockResponse::json(500, r#"{"error":{"message":"boom"}}"#));
    fb.set("/chat/completions", MockResponse::json(500, r#"{"error":{"message":"boom2"}}"#));
    let r = start_router("all-fail", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "opencodego", 80)
    });
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    if resp.status != 502 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 502);
    let v = resp.json();
    assert_eq!(v["error"]["type"], "all_accounts_failed");
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("go-1") && msg.contains("deepseek-official"), "msg={}", msg);
}

#[test]
fn client_model_string_pins_an_endpoint_and_is_otherwise_ignored() {
    let plan_a = Mock::start();
    let plan_b = Mock::start();
    let cash = Mock::start();
    let r = start_router("pinning", "2026-09-16T02:00:00Z", &|port| {
        format!(
            r#"server:
  host: 127.0.0.1
  port: {port}
log:
  level: debug
  file: ""
router:
  mode: auto
plans:
  - name: plan-a
    url: {a}/v1
    key: k1
    model: model-a
    mode: both
    order: 10
    weight: 50
  - name: plan-b
    url: {b}/v1
    key: k2
    model: model-b
    mode: both
    order: 20
    weight: 50
fallback:
  - name: cash
    url: {c}
    key: k3
    model: model-c
    mode: both
    rule: never
"#,
            port = port,
            a = plan_a.url(),
            b = plan_b.url(),
            c = cash.url()
        )
    });
    // 1) an arbitrary model string is accepted and routed normally (order decides)
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.header("x-router-account"), Some("plan-a"));
    assert_eq!(resp.header("x-router-model"), Some("model-a"));

    // 2) every endpoint sends its own hard-coded model, whatever the client wrote
    let rec = plan_a.last("/v1/chat/completions").unwrap();
    assert_eq!(rec.json()["model"], "model-a");

    // 3) "name/anything" pins that endpoint and bypasses the routing policy
    let body = r#"{"model":"plan-b/whatever","messages":[{"role":"user","content":"hi"}],"max_tokens":16}"#;
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(body), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.header("x-router-account"), Some("plan-b"));
    assert_eq!(resp.header("x-router-model"), Some("model-b"));

    // 4) provider/model pinning works too
    let body = r#"{"model":"generic/model-c","messages":[{"role":"user","content":"hi"}],"max_tokens":16}"#;
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(body), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.header("x-router-account"), Some("cash"));
    assert_eq!(resp.header("x-router-model"), Some("model-c"));

    // 5) /v1/models advertises exactly one id
    let models = request(r.port, "GET", "/v1/models", None, &[]);
    let payload = models.json();
    let ids: Vec<String> = payload["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec!["ar-ocg-router".to_string()]);
}

#[test]
fn any_client_model_name_is_accepted() {
    let go = Mock::start();
    let fb = Mock::start();
    let r = start_router("any-model", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "plans", 80)
    });
    for client_model in ["ar-ocg-router", "deepseek-flash", "gpt-5.6-sol", "totally-made-up"] {
        let body = format!(
            r#"{{"model":"{}","messages":[{{"role":"user","content":"hi"}}],"max_tokens":16}}"#,
            client_model
        );
        let resp = request(r.port, "POST", "/v1/chat/completions", Some(&body), &[]);
        if resp.status != 200 {
            dump_log(&r);
        }
        assert_eq!(resp.status, 200, "client model {:?} must be accepted", client_model);
        assert_eq!(resp.header("x-router-account"), Some("go-1"));
    }
}

#[test]
fn failing_endpoint_is_skipped_but_the_others_keep_serving() {
    let bad = Mock::start();
    let good = Mock::start();
    // the "bad" endpoint always answers 404 UnsupportedModel => configuration mistake
    bad.set(
        "/v1/chat/completions",
        MockResponse::json(
            404,
            r#"{"error":{"message":"The requested model does not support this plan","type":"UnsupportedModel"}}"#,
        ),
    );
    let r = start_router("skip-health", "2026-09-16T02:00:00Z", &|port| {
        format!(
            r#"server:
  host: 127.0.0.1
  port: {port}
log:
  level: debug
  file: ""
router:
  mode: plans
  cooldown_secs: 1
  skip_after_failures: 2
  skip_secs: 300
plans:
  - name: bad-plan
    url: {bad}/v1
    key: k1
    model: wrong-model
    mode: both
    order: 10
    weight: 50
  - name: good-plan
    url: {good}/v1
    key: k2
    model: right-model
    mode: both
    order: 20
    weight: 50
fallback:
  - name: cash
    url: {c}
    key: k3
    model: cash-model
    mode: both
    rule: never
"#,
            port = port,
            bad = bad.url(),
            good = good.url(),
            c = bad.url()
        )
    });
    // requests 1 and 2 hit the broken endpoint first, then fall back to the good one
    for i in 0..2 {
        let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
        if resp.status != 200 {
            dump_log(&r);
        }
        assert_eq!(resp.header("x-router-account"), Some("good-plan"), "request {}", i);
    }
    assert_eq!(bad.hits("/v1/chat/completions"), 2, "the broken endpoint was tried twice");
    assert_eq!(good.hits("/v1/chat/completions"), 2);

    // after 2 consecutive failures the endpoint is skipped entirely: no more attempts on it
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    assert_eq!(resp.status, 200);
    assert_eq!(
        bad.hits("/v1/chat/completions"),
        2,
        "a skipped endpoint must not be tried again"
    );

    let stats = request(r.port, "GET", "/router/stats", None, &[]);
    let v = stats.json();
    let bad_acc = v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "bad-plan")
        .unwrap()
        .clone();
    assert_eq!(bad_acc["health"]["skipped"], true, "{}", bad_acc["health"]);
    assert_eq!(bad_acc["health"]["failure_streak"], 2);
    assert!(bad_acc["health"]["last_reason"]
        .as_str()
        .unwrap()
        .contains("404"));
}

#[test]
fn config_reload_applies_new_policy() {
    let go = Mock::start();
    let fb = Mock::start();
    let port = free_port();
    let dir = std::env::temp_dir().join(format!("ar-ocg-router-reload-{}", free_port()));
    let _ = std::fs::create_dir_all(&dir);
    let cfg_path = dir.join("config.yaml");
    let log = dir.join("router.log");
    let base = config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "fallback", 80);
    std::fs::write(&cfg_path, &base).unwrap();
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_ar-ocg-router"))
        .arg("--config")
        .arg(&cfg_path)
        .arg("--log-level")
        .arg("debug")
        .arg("--log-file")
        .arg(&log)
        .env("AR_OCG_ROUTER_FAKE_NOW", "2026-09-16T05:00:00Z")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let r = Router {
        child,
        port,
        log: log.clone(),
    };
    wait_ready(port);
    let resp = request(port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    assert_eq!(resp.header("x-router-account"), Some("deepseek-official"));
    std::fs::write(
        &cfg_path,
        base.replace("idle_prefer: fallback", "idle_prefer: plans"),
    )
    .unwrap();
    let mut switched = false;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(300));
        let resp = request(port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
        if resp.header("x-router-account") == Some("go-1") {
            switched = true;
            break;
        }
    }
    if !switched {
        dump_log(&r);
    }
    assert!(switched, "config reload should switch the preferred tier");
}

#[test]
fn plan_order_overrides_configuration_order() {
    let plan_a = Mock::start();
    let plan_b = Mock::start();
    let cash = Mock::start();
    let r = start_router("plan-order", "2026-09-16T02:00:00Z", &|port| {
        format!(
            r#"server:
  host: 127.0.0.1
  port: {port}
log:
  level: debug
  file: ""
router:
  mode: auto
plans:
  - name: plan-a
    url: {a}/v1
    provider: opencodego
    key: k1
    model: deepseek-flash
    mode: both
    order: 20
    weight: 50
  - name: plan-b
    url: {b}/v1
    provider: opencodego
    key: k2
    model: deepseek-flash
    mode: both
    order: 5
    weight: 50
fallback:
  - name: cash
    url: {c}
    key: k3
    model: deepseek-flash
    mode: both
    rule: always
"#,
            port = port,
            a = plan_a.url(),
            b = plan_b.url(),
            c = cash.url()
        )
    });
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("x-router-account"), Some("plan-b"), "lower order wins");
    assert_eq!(resp.header("x-router-kind"), Some("plans"));
    assert_eq!(plan_b.hits("/v1/chat/completions"), 1);
    assert_eq!(plan_a.hits("/v1/chat/completions"), 0);
    assert_eq!(cash.hits("/chat/completions"), 0);
}

#[test]
fn token_budgeted_plan_switches_to_cash_when_the_ledger_runs_out() {
    let plan = Mock::start();
    let cash = Mock::start();
    // 05:00Z -> off-peak, where surplus_first decides between the plan and cash
    let r = start_router("token-budget", "2026-09-16T05:00:00Z", &|port| {
        format!(
            r#"server:
  host: 127.0.0.1
  port: {port}
log:
  level: debug
  file: ""
router:
  mode: auto
  idle_prefer: surplus_first
  surplus_max_pct: 80
  surplus_projection: false
plans:
  - name: plan-tokens
    url: {p}/v1
    provider: generic
    key: k1
    model: deepseek-flash
    mode: both
    quota: {{ unit: tokens, monthly: 1000, probe: none }}
fallback:
  - name: cash
    url: {c}
    key: k2
    model: deepseek-flash
    mode: both
    rule: always
"#,
            port = port,
            p = plan.url(),
            c = cash.url()
        )
    });
    // the mock reports 1000 prompt + 200 completion tokens per call, i.e. 1200 tokens
    let first = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    if first.status != 200 {
        dump_log(&r);
    }
    assert_eq!(first.header("x-router-account"), Some("plan-tokens"), "prepaid plan first");

    let second = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    if second.status != 200 {
        dump_log(&r);
    }
    assert_eq!(
        second.header("x-router-account"),
        Some("cash"),
        "1200 local tokens against a 1000 token budget must stop the plan being preferred"
    );
    assert_eq!(plan.hits("/v1/chat/completions"), 1);
    assert_eq!(cash.hits("/chat/completions"), 1);

    let stats = request(r.port, "GET", "/router/stats", None, &[]);
    let v = stats.json();
    let acc = v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "plan-tokens")
        .unwrap()
        .clone();
    assert_eq!(acc["kind"], "plans");
    assert_eq!(acc["quota"]["unit"], "tokens");
    assert_eq!(acc["quota"]["source"], "local", "measured from the local ledger");
    let monthly = acc["quota"]["monthly"]["percent"].as_f64().unwrap();
    assert!(monthly >= 100.0, "monthly percent={}", monthly);
    assert_eq!(v["counters"]["plans_used"], 1);
    assert_eq!(v["counters"]["cash_used"], 1);
}

#[test]
fn tight_plan_is_reserved_for_peak_and_defers_to_cash_off_peak() {
    let plan = Mock::start();
    let cash = Mock::start();
    // the plan's own usage API reports 95% used -> measurable, and not surplus
    plan.set_quota_pct(95.0);
    let cfg = |port: u16| {
        format!(
            r#"server:
  host: 127.0.0.1
  port: {port}
log:
  level: debug
  file: ""
router:
  mode: auto
  idle_prefer: surplus_first
  surplus_max_pct: 80
  surplus_projection: false
plans:
  - name: plan-tight
    url: {p}/v1
    provider: opencodego
    key: k1
    model: deepseek-flash
    mode: both
    order: 10
    quota: {{ unit: usd, rolling: 12, weekly: 30, monthly: 60, probe: usage }}
fallback:
  - name: cash
    url: {c}
    key: k2
    model: deepseek-flash
    mode: both
    rule: always
"#,
            port = port,
            p = plan.url(),
            c = cash.url()
        )
    };
    // off-peak: the plan is tight, so spend the cheap cash instead of the quota we want for peak
    let off = start_router("tight-offpeak", "2026-09-16T05:00:00Z", &cfg);
    wait_quota(off.port, "plan-tight");
    let r1 = request(off.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    if r1.status != 200 {
        dump_log(&off);
    }
    assert_eq!(
        r1.header("x-router-account"),
        Some("cash"),
        "off-peak + tight plan must use cash first"
    );
    let log = std::fs::read_to_string(&off.log).unwrap_or_default();
    assert!(
        log.contains("buckets=plans-surplus>fallback>plans-tight"),
        "off-peak bucket order should be plans-surplus > fallback > plans-tight, log tail: {}",
        log.lines().rev().take(4).collect::<Vec<_>>().join(" | ")
    );

    assert_eq!(
        plan.hits("/v1/chat/completions"),
        0,
        "the off-peak request must not have touched the tight plan"
    );
    assert_eq!(cash.hits("/chat/completions"), 1);

    // peak: cash is expensive, so the plan is used even though it is tight
    let peak = start_router("tight-peak", "2026-09-16T02:00:00Z", &cfg);
    wait_quota(peak.port, "plan-tight");
    let r2 = request(peak.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    if r2.status != 200 {
        dump_log(&peak);
    }
    assert_eq!(r2.header("x-router-account"), Some("plan-tight"), "peak keeps the plan");
    assert_eq!(r2.header("x-router-kind"), Some("plans"));
    assert_eq!(
        plan.hits("/v1/chat/completions"),
        1,
        "the peak request must have used the plan"
    );
    assert_eq!(cash.hits("/chat/completions"), 1, "only the off-peak request used cash");
}

#[test]
fn session_affinity_keeps_a_conversation_on_one_endpoint() {
    let ep_a = Mock::start();
    let ep_b = Mock::start();
    let cfg = |port: u16, affinity: bool| {
        format!(
            r#"server:
  host: 127.0.0.1
  port: {port}
log:
  level: debug
  file: ""
router:
  mode: plans
  selection: weighted
  session_affinity: {aff}
  session_affinity_ttl_secs: 600
  session_fallback: process
plans:
  - name: ep-a
    url: {a}/v1
    key: k1
    model: model-a
    inject_session: true
    mode: both
    order: 10
    weight: 50
  - name: ep-b
    url: {b}/v1
    key: k2
    model: model-b
    inject_session: true
    mode: both
    order: 10
    weight: 50
fallback:
  - name: cash
    url: {a}/v1
    key: k3
    model: model-a
    mode: both
    rule: never
"#,
            port = port,
            aff = affinity,
            a = ep_a.url(),
            b = ep_b.url()
        )
    };

    // with affinity ON, the same conversation must keep landing on the same endpoint
    let r = start_router("affinity-on", "2026-09-16T02:00:00Z", &|port| cfg(port, true));
    let hdr = [("x-session-id", "conv-affinity-1")];
    let first = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &hdr);
    assert_eq!(first.status, 200);
    let chosen = first.header("x-router-account").unwrap().to_string();
    for _ in 0..4 {
        let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &hdr);
        assert_eq!(
            resp.header("x-router-account"),
            Some(chosen.as_str()),
            "the conversation must stay on one endpoint"
        );
    }
    let total = ep_a.hits("/v1/chat/completions") + ep_b.hits("/v1/chat/completions");
    assert_eq!(total, 5, "all five requests went to an endpoint");

    // and the endpoint that was used must have received the upstream session header
    let used = if chosen == "ep-a" { &ep_a } else { &ep_b };
    assert_eq!(
        used.last("/v1/chat/completions").unwrap().header("x-opencode-session"),
        Some("conv-affinity-1"),
        "the conversation id is forwarded so the upstream can scope its cache"
    );

    // a different conversation is free to land elsewhere (here: the other endpoint or the same,
    // both are legal - we only assert it is served)
    let other = request(
        r.port,
        "POST",
        "/v1/chat/completions",
        Some(CHAT_BODY),
        &[("x-session-id", "conv-affinity-2")],
    );
    assert_eq!(other.status, 200);
}

// ===================================================================== web console API

/// The console edits the same file the router reads, so the critical property is: a save is
/// validated first, applied atomically, and picked up by the live process.
#[test]
fn admin_api_reads_config_and_hides_secrets() {
    let go = Mock::start();
    let r = start_router("api-config", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &go.url(), "opencodego", 80)
    });
    let resp = request(r.port, "GET", "/api/config", None, &[]);
    assert_eq!(resp.status, 200);
    let v = resp.json();
    assert!(v["endpoints"].as_array().unwrap().len() >= 2);
    assert_eq!(v["server"]["port"].as_u64().unwrap(), r.port as u64, "the live port is reported");
    // A literal key must never travel back to the browser.
    for ep in v["endpoints"].as_array().unwrap() {
        let key = ep["key"].as_str().unwrap_or("");
        assert!(!key.starts_with("sk-"), "the key leaked to the client: {}", key);
        assert_eq!(key, "********");
    }
    assert_eq!(v["ui"]["managed"], true);
}

/// A save round-trips through the file and the running process, and the stored keys survive
/// (the browser only ever saw the placeholder).
#[test]
fn admin_api_save_round_trips_without_destroying_keys() {
    let go = Mock::start();
    let r = start_router("api-save", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &go.url(), "opencodego", 80)
    });
    let mut doc = request(r.port, "GET", "/api/config", None, &[]).json();
    doc["router"]["surplus_max_pct"] = serde_json::json!(77);
    doc["router"]["selection"] = serde_json::json!("round_robin");
    let resp = request(r.port, "PUT", "/api/config", Some(&doc.to_string()), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200, "save failed: {}", resp.body);
    let saved = resp.json();
    assert_eq!(saved["saved"], true);
    assert_eq!(saved["reloaded"], true);

    // the change is live (surplus_max_pct is reported by stats; selection only by the config API)
    let stats = request(r.port, "GET", "/router/stats", None, &[]).json();
    assert_eq!(stats["router"]["surplus_max_pct"].as_f64().unwrap(), 77.0);
    let live = request(r.port, "GET", "/api/config", None, &[]).json();
    assert_eq!(live["router"]["selection"].as_str(), Some("round_robin"));

    // and the keys still work: a proxied request still reaches the upstream
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    assert_eq!(resp.status, 200);
    assert!(go.hits("/v1/chat/completions") > 0);

    // a backup was taken before the overwrite
    let backups = request(r.port, "GET", "/api/backups", None, &[]).json();
    assert!(
        !backups["backups"].as_array().unwrap().is_empty(),
        "saving must leave a backup"
    );
}

/// Invalid documents must be rejected *before* the working config is touched.
#[test]
fn admin_api_rejects_bad_config_and_keeps_the_old_one() {
    let go = Mock::start();
    let r = start_router("api-reject", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &go.url(), "opencodego", 80)
    });
    let before = request(r.port, "GET", "/api/config", None, &[]).json();

    // no endpoints at all
    let empty = serde_json::json!({
        "server": {"host": "127.0.0.1", "port": r.port},
        "router": {},
        "compat": {},
        "endpoints": []
    });
    let resp = request(r.port, "PUT", "/api/config", Some(&empty.to_string()), &[]);
    assert_eq!(resp.status, 400);
    assert!(resp.json()["error"]["message"].is_string());

    // a new endpoint without a key
    let mut bad = before.clone();
    bad["endpoints"].as_array_mut().unwrap().push(serde_json::json!({
        "name": "no-key",
        "kind": "plans",
        "url": "https://example.com/v1",
        "key": "",
        "model": "m",
        "modes": ["both"],
        "order": 99,
        "weight": 10,
        "quota": {},
        "headers": {},
        "rules": ["always"]
    }));
    let resp = request(r.port, "PUT", "/api/config", Some(&bad.to_string()), &[]);
    assert_eq!(resp.status, 400, "a keyless new endpoint must be refused");

    // invalid raw YAML
    let resp = request(
        r.port,
        "PUT",
        "/api/config/raw",
        Some(&serde_json::json!({"text": "server: [unclosed\n  bad"}).to_string()),
        &[],
    );
    assert_eq!(resp.status, 400);

    // the live config is untouched and the router still serves
    let after = request(r.port, "GET", "/api/config", None, &[]).json();
    assert_eq!(
        before["endpoints"].as_array().unwrap().len(),
        after["endpoints"].as_array().unwrap().len(),
        "a rejected save must not change the config"
    );
    assert_eq!(request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]).status, 200);
}

/// Enable/disable is applied immediately, and a disabled endpoint stops being selected while
/// the others keep serving.
#[test]
fn admin_api_can_disable_and_reenable_an_endpoint() {
    let a = Mock::start();
    let b = Mock::start();
    let r = start_router("api-toggle", "2026-09-16T02:00:00Z", &|port| {
        format!(
            r#"server:
  host: 127.0.0.1
  port: {port}
log:
  level: debug
plans:
  - name: ep-a
    url: {a}/v1
    provider: opencodego
    key: sk-aaa
    model: deepseek-flash
    mode: both
    order: 10
    weight: 90
    inject_session: true
    quota: {{ unit: usd, rolling: 12, weekly: 30, monthly: 60, probe: usage }}
  - name: ep-b
    url: {b}/v1
    provider: opencodego
    key: sk-bbb
    model: deepseek-flash
    mode: both
    order: 20
    weight: 10
    inject_session: true
    quota: {{ unit: usd, rolling: 12, weekly: 30, monthly: 60, probe: usage }}
"#,
            port = port,
            a = a.url(),
            b = b.url()
        )
    });

    let resp = request(
        r.port,
        "POST",
        "/api/endpoints/ep-a/state",
        Some(&serde_json::json!({"enabled": false}).to_string()),
        &[],
    );
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["enabled"], false);

    // ep-a must no longer be chosen; every request now lands on ep-b
    for _ in 0..3 {
        let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("x-router-account"), Some("ep-b"));
    }
    assert_eq!(a.hits("/v1/chat/completions"), 0, "the disabled endpoint must not be used");

    // re-enabling brings it back
    let resp = request(
        r.port,
        "POST",
        "/api/endpoints/ep-a/state",
        Some(&serde_json::json!({"enabled": true}).to_string()),
        &[],
    );
    assert_eq!(resp.status, 200);
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    assert_eq!(resp.header("x-router-account"), Some("ep-a"));
}

/// Duplicating an endpoint copies it under a free name and persists it.
#[test]
fn admin_api_duplicates_an_endpoint() {
    let go = Mock::start();
    let r = start_router("api-dup", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &go.url(), "opencodego", 80)
    });
    let before = request(r.port, "GET", "/api/config", None, &[]).json();
    let first = before["endpoints"][0]["name"].as_str().unwrap().to_string();

    let resp = request(
        r.port,
        "POST",
        &format!("/api/endpoints/{}/duplicate", first),
        Some("{}"),
        &[],
    );
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200, "duplicate failed: {}", resp.body);
    let copy = resp.json()["name"].as_str().unwrap().to_string();
    assert_ne!(copy, first);

    let after = request(r.port, "GET", "/api/config", None, &[]).json();
    assert_eq!(
        after["endpoints"].as_array().unwrap().len(),
        before["endpoints"].as_array().unwrap().len() + 1
    );
    // the copy is a real endpoint the router knows about
    let stats = request(r.port, "GET", "/router/stats", None, &[]).json();
    assert!(stats["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["name"].as_str() == Some(copy.as_str())));
}

/// A backup can be listed, downloaded and restored, and restoring rolls the live config back.
#[test]
fn admin_api_backup_list_download_and_restore() {
    let go = Mock::start();
    let r = start_router("api-backup", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &go.url(), "opencodego", 80)
    });
    let original = request(r.port, "GET", "/api/config", None, &[]).json();
    let original_pct = original["router"]["surplus_max_pct"].as_f64().unwrap();

    // take a backup of the current state
    let resp = request(r.port, "POST", "/api/backups", Some("{}"), &[]);
    assert_eq!(resp.status, 200, "backup failed: {}", resp.body);
    let name = resp.json()["name"].as_str().unwrap().to_string();

    // pull it back out as a file
    let dl = request(r.port, "GET", &format!("/api/backups/{}", name), None, &[]);
    assert_eq!(dl.status, 200);
    assert!(dl.body.contains("surplus_max_pct"));

    // change the config, then restore the backup
    let mut doc = original.clone();
    doc["router"]["surplus_max_pct"] = serde_json::json!(12);
    let resp = request(r.port, "PUT", "/api/config", Some(&doc.to_string()), &[]);
    assert_eq!(resp.status, 200);
    assert_eq!(
        request(r.port, "GET", "/router/stats", None, &[]).json()["router"]["surplus_max_pct"]
            .as_f64()
            .unwrap(),
        12.0
    );

    let resp = request(r.port, "POST", &format!("/api/backups/{}/restore", name), Some("{}"), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200, "restore failed: {}", resp.body);
    assert_eq!(
        request(r.port, "GET", "/router/stats", None, &[]).json()["router"]["surplus_max_pct"]
            .as_f64()
            .unwrap(),
        original_pct,
        "restoring must roll the live policy back"
    );
}

/// The API refuses to be talked into reading files outside the backup directory.
#[test]
fn admin_api_rejects_path_traversal() {
    let go = Mock::start();
    let r = start_router("api-traversal", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &go.url(), "opencodego", 80)
    });
    for path in [
        "/api/backups/..%2Fconfig.yaml",
        "/api/backups/../config.yaml",
        "/api/backups/evil.sh/restore",
        "/api/backups/notaconfig.yaml/restore",
        "/api/config/../../config.yaml",
    ] {
        let resp = request(r.port, "GET", path, None, &[]);
        assert!(resp.status >= 400, "{} must not be served (got {})", path, resp.status);
    }
    // and the live config is still intact
    assert_eq!(request(r.port, "GET", "/api/config", None, &[]).status, 200);
}

/// The console itself is embedded in the binary and served from "/".
#[test]
fn web_console_is_served_and_static_assets_are_sane() {
    let go = Mock::start();
    let r = start_router("api-ui", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &go.url(), "opencodego", 80)
    });
    let index = request(r.port, "GET", "/", None, &[]);
    assert_eq!(index.status, 200);
    assert_eq!(index.header("content-type"), Some("text/html; charset=utf-8"));
    assert!(index.body.contains("<!DOCTYPE html>"), "the console must be served at /");
    assert_eq!(index.header("x-frame-options"), Some("DENY"));

    // a client-side route resolves to its exported page
    let settings = request(r.port, "GET", "/settings/", None, &[]);
    assert_eq!(settings.status, 200);
    assert_eq!(settings.header("content-type"), Some("text/html; charset=utf-8"));

    // the plain-text banner is still reachable for scripts
    let banner = request(r.port, "GET", "/router/banner", None, &[]);
    assert_eq!(banner.status, 200);
    assert!(banner.body.contains("ar-OCG-Router"));

    // /health and the proxy are untouched by the console
    assert_eq!(request(r.port, "GET", "/health", None, &[]).status, 200);
    let chat = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    assert_eq!(chat.status, 200);
    assert!(chat.header("x-router-account").is_some());
}

/// The console is only for the person running the router: when gateway keys are configured,
/// they gate the admin API too.
#[test]
fn admin_api_is_gated_by_client_keys() {
    let go = Mock::start();
    let r = start_router("api-auth", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &go.url(), "opencodego", 80).replace(
            "  max_connections: 64",
            "  max_connections: 64\n  client_keys:\n    - sk-router-secret",
        )
    });
    let resp = request(r.port, "GET", "/api/config", None, &[]);
    assert_eq!(resp.status, 401, "the admin API must require the gateway key");
    let resp = request(
        r.port,
        "GET",
        "/api/config",
        None,
        &[("authorization", "Bearer sk-router-secret")],
    );
    assert_eq!(resp.status, 200);
}

/// Browser noise (favicon, a typo, a stale asset) must be answered locally. Before this was
/// pinned, an unmatched GET fell through to the proxy and was forwarded upstream as a request.
#[test]
fn unmatched_non_v1_paths_are_not_forwarded_upstream() {
    let go = Mock::start();
    let r = start_router("no-upstream-leak", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &go.url(), "opencodego", 80)
    });
    let before = go.hits("/favicon.ico") + go.posts().len();

    // An asset that does not exist is a hard 404 - never a proxied request.
    for path in ["/favicon.ico", "/_next/static/chunks/missing.js", "/nope.png"] {
        let resp = request(r.port, "GET", path, None, &[]);
        assert_eq!(resp.status, 404, "{} must be answered locally", path);
    }
    // A path without a file extension is a client-side route: the SPA shell is served.
    let spa = request(r.port, "GET", "/definitely-not-a-route", None, &[]);
    assert_eq!(spa.status, 200);
    assert_eq!(spa.header("content-type"), Some("text/html; charset=utf-8"));

    let after = go.hits("/favicon.ico") + go.posts().len();
    assert_eq!(
        before, after,
        "answering a console request must never generate upstream traffic"
    );

    // the API and the proxy still work
    assert_eq!(request(r.port, "GET", "/v1/models", None, &[]).status, 200);
    assert_eq!(request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]).status, 200);
}

/// The model picker and the library: the catalog is advisory, the library is persistent, and
/// neither may be confused with the endpoint's own authoritative model id.
#[test]
fn model_library_and_catalog_endpoints() {
    let go = Mock::start();
    // The mock already answers /v1/models with two ids.
    let r = start_router("models-api", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &go.url(), "opencodego", 80)
    });

    // ---- the shipped library is available without any setup
    let lib = request(r.port, "GET", "/api/library", None, &[]);
    assert_eq!(lib.status, 200);
    let lib = lib.json();
    let entries = lib["models"].as_array().unwrap();
    assert!(!entries.is_empty(), "a built-in library must ship with the binary");
    let flash = entries
        .iter()
        .find(|m| m["id"].as_str() == Some("deepseek-flash"))
        .expect("deepseek-flash must be in the built-in library");
    assert!(flash["context_tokens"].as_u64().unwrap() > 0);
    assert!(flash["reasoning_levels"].as_array().unwrap().len() > 0);
    assert!(flash["aliases"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a.as_str() == Some("deepseek-v4.1-flash")));

    // ---- the live catalog comes from the endpoint and is annotated by the library
    let cat = request(r.port, "GET", "/api/endpoints/go-1/models", None, &[]);
    assert_eq!(cat.status, 200);
    let cat = cat.json();
    assert_eq!(cat["ok"], true);
    assert_eq!(cat["current"].as_str(), Some("deepseek-flash"));
    let ids: Vec<&str> = cat["models"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m.as_str())
        .collect();
    assert!(ids.contains(&"deepseek-v4-pro"), "ids={:?}", ids);
    // Both ids are known to the library and share a context size, so they rank above anything
    // unknown and keep library order. Ranking must never drop a model the upstream listed.
    assert_eq!(ids.len(), 2, "every upstream id must survive ranking: {:?}", ids);
    assert!(ids.contains(&"deepseek-v4-flash"), "ids={:?}", ids);

    // ---- an unknown endpoint is a 404, not an empty catalog
    assert_eq!(request(r.port, "GET", "/api/endpoints/nope/models", None, &[]).status, 404);

    // ---- saving is validated: duplicates and an empty library are refused
    let dup = serde_json::json!({"models": [{"id": "a"}, {"id": "A"}]});
    let resp = request(r.port, "PUT", "/api/library", Some(&dup.to_string()), &[]);
    assert_eq!(resp.status, 400);
    let empty = serde_json::json!({"models": []});
    assert_eq!(request(r.port, "PUT", "/api/library", Some(&empty.to_string()), &[]).status, 400);

    // ---- a valid save persists and round-trips
    let custom = serde_json::json!({"models": [{
        "id": "my-model",
        "label": "My Model",
        "context_tokens": 65536,
        "max_output_tokens": 4096,
        "reasoning_levels": ["low", "high"],
        "input_modalities": ["text", "image"],
        "output_modalities": ["text"],
        "aliases": ["my-model-alias"],
        "tags": ["custom"],
        "notes": "hello",
    }]});
    let resp = request(r.port, "PUT", "/api/library", Some(&custom.to_string()), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200, "save failed: {}", resp.body);
    let after = request(r.port, "GET", "/api/library", None, &[]).json();
    let m = &after["models"][0];
    assert_eq!(m["id"].as_str(), Some("my-model"));
    assert_eq!(m["context_tokens"].as_u64(), Some(65536));
    assert_eq!(m["reasoning_levels"].as_array().unwrap().len(), 2);
    assert_eq!(m["input_modalities"].as_array().unwrap().len(), 2);

    // The library lives beside the config, not inside it: rewriting config.yaml must not
    // destroy it (the console regenerates the config on every save).
    let mut doc = request(r.port, "GET", "/api/config", None, &[]).json();
    doc["router"]["surplus_max_pct"] = serde_json::json!(42);
    assert_eq!(request(r.port, "PUT", "/api/config", Some(&doc.to_string()), &[]).status, 200);
    let survived = request(r.port, "GET", "/api/library", None, &[]).json();
    assert_eq!(survived["models"][0]["id"].as_str(), Some("my-model"));
}

/// The console's YAML writer carries its own copy of every default. If one drifts from the
/// parser's, saving a partial document silently changes a setting the user never touched.
/// This pins max_body_bytes, which had drifted to 256 MiB against a 64 MiB parser default.
#[test]
fn saving_a_partial_document_keeps_parser_defaults() {
    let go = Mock::start();
    let r = start_router("defaults-drift", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &go.url(), "opencodego", 80)
    });
    let before = request(r.port, "GET", "/api/config", None, &[]).json();
    let body = before["server"]["max_body_bytes"].as_u64().unwrap();
    assert_eq!(body, 64 * 1024 * 1024, "the parser default is 64 MiB");

    // Save a document that omits the field entirely, as a hand-written API client might.
    let mut doc = before.clone();
    doc["server"].as_object_mut().unwrap().remove("max_body_bytes");
    let resp = request(r.port, "PUT", "/api/config", Some(&doc.to_string()), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200, "save failed: {}", resp.body);

    let after = request(r.port, "GET", "/api/config", None, &[]).json();
    assert_eq!(
        after["server"]["max_body_bytes"].as_u64().unwrap(),
        64 * 1024 * 1024,
        "an omitted field must fall back to the parser default, not the writer's guess"
    );

    // The rest of the settings must survive the same round trip untouched.
    for key in ["host", "port", "max_connections", "idle_timeout_secs", "read_timeout_secs"] {
        assert_eq!(before["server"][key], after["server"][key], "{} changed", key);
    }
}
/// The console writes the whole document back on every save, so a faithful round trip is the
/// contract: GET -> PUT (unchanged) -> GET must be an identity. This is the test that would have
/// caught the peak_windows / plan-rule / max_body_bytes losses in one go.
#[test]
fn console_round_trip_is_identity() {
    let go = Mock::start();
    let fb = Mock::start();
    let r = start_router("roundtrip", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "surplus_first", 80)
    });
    let before = request(r.port, "GET", "/api/config", None, &[]).json();
    let resp = request(r.port, "PUT", "/api/config", Some(&before.to_string()), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200, "save failed: {}", resp.body);
    let after = request(r.port, "GET", "/api/config", None, &[]).json();

    for section in ["server", "log", "router", "compat"] {
        assert_eq!(before[section], after[section], "section {} changed", section);
    }
    let a = before["endpoints"].as_array().unwrap();
    let b = after["endpoints"].as_array().unwrap();
    assert_eq!(a.len(), b.len(), "an endpoint disappeared");
    for e in a {
        let name = e["name"].as_str().unwrap();
        let same = b
            .iter()
            .find(|x| x["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("endpoint {} disappeared", name));
        assert_eq!(e, same, "endpoint {} changed", name);
    }
}

/// Non-default policy and per-endpoint settings must survive a console save. Each field here was
/// silently reset before: peak_windows was hard-coded, a plan's rule list was not written at all
/// (only cash accounts got `rule:`), no_error_fallback was written as a top-level key the parser
/// never reads, and max_body_mb was dropped in favour of a key the parser ignored.
#[test]
fn console_save_keeps_non_default_policy_and_rules() {
    let go = Mock::start();
    let fb = Mock::start();
    let r = start_router("keep-nondefault", "2026-09-16T02:00:00Z", &|port| {
        format!(
            r#"server:
  host: 127.0.0.1
  port: {port}
  max_connections: 64
  max_body_mb: 7
log:
  level: debug
  file: ""
router:
  mode: auto
  peak_windows: "Mon-Fri 09:00-12:00 UTC"
  request_timeout_secs: 123
  user_agent: "custom-agent/9.9"
plans:
  - name: go-1
    url: {go}
    key: sk-go-test
    model: deepseek-flash
    mode: both
    provider: opencodego
    rule: [offpeak]
    weight: 60
fallback:
  - name: ds-1
    url: {fb}
    key: sk-fb-test
    model: deepseek-flash
    mode: both
    provider: deepseek
    rule: [always, no_error_fallback]
    weight: 50
"#,
            port = port,
            go = format!("{}/v1", go.url()),
            fb = fb.url()
        )
    });

    let before = request(r.port, "GET", "/api/config", None, &[]).json();
    assert_eq!(before["server"]["max_body_bytes"].as_u64(), Some(7 * 1024 * 1024));
    assert_eq!(before["router"]["peak_windows"].as_str(), Some("Mon-Fri 09:00-12:00 UTC"));
    assert_eq!(before["router"]["request_timeout_secs"].as_u64(), Some(123));
    assert_eq!(before["router"]["user_agent"].as_str(), Some("custom-agent/9.9"));

    let resp = request(r.port, "PUT", "/api/config", Some(&before.to_string()), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200, "save failed: {}", resp.body);
    let after = request(r.port, "GET", "/api/config", None, &[]).json();

    assert_eq!(
        after["server"]["max_body_bytes"].as_u64(),
        Some(7 * 1024 * 1024),
        "max_body_mb was dropped and the body limit reverted to the default"
    );
    assert_eq!(
        after["router"]["peak_windows"].as_str(),
        Some("Mon-Fri 09:00-12:00 UTC"),
        "a custom peak window was overwritten by the built-in default"
    );
    assert_eq!(after["router"]["request_timeout_secs"].as_u64(), Some(123));
    assert_eq!(after["router"]["user_agent"].as_str(), Some("custom-agent/9.9"));

    let eps = after["endpoints"].as_array().unwrap();
    let plan = eps.iter().find(|e| e["name"] == "go-1").expect("plan missing");
    assert_eq!(
        plan["rules"],
        serde_json::json!(["offpeak"]),
        "a plan's rule list was reset (rules used to be written for cash accounts only)"
    );
    let cash = eps.iter().find(|e| e["name"] == "ds-1").expect("cash missing");
    assert_eq!(
        cash["no_error_fallback"],
        serde_json::json!(true),
        "no_error_fallback was written as a key the parser never reads"
    );
}
/// Renaming an endpoint must not lose its credential. The console never sees the real key - it
/// round-trips the "********" placeholder - so the save carries the previous name to let the
/// server resolve it. Before this, every rename failed with
/// `endpoint "...": key is required for a new endpoint`.
#[test]
fn renaming_an_endpoint_keeps_its_key() {
    let go = Mock::start();
    let fb = Mock::start();
    let r = start_router("rename-keeps-key", "2026-09-16T02:00:00Z", &|port| {
        config_peak_offpeak(port, &format!("{}/v1", go.url()), &fb.url(), "surplus_first", 80)
    });
    let mut doc = request(r.port, "GET", "/api/config", None, &[]).json();

    // Exactly what the console sends: the placeholder key, a new name, and the old one.
    let eps = doc["endpoints"].as_array_mut().unwrap();
    let plan = eps
        .iter_mut()
        .find(|e| e["name"] == "go-1")
        .expect("plan endpoint missing");
    assert_eq!(
        plan["key"].as_str(),
        Some("********"),
        "the console must never receive the real key"
    );
    plan["name"] = serde_json::json!("go-renamed");
    plan["renamed_from"] = serde_json::json!("go-1");

    let resp = request(r.port, "PUT", "/api/config", Some(&doc.to_string()), &[]);
    if resp.status != 200 {
        dump_log(&r);
    }
    assert_eq!(resp.status, 200, "rename was rejected: {}", resp.body);

    let after = request(r.port, "GET", "/api/config", None, &[]).json();
    let names: Vec<&str> = after["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert!(names.contains(&"go-renamed"), "rename did not stick: {:?}", names);
    assert!(!names.contains(&"go-1"), "the old name survived: {:?}", names);

    // The proof that the credential survived: the request reaches the upstream with the
    // configured key, not an empty one.
    let resp = request(r.port, "POST", "/v1/chat/completions", Some(CHAT_BODY), &[]);
    assert_eq!(resp.status, 200, "request after rename failed: {}", resp.body);
    let sent = go
        .posts()
        .last()
        .map(|p| p.header("authorization").unwrap_or("").to_string())
        .unwrap_or_default();
    assert_eq!(
        sent, "Bearer sk-go-test",
        "the renamed endpoint lost its key (upstream saw {:?})",
        sent
    );
}


