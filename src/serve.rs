use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    response::Response,
    Router,
};
use tokio::signal;
use tower_http::compression::CompressionLayer;

use crate::html;
use crate::web_assets;

// ---------------------------------------------------------------------------
// Tailscale detection
// ---------------------------------------------------------------------------

/// Parse raw bytes from `tailscale status --json` and extract the trimmed
/// `Self.DNSName`.
///
/// Returns `Err(reason)` (a short lowercase slug) when:
/// - The bytes are not valid JSON.
/// - The `Self` key or `DNSName` field is absent.
/// - `DNSName` is empty after stripping the trailing `.`.
///
/// All error paths use `?`/`Result` propagation — zero `unwrap()`/`expect()`.
pub fn parse_tailscale_dns_name(output: &[u8]) -> Result<String, String> {
    let json: serde_json::Value =
        serde_json::from_slice(output).map_err(|e| format!("json-parse: {e}"))?;

    let dns_name = json
        .get("Self")
        .and_then(|s| s.get("DNSName"))
        .and_then(|d| d.as_str())
        .ok_or_else(|| "no-DNSName".to_owned())?;

    let trimmed = dns_name.trim_end_matches('.');
    if trimmed.is_empty() {
        return Err("empty-DNSName".to_owned());
    }

    Ok(trimmed.to_owned())
}

/// Attempt to obtain the Tailscale hostname by running `tailscale status --json`.
///
/// Any subprocess error, JSON parse failure, or missing/empty `DNSName` is
/// silently treated as "no Tailscale available" and logged at debug level.
/// This function never panics.
fn tailscale_dns_name() -> Option<String> {
    let output = match std::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[tailscale] skipped reason=subprocess-error: {e}");
            return None;
        }
    };

    match parse_tailscale_dns_name(&output.stdout) {
        Ok(name) => Some(name),
        Err(reason) => {
            eprintln!("[tailscale] skipped reason={reason}");
            None
        }
    }
}

/// Maximum number of consecutive ports to try before giving up.
const MAX_PORT_ATTEMPTS: u16 = 100;

/// Maximum file size that will be read and served (16 MiB).
pub const MAX_FILE_SIZE: u64 = 16 * 1024 * 1024;

/// Minimal server configuration (extended by later issues).
pub struct AppConfig;

/// Shared application state passed to all request handlers via `Arc<AppState>`.
pub struct AppState {
    /// Base directory from which markdown files and assets are served.
    pub serve_root: PathBuf,
    /// Canonicalized `serve_root` used for symlink-safe containment checks (R1).
    pub canonical_root: PathBuf,
    /// The primary markdown entry file.
    pub entry_file: PathBuf,
    /// Server configuration.
    pub config: AppConfig,
    /// Precomputed strong ETag for the embedded CSS asset (`/assets/mdmd.css`).
    pub css_etag: String,
    /// Precomputed strong ETag for the embedded JS asset (`/assets/mdmd.js`).
    pub js_etag: String,
    /// `Last-Modified` timestamp for embedded static assets, derived from the
    /// binary's own modification time.  Falls back to the Unix epoch.
    pub asset_mtime: SystemTime,
}

// ---------------------------------------------------------------------------
// Cache validation helpers
// ---------------------------------------------------------------------------

/// Compute a 64-bit FNV-1a hash of `data`.
///
/// FNV-1a is used here for its speed — it is suitable for cache-validation
/// ETags but NOT for any cryptographic purpose.  Algorithm:
///   hash = offset_basis
///   for each byte: hash ^= byte; hash *= FNV_prime
/// with `offset_basis` = 14695981039346656037 and `FNV_prime` = 1099511628211
/// (the standard 64-bit FNV-1a constants).
///
/// To change the hash algorithm, replace only this function and update the
/// comment above — all callers go through `compute_etag`.
pub fn fnv1a_64(data: &[u8]) -> u64 {
    // 64-bit FNV-1a constants from the FNV specification.
    const FNV_PRIME: u64 = 1099511628211;
    const FNV_OFFSET_BASIS: u64 = 14695981039346656037;
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Format `data` as a strong HTTP ETag: a quoted 16-char hex string.
///
/// Returns a value of the form `"<16 hex chars>"` (strong ETag, RFC 7232 §2.3).
pub fn compute_etag(data: &[u8]) -> String {
    format!("\"{:016x}\"", fnv1a_64(data))
}

/// Format a `SystemTime` as an RFC 7231 HTTP-date string
/// (e.g. `"Mon, 02 Jan 2006 15:04:05 GMT"`).
///
/// Returns `None` if `t` is before the Unix epoch.
pub fn format_http_date(t: SystemTime) -> Option<String> {
    Some(httpdate::fmt_http_date(t))
}

/// Parse an RFC 7231 HTTP-date string into a `SystemTime`.
///
/// Returns `None` on any parse failure.
pub fn parse_http_date(s: &str) -> Option<SystemTime> {
    httpdate::parse_http_date(s).ok()
}

/// Return `true` when the conditional `If-None-Match` header indicates the
/// response has not changed.
///
/// Matches `*` (any representation) or any ETag in the comma-separated list
/// that equals `etag`.
pub fn etag_matches(if_none_match: &str, etag: &str) -> bool {
    let trimmed = if_none_match.trim();
    if trimmed == "*" {
        return true;
    }
    trimmed.split(',').any(|e| e.trim() == etag)
}

/// Return `true` when the `If-Modified-Since` condition means the resource
/// should be returned as 304 (i.e. it has NOT been modified since `ims_header`).
///
/// Per RFC 7232 §3.3: the condition is true (304 appropriate) when
/// `mtime` is no later than the parsed date.  Returns `false` on parse failure
/// so the request falls through to a normal 200 response.
pub fn not_modified_since(ims_header: &str, mtime: SystemTime) -> bool {
    match parse_http_date(ims_header) {
        Some(req_time) => {
            // Truncate mtime to whole seconds (HTTP dates have 1-second resolution).
            let mtime_secs = mtime
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(d.as_secs()))
                .unwrap_or(SystemTime::UNIX_EPOCH);
            mtime_secs <= req_time
        }
        None => false,
    }
}

/// Attempt to bind a TCP listener on `bind_addr` starting at `start_port`.
///
/// On `EADDRINUSE` the port is incremented by one and the attempt is retried up
/// to `MAX_PORT_ATTEMPTS` times.  Any other OS error causes an immediate failure
/// without further retries.
///
/// Returns the bound `TcpListener` and the actual port on success, or a
/// descriptive `String` error on failure.
pub fn bind_with_retry(bind_addr: &str, start_port: u16) -> Result<(TcpListener, u16), String> {
    let mut port = start_port;
    eprintln!("[bind] trying port={}", port);
    for _ in 0..MAX_PORT_ATTEMPTS {
        let addr = format!("{}:{}", bind_addr, port);
        match TcpListener::bind(&addr) {
            Ok(listener) => {
                eprintln!("[bind] success port={}", port);
                return Ok((listener, port));
            }
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                let next = port.wrapping_add(1);
                eprintln!("[bind] EADDRINUSE, trying {}", next);
                port = next;
            }
            Err(e) => {
                return Err(format!("bind {}:{} failed: {}", bind_addr, port, e));
            }
        }
    }
    Err(format!(
        "exhausted {} port candidates starting at {}; all ports in use",
        MAX_PORT_ATTEMPTS, start_port,
    ))
}

// ---------------------------------------------------------------------------
// Path resolution helpers
// ---------------------------------------------------------------------------

/// Percent-decode a URL path byte-by-byte (RFC 3986 §2.1).
///
/// Returns `Err(())` if the encoding is malformed (truncated `%XX` sequence or
/// non-hex digit) or if the decoded byte sequence is not valid UTF-8.
pub fn percent_decode(encoded: &str) -> Result<String, ()> {
    let bytes = encoded.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(()); // truncated sequence
            }
            let hi = hex_digit(bytes[i + 1])?;
            let lo = hex_digit(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

fn hex_digit(b: u8) -> Result<u8, ()> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(()),
    }
}

/// Normalize a decoded URL path, stripping `.` and `..` components.
///
/// Splits on `/`, ignores empty components and `.`, resolves `..` by popping
/// the stack.  Returns `None` if a `..` would escape the root (stack underflow),
/// which signals a path-traversal attempt.
pub fn normalize_path(decoded: &str) -> Option<PathBuf> {
    let mut parts: Vec<&str> = Vec::new();
    for component in decoded.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                // Attempted traversal above root → reject.
                if parts.pop().is_none() {
                    return None;
                }
            }
            name => parts.push(name),
        }
    }
    let mut path = PathBuf::new();
    for part in &parts {
        path.push(part);
    }
    Some(path)
}

/// Derive the `Content-Type` value from a file extension (case-insensitive).
///
/// Returns `application/octet-stream` for any unrecognised extension so that
/// browsers never perform MIME sniffing on unknown types.
pub fn mime_for_ext(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "md" | "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css",
        "js" => "text/javascript",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// Attempt to resolve a candidate path to an existing file using fallback rules.
///
/// Resolution order (matches issue step 4):
/// 1. `candidate` itself (exact file).
/// 2. If `candidate` is a directory: `candidate/README.md` then `candidate/index.md`.
/// 3. If `candidate` has no extension: `candidate.md` (extensionless).
///
/// Returns `(resolved_path, branch_name)` on success, `None` if not found.
async fn resolve_candidate(candidate: &Path) -> Option<(PathBuf, &'static str)> {
    match tokio::fs::metadata(candidate).await {
        Ok(meta) if meta.is_file() => {
            return Some((candidate.to_path_buf(), "exact"));
        }
        Ok(meta) if meta.is_dir() => {
            // Directory: try README.md then index.md.
            let readme = candidate.join("README.md");
            if tokio::fs::metadata(&readme)
                .await
                .map(|m| m.is_file())
                .unwrap_or(false)
            {
                return Some((readme, "readme"));
            }
            let index = candidate.join("index.md");
            if tokio::fs::metadata(&index)
                .await
                .map(|m| m.is_file())
                .unwrap_or(false)
            {
                return Some((index, "index"));
            }
            return None;
        }
        _ => {}
    }

    // Extensionless fallback: append ".md" when the candidate has no extension.
    if candidate.extension().is_none() {
        let with_md = candidate.with_extension("md");
        if tokio::fs::metadata(&with_md)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            return Some((with_md, "extensionless"));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

/// 304 Not Modified response with `ETag` and `Last-Modified` headers preserved.
fn not_modified_response(etag: &str, last_modified: &str) -> Response {
    Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(header::ETAG, etag)
        .header(header::LAST_MODIFIED, last_modified)
        .body(Body::empty())
        .expect("not_modified_response builder is infallible")
}

/// 404 Not Found with mandatory security headers.
fn not_found_response() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from("Not Found"))
        .expect("not_found_response builder is infallible")
}

/// 413 Content Too Large with mandatory security headers.
fn too_large_response(norm_path: &str, size: u64) -> Response {
    let body = format!(
        "Content Too Large: {} ({} bytes exceeds {} byte limit)",
        norm_path, size, MAX_FILE_SIZE
    );
    Response::builder()
        .status(StatusCode::PAYLOAD_TOO_LARGE)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from(body))
        .expect("too_large_response builder is infallible")
}

/// Return `true` when the query string contains the `raw=1` parameter.
///
/// Parses the raw query string (e.g. `"raw=1&foo=bar"`) by splitting on `&`
/// and looking for an exact match of `"raw=1"`.
fn is_raw_mode(query: &str) -> bool {
    query.split('&').any(|param| param == "raw=1")
}

// ---------------------------------------------------------------------------
// Axum request handler
// ---------------------------------------------------------------------------

/// Main request handler: implements the 7-step secure path resolution pipeline
/// plus cache validation (ETag / Last-Modified) and conditional-request (304)
/// handling.
///
/// Steps:
/// 0. Early-exit: `/assets/mdmd.css` and `/assets/mdmd.js` are served from
///    embedded constants without touching the file system.
/// 1. Percent-decode the raw request path (before any normalisation).
/// 2. Normalise: strip `.`/`..` via component iteration; reject traversal above root.
/// 3. Construct candidate = `serve_root` + normalised path.
/// 4. Fallback resolution: exact → `.md` (extensionless) → `README.md`/`index.md`.
/// 5. (R1) Canonicalise the resolved path and re-verify containment in `canonical_root`.
/// 6. (R5) Stat the file; reject with 413 if size exceeds `MAX_FILE_SIZE`.
/// 7. Dispatch: `.md` files are rendered as HTML (or returned as `text/plain` when
///    `?raw=1` is present); all other files are served as static assets.
///
/// All 200 responses include `ETag`, `Last-Modified`, and
/// `X-Content-Type-Options: nosniff` headers.  Conditional requests
/// (`If-None-Match`, `If-Modified-Since`) are evaluated and may produce a
/// 304 Not Modified response with no body.
async fn serve_handler(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let raw_path = req.uri().path().to_owned();
    let query = req.uri().query().unwrap_or("").to_owned();

    // Extract conditional request headers once, before any branching.
    let if_none_match = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let if_modified_since = req
        .headers()
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    // Log approximate compression encoding from Accept-Encoding header.
    let accept_encoding = req
        .headers()
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let compression_enc = if accept_encoding.contains("br") {
        "br"
    } else if accept_encoding.contains("gzip") {
        "gzip"
    } else {
        "none"
    };
    eprintln!("[compression] encoding={compression_enc}");

    // Step 0: serve embedded static assets early — no filesystem access needed.
    if raw_path == "/assets/mdmd.css" {
        let etag = &state.css_etag;
        let last_modified = format_http_date(state.asset_mtime)
            .unwrap_or_else(|| "Thu, 01 Jan 1970 00:00:00 GMT".to_owned());

        // Evaluate If-None-Match first (RFC 7232 §6 preference order).
        if let Some(ref inm) = if_none_match {
            if etag_matches(inm, etag) {
                eprintln!("[cache] path={raw_path} etag={etag} status=304");
                return not_modified_response(etag, &last_modified);
            }
        } else if let Some(ref ims) = if_modified_since {
            if not_modified_since(ims, state.asset_mtime) {
                eprintln!("[cache] path={raw_path} etag={etag} status=304");
                return not_modified_response(etag, &last_modified);
            }
        }

        eprintln!("[cache] path={raw_path} etag={etag} status=200");
        eprintln!("[request] path={raw_path} mode=asset");
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/css; charset=utf-8")
            .header("X-Content-Type-Options", "nosniff")
            .header(header::ETAG, etag.as_str())
            .header(header::LAST_MODIFIED, last_modified)
            .body(Body::from(web_assets::CSS))
            .expect("css asset response builder is infallible");
    }
    if raw_path == "/assets/mdmd.js" {
        let etag = &state.js_etag;
        let last_modified = format_http_date(state.asset_mtime)
            .unwrap_or_else(|| "Thu, 01 Jan 1970 00:00:00 GMT".to_owned());

        if let Some(ref inm) = if_none_match {
            if etag_matches(inm, etag) {
                eprintln!("[cache] path={raw_path} etag={etag} status=304");
                return not_modified_response(etag, &last_modified);
            }
        } else if let Some(ref ims) = if_modified_since {
            if not_modified_since(ims, state.asset_mtime) {
                eprintln!("[cache] path={raw_path} etag={etag} status=304");
                return not_modified_response(etag, &last_modified);
            }
        }

        eprintln!("[cache] path={raw_path} etag={etag} status=200");
        eprintln!("[request] path={raw_path} mode=asset");
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/javascript; charset=utf-8")
            .header("X-Content-Type-Options", "nosniff")
            .header(header::ETAG, etag.as_str())
            .header(header::LAST_MODIFIED, last_modified)
            .body(Body::from(web_assets::JS))
            .expect("js asset response builder is infallible");
    }

    // Step 1: percent-decode.
    let decoded = match percent_decode(&raw_path) {
        Ok(d) => d,
        Err(_) => {
            eprintln!("[resolve] path={raw_path} branch=denied reason=invalid-percent-encoding");
            return not_found_response();
        }
    };

    // Reject null bytes anywhere in the decoded path.
    if decoded.contains('\0') {
        eprintln!("[resolve] path={raw_path} branch=denied reason=null-byte");
        return not_found_response();
    }

    // Step 2: normalise.
    let normalized = match normalize_path(&decoded) {
        Some(n) => n,
        None => {
            eprintln!("[resolve] path={raw_path} branch=denied reason=path-traversal");
            return not_found_response();
        }
    };

    let norm_display = normalized.display().to_string();

    // Step 3: construct candidate.
    // When the normalized path is empty (i.e. request for "/"), serve the entry file.
    let candidate = if normalized == PathBuf::new() {
        state.entry_file.clone()
    } else {
        state.serve_root.join(&normalized)
    };

    // Step 4: fallback resolution.
    let (resolved, branch) = match resolve_candidate(&candidate).await {
        Some(r) => r,
        None => {
            eprintln!("[resolve] path={norm_display} branch=denied reason=not-found");
            return not_found_response();
        }
    };

    // Step 5 (R1): canonicalise and re-verify containment (symlink-safe).
    let canonical = match tokio::fs::canonicalize(&resolved).await {
        Ok(c) => c,
        Err(_) => {
            eprintln!("[resolve] path={norm_display} branch=denied reason=canonicalize-failed");
            return not_found_response();
        }
    };

    if !canonical.starts_with(&state.canonical_root) {
        eprintln!(
            "[resolve] path={norm_display} branch=denied reason=outside-root canonical={}",
            canonical.display()
        );
        return not_found_response();
    }

    // Step 6 (R5): file size guard — stat before reading; also capture mtime.
    let file_meta = match tokio::fs::metadata(&canonical).await {
        Ok(m) => m,
        Err(_) => {
            eprintln!("[resolve] path={norm_display} branch=denied reason=metadata-failed");
            return not_found_response();
        }
    };
    let size = file_meta.len();
    let mtime = file_meta.modified().ok();

    if size > MAX_FILE_SIZE {
        eprintln!("[resolve] path={norm_display} branch=denied reason=too-large size={size}");
        return too_large_response(&norm_display, size);
    }

    eprintln!("[resolve] path={norm_display} branch={branch} size={size}");

    // Step 7: dispatch on extension.
    let ext = canonical.extension().and_then(|e| e.to_str()).unwrap_or("");

    if ext.eq_ignore_ascii_case("md") {
        let content = match tokio::fs::read_to_string(&canonical).await {
            Ok(c) => c,
            Err(_) => return not_found_response(),
        };

        // ?raw=1 — return the markdown source as plain text.
        if is_raw_mode(&query) {
            let body_bytes = content.as_bytes();
            let etag = compute_etag(body_bytes);
            let last_modified = mtime
                .and_then(format_http_date)
                .unwrap_or_else(|| "Thu, 01 Jan 1970 00:00:00 GMT".to_owned());

            if let Some(ref inm) = if_none_match {
                if etag_matches(inm, &etag) {
                    eprintln!("[cache] path={norm_display} etag={etag} status=304");
                    return not_modified_response(&etag, &last_modified);
                }
            } else if let Some(ref ims) = if_modified_since {
                if let Some(mt) = mtime {
                    if not_modified_since(ims, mt) {
                        eprintln!("[cache] path={norm_display} etag={etag} status=304");
                        return not_modified_response(&etag, &last_modified);
                    }
                }
            }

            eprintln!("[cache] path={norm_display} etag={etag} status=200");
            eprintln!("[request] path={norm_display} mode=raw");
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .header("X-Content-Type-Options", "nosniff")
                .header(header::ETAG, etag)
                .header(header::LAST_MODIFIED, last_modified)
                .body(Body::from(content))
                .expect("raw mode response builder is infallible");
        }

        // Default: render as a full HTML page with TOC shell.
        let (html_body, headings) =
            html::render_markdown(&content, &canonical, &state.canonical_root);
        let page = html::build_page_shell(&html_body, &headings, &canonical, &state.canonical_root);

        let etag = compute_etag(page.as_bytes());
        let last_modified = mtime
            .and_then(format_http_date)
            .unwrap_or_else(|| "Thu, 01 Jan 1970 00:00:00 GMT".to_owned());

        if let Some(ref inm) = if_none_match {
            if etag_matches(inm, &etag) {
                eprintln!("[cache] path={norm_display} etag={etag} status=304");
                return not_modified_response(&etag, &last_modified);
            }
        } else if let Some(ref ims) = if_modified_since {
            if let Some(mt) = mtime {
                if not_modified_since(ims, mt) {
                    eprintln!("[cache] path={norm_display} etag={etag} status=304");
                    return not_modified_response(&etag, &last_modified);
                }
            }
        }

        eprintln!("[cache] path={norm_display} etag={etag} status=200");
        eprintln!("[request] path={norm_display} mode=rendered");
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header("X-Content-Type-Options", "nosniff")
            .header(header::ETAG, etag)
            .header(header::LAST_MODIFIED, last_modified)
            .body(Body::from(page))
            .expect("serve_handler md response builder is infallible")
    } else {
        // Serve as a static asset with the derived MIME type.
        let bytes = match tokio::fs::read(&canonical).await {
            Ok(b) => b,
            Err(_) => return not_found_response(),
        };

        let etag = compute_etag(&bytes);
        let last_modified = mtime
            .and_then(format_http_date)
            .unwrap_or_else(|| "Thu, 01 Jan 1970 00:00:00 GMT".to_owned());

        if let Some(ref inm) = if_none_match {
            if etag_matches(inm, &etag) {
                eprintln!("[cache] path={norm_display} etag={etag} status=304");
                return not_modified_response(&etag, &last_modified);
            }
        } else if let Some(ref ims) = if_modified_since {
            if let Some(mt) = mtime {
                if not_modified_since(ims, mt) {
                    eprintln!("[cache] path={norm_display} etag={etag} status=304");
                    return not_modified_response(&etag, &last_modified);
                }
            }
        }

        eprintln!("[cache] path={norm_display} etag={etag} status=200");
        let content_type = mime_for_ext(ext);
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header("X-Content-Type-Options", "nosniff")
            .header(header::ETAG, etag)
            .header(header::LAST_MODIFIED, last_modified)
            .body(Body::from(bytes))
            .expect("serve_handler asset response builder is infallible")
    }
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

/// Start the HTTP server for the given markdown `file`.
///
/// Binds to `bind_addr` starting at `start_port`, retrying on `EADDRINUSE` up
/// to 100 times.  The server shuts down cleanly when SIGINT (Ctrl+C) is
/// received.
pub async fn run_serve(file: String, bind_addr: String, start_port: u16) -> io::Result<()> {
    let entry_file = std::fs::canonicalize(&file).unwrap_or_else(|_| PathBuf::from(&file));
    let serve_root = entry_file
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let canonical_root = std::fs::canonicalize(&serve_root).unwrap_or_else(|_| serve_root.clone());

    // Precompute ETags for embedded static assets (stable for the lifetime of
    // this server process — embedded bytes never change at runtime).
    let css_etag = compute_etag(web_assets::CSS.as_bytes());
    let js_etag = compute_etag(web_assets::JS.as_bytes());

    // Use the binary's own mtime as Last-Modified for embedded assets, falling
    // back to the Unix epoch when the path or metadata is unavailable.
    let asset_mtime = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let state = Arc::new(AppState {
        serve_root,
        canonical_root,
        entry_file,
        config: AppConfig,
        css_etag,
        js_etag,
        asset_mtime,
    });

    let (std_listener, bound_port) = bind_with_retry(&bind_addr, start_port).map_err(|msg| {
        eprintln!("Error: {}", msg);
        io::Error::new(io::ErrorKind::AddrInUse, msg)
    })?;

    std_listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(std_listener)?;

    // CompressionLayer transparently compresses text responses using gzip or
    // brotli based on the client's Accept-Encoding header.  It is added as the
    // outermost layer so it wraps all handler responses.
    let app = Router::new()
        .fallback(serve_handler)
        .with_state(state.clone())
        .layer(CompressionLayer::new());

    eprintln!("[serve] listening on {}:{}", bind_addr, bound_port);

    // Stable startup banner (stdout) for integration/e2e checks.
    println!("mdmd serve");
    println!("root:  {}", state.canonical_root.display());
    println!("entry: {}", state.entry_file.display());
    // Always print localhost URL.
    println!("url:   http://127.0.0.1:{bound_port}");

    // Conditionally print Tailscale URL when available.
    let tailscale_host = tokio::task::spawn_blocking(tailscale_dns_name)
        .await
        .ok()
        .flatten();
    if let Some(ref host) = tailscale_host {
        println!("url:   http://{host}:{bound_port}");
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            signal::ctrl_c()
                .await
                .expect("failed to install SIGINT handler");
            eprintln!("[shutdown] complete");
        })
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_tailscale_dns_name ---

    #[test]
    fn tailscale_valid_json_trims_trailing_dot() {
        let json = br#"{"Self":{"DNSName":"hostname.ts.net."}}"#;
        let result = parse_tailscale_dns_name(json).unwrap();
        assert_eq!(result, "hostname.ts.net");
    }

    #[test]
    fn tailscale_trailing_dot_only_is_empty_err() {
        let json = br#"{"Self":{"DNSName":"."}}"#;
        let err = parse_tailscale_dns_name(json).unwrap_err();
        assert!(err.contains("empty-DNSName"), "got: {err}");
    }

    #[test]
    fn tailscale_empty_json_object_returns_err() {
        let json = b"{}";
        assert!(parse_tailscale_dns_name(json).is_err());
    }

    #[test]
    fn tailscale_missing_self_key_returns_err() {
        let json = br#"{"Other":{"DNSName":"hostname.ts.net."}}"#;
        let err = parse_tailscale_dns_name(json).unwrap_err();
        assert!(err.contains("no-DNSName"), "got: {err}");
    }

    #[test]
    fn tailscale_missing_dnsname_field_returns_err() {
        let json = br#"{"Self":{"Status":"Running"}}"#;
        let err = parse_tailscale_dns_name(json).unwrap_err();
        assert!(err.contains("no-DNSName"), "got: {err}");
    }

    #[test]
    fn tailscale_malformed_json_returns_err() {
        let json = b"not valid json {{{";
        let err = parse_tailscale_dns_name(json).unwrap_err();
        assert!(err.contains("json-parse"), "got: {err}");
    }

    #[test]
    fn tailscale_empty_bytes_returns_err() {
        let json = b"";
        let err = parse_tailscale_dns_name(json).unwrap_err();
        assert!(err.contains("json-parse"), "got: {err}");
    }

    #[test]
    fn tailscale_subprocess_failure_returns_none() {
        // Running a non-existent command should produce Err from output(),
        // which tailscale_dns_name() converts to None without panicking.
        let result = std::process::Command::new("__tailscale_does_not_exist__")
            .args(["status", "--json"])
            .output();
        // Verify the OS error is captured (not panicked); tailscale_dns_name()
        // handles this same Err by returning None.
        assert!(result.is_err(), "expected command-not-found error");
    }

    // --- is_raw_mode ---

    #[test]
    fn raw_mode_detected_when_param_present() {
        assert!(is_raw_mode("raw=1"));
        assert!(is_raw_mode("foo=bar&raw=1"));
        assert!(is_raw_mode("raw=1&foo=bar"));
    }

    #[test]
    fn raw_mode_not_detected_when_absent() {
        assert!(!is_raw_mode(""));
        assert!(!is_raw_mode("raw=0"));
        assert!(!is_raw_mode("foo=bar"));
        assert!(!is_raw_mode("raw=1x"));
        assert!(!is_raw_mode("xraw=1"));
    }

    // --- percent_decode ---

    #[test]
    fn decode_plain_ascii() {
        assert_eq!(percent_decode("/docs/guide").unwrap(), "/docs/guide");
    }

    #[test]
    fn decode_dot_dot_lowercase() {
        // %2e%2e → ".."
        assert_eq!(percent_decode("%2e%2e").unwrap(), "..");
    }

    #[test]
    fn decode_dot_dot_uppercase() {
        // %2E%2E → ".."
        assert_eq!(percent_decode("%2E%2E").unwrap(), "..");
    }

    #[test]
    fn decode_slash_lowercase() {
        // %2f → "/"
        assert_eq!(percent_decode("%2f").unwrap(), "/");
    }

    #[test]
    fn decode_slash_uppercase() {
        // %2F → "/"
        assert_eq!(percent_decode("%2F").unwrap(), "/");
    }

    #[test]
    fn decode_mixed_case_encoded_dotdot_slash() {
        // %2e%2e%2f → "../"
        assert_eq!(percent_decode("%2e%2e%2f").unwrap(), "../");
    }

    #[test]
    fn decode_truncated_sequence_is_error() {
        assert!(percent_decode("%2").is_err());
        assert!(percent_decode("%").is_err());
    }

    #[test]
    fn decode_invalid_hex_is_error() {
        assert!(percent_decode("%zz").is_err());
    }

    #[test]
    fn decode_invalid_utf8_sequence_is_error() {
        // %80 is a lone continuation byte — invalid UTF-8.
        assert!(percent_decode("%80").is_err());
    }

    // --- normalize_path ---

    #[test]
    fn normalize_simple_path() {
        assert_eq!(
            normalize_path("/docs/guide").unwrap(),
            PathBuf::from("docs/guide")
        );
    }

    #[test]
    fn normalize_root_gives_empty() {
        assert_eq!(normalize_path("/").unwrap(), PathBuf::new());
    }

    #[test]
    fn normalize_dot_components_stripped() {
        assert_eq!(normalize_path("/a/./b").unwrap(), PathBuf::from("a/b"));
    }

    #[test]
    fn normalize_dotdot_within_root() {
        // /a/b/../c → a/c  (stays inside root)
        assert_eq!(normalize_path("/a/b/../c").unwrap(), PathBuf::from("a/c"));
    }

    #[test]
    fn normalize_traversal_above_root_rejected() {
        // /../etc/passwd → None (traversal above root)
        assert!(normalize_path("/../etc/passwd").is_none());
    }

    #[test]
    fn normalize_multi_level_traversal_rejected() {
        assert!(normalize_path("/../../etc/passwd").is_none());
    }

    #[test]
    fn normalize_safe_then_escape_rejected() {
        // /a/../../etc/passwd — goes into a, then pops a, then tries to pop empty → None
        assert!(normalize_path("/a/../../etc/passwd").is_none());
    }

    #[test]
    fn normalize_encoded_dotdot_after_decode() {
        // Simulate full pipeline: decode %2e%2e → ".." then normalize
        let decoded = percent_decode("/%2e%2e/etc/passwd").unwrap();
        assert!(
            normalize_path(&decoded).is_none(),
            "traversal via %2e%2e must be rejected"
        );
    }

    #[test]
    fn normalize_encoded_slash_and_dotdot() {
        // %2e%2e%2fetc%2fpasswd → ../etc/passwd  (slash also encoded)
        let decoded = percent_decode("/%2e%2e%2fetc%2fpasswd").unwrap();
        assert!(
            normalize_path(&decoded).is_none(),
            "traversal via %2e%2e%2f must be rejected"
        );
    }

    #[test]
    fn normalize_mixed_case_encoded_dotdot() {
        // /%2E%2E/ → "../" path component
        let decoded = percent_decode("/%2E%2E/").unwrap();
        assert!(
            normalize_path(&decoded).is_none(),
            "%2E%2E traversal must be rejected"
        );
    }

    #[test]
    fn normalize_trailing_slash_ok() {
        // /docs/ → "docs" (trailing slash produces empty component, which is ignored)
        assert_eq!(normalize_path("/docs/").unwrap(), PathBuf::from("docs"));
    }

    // --- mime_for_ext ---

    #[test]
    fn mime_html_extensions() {
        assert_eq!(mime_for_ext("html"), "text/html; charset=utf-8");
        assert_eq!(mime_for_ext("htm"), "text/html; charset=utf-8");
        assert_eq!(mime_for_ext("md"), "text/html; charset=utf-8");
    }

    #[test]
    fn mime_css_js() {
        assert_eq!(mime_for_ext("css"), "text/css");
        assert_eq!(mime_for_ext("js"), "text/javascript");
    }

    #[test]
    fn mime_images() {
        assert_eq!(mime_for_ext("png"), "image/png");
        assert_eq!(mime_for_ext("jpg"), "image/jpeg");
        assert_eq!(mime_for_ext("jpeg"), "image/jpeg");
        assert_eq!(mime_for_ext("svg"), "image/svg+xml");
        assert_eq!(mime_for_ext("gif"), "image/gif");
        assert_eq!(mime_for_ext("ico"), "image/x-icon");
    }

    #[test]
    fn mime_fonts_docs() {
        assert_eq!(mime_for_ext("woff2"), "font/woff2");
        assert_eq!(mime_for_ext("pdf"), "application/pdf");
    }

    #[test]
    fn mime_unknown_extension_is_octet_stream() {
        assert_eq!(mime_for_ext("xyz"), "application/octet-stream");
        assert_eq!(mime_for_ext(""), "application/octet-stream");
        assert_eq!(mime_for_ext("bin"), "application/octet-stream");
    }

    #[test]
    fn mime_extension_case_insensitive() {
        assert_eq!(mime_for_ext("PNG"), "image/png");
        assert_eq!(mime_for_ext("SVG"), "image/svg+xml");
        assert_eq!(mime_for_ext("MD"), "text/html; charset=utf-8");
    }

    // --- Symlink containment check (R1) ---

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_outside_root_fails_containment_check() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("mdmd_symlink_test_{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();

        // Create a file outside the serve root.
        let outside = std::env::temp_dir().join(format!("mdmd_outside_{}.txt", std::process::id()));
        std::fs::write(&outside, b"secret").unwrap();

        // Create an in-tree symlink that points to the outside file.
        let link = base.join("evil.txt");
        let _ = std::fs::remove_file(&link);
        symlink(&outside, &link).unwrap();

        let canonical_root = std::fs::canonicalize(&base).unwrap();
        let canonical_link = tokio::fs::canonicalize(&link).await.unwrap();

        // The canonical path of the symlink target should NOT be inside the root.
        assert!(
            !canonical_link.starts_with(&canonical_root),
            "symlink to outside file should fail containment check"
        );

        // Cleanup.
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir(&base);
    }

    // --- resolve_candidate (async, requires real files) ---

    #[tokio::test]
    async fn resolve_exact_file() {
        let dir = std::env::temp_dir().join(format!("mdmd_resolve_exact_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("page.md"), b"# Hello").unwrap();

        let candidate = dir.join("page.md");
        let (path, branch) = resolve_candidate(&candidate).await.unwrap();
        assert_eq!(branch, "exact");
        assert_eq!(path, candidate);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn resolve_extensionless_falls_back_to_md() {
        let dir = std::env::temp_dir().join(format!("mdmd_resolve_ext_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("guide.md"), b"# Guide").unwrap();

        // Request is for "guide" (no extension).
        let candidate = dir.join("guide");
        let (path, branch) = resolve_candidate(&candidate).await.unwrap();
        assert_eq!(branch, "extensionless");
        assert_eq!(path, dir.join("guide.md"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn resolve_directory_readme() {
        let dir = std::env::temp_dir().join(format!("mdmd_resolve_readme_{}", std::process::id()));
        let sub = dir.join("docs");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("README.md"), b"# Readme").unwrap();

        let candidate = sub.clone();
        let (path, branch) = resolve_candidate(&candidate).await.unwrap();
        assert_eq!(branch, "readme");
        assert_eq!(path, sub.join("README.md"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn resolve_directory_index_fallback() {
        let dir = std::env::temp_dir().join(format!("mdmd_resolve_index_{}", std::process::id()));
        let sub = dir.join("docs");
        std::fs::create_dir_all(&sub).unwrap();
        // No README.md — only index.md.
        std::fs::write(sub.join("index.md"), b"# Index").unwrap();

        let candidate = sub.clone();
        let (path, branch) = resolve_candidate(&candidate).await.unwrap();
        assert_eq!(branch, "index");
        assert_eq!(path, sub.join("index.md"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn resolve_nonexistent_returns_none() {
        let dir = std::env::temp_dir().join(format!("mdmd_resolve_missing_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let candidate = dir.join("no_such_file");
        assert!(resolve_candidate(&candidate).await.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
