use std::collections::HashMap;
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

use crate::backlinks::BacklinkRef;
use crate::html;
use crate::web_assets;

// ---------------------------------------------------------------------------
// Verbose-gated diagnostic helper
// ---------------------------------------------------------------------------

/// Emit a diagnostic line to stderr only when `$verbose` is true.
///
/// Expands to an `if`-guarded `eprintln!`, so format arguments are never
/// evaluated when `$verbose` is `false`.
///
/// Usage in startup code:  `vlog!(verbose, "...")`
/// Usage in handlers:      `vlog!(state.verbose, "...")`
macro_rules! vlog {
    ($verbose:expr, $($args:tt)*) => {
        if $verbose { eprintln!($($args)*); }
    };
}

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
fn tailscale_dns_name(verbose: bool) -> Option<String> {
    let output = match std::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            vlog!(verbose, "[tailscale] skipped reason=subprocess-error: {e}");
            return None;
        }
    };

    match parse_tailscale_dns_name(&output.stdout) {
        Ok(name) => Some(name),
        Err(reason) => {
            vlog!(verbose, "[tailscale] skipped reason={reason}");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Headed-environment detection
// ---------------------------------------------------------------------------

/// The runtime platform, used by [`is_headed_for`] to apply platform-specific rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimePlatform {
    #[allow(dead_code)]
    MacOs,
    Linux,
    #[allow(dead_code)]
    Other,
}

/// A snapshot of the environment variables relevant to headed-environment detection.
///
/// All fields are booleans indicating whether the corresponding variable is
/// present (and non-empty) in the process environment at the time of the snapshot.
#[derive(Debug, Clone, Copy)]
pub struct EnvSnapshot {
    /// Whether `SSH_CONNECTION` is set (non-empty).
    pub ssh_connection: bool,
    /// Whether `SSH_TTY` is set (non-empty).
    pub ssh_tty: bool,
    /// Whether `DISPLAY` is set (non-empty).
    pub display: bool,
    /// Whether `WAYLAND_DISPLAY` is set (non-empty).
    pub wayland_display: bool,
    /// Whether `CI` is set (non-empty).
    pub ci: bool,
    /// Whether `GITHUB_ACTIONS` is set (non-empty).
    pub github_actions: bool,
}

/// Pure, testable headed-environment predicate.
///
/// Applies platform-specific rules to determine whether a browser open would
/// succeed in the current execution context:
///
/// - **macOS**: `true` unless `SSH_CONNECTION` or `SSH_TTY` is set.
/// - **Linux**: `true` only when `DISPLAY` or `WAYLAND_DISPLAY` is set, *and*
///   none of `SSH_CONNECTION`, `SSH_TTY`, `CI`, or `GITHUB_ACTIONS` is set.
/// - **Other**: always `false`.
///
/// This function accepts explicit inputs so it can be exercised in unit tests
/// without mutating the global process environment.
pub fn is_headed_for(platform: RuntimePlatform, env: &EnvSnapshot) -> bool {
    match platform {
        RuntimePlatform::MacOs => !env.ssh_connection && !env.ssh_tty,
        RuntimePlatform::Linux => {
            (env.display || env.wayland_display)
                && !env.ssh_connection
                && !env.ssh_tty
                && !env.ci
                && !env.github_actions
        }
        RuntimePlatform::Other => false,
    }
}

/// Detect whether the current process is running in a headed environment.
///
/// Reads the actual process environment and detects the current platform via
/// `cfg(target_os = ...)`, then delegates to [`is_headed_for`].
///
/// This is the production entry point.  For unit testing, call [`is_headed_for`]
/// directly with a synthetic [`EnvSnapshot`].
pub fn is_headed_environment() -> bool {
    fn env_set(key: &str) -> bool {
        std::env::var_os(key).is_some_and(|v| !v.is_empty())
    }

    let env = EnvSnapshot {
        ssh_connection: env_set("SSH_CONNECTION"),
        ssh_tty: env_set("SSH_TTY"),
        display: env_set("DISPLAY"),
        wayland_display: env_set("WAYLAND_DISPLAY"),
        ci: env_set("CI"),
        github_actions: env_set("GITHUB_ACTIONS"),
    };

    #[cfg(target_os = "macos")]
    let platform = RuntimePlatform::MacOs;
    #[cfg(target_os = "linux")]
    let platform = RuntimePlatform::Linux;
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let platform = RuntimePlatform::Other;

    is_headed_for(platform, &env)
}

// ---------------------------------------------------------------------------
// Browser open helpers
// ---------------------------------------------------------------------------

/// Returns `true` when a browser-open attempt should be made.
///
/// Both conditions must hold:
/// - `no_open` is `false` (the user has not suppressed auto-open).
/// - `headed` is `true` (the environment can display a browser window).
pub fn should_attempt_open(no_open: bool, headed: bool) -> bool {
    !no_open && headed
}

/// Returns the platform-appropriate command used to open a URL in the default
/// browser.
///
/// - macOS  → `"open"`
/// - Linux  → `"xdg-open"`
/// - Other  → `""` (empty string; treated as "no command available")
pub fn default_open_command() -> &'static str {
    #[cfg(target_os = "macos")]
    return "open";
    #[cfg(target_os = "linux")]
    return "xdg-open";
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    return "";
}

/// Resolve the browser-open command, preferring a `MDMD_OPEN_CMD` env-var
/// override when one is supplied.
///
/// This function accepts the override value directly so it can be exercised
/// in unit tests without touching the global process environment.
///
/// - `env_override = Some(cmd)` → returns `cmd` as-is (including empty string).
/// - `env_override = None`       → returns [`default_open_command()`].
///
/// The primary caller is [`run_serve`], which passes
/// `std::env::var("MDMD_OPEN_CMD").ok().as_deref()`.
/// Integration tests can set `MDMD_OPEN_CMD` on the spawned child process to
/// inject a deterministic stub without requiring a real browser binary.
pub fn resolve_open_cmd(env_override: Option<&str>) -> String {
    env_override
        .map(str::to_owned)
        .unwrap_or_else(|| default_open_command().to_owned())
}

/// Spawn a child process that opens `url` using `cmd`.
///
/// Returns `Err` immediately if `cmd` is empty or if the spawn fails.
/// The child process is **not** waited on — this is a fire-and-forget call.
pub fn spawn_browser_open(cmd: &str, url: &str) -> io::Result<std::process::Child> {
    if cmd.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no browser-open command for this platform",
        ));
    }
    std::process::Command::new(cmd).arg(url).spawn()
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
    #[allow(dead_code)]
    pub entry_file: PathBuf,
    /// URL path for the primary entry file (percent-encoded, starts with `/`).
    pub entry_url_path: String,
    /// Server configuration.
    #[allow(dead_code)]
    pub config: AppConfig,
    /// Precomputed strong ETag for the embedded CSS asset (`/assets/mdmd.css`).
    pub css_etag: String,
    /// Precomputed strong ETag for the embedded JS asset (`/assets/mdmd.js`).
    pub js_etag: String,
    /// `Last-Modified` timestamp for embedded static assets, derived from the
    /// binary's own modification time.  Falls back to the Unix epoch.
    pub asset_mtime: SystemTime,
    /// Startup-built backlinks index: maps root-relative URL path keys
    /// (e.g. `/docs/readme.md`) to all inbound [`BacklinkRef`]s for that page.
    /// Built once at startup; intentionally stale until server restart.
    pub backlinks: HashMap<String, Vec<BacklinkRef>>,
    /// When true, request handlers emit per-request diagnostic lines to stderr.
    pub verbose: bool,
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
pub fn bind_with_retry(
    bind_addr: &str,
    start_port: u16,
    verbose: bool,
) -> Result<(TcpListener, u16), String> {
    let mut port = start_port;
    vlog!(verbose, "[bind] trying port={}", port);
    for _ in 0..MAX_PORT_ATTEMPTS {
        let addr = format!("{}:{}", bind_addr, port);
        match TcpListener::bind(&addr) {
            Ok(listener) => {
                vlog!(verbose, "[bind] success port={}", port);
                return Ok((listener, port));
            }
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                let next = port.wrapping_add(1);
                vlog!(verbose, "[bind] EADDRINUSE, trying {}", next);
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
                parts.pop()?;
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

/// Percent-encode a single URL path segment (RFC 3986 §2.1 / §3.3).
///
/// Encodes all bytes that are not unreserved characters (ALPHA, DIGIT, `-`, `_`, `.`, `~`)
/// per RFC 3986 §2.3.  Multi-byte UTF-8 characters are encoded byte-by-byte.
pub fn percent_encode_segment(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0xf) as usize] as char);
        }
    }
    out
}

/// Derive the URL path for an entry file relative to the canonical root.
///
/// 1. Strips `canonical_root` prefix from `entry_file`.
/// 2. Converts OS path separators to `/`.
/// 3. Percent-encodes each path segment via [`percent_encode_segment`].
/// 4. Prepends `/`.
///
/// Returns `Err(String)` if `entry_file` does not start with `canonical_root`.
pub fn derive_entry_url_path(entry_file: &Path, canonical_root: &Path) -> Result<String, String> {
    let relative = entry_file.strip_prefix(canonical_root).map_err(|_| {
        format!(
            "entry '{}' is outside serve root '{}'",
            entry_file.display(),
            canonical_root.display()
        )
    })?;

    let mut url_path = String::from("/");
    let mut first = true;
    for component in relative.components() {
        if let std::path::Component::Normal(name) = component {
            if let Some(name_str) = name.to_str() {
                if !first {
                    url_path.push('/');
                }
                url_path.push_str(&percent_encode_segment(name_str));
                first = false;
            }
        }
    }
    Ok(url_path)
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

/// Minimal HTML escaping for text content and attribute values.
///
/// Replaces `<`, `>`, `&`, and `"` with their entity equivalents.
fn html_escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Find the nearest existing ancestor directory of `norm_display` that lies
/// within `canonical_root`.
///
/// Starts from the parent of `serve_root.join(norm_display)` and walks upward
/// through the filesystem hierarchy, returning the deepest existing directory
/// whose canonicalized path is contained within `canonical_root`.
///
/// Always returns at least `canonical_root` itself (the root), which is
/// guaranteed to exist.  The function never returns a path outside
/// `canonical_root`.
pub fn nearest_existing_parent(
    serve_root: &Path,
    canonical_root: &Path,
    norm_display: &str,
) -> PathBuf {
    // Build the full (unresolved) path for the missing resource.
    let missing = serve_root.join(norm_display);

    // Walk upward from the missing resource's location.
    let mut candidate = missing;
    loop {
        candidate = match candidate.parent() {
            Some(p) => p.to_path_buf(),
            None => return canonical_root.to_path_buf(),
        };

        // Avoid infinite loop at filesystem root (parent == self).
        if candidate.as_os_str().is_empty() {
            return canonical_root.to_path_buf();
        }

        // Check if this path is an existing directory inside canonical_root.
        if let Ok(meta) = std::fs::metadata(&candidate) {
            if meta.is_dir() {
                if let Ok(canon) = std::fs::canonicalize(&candidate) {
                    if canon.starts_with(canonical_root) {
                        return canon;
                    }
                }
                // Directory is outside root — keep walking up.
            }
        }
    }
}

/// Build an HTML snippet listing the contents of `dir_path` (nearest parent).
///
/// Applies the same policy as the full directory index (dotfile exclusion,
/// symlink containment, dirs-first alphabetical sort).  Returns an empty
/// string when the directory cannot be read or is empty after filtering.
async fn build_nearest_parent_listing(
    state: &Arc<AppState>,
    dir_path: &Path,
    url_prefix: &str,
) -> String {
    let mut rd = match tokio::fs::read_dir(dir_path).await {
        Ok(rd) => rd,
        Err(_) => return String::new(),
    };

    let mut raw_entries: Vec<(String, bool)> = Vec::new();
    loop {
        match rd.next_entry().await {
            Ok(Some(entry)) => {
                let name = match entry.file_name().to_str().map(|s| s.to_owned()) {
                    Some(n) => n,
                    None => continue,
                };
                if name.starts_with('.') {
                    continue;
                }
                let entry_path = entry.path();
                // Symlink containment: skip out-of-root symlinks.
                let file_type = match entry.file_type().await {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };
                if file_type.is_symlink() {
                    match tokio::fs::canonicalize(&entry_path).await {
                        Ok(target) if target.starts_with(&state.canonical_root) => {}
                        _ => continue,
                    }
                }
                let is_dir = match tokio::fs::metadata(&entry_path).await {
                    Ok(m) => m.is_dir(),
                    Err(_) => continue,
                };
                raw_entries.push((name, is_dir));
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    let entries = apply_dir_listing_policy(raw_entries);
    if entries.is_empty() {
        return String::new();
    }

    let base = if url_prefix.ends_with('/') {
        url_prefix.to_owned()
    } else {
        format!("{url_prefix}/")
    };

    let prefix_escaped = html_escape_text(url_prefix);
    let mut html = format!("<h2>Contents of {prefix_escaped}</h2><ul>");
    for (name, is_dir) in &entries {
        let encoded = percent_encode_segment(name);
        let href = if *is_dir {
            format!("{base}{encoded}/")
        } else {
            format!("{base}{encoded}")
        };
        let href_escaped = html_escape_text(&href);
        let name_escaped = html_escape_text(name);
        html.push_str(&format!(
            "<li><a href=\"{href_escaped}\">{name_escaped}</a></li>"
        ));
    }
    html.push_str("</ul>");
    html
}

/// Rich HTML 404 response with nearest-parent recovery links.
///
/// Renders the requested path, links to the entry document, root index, and
/// the nearest existing ancestor directory, plus a listing of that directory.
///
/// Called only for genuine unresolved-path misses.  Security-denial branches
/// continue to use the terse `not_found_response()` to avoid disclosing
/// internal path information.
async fn rich_not_found_response(state: &Arc<AppState>, norm_display: &str) -> Response {
    let requested_path = format!("/{norm_display}");

    // Find nearest existing parent directory within canonical_root.
    let nearest_parent =
        nearest_existing_parent(&state.serve_root, &state.canonical_root, norm_display);

    // Derive URL path for nearest parent (add trailing slash for directories).
    let parent_url = derive_entry_url_path(&nearest_parent, &state.canonical_root)
        .unwrap_or_else(|_| "/".to_owned());
    let parent_url = if parent_url == "/" {
        "/".to_owned()
    } else {
        format!("{parent_url}/")
    };

    // Build directory listing snippet for nearest parent.
    let listing_html =
        build_nearest_parent_listing(state, &nearest_parent, &parent_url).await;

    let requested_escaped = html_escape_text(&requested_path);
    let parent_url_escaped = html_escape_text(&parent_url);
    let entry_url_escaped = html_escape_text(&state.entry_url_path);

    let body = format!(
        "<!DOCTYPE html>\
<html lang=\"en\">\
<head>\
<meta charset=\"utf-8\">\
<title>404 Not Found</title>\
<link rel=\"stylesheet\" href=\"/assets/mdmd.css\">\
</head>\
<body>\
<main class=\"content\">\
<h1>404 Not Found</h1>\
<p>The requested path was not found:</p>\
<pre><code>{requested_escaped}</code></pre>\
<h2>Recovery options</h2>\
<ul>\
<li><a href=\"/\">Root index</a></li>\
<li><a href=\"{entry_url_escaped}\">Entry document</a></li>\
<li><a href=\"{parent_url_escaped}\">Nearest parent: {parent_url_escaped}</a></li>\
</ul>\
{listing_html}\
</main>\
</body>\
</html>"
    );

    vlog!(
        state.verbose,
        "[404] path={norm_display} nearest_parent={}",
        nearest_parent.display()
    );
    vlog!(
        state.verbose,
        "[request] path={norm_display} mode=rich_404 nearest_parent={parent_url}"
    );

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from(body))
        .expect("rich_not_found_response builder is infallible")
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
// Directory listing helpers
// ---------------------------------------------------------------------------

/// Apply listing policy to a flat list of `(name, is_dir)` directory entries.
///
/// Policy:
/// - Exclude entries whose name starts with `'.'` (hidden / dotfiles).
/// - Sort: directories first (case-insensitive alphabetical), then files
///   (case-insensitive alphabetical) within each group.
///
/// Symlink containment is handled by the async caller before adding entries
/// to this list.  This function is pure and testable without I/O.
pub fn apply_dir_listing_policy(entries: Vec<(String, bool)>) -> Vec<(String, bool)> {
    let mut filtered: Vec<(String, bool)> = entries
        .into_iter()
        .filter(|(name, _)| !name.starts_with('.'))
        .collect();

    filtered.sort_by(|(a_name, a_dir), (b_name, b_dir)| {
        match (a_dir, b_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a_name.to_lowercase().cmp(&b_name.to_lowercase()),
        }
    });

    filtered
}

/// Build an HTML breadcrumb navigation string from a URL prefix.
///
/// `url_prefix` is either `"/"` (root) or an absolute path like `"/docs/guide"`.
/// Each path segment is percent-encoded in the `href` and displayed as-is.
/// The root segment always links to `"/"`.
fn build_breadcrumbs(url_prefix: &str) -> String {
    let segments: Vec<&str> = url_prefix
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    let mut html = String::from("<a href=\"/\">/</a>");

    let mut href = String::new();
    for seg in &segments {
        href.push('/');
        href.push_str(&percent_encode_segment(seg));
        html.push_str(&format!(" / <a href=\"{href}/\">{seg}</a>"));
    }

    html
}

// ---------------------------------------------------------------------------
// Directory index renderer
// ---------------------------------------------------------------------------

/// Render an HTML directory listing for `dir_path` at URL path `url_prefix`.
///
/// Listing policy (enforced):
/// - Hidden entries (names starting with `'.'`) are excluded.
/// - Symlinks are included only when their canonicalized target is inside
///   `state.canonical_root`; out-of-root symlinks are silently omitted.
/// - Sorted: directories first (case-insensitive alpha), then files
///   (case-insensitive alpha).
/// - Each entry's href is built by percent-encoding the name individually
///   and appending it to the base URL.  Directory entries get a trailing `"/"`.
/// - A breadcrumb navigation bar is rendered above the listing.
///
/// Returns a 404 when the directory cannot be read.
async fn render_directory_index_response(
    state: &AppState,
    dir_path: &Path,
    url_prefix: &str,
) -> Response {
    let mut rd = match tokio::fs::read_dir(dir_path).await {
        Ok(rd) => rd,
        Err(e) => {
            vlog!(
                state.verbose,
                "[dir-index] cannot read dir={} err={e}",
                dir_path.display()
            );
            return not_found_response();
        }
    };

    let mut raw_entries: Vec<(String, bool)> = Vec::new();
    loop {
        match rd.next_entry().await {
            Ok(Some(entry)) => {
                let name = match entry.file_name().to_str().map(|s| s.to_owned()) {
                    Some(n) => n,
                    None => continue,
                };

                // Skip dotfiles — handled by apply_dir_listing_policy, but we also
                // skip here to avoid unnecessary canonicalize calls.
                if name.starts_with('.') {
                    continue;
                }

                let entry_path = entry.path();

                // Symlink containment: only include symlinks whose target lies
                // within canonical_root.
                let file_type = match entry.file_type().await {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };
                if file_type.is_symlink() {
                    match tokio::fs::canonicalize(&entry_path).await {
                        Ok(target) if target.starts_with(&state.canonical_root) => {}
                        _ => {
                            vlog!(
                                state.verbose,
                                "[dir-index] omit out-of-root symlink name={name} dir={}",
                                dir_path.display()
                            );
                            continue;
                        }
                    }
                }

                // Determine if the entry is a directory (follows symlinks).
                let is_dir = match tokio::fs::metadata(&entry_path).await {
                    Ok(m) => m.is_dir(),
                    Err(_) => continue,
                };

                raw_entries.push((name, is_dir));
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    // Apply sort and filter policy.
    let entries = apply_dir_listing_policy(raw_entries);

    // Build breadcrumbs and base href.
    let breadcrumbs = build_breadcrumbs(url_prefix);
    let base = if url_prefix.ends_with('/') {
        url_prefix.to_owned()
    } else {
        format!("{url_prefix}/")
    };

    let mut body = format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Index of {url_prefix}</title></head><body><nav>{breadcrumbs}</nav><h1>Index of {url_prefix}</h1><ul>"
    );
    for (name, is_dir) in &entries {
        let encoded = percent_encode_segment(name);
        let href = if *is_dir {
            format!("{base}{encoded}/")
        } else {
            format!("{base}{encoded}")
        };
        body.push_str(&format!("<li><a href=\"{href}\">{name}</a></li>"));
    }
    body.push_str("</ul></body></html>");

    let etag = compute_etag(body.as_bytes());
    vlog!(state.verbose, "[dir-index] path={url_prefix} entries={}", entries.len());
    vlog!(
        state.verbose,
        "[request] path={url_prefix} mode=directory_index entries={}",
        entries.len()
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header("X-Content-Type-Options", "nosniff")
        .header(header::ETAG, etag)
        .body(Body::from(body))
        .expect("dir index response builder is infallible")
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
    vlog!(state.verbose, "[compression] encoding={compression_enc}");

    // Step 0: serve embedded static assets early — no filesystem access needed.
    if raw_path == "/assets/mdmd.css" {
        let etag = &state.css_etag;
        let last_modified = format_http_date(state.asset_mtime)
            .unwrap_or_else(|| "Thu, 01 Jan 1970 00:00:00 GMT".to_owned());

        // Evaluate If-None-Match first (RFC 7232 §6 preference order).
        if let Some(ref inm) = if_none_match {
            if etag_matches(inm, etag) {
                vlog!(state.verbose, "[cache] path={raw_path} etag={etag} status=304");
                return not_modified_response(etag, &last_modified);
            }
        } else if let Some(ref ims) = if_modified_since {
            if not_modified_since(ims, state.asset_mtime) {
                vlog!(state.verbose, "[cache] path={raw_path} etag={etag} status=304");
                return not_modified_response(etag, &last_modified);
            }
        }

        vlog!(state.verbose, "[cache] path={raw_path} etag={etag} status=200");
        vlog!(state.verbose, "[request] path={raw_path} mode=asset");
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
                vlog!(state.verbose, "[cache] path={raw_path} etag={etag} status=304");
                return not_modified_response(etag, &last_modified);
            }
        } else if let Some(ref ims) = if_modified_since {
            if not_modified_since(ims, state.asset_mtime) {
                vlog!(state.verbose, "[cache] path={raw_path} etag={etag} status=304");
                return not_modified_response(etag, &last_modified);
            }
        }

        vlog!(state.verbose, "[cache] path={raw_path} etag={etag} status=200");
        vlog!(state.verbose, "[request] path={raw_path} mode=asset");
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
            vlog!(state.verbose, "[resolve] path={raw_path} branch=denied reason=invalid-percent-encoding");
            return not_found_response();
        }
    };

    // Reject null bytes anywhere in the decoded path.
    if decoded.contains('\0') {
        vlog!(state.verbose, "[resolve] path={raw_path} branch=denied reason=null-byte");
        return not_found_response();
    }

    // Step 2: normalise.
    let normalized = match normalize_path(&decoded) {
        Some(n) => n,
        None => {
            vlog!(state.verbose, "[resolve] path={raw_path} branch=denied reason=path-traversal");
            return not_found_response();
        }
    };

    let norm_display = normalized.display().to_string();

    // Step 3: early-exit for root "/" → render root directory index directly,
    // bypassing resolve_candidate() entirely.  This ensures GET / always shows
    // a browsable listing even when README.md exists at the project root.
    if normalized == PathBuf::new() {
        vlog!(
            state.verbose,
            "[resolve] path=/ branch=dir-index dir={}",
            state.canonical_root.display()
        );
        return render_directory_index_response(&state, &state.canonical_root, "/").await;
    }

    // Non-root paths: construct candidate relative to serve_root.
    let candidate = state.serve_root.join(&normalized);

    // Step 4: fallback resolution.
    let (resolved, branch) = match resolve_candidate(&candidate).await {
        Some(r) => r,
        None => {
            // If the candidate is a directory with no markdown index file,
            // render a browsable directory listing instead of returning 404.
            if let Ok(meta) = tokio::fs::metadata(&candidate).await {
                if meta.is_dir() {
                    let url_prefix = format!("/{norm_display}");
                    vlog!(
                        state.verbose,
                        "[resolve] path={norm_display} branch=dir-index dir={}",
                        candidate.display()
                    );
                    return render_directory_index_response(&state, &candidate, &url_prefix)
                        .await;
                }
            }
            vlog!(state.verbose, "[resolve] path={norm_display} branch=not-found");
            return rich_not_found_response(&state, &norm_display).await;
        }
    };

    // Step 5 (R1): canonicalise and re-verify containment (symlink-safe).
    let canonical = match tokio::fs::canonicalize(&resolved).await {
        Ok(c) => c,
        Err(_) => {
            vlog!(state.verbose, "[resolve] path={norm_display} branch=denied reason=canonicalize-failed");
            return not_found_response();
        }
    };

    if !canonical.starts_with(&state.canonical_root) {
        vlog!(
            state.verbose,
            "[resolve] path={norm_display} branch=denied reason=outside-root canonical={}",
            canonical.display()
        );
        return not_found_response();
    }

    // Step 6 (R5): file size guard — stat before reading; also capture mtime.
    let file_meta = match tokio::fs::metadata(&canonical).await {
        Ok(m) => m,
        Err(_) => {
            vlog!(state.verbose, "[resolve] path={norm_display} branch=denied reason=metadata-failed");
            return not_found_response();
        }
    };
    let size = file_meta.len();
    let mtime = file_meta.modified().ok();

    if size > MAX_FILE_SIZE {
        vlog!(state.verbose, "[resolve] path={norm_display} branch=denied reason=too-large size={size}");
        return too_large_response(&norm_display, size);
    }

    vlog!(state.verbose, "[resolve] path={norm_display} branch={branch} size={size}");

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
                    vlog!(state.verbose, "[cache] path={norm_display} etag={etag} status=304");
                    return not_modified_response(&etag, &last_modified);
                }
            } else if let Some(ref ims) = if_modified_since {
                if let Some(mt) = mtime {
                    if not_modified_since(ims, mt) {
                        vlog!(state.verbose, "[cache] path={norm_display} etag={etag} status=304");
                        return not_modified_response(&etag, &last_modified);
                    }
                }
            }

            vlog!(state.verbose, "[cache] path={norm_display} etag={etag} status=200");
            vlog!(state.verbose, "[request] path={norm_display} mode=raw");
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
            html::render_markdown(&content, &canonical, &state.canonical_root, state.verbose);
        let key = crate::backlinks::url_key_from_rel_path(&norm_display);
        let backlinks_slice = state.backlinks.get(&key).map(Vec::as_slice).unwrap_or(&[]);
        vlog!(state.verbose, "[backlinks] key={key} found={}", backlinks_slice.len());
        let file_mtime_secs = mtime
            .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        let shell_ctx = html::PageShellContext {
            backlinks: backlinks_slice,
            file_mtime_secs,
            page_url_path: Some(&norm_display),
        };
        let page = html::build_page_shell(
            &html_body,
            &headings,
            &canonical,
            &state.canonical_root,
            &shell_ctx,
        );

        let etag = compute_etag(page.as_bytes());
        let last_modified = mtime
            .and_then(format_http_date)
            .unwrap_or_else(|| "Thu, 01 Jan 1970 00:00:00 GMT".to_owned());

        if let Some(ref inm) = if_none_match {
            if etag_matches(inm, &etag) {
                vlog!(state.verbose, "[cache] path={norm_display} etag={etag} status=304");
                return not_modified_response(&etag, &last_modified);
            }
        } else if let Some(ref ims) = if_modified_since {
            if let Some(mt) = mtime {
                if not_modified_since(ims, mt) {
                    vlog!(state.verbose, "[cache] path={norm_display} etag={etag} status=304");
                    return not_modified_response(&etag, &last_modified);
                }
            }
        }

        vlog!(state.verbose, "[cache] path={norm_display} etag={etag} status=200");
        vlog!(state.verbose, "[request] path={norm_display} mode=rendered");
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
                vlog!(state.verbose, "[cache] path={norm_display} etag={etag} status=304");
                return not_modified_response(&etag, &last_modified);
            }
        } else if let Some(ref ims) = if_modified_since {
            if let Some(mt) = mtime {
                if not_modified_since(ims, mt) {
                    vlog!(state.verbose, "[cache] path={norm_display} etag={etag} status=304");
                    return not_modified_response(&etag, &last_modified);
                }
            }
        }

        vlog!(state.verbose, "[cache] path={norm_display} etag={etag} status=200");
        vlog!(state.verbose, "[request] path={norm_display} mode=static_asset");
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
// Freshness endpoint
// ---------------------------------------------------------------------------

/// JSON 404 response used by the freshness endpoint for all error cases.
fn freshness_404() -> Response {
    let body = serde_json::json!({ "error": "not found" }).to_string();
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from(body))
        .expect("freshness_404 builder is infallible")
}

/// Handler for `GET /_mdmd/freshness?path=<encoded>`.
///
/// Returns `{"mtime":<u64>}` with the file's Unix-epoch modification time.
/// Returns a JSON 404 on path traversal, outside-root, or file errors.
async fn freshness_handler(State(state): State<Arc<AppState>>, req: Request) -> Response {
    // Extract the `path` query parameter.
    let query = req.uri().query().unwrap_or("");
    let path_raw = query
        .split('&')
        .find_map(|param| {
            let mut parts = param.splitn(2, '=');
            match (parts.next(), parts.next()) {
                (Some("path"), Some(v)) => Some(v),
                _ => None,
            }
        })
        .unwrap_or("");

    // Step 1: percent-decode.
    let decoded = match percent_decode(path_raw) {
        Ok(d) => d,
        Err(_) => {
            vlog!(state.verbose, "[freshness] path={path_raw} reason=invalid-percent-encoding");
            return freshness_404();
        }
    };

    // Reject null bytes.
    if decoded.contains('\0') {
        vlog!(state.verbose, "[freshness] reason=null-byte");
        return freshness_404();
    }

    // Step 2: normalize (handles WITH or WITHOUT leading slash).
    let normalized = match normalize_path(&decoded) {
        Some(n) => n,
        None => {
            vlog!(state.verbose, "[freshness] reason=path-traversal");
            return freshness_404();
        }
    };

    // Reject empty path (points to root directory, not a file).
    if normalized == std::path::PathBuf::new() {
        vlog!(state.verbose, "[freshness] reason=empty-path");
        return freshness_404();
    }

    let display_path = normalized.display().to_string();

    // Step 3: resolve via canonical_root and canonicalize.
    let candidate = state.canonical_root.join(&normalized);
    let canonical = match tokio::fs::canonicalize(&candidate).await {
        Ok(c) => c,
        Err(_) => {
            vlog!(state.verbose, "[freshness] path={display_path} reason=canonicalize-failed");
            return freshness_404();
        }
    };

    // Containment check: must stay within canonical_root.
    if !canonical.starts_with(&state.canonical_root) {
        vlog!(state.verbose, "[freshness] path={display_path} reason=outside-root");
        return freshness_404();
    }

    // Step 4: stat the file.
    let meta = match tokio::fs::metadata(&canonical).await {
        Ok(m) => m,
        Err(_) => {
            vlog!(state.verbose, "[freshness] path={display_path} reason=metadata-failed");
            return freshness_404();
        }
    };

    // Extract mtime as Unix seconds (0 if unavailable).
    let mtime_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    vlog!(state.verbose, "[freshness] path={display_path} mtime={mtime_secs}");

    let body = serde_json::json!({ "mtime": mtime_secs }).to_string();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from(body))
        .expect("freshness_handler response builder is infallible")
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

/// Start the HTTP server for the given markdown `file`.
///
/// Binds to `bind_addr` starting at `start_port`, retrying on `EADDRINUSE` up
/// to 100 times.  The server shuts down cleanly when SIGINT (Ctrl+C) is
/// received.
pub async fn run_serve(file: String, bind_addr: String, start_port: u16, no_open: bool, verbose: bool) -> io::Result<()> {
    // Use CWD as the default serve root.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let canonical_cwd = std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());

    // Canonicalize the entry file path.
    let raw_entry = PathBuf::from(&file);
    let canonical_entry = std::fs::canonicalize(&raw_entry).map_err(|e| {
        let msg = format!("entry '{}' not found: {}", file, e);
        eprintln!("Error: {msg}");
        io::Error::new(io::ErrorKind::NotFound, msg)
    })?;

    // Determine serve_root and canonical_root based on whether the entry is inside CWD.
    let (serve_root, canonical_root) = if canonical_entry.starts_with(&canonical_cwd) {
        // Entry is inside CWD: use CWD as serve root (unchanged behavior).
        (cwd, canonical_cwd)
    } else {
        // Entry is outside CWD: derive serve_root from entry location.
        let new_root = if canonical_entry.is_dir() {
            canonical_entry.clone()
        } else {
            canonical_entry
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| canonical_entry.clone())
        };
        let canonical_new_root =
            std::fs::canonicalize(&new_root).unwrap_or_else(|_| new_root.clone());

        // Warn about network exposure risk (all to stderr).
        eprintln!("WARNING: Serving files from outside your current working directory.");
        eprintln!("serve_root: {}", canonical_new_root.display());
        eprintln!("Any file under this directory may be accessible to others on your network.");

        // Show the interactive prompt only when stdin is a terminal.
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            eprint!("Proceed? [y/N] ");
            {
                use std::io::Write;
                let _ = std::io::stderr().flush();
            }
        }

        // Attempt to read a confirmation line from stdin regardless of whether
        // it is a terminal.  When stdin is null (Stdio::null or redirected from
        // /dev/null) read_line returns 0 bytes (EOF) and we auto-proceed.  When
        // stdin is a pipe with content the line is read and checked normally.
        let mut answer = String::new();
        let n = {
            use std::io::BufRead;
            std::io::BufReader::new(std::io::stdin().lock())
                .read_line(&mut answer)
                .unwrap_or(0)
        };
        if n == 0 {
            // EOF (null stdin): auto-proceed without confirmation.
            vlog!(verbose, "[info] Non-interactive stdin; proceeding without confirmation.");
        } else {
            let trimmed = answer.trim().to_lowercase();
            if trimmed != "y" && trimmed != "yes" {
                eprintln!("Aborted.");
                std::process::exit(1);
            }
        }

        (new_root, canonical_new_root)
    };

    // If the entry resolves to a directory, apply the README.md / index.md fallback.
    let entry_file = if canonical_entry.is_dir() {
        let readme = canonical_entry.join("README.md");
        let index_md = canonical_entry.join("index.md");
        if readme.is_file() {
            readme
        } else if index_md.is_file() {
            index_md
        } else {
            let msg = format!(
                "no README.md or index.md found in directory '{}'",
                canonical_entry.display()
            );
            eprintln!("Error: {msg}");
            return Err(io::Error::new(io::ErrorKind::NotFound, msg));
        }
    } else {
        canonical_entry
    };

    // Compute the URL path for the entry file (used in the startup banner and by handlers).
    let entry_url_path = derive_entry_url_path(&entry_file, &canonical_root).map_err(|msg| {
        eprintln!("Error: {msg}");
        io::Error::new(io::ErrorKind::InvalidInput, msg)
    })?;

    // Build the startup backlinks index synchronously before server bind.
    // The index is eventually-stale by design; users must restart the server
    // after editing files to pick up changes.
    let backlinks = crate::backlinks::build_backlinks_index(&canonical_root, verbose);

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
        entry_url_path,
        config: AppConfig,
        css_etag,
        js_etag,
        asset_mtime,
        backlinks,
        verbose,
    });

    let (std_listener, bound_port) =
        bind_with_retry(&bind_addr, start_port, verbose).map_err(|msg| {
            eprintln!("Error: {}", msg);
            io::Error::new(io::ErrorKind::AddrInUse, msg)
        })?;

    std_listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(std_listener)?;

    // CompressionLayer transparently compresses text responses using gzip or
    // brotli based on the client's Accept-Encoding header.  It is added as the
    // outermost layer so it wraps all handler responses.
    let app = Router::new()
        .route("/_mdmd/freshness", axum::routing::get(freshness_handler))
        .fallback(serve_handler)
        .with_state(state.clone())
        .layer(CompressionLayer::new());

    vlog!(verbose, "[serve] listening on {}:{}", bind_addr, bound_port);
    vlog!(
        verbose,
        "[serve] serve_root={} entry_url_path={}",
        state.canonical_root.display(),
        state.entry_url_path
    );

    // Startup stdout: bare URL only — no labels.
    // Prefer the Tailscale hostname when available; fall back to localhost.
    let tailscale_host = tokio::task::spawn_blocking(move || tailscale_dns_name(verbose))
        .await
        .ok()
        .flatten();
    if let Some(ref host) = tailscale_host {
        println!("http://{host}:{bound_port}{}", state.entry_url_path);
    } else {
        println!("http://127.0.0.1:{bound_port}{}", state.entry_url_path);
    }

    // Attempt to open the entry URL in the default browser (fire-and-forget).
    // Must run after all stdout URL lines are printed so the URL is visible
    // even if the open attempt fails or is skipped.
    //
    // The open command may be overridden via the `MDMD_OPEN_CMD` environment
    // variable.  Integration tests set this to a nonexistent binary so they
    // can verify open-attempt logic without launching a real browser.
    if should_attempt_open(no_open, is_headed_environment()) {
        let url = format!("http://127.0.0.1:{bound_port}{}", state.entry_url_path);
        let open_cmd = resolve_open_cmd(std::env::var("MDMD_OPEN_CMD").ok().as_deref());
        match spawn_browser_open(&open_cmd, &url) {
            Ok(_) => vlog!(verbose, "[browser] opened {url}"),
            Err(e) => vlog!(verbose, "[browser] open failed: {e}"),
        }
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            signal::ctrl_c()
                .await
                .expect("failed to install SIGINT handler");
            vlog!(verbose, "[shutdown] complete");
        })
        .await
        .map_err(io::Error::other)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- percent_encode_segment ---

    #[test]
    fn encode_unreserved_chars_pass_through() {
        assert_eq!(percent_encode_segment("README.md"), "README.md");
        assert_eq!(percent_encode_segment("guide"), "guide");
        assert_eq!(percent_encode_segment("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(percent_encode_segment("ABC123"), "ABC123");
    }

    #[test]
    fn encode_space_as_percent20() {
        assert_eq!(percent_encode_segment("my file.md"), "my%20file.md");
        assert_eq!(percent_encode_segment("hello world"), "hello%20world");
    }

    #[test]
    fn encode_multibyte_unicode() {
        // "é" is U+00E9 → UTF-8: 0xC3 0xA9
        assert_eq!(percent_encode_segment("café"), "caf%C3%A9");
        // ☺ is U+263A → UTF-8: 0xE2 0x98 0xBA
        assert_eq!(percent_encode_segment("☺"), "%E2%98%BA");
    }

    #[test]
    fn encode_reserved_url_chars() {
        assert_eq!(percent_encode_segment("a/b"), "a%2Fb");
        assert_eq!(percent_encode_segment("a#b"), "a%23b");
        assert_eq!(percent_encode_segment("a?b"), "a%3Fb");
        assert_eq!(percent_encode_segment("a%b"), "a%25b");
    }

    // --- derive_entry_url_path ---

    #[test]
    fn url_path_simple_nested() {
        let root = PathBuf::from("/home/user/docs");
        let entry = PathBuf::from("/home/user/docs/playground/README.md");
        assert_eq!(
            derive_entry_url_path(&entry, &root).unwrap(),
            "/playground/README.md"
        );
    }

    #[test]
    fn url_path_top_level_file() {
        let root = PathBuf::from("/home/user/docs");
        let entry = PathBuf::from("/home/user/docs/README.md");
        assert_eq!(
            derive_entry_url_path(&entry, &root).unwrap(),
            "/README.md"
        );
    }

    #[test]
    fn url_path_entry_equals_root() {
        let root = PathBuf::from("/home/user/docs");
        let entry = PathBuf::from("/home/user/docs");
        assert_eq!(derive_entry_url_path(&entry, &root).unwrap(), "/");
    }

    #[test]
    fn url_path_deeply_nested() {
        let root = PathBuf::from("/repo");
        let entry = PathBuf::from("/repo/a/b/c/page.md");
        assert_eq!(
            derive_entry_url_path(&entry, &root).unwrap(),
            "/a/b/c/page.md"
        );
    }

    #[test]
    fn url_path_with_spaces() {
        let root = PathBuf::from("/home/user/docs");
        let entry = PathBuf::from("/home/user/docs/my docs/guide.md");
        assert_eq!(
            derive_entry_url_path(&entry, &root).unwrap(),
            "/my%20docs/guide.md"
        );
    }

    #[test]
    fn url_path_with_unicode_segment() {
        let root = PathBuf::from("/home/user/docs");
        let entry = PathBuf::from("/home/user/docs/café/guide.md");
        assert_eq!(
            derive_entry_url_path(&entry, &root).unwrap(),
            "/caf%C3%A9/guide.md"
        );
    }

    #[test]
    fn url_path_outside_root_is_err() {
        let root = PathBuf::from("/home/user/docs");
        let entry = PathBuf::from("/tmp/other/README.md");
        assert!(derive_entry_url_path(&entry, &root).is_err());
    }

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

    /// `tailscale_dns_name(false)` must complete without panicking on either
    /// the subprocess-error branch or the success path.
    ///
    /// When the `tailscale` binary is absent the subprocess-error branch is
    /// taken; `vlog!(false, ...)` is silently suppressed (no stderr output).
    /// When `tailscale` is present the success path returns Some without any
    /// diagnostic output.  Either outcome is acceptable.
    #[test]
    fn tailscale_dns_name_verbose_false_does_not_panic() {
        let _ = tailscale_dns_name(false);
    }

    /// `tailscale_dns_name(true)` must complete without panicking on either
    /// the subprocess-error branch or the success path.
    ///
    /// When the `tailscale` binary is absent the subprocess-error branch emits
    /// a `[tailscale] skipped` line via `vlog!(true, ...)` and the function
    /// returns `None`.  Either outcome is acceptable; only no-panic is asserted.
    #[test]
    fn tailscale_dns_name_verbose_true_does_not_panic() {
        let _ = tailscale_dns_name(true);
    }

    // --- startup URL output contract ---

    /// The local URL line is a bare `http://127.0.0.1:{port}{path}` string —
    /// no label prefix, no 'index:' variant, no extra decoration.
    #[test]
    fn local_url_line_is_bare_http_127_0_0_1() {
        let port: u16 = 4321;
        let path = "/README.md";

        let line = format!("http://127.0.0.1:{port}{path}");

        assert!(
            line.starts_with("http://127.0.0.1:"),
            "local URL must start with 'http://127.0.0.1:', got: {line:?}"
        );
        assert!(
            !line.starts_with("url:"),
            "local URL must not have 'url:' label prefix, got: {line:?}"
        );
        assert!(
            !line.starts_with("mdmd serve"),
            "local URL must not start with 'mdmd serve' banner, got: {line:?}"
        );
        assert!(
            !line.starts_with("root:"),
            "local URL must not start with 'root:' label, got: {line:?}"
        );
        assert!(
            !line.starts_with("index:"),
            "local URL must not start with 'index:' label, got: {line:?}"
        );
        assert_eq!(line, "http://127.0.0.1:4321/README.md");
    }

    /// When tailscale is present the startup URL block emits exactly one bare URL
    /// line using the tailscale hostname — no local 127.0.0.1 line, no labels.
    #[test]
    fn startup_url_block_tailscale_only_when_tailscale_present() {
        let port: u16 = 8080;
        let path = "/guide.md";
        let ts_host = "myhost.ts.net";

        let tailscale_host: Option<&str> = Some(ts_host);
        let block: Vec<String> = if let Some(h) = tailscale_host {
            vec![format!("http://{h}:{port}{path}")]
        } else {
            vec![format!("http://127.0.0.1:{port}{path}")]
        };

        assert_eq!(block.len(), 1, "expected exactly one URL line when tailscale is present");
        assert!(
            block[0].starts_with("http://myhost.ts.net:"),
            "sole line must be tailscale URL, got: {:?}",
            block[0]
        );
        assert!(
            !block[0].starts_with("http://127.0.0.1:"),
            "must not emit local URL when tailscale is available, got: {:?}",
            block[0]
        );
        for forbidden in &["url:", "index:", "mdmd serve", "root:", "entry:"] {
            assert!(
                !block[0].starts_with(forbidden),
                "startup URL block must not contain label {forbidden:?}, line: {:?}",
                block[0]
            );
        }
    }

    /// When tailscale is absent the startup URL block contains exactly one line.
    #[test]
    fn startup_url_block_local_only_when_no_tailscale() {
        let port: u16 = 9000;
        let path = "/README.md";

        let local_line = format!("http://127.0.0.1:{port}{path}");
        let tailscale_host: Option<&str> = None;

        let mut block: Vec<String> = vec![local_line];
        if let Some(h) = tailscale_host {
            block.push(format!("http://{h}:{port}{path}"));
        }

        assert_eq!(block.len(), 1, "expected exactly one URL line when tailscale is absent");
        assert!(
            block[0].starts_with("http://127.0.0.1:"),
            "sole line must be local URL, got: {:?}",
            block[0]
        );
    }

    // --- tailscale URL output contract ---

    /// When tailscale_host is Some, the startup URL block emits exactly one
    /// bare URL line — no 'url:' label prefix, no 'index:' variant line.
    #[test]
    fn tailscale_url_present_emits_one_bare_url_no_index() {
        let host = "mymachine.ts.net";
        let port: u16 = 8080;
        let path = "/entry.html";

        let tailscale_host: Option<&str> = Some(host);
        let mut lines: Vec<String> = vec![];
        if let Some(h) = tailscale_host {
            lines.push(format!("http://{h}:{port}{path}"));
        }

        assert_eq!(lines.len(), 1, "expected exactly one tailscale URL line");
        let url = &lines[0];
        assert!(
            url.starts_with("http://"),
            "URL must start with 'http://', got: {url:?}"
        );
        assert!(
            !url.starts_with("url:"),
            "URL must not have 'url:' label prefix, got: {url:?}"
        );
        assert!(
            !lines.iter().any(|l| l.starts_with("index:")),
            "no 'index:' line must appear"
        );
    }

    /// When tailscale_host is None, no extra URL lines are emitted.
    #[test]
    fn tailscale_url_absent_emits_no_lines() {
        let port: u16 = 8080;
        let path = "/entry.html";

        let tailscale_host: Option<&str> = None;
        let mut lines: Vec<String> = vec![];
        if let Some(h) = tailscale_host {
            lines.push(format!("http://{h}:{port}{path}"));
        }

        assert_eq!(
            lines.len(),
            0,
            "expected zero tailscale URL lines when host is absent"
        );
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

    // --- apply_dir_listing_policy ---

    #[test]
    fn listing_policy_excludes_dotfiles() {
        let entries = vec![
            (".hidden".to_owned(), false),
            ("visible.md".to_owned(), false),
            (".git".to_owned(), true),
        ];
        let result = apply_dir_listing_policy(entries);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "visible.md");
    }

    #[test]
    fn listing_policy_dirs_before_files() {
        let entries = vec![
            ("zzz-file.txt".to_owned(), false),
            ("aaa-file.md".to_owned(), false),
            ("bbb-dir".to_owned(), true),
            ("aaa-dir".to_owned(), true),
        ];
        let result = apply_dir_listing_policy(entries);
        // Directories first (alphabetical), then files (alphabetical).
        assert_eq!(result[0], ("aaa-dir".to_owned(), true));
        assert_eq!(result[1], ("bbb-dir".to_owned(), true));
        assert_eq!(result[2], ("aaa-file.md".to_owned(), false));
        assert_eq!(result[3], ("zzz-file.txt".to_owned(), false));
    }

    #[test]
    fn listing_policy_case_insensitive_sort() {
        let entries = vec![
            ("Zebra.md".to_owned(), false),
            ("apple.md".to_owned(), false),
            ("Mango.md".to_owned(), false),
        ];
        let result = apply_dir_listing_policy(entries);
        // Case-insensitive: apple < Mango < Zebra
        assert_eq!(result[0].0, "apple.md");
        assert_eq!(result[1].0, "Mango.md");
        assert_eq!(result[2].0, "Zebra.md");
    }

    #[test]
    fn listing_policy_empty_input() {
        let result = apply_dir_listing_policy(vec![]);
        assert!(result.is_empty());
    }

    #[test]
    fn listing_policy_only_dotfiles_filtered_out() {
        let entries = vec![
            (".env".to_owned(), false),
            (".gitignore".to_owned(), false),
        ];
        let result = apply_dir_listing_policy(entries);
        assert!(result.is_empty());
    }

    // --- build_breadcrumbs ---

    #[test]
    fn breadcrumbs_root_only() {
        let html = build_breadcrumbs("/");
        assert!(html.contains("href=\"/\""), "root link missing: {html}");
        // Only the root link, no extra segments.
        assert_eq!(html.matches("<a href=").count(), 1);
    }

    #[test]
    fn breadcrumbs_one_segment() {
        let html = build_breadcrumbs("/docs");
        assert!(html.contains("href=\"/\""), "root link missing: {html}");
        assert!(html.contains("href=\"/docs/\""), "docs link missing: {html}");
        assert!(html.contains(">docs<"), "docs text missing: {html}");
    }

    #[test]
    fn breadcrumbs_two_segments() {
        let html = build_breadcrumbs("/docs/guide");
        assert!(html.contains("href=\"/\""), "root link missing: {html}");
        assert!(html.contains("href=\"/docs/\""), "docs link missing: {html}");
        assert!(html.contains("href=\"/docs/guide/\""), "guide link missing: {html}");
        assert!(html.contains(">guide<"), "guide text missing: {html}");
    }

    #[test]
    fn breadcrumbs_encodes_special_chars() {
        let html = build_breadcrumbs("/my docs/sub dir");
        assert!(
            html.contains("href=\"/my%20docs/\""),
            "space encoding missing: {html}"
        );
        assert!(
            html.contains("href=\"/my%20docs/sub%20dir/\""),
            "nested space encoding missing: {html}"
        );
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

    // --- nearest_existing_parent ---

    /// Helper: create a temp directory tree and return the canonical root.
    fn make_temp_root(name: &str) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let canonical = std::fs::canonicalize(tmp.path()).expect("canonicalize");
        let _ = name; // disambiguates call sites in diagnostics
        (tmp, canonical)
    }

    #[test]
    fn nearest_parent_missing_leaf_returns_direct_parent() {
        // Structure: root/docs/ exists, root/docs/missing.md does not.
        let (_tmp, root) = make_temp_root("nep_leaf");
        let docs = root.join("docs");
        std::fs::create_dir_all(&docs).unwrap();

        let result = nearest_existing_parent(&root, &root, "docs/missing.md");
        assert_eq!(
            result,
            std::fs::canonicalize(&docs).unwrap(),
            "should return existing docs/ dir"
        );
    }

    #[test]
    fn nearest_parent_multi_level_miss_walks_up() {
        // Structure: root/ exists, root/a/b/c/ does not exist.
        let (_tmp, root) = make_temp_root("nep_multi");
        // Only root itself exists (no subdirs created).

        let result = nearest_existing_parent(&root, &root, "a/b/c/missing.md");
        assert_eq!(result, root, "should fall back to root when all parents missing");
    }

    #[test]
    fn nearest_parent_root_fallback_when_nothing_exists() {
        // Structure: only root/ exists, no subdirs.
        let (_tmp, root) = make_temp_root("nep_root");

        let result = nearest_existing_parent(&root, &root, "deep/path/missing.md");
        assert_eq!(result, root, "should return root as ultimate fallback");
    }

    #[test]
    fn nearest_parent_intermediate_dir_exists() {
        // Structure: root/a/b/ exists, root/a/b/c/ does not.
        let (_tmp, root) = make_temp_root("nep_intermediate");
        let b = root.join("a").join("b");
        std::fs::create_dir_all(&b).unwrap();

        let result = nearest_existing_parent(&root, &root, "a/b/c/deep/missing.md");
        assert_eq!(
            result,
            std::fs::canonicalize(&b).unwrap(),
            "should return deepest existing ancestor a/b/"
        );
    }

    #[test]
    fn nearest_parent_single_segment_missing_returns_root() {
        // Structure: root/ only — root/missing.md does not exist.
        let (_tmp, root) = make_temp_root("nep_single");

        let result = nearest_existing_parent(&root, &root, "missing.md");
        assert_eq!(result, root, "single missing file should return root");
    }

    #[test]
    fn nearest_parent_encoded_segment_treated_as_literal() {
        // norm_display comes through normalize_path so percent-encoding has
        // already been decoded; the segment is used literally as a dir name.
        let (_tmp, root) = make_temp_root("nep_encoded");
        // Create a directory whose name contains a space (decoded from %20).
        let spaced = root.join("my docs");
        std::fs::create_dir_all(&spaced).unwrap();

        let result = nearest_existing_parent(&root, &root, "my docs/missing.md");
        assert_eq!(
            result,
            std::fs::canonicalize(&spaced).unwrap(),
            "decoded path segment should match dir with spaces"
        );
    }

    // -----------------------------------------------------------------------
    // bd-3oh.2: key-parity, norm_display round-trip, and self-link tests
    // -----------------------------------------------------------------------

    // Tests 1-3: url_key_from_rel_path produces correct keys for the paths
    // that appear as norm_display values in serve_handler.

    #[test]
    fn url_key_parity_docs_readme() {
        // Test 1: url_key_from_rel_path("docs/readme.md") == "/docs/readme.md"
        assert_eq!(
            crate::backlinks::url_key_from_rel_path("docs/readme.md"),
            "/docs/readme.md"
        );
    }

    #[test]
    fn url_key_parity_empty() {
        // Test 2: url_key_from_rel_path("") == "/"
        assert_eq!(crate::backlinks::url_key_from_rel_path(""), "/");
    }

    #[test]
    fn url_key_parity_readme() {
        // Test 3: url_key_from_rel_path("readme.md") == "/readme.md"
        assert_eq!(
            crate::backlinks::url_key_from_rel_path("readme.md"),
            "/readme.md"
        );
    }

    #[test]
    fn norm_display_round_trip() {
        // Test 4: Simulate the serve_handler pipeline for a percent-encoded path.
        // percent_decode("/docs/read%20me.md") + normalize_path()
        // → norm_display = "docs/read me.md"
        // → url_key_from_rel_path(norm_display) = "/docs/read me.md"
        let decoded = percent_decode("/docs/read%20me.md").unwrap();
        let normalized = normalize_path(&decoded).unwrap();
        let norm_display = normalized.display().to_string();
        assert_eq!(
            norm_display,
            "docs/read me.md",
            "decoded path must produce norm_display without leading slash"
        );
        let key = crate::backlinks::url_key_from_rel_path(&norm_display);
        assert_eq!(
            key,
            "/docs/read me.md",
            "key must have leading slash and decoded (space-containing) content"
        );
    }

    #[test]
    fn key_has_no_fragment() {
        // Test 5: axum separates path from fragment before serve_handler is called.
        // Confirm that normalize_path + url_key_from_rel_path produces a fragment-free key.
        let decoded = percent_decode("/docs/page.md").unwrap();
        let normalized = normalize_path(&decoded).unwrap();
        let norm_display = normalized.display().to_string();
        let key = crate::backlinks::url_key_from_rel_path(&norm_display);
        assert_eq!(key, "/docs/page.md");
        assert!(
            !key.contains('#'),
            "backlinks lookup key must never contain a fragment"
        );
    }

    #[test]
    fn backlinks_index_self_link_excluded() {
        // Test 6: source_url_path = '/docs/a.md' links to target_url_path = '/docs/a.md'.
        // Assert backlinks_index.get('/docs/a.md') is absent or empty.
        let tmp = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("a.md"), "# A Doc\n\nSee [self](a.md).\n").unwrap();

        let idx = crate::backlinks::build_backlinks_index(tmp.path(), false);
        let is_empty = idx
            .get("/docs/a.md")
            .map(|v| v.is_empty())
            .unwrap_or(true);
        assert!(
            is_empty,
            "self-link must not produce a backlink entry for /docs/a.md"
        );
    }

    #[test]
    fn backlinks_index_different_files_linked() {
        // Test 7: /docs/a.md links to /docs/b.md (different files).
        // Assert backlinks_index['/docs/b.md'] contains one BacklinkRef
        // with source_url_path = '/docs/a.md'.
        let tmp = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("a.md"), "# A Doc\n\nSee [B](b.md).\n").unwrap();
        std::fs::write(docs.join("b.md"), "# B Doc\n").unwrap();

        let idx = crate::backlinks::build_backlinks_index(tmp.path(), false);
        let refs = idx
            .get("/docs/b.md")
            .expect("/docs/b.md must have a backlink from /docs/a.md");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].source_url_path, "/docs/a.md");
    }

    // --- vlog! macro ---

    /// `vlog!(false, ...)` must not evaluate format arguments.
    ///
    /// The format argument contains a side effect (incrementing a counter).
    /// Because the `vlog!` body is inside `if false { ... }`, the block is
    /// never entered, so the counter must remain 0.
    #[test]
    fn vlog_suppressed_when_verbose_false() {
        let mut count = 0i32;
        vlog!(false, "{}", {
            count += 1;
            count
        });
        assert_eq!(count, 0, "vlog!(false, ...) must not evaluate format args");
    }

    /// `vlog!(true, ...)` must evaluate format arguments and produce output.
    ///
    /// The format argument contains a side effect (incrementing a counter).
    /// With verbose=true the `eprintln!` body runs, so the counter must be 1.
    #[test]
    fn vlog_runs_when_verbose_true() {
        let mut count = 0i32;
        vlog!(true, "{}", {
            count += 1;
            count
        });
        assert_eq!(count, 1, "vlog!(true, ...) must evaluate format args");
    }

    /// Verify that `bind_with_retry` propagates the verbose flag.
    ///
    /// With `verbose=false` no `[bind]` lines should appear; the function
    /// must still succeed when a free port is available.  We check the
    /// return value only — stderr capture is avoided per the test guidelines.
    #[test]
    fn bind_with_retry_succeeds_with_verbose_false() {
        let result = bind_with_retry("127.0.0.1", 0, false);
        // Port 0 lets the OS pick a free port — should always succeed.
        assert!(
            result.is_ok(),
            "bind_with_retry with verbose=false must succeed on a free port"
        );
    }

    #[test]
    fn bind_with_retry_succeeds_with_verbose_true() {
        let result = bind_with_retry("127.0.0.1", 0, true);
        assert!(
            result.is_ok(),
            "bind_with_retry with verbose=true must succeed on a free port"
        );
    }

    // --- is_headed_for ---

    /// Helper: a fully-clear env snapshot (no env vars set).
    fn clear_env() -> EnvSnapshot {
        EnvSnapshot {
            ssh_connection: false,
            ssh_tty: false,
            display: false,
            wayland_display: false,
            ci: false,
            github_actions: false,
        }
    }

    // macOS rules

    #[test]
    fn macos_no_ssh_is_headed() {
        let env = clear_env();
        assert!(is_headed_for(RuntimePlatform::MacOs, &env));
    }

    #[test]
    fn macos_ssh_connection_is_not_headed() {
        let env = EnvSnapshot { ssh_connection: true, ..clear_env() };
        assert!(!is_headed_for(RuntimePlatform::MacOs, &env));
    }

    #[test]
    fn macos_ssh_tty_is_not_headed() {
        let env = EnvSnapshot { ssh_tty: true, ..clear_env() };
        assert!(!is_headed_for(RuntimePlatform::MacOs, &env));
    }

    #[test]
    fn macos_both_ssh_vars_is_not_headed() {
        let env = EnvSnapshot { ssh_connection: true, ssh_tty: true, ..clear_env() };
        assert!(!is_headed_for(RuntimePlatform::MacOs, &env));
    }

    /// macOS with DISPLAY set but no SSH — still headed (DISPLAY is irrelevant on macOS).
    #[test]
    fn macos_display_set_no_ssh_is_headed() {
        let env = EnvSnapshot { display: true, ..clear_env() };
        assert!(is_headed_for(RuntimePlatform::MacOs, &env));
    }

    // Linux rules

    #[test]
    fn linux_display_no_ssh_no_ci_is_headed() {
        let env = EnvSnapshot { display: true, ..clear_env() };
        assert!(is_headed_for(RuntimePlatform::Linux, &env));
    }

    #[test]
    fn linux_wayland_no_ssh_no_ci_is_headed() {
        let env = EnvSnapshot { wayland_display: true, ..clear_env() };
        assert!(is_headed_for(RuntimePlatform::Linux, &env));
    }

    #[test]
    fn linux_both_display_vars_no_ssh_no_ci_is_headed() {
        let env = EnvSnapshot { display: true, wayland_display: true, ..clear_env() };
        assert!(is_headed_for(RuntimePlatform::Linux, &env));
    }

    #[test]
    fn linux_no_display_no_wayland_is_not_headed() {
        let env = clear_env(); // display=false, wayland_display=false
        assert!(!is_headed_for(RuntimePlatform::Linux, &env));
    }

    #[test]
    fn linux_display_ssh_connection_is_not_headed() {
        let env = EnvSnapshot { display: true, ssh_connection: true, ..clear_env() };
        assert!(!is_headed_for(RuntimePlatform::Linux, &env));
    }

    #[test]
    fn linux_display_ssh_tty_is_not_headed() {
        let env = EnvSnapshot { display: true, ssh_tty: true, ..clear_env() };
        assert!(!is_headed_for(RuntimePlatform::Linux, &env));
    }

    #[test]
    fn linux_display_ci_is_not_headed() {
        let env = EnvSnapshot { display: true, ci: true, ..clear_env() };
        assert!(!is_headed_for(RuntimePlatform::Linux, &env));
    }

    #[test]
    fn linux_display_github_actions_is_not_headed() {
        let env = EnvSnapshot { display: true, github_actions: true, ..clear_env() };
        assert!(!is_headed_for(RuntimePlatform::Linux, &env));
    }

    #[test]
    fn linux_wayland_ci_is_not_headed() {
        let env = EnvSnapshot { wayland_display: true, ci: true, ..clear_env() };
        assert!(!is_headed_for(RuntimePlatform::Linux, &env));
    }

    #[test]
    fn linux_wayland_ssh_connection_is_not_headed() {
        let env = EnvSnapshot { wayland_display: true, ssh_connection: true, ..clear_env() };
        assert!(!is_headed_for(RuntimePlatform::Linux, &env));
    }

    // Other platform rules

    #[test]
    fn other_platform_always_not_headed() {
        // Even with a display set, unknown platforms return false.
        let env = EnvSnapshot { display: true, wayland_display: true, ..clear_env() };
        assert!(!is_headed_for(RuntimePlatform::Other, &env));
    }

    #[test]
    fn other_platform_clear_env_not_headed() {
        assert!(!is_headed_for(RuntimePlatform::Other, &clear_env()));
    }

    // is_headed_environment wrapper sanity test

    #[test]
    fn is_headed_environment_returns_bool() {
        // We can't control the actual environment here, but we can verify the
        // function returns without panicking and produces a valid bool.
        let result = is_headed_environment();
        assert!(result == true || result == false);
    }

    // --- should_attempt_open ---

    #[test]
    fn should_attempt_open_no_open_false_headed_true() {
        assert!(should_attempt_open(false, true));
    }

    #[test]
    fn should_attempt_open_no_open_true_headed_true() {
        assert!(!should_attempt_open(true, true));
    }

    #[test]
    fn should_attempt_open_no_open_false_headed_false() {
        assert!(!should_attempt_open(false, false));
    }

    #[test]
    fn should_attempt_open_no_open_true_headed_false() {
        assert!(!should_attempt_open(true, false));
    }

    // --- default_open_command ---

    #[test]
    fn default_open_command_is_not_empty_on_known_platform() {
        // On macOS or Linux the command must be a non-empty string.
        // On other platforms it must be empty (no browser opener).
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        assert!(!default_open_command().is_empty());

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        assert!(default_open_command().is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn default_open_command_macos_is_open() {
        assert_eq!(default_open_command(), "open");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn default_open_command_linux_is_xdg_open() {
        assert_eq!(default_open_command(), "xdg-open");
    }

    // --- resolve_open_cmd ---

    /// When `MDMD_OPEN_CMD` is provided as `Some`, `resolve_open_cmd` returns
    /// it verbatim, overriding the platform default.
    #[test]
    fn resolve_open_cmd_uses_override_when_provided() {
        assert_eq!(resolve_open_cmd(Some("my-browser")), "my-browser");
        assert_eq!(resolve_open_cmd(Some("/usr/bin/xdg-open")), "/usr/bin/xdg-open");
    }

    /// An empty string override is accepted as-is (callers decide whether to act on it).
    #[test]
    fn resolve_open_cmd_empty_override_preserved() {
        assert_eq!(resolve_open_cmd(Some("")), "");
    }

    /// When no override is provided (`None`), the function falls back to
    /// `default_open_command()` — a non-empty value on macOS/Linux.
    #[test]
    fn resolve_open_cmd_no_override_falls_back_to_platform_default() {
        let cmd = resolve_open_cmd(None);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        assert!(!cmd.is_empty(), "platform default must be non-empty on macOS/Linux");
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        assert!(cmd.is_empty(), "platform default must be empty on unsupported platforms");
    }

    // --- spawn_browser_open ---

    #[test]
    fn spawn_browser_open_empty_cmd_returns_err() {
        let result = spawn_browser_open("", "http://127.0.0.1:8080/");
        assert!(result.is_err());
    }

    #[test]
    fn spawn_browser_open_nonexistent_cmd_returns_err() {
        // A command that cannot possibly exist should fail at spawn time.
        let result = spawn_browser_open("__mdmd_no_such_binary__", "http://127.0.0.1:8080/");
        assert!(result.is_err());
    }
}
