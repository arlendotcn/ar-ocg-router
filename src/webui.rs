//! The embedded web console.
//!
//! The whole UI (a static Next.js export) is compiled into the binary, so a single executable
//! still serves everything - no asset directory to ship or lose. When the crate is built
//! without the UI (for example a quick `cargo build` during development), the module falls back
//! to reading `web/out` from disk so `next dev` and a local run still work.

use std::borrow::Cow;

use crate::httpd::{Request, Responder};

#[cfg(feature = "webui")]
static EMBEDDED: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/web/out");

/// A resolved asset: its bytes and the content type to serve it with.
pub struct Asset {
    pub bytes: Cow<'static, [u8]>,
    pub content_type: &'static str,
    /// Content-hashed assets can be cached forever; index.html must not be.
    pub immutable: bool,
}

pub fn is_embedded() -> bool {
    cfg!(feature = "webui")
}

fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

/// Look up one file. Returns None when it does not exist (the caller then tries index.html, so
/// client-side routes deep-link correctly).
pub fn lookup(path: &str) -> Option<Asset> {
    let clean = sanitize(path)?;
    #[cfg(feature = "webui")]
    {
        if let Some(file) = EMBEDDED.get_file(&clean) {
            let bytes = file.contents();
            let ct = content_type(&clean);
            let immutable = clean.starts_with("_next/static/");
            // A static slice from the binary needs no copy at all.
            let bytes = Cow::Borrowed(unsafe {
                std::slice::from_raw_parts(bytes.as_ptr(), bytes.len())
            });
            return Some(Asset { bytes, content_type: ct, immutable });
        }
        return None;
    }
    #[cfg(not(feature = "webui"))]
    {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web/out");
        let full = root.join(&clean);
        let bytes = std::fs::read(&full).ok()?;
        let ct = content_type(&clean);
        let immutable = clean.starts_with("_next/static/");
        Some(Asset { bytes: Cow::Owned(bytes), content_type: ct, immutable })
    }
}

/// index.html for any unknown path (client-side routing).
pub fn index() -> Option<Asset> {
    lookup("index.html")
}

/// Reject traversal and absolute paths; keep the rest verbatim so hashed asset names match.
fn sanitize(path: &str) -> Option<String> {
    let path = path.trim_start_matches('/');
    let path = path.split('?').next().unwrap_or(path);
    if path.is_empty() {
        return Some("index.html".to_string());
    }
    if path.contains("..") || path.contains('\\') || path.contains('\0') {
        return None;
    }
    if path.starts_with("_next/") || path.starts_with("assets/") || !path.contains('/') {
        return Some(path.to_string());
    }
    // Nested routes exist as directories in the export (endpoints/index.html).
    Some(path.trim_end_matches('/').to_string())
}

/// Try to serve `req` from the console. Returns false when nothing matched.
pub fn serve(req: &Request, out: &mut Responder) -> bool {
    if req.method != "GET" && req.method != "HEAD" {
        return false;
    }
    let path = req.path.trim_start_matches('/');
    if path.is_empty() {
        return serve_asset(out, req, "index.html");
    }
    // A directory route in a static export is <route>/index.html.
    if let Some(asset) = lookup(path) {
        return send_asset(out, req, asset);
    }
    let nested = format!("{}/index.html", path.trim_end_matches('/'));
    if let Some(asset) = lookup(&nested) {
        return send_asset(out, req, asset);
    }
    // Unknown path: hand it to the SPA only when it is not obviously an asset request.
    if path.contains('.') {
        return false;
    }
    match index() {
        Some(asset) => send_asset(out, req, asset),
        None => false,
    }
}

fn serve_asset(out: &mut Responder, req: &Request, name: &str) -> bool {
    match lookup(name) {
        Some(asset) => send_asset(out, req, asset),
        None => false,
    }
}

fn send_asset(out: &mut Responder, req: &Request, asset: Asset) -> bool {
    let cache = if asset.immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let headers = vec![
        ("Content-Type".to_string(), asset.content_type.to_string()),
        ("Cache-Control".to_string(), cache.to_string()),
        ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
        // The console is same-origin only; keep it out of frames and off other origins.
        ("X-Frame-Options".to_string(), "DENY".to_string()),
        ("Referrer-Policy".to_string(), "no-referrer".to_string()),
    ];
    let len = asset.bytes.len();
    if out.send_head(200, &headers, Some(len), req.keep_alive, &req.version).is_err() {
        return true;
    }
    if req.method != "HEAD" {
        let _ = out.send_body(&asset.bytes);
    }
    let _ = out.finish();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_traversal() {
        assert!(sanitize("../config.yaml").is_none());
        assert!(sanitize("_next/../../etc/passwd").is_none());
        assert!(sanitize("a\\b").is_none());
        assert_eq!(sanitize("/").unwrap(), "index.html");
        assert_eq!(sanitize("/endpoints").unwrap(), "endpoints");
        assert_eq!(sanitize("/_next/static/chunks/a.js").unwrap(), "_next/static/chunks/a.js");
    }

    #[test]
    fn content_types_cover_the_export() {
        assert!(content_type("a.js").starts_with("text/javascript"));
        assert_eq!(content_type("a.woff2"), "font/woff2");
        assert_eq!(content_type("a.css"), "text/css; charset=utf-8");
    }
}
