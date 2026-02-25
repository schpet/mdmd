use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tempfile::TempDir;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(6);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_FILE_SIZE: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy)]
struct FixtureOptions {
    include_subdir_readme: bool,
    include_large_file: bool,
    /// Create a `.hidden-dotfile` in the root (for directory-listing exclusion tests).
    include_dotfiles: bool,
    /// Create a `nested/` subdirectory containing `nested-doc.md` and `nested/inner/`
    /// (for deep directory listing and nested-structure tests).
    include_nested_dirs: bool,
}

impl Default for FixtureOptions {
    fn default() -> Self {
        Self {
            include_subdir_readme: true,
            include_large_file: false,
            include_dotfiles: false,
            include_nested_dirs: false,
        }
    }
}

struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
    entry: PathBuf,
}

impl Fixture {
    fn new(opts: FixtureOptions) -> Self {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let root = tmp.path().to_path_buf();

        let readme = root.join("README.md");
        fs::write(
            &readme,
            "# Home\n\n## TOC Section\n\n[Guide](guide.md)\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n- [ ] todo\n- [x] done\n\n```mermaid\ngraph TD;\nA-->B;\n```\n\n<script>alert(1)</script>\n",
        )
        .expect("write README");

        fs::write(root.join("guide.md"), "# Guide\n\nGuide content.\n").expect("write guide");

        let subdir = root.join("subdir");
        fs::create_dir_all(&subdir).expect("create subdir");
        if opts.include_subdir_readme {
            fs::write(subdir.join("README.md"), "# Subdir Readme\n").expect("write subdir readme");
        }
        fs::write(subdir.join("index.md"), "# Subdir Index\n").expect("write subdir index");

        fs::write(
            root.join("image.png"),
            [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'],
        )
        .expect("write image");

        if opts.include_large_file {
            let path = root.join("oversized.md");
            let file = fs::File::create(path).expect("create oversized file");
            file.set_len(MAX_FILE_SIZE + 1)
                .expect("set oversized file len");
        }

        if opts.include_dotfiles {
            fs::write(root.join(".hidden-dotfile"), "hidden content\n")
                .expect("write dotfile");
        }

        if opts.include_nested_dirs {
            let nested = root.join("nested");
            fs::create_dir_all(nested.join("inner")).expect("create nested/inner dir");
            fs::write(nested.join("nested-doc.md"), "# Nested Doc\n")
                .expect("write nested/nested-doc.md");
        }

        Self {
            _tmp: tmp,
            root,
            entry: readme,
        }
    }
}

struct ResponseSnapshot {
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl ResponseSnapshot {
    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_owned())
    }

    fn context(&self) -> String {
        let mut hdrs = String::new();
        for (k, v) in &self.headers {
            let value = v.to_str().unwrap_or("<non-utf8>");
            hdrs.push_str(&format!("{}: {}\n", k.as_str(), value));
        }
        format!(
            "status={}\nheaders:\n{}\nbody:\n{}",
            self.status,
            hdrs,
            self.body_text()
        )
    }
}

struct ServerHandle {
    child: Option<Child>,
    base_url: String,
    port: u16,
}

impl ServerHandle {
    fn new(scenario: &str, fixture: &Fixture) -> Self {
        let port = free_port();
        eprintln!("[TEST] scenario={} port={}", scenario, port);

        let mut child = Command::new(bin_path())
            .arg("serve")
            .arg("--bind")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg(&fixture.entry)
            .current_dir(&fixture.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn mdmd serve");

        let base_url = format!("http://127.0.0.1:{port}");
        wait_for_server_ready(&mut child, &base_url);

        Self {
            child: Some(child),
            base_url,
            port,
        }
    }

    /// Like [`Self::new`] but passes `--verbose` to the server process.
    fn new_verbose(scenario: &str, fixture: &Fixture) -> Self {
        let port = free_port();
        eprintln!("[TEST] scenario={} port={} verbose=true", scenario, port);

        let mut child = Command::new(bin_path())
            .arg("serve")
            .arg("--verbose")
            .arg("--bind")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg(&fixture.entry)
            .current_dir(&fixture.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn mdmd serve --verbose");

        let base_url = format!("http://127.0.0.1:{port}");
        wait_for_server_ready(&mut child, &base_url);

        Self {
            child: Some(child),
            base_url,
            port,
        }
    }

    fn url(&self, path_and_query: &str) -> String {
        format!("{}{}", self.base_url, path_and_query)
    }

    fn shutdown_with_sigint(mut self) -> Output {
        let mut child = self.child.take().expect("server child exists");
        send_sigint(child.id());
        wait_with_timeout(&mut child, Duration::from_secs(5));
        child.wait_with_output().expect("collect server output")
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}

fn bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_mdmd").expect("CARGO_BIN_EXE_mdmd is set by cargo test")
}

fn client() -> Client {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("build reqwest client")
}

fn client_no_auto_decode() -> Client {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .no_gzip()
        .no_brotli()
        .build()
        .expect("build reqwest client")
}

fn fetch(client: &Client, url: &str) -> ResponseSnapshot {
    let resp = client
        .get(url)
        .send()
        .unwrap_or_else(|e| panic!("GET {} failed: {e}", url));
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = resp
        .bytes()
        .unwrap_or_else(|e| panic!("read body for {} failed: {e}", url))
        .to_vec();

    ResponseSnapshot {
        status,
        headers,
        body,
    }
}

fn fetch_with_headers(client: &Client, url: &str, headers: &[(&str, &str)]) -> ResponseSnapshot {
    let mut map = HeaderMap::new();
    for (k, v) in headers {
        let name = HeaderName::from_bytes(k.as_bytes()).expect("valid header name");
        let value = HeaderValue::from_str(v).expect("valid header value");
        map.insert(name, value);
    }

    let resp = client
        .get(url)
        .headers(map)
        .send()
        .unwrap_or_else(|e| panic!("GET {} failed: {e}", url));
    let status = resp.status().as_u16();
    let out_headers = resp.headers().clone();
    let body = resp
        .bytes()
        .unwrap_or_else(|e| panic!("read body for {} failed: {e}", url))
        .to_vec();

    ResponseSnapshot {
        status,
        headers: out_headers,
        body,
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind free port");
    listener.local_addr().expect("local addr").port()
}

fn wait_for_server_ready(child: &mut Child, base_url: &str) {
    let ready_client = Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .expect("build readiness client");

    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait server") {
            let mut stdout = String::new();
            let mut stderr = String::new();
            if let Some(mut out) = child.stdout.take() {
                let _ = out.read_to_string(&mut stdout);
            }
            if let Some(mut err) = child.stderr.take() {
                let _ = err.read_to_string(&mut stderr);
            }
            panic!(
                "server exited early status={}\nstdout:\n{}\nstderr:\n{}",
                status, stdout, stderr
            );
        }

        if ready_client.get(format!("{}/", base_url)).send().is_ok() {
            return;
        }

        if start.elapsed() > STARTUP_TIMEOUT {
            panic!("server did not become ready within {:?}", STARTUP_TIMEOUT);
        }

        thread::sleep(Duration::from_millis(50));
    }
}

fn assert_status(resp: &ResponseSnapshot, expected: u16) {
    assert_eq!(
        resp.status,
        expected,
        "unexpected HTTP status\n{}",
        resp.context()
    );
}

fn assert_header_contains(resp: &ResponseSnapshot, name: &str, needle: &str) {
    let value = resp
        .header(name)
        .unwrap_or_else(|| panic!("missing header '{}'\n{}", name, resp.context()));
    assert!(
        value.contains(needle),
        "header '{}' value '{}' does not contain '{}'\n{}",
        name,
        value,
        needle,
        resp.context()
    );
}

fn assert_header_eq(resp: &ResponseSnapshot, name: &str, expected: &str) {
    let value = resp
        .header(name)
        .unwrap_or_else(|| panic!("missing header '{}'\n{}", name, resp.context()));
    assert_eq!(
        value,
        expected,
        "unexpected header '{}'\n{}",
        name,
        resp.context()
    );
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) {
    let start = std::time::Instant::now();
    loop {
        if child.try_wait().expect("try_wait child").is_some() {
            return;
        }
        if start.elapsed() >= timeout {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn send_sigint(pid: u32) {
    let status = Command::new("kill")
        .arg("-INT")
        .arg(pid.to_string())
        .status()
        .expect("send SIGINT");
    assert!(status.success(), "kill -INT failed for pid {pid}");
}

#[cfg(not(unix))]
fn send_sigint(_pid: u32) {
    panic!("SIGINT test is only supported on unix");
}

fn raw_http_status(port: u16, path: &str) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect raw http");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path, port
    );
    stream.write_all(req.as_bytes()).expect("write raw request");

    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).expect("read raw response");
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    let status_line = lines.next().expect("status line present");
    let mut parts = status_line.split_whitespace();
    let _http = parts.next().expect("http version present");
    let code = parts.next().expect("status code present");
    code.parse::<u16>().expect("parse status code")
}

#[test]
fn test_serve_basic_html() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_basic_html", &fixture);

    let resp = fetch(&client(), &server.url("/README.md"));
    assert_status(&resp, 200);
    assert_header_contains(&resp, "content-type", "text/html");
}

#[test]
fn test_serve_toc_present() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_toc_present", &fixture);

    let resp = fetch(&client(), &server.url("/README.md"));
    assert_status(&resp, 200);
    let body = resp.body_text();
    assert!(
        body.contains("<nav class=\"toc-sidebar\">") && body.contains("href=\"#home\""),
        "TOC not present\n{}",
        resp.context()
    );
}

#[test]
fn test_serve_raw_mode() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_raw_mode", &fixture);

    let resp = fetch(&client(), &server.url("/README.md?raw=1"));
    assert_status(&resp, 200);
    assert_header_contains(&resp, "content-type", "text/plain");
    assert!(
        resp.body_text().contains("# Home"),
        "raw markdown source missing\n{}",
        resp.context()
    );
}

#[test]
fn test_serve_table_rendered() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_table_rendered", &fixture);

    let resp = fetch(&client(), &server.url("/README.md"));
    assert_status(&resp, 200);
    assert!(
        resp.body_text().contains("<table>"),
        "table not rendered\n{}",
        resp.context()
    );
}

#[test]
fn test_serve_task_list_rendered() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_task_list_rendered", &fixture);

    let resp = fetch(&client(), &server.url("/README.md"));
    assert_status(&resp, 200);
    assert!(
        resp.body_text().contains("<input") && resp.body_text().contains("checkbox"),
        "task list checkbox not rendered\n{}",
        resp.context()
    );
}

#[test]
fn test_serve_mermaid_placeholder() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_mermaid_placeholder", &fixture);

    let resp = fetch(&client(), &server.url("/README.md"));
    assert_status(&resp, 200);
    assert!(
        resp.body_text().contains("class=\"mermaid\""),
        "mermaid placeholder missing\n{}",
        resp.context()
    );
}

#[test]
fn test_serve_mermaid_cdn_script() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_mermaid_cdn_script", &fixture);

    let resp = fetch(&client(), &server.url("/README.md"));
    assert_status(&resp, 200);
    assert!(
        resp.body_text()
            .contains("https://cdn.jsdelivr.net/npm/mermaid@10.9.3/dist/mermaid.min.js"),
        "pinned mermaid CDN script missing\n{}",
        resp.context()
    );
}

#[test]
fn test_serve_root_index() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_root_index", &fixture);

    let resp = fetch(&client(), &server.url("/"));
    assert_status(&resp, 200);
    assert_header_contains(&resp, "content-type", "text/html");
    let body = resp.body_text();
    // Must render a directory index, not entry markdown content.
    assert!(
        body.contains("Index of /"),
        "root index missing listing header\n{}",
        resp.context()
    );
    // At least one fixture root file must appear in the listing.
    assert!(
        body.contains("README.md") || body.contains("guide.md"),
        "root index missing fixture file entries\n{}",
        resp.context()
    );
    // Must NOT serve the raw markdown source as the page.
    assert!(
        !body.contains("# Home"),
        "GET / must not serve raw markdown source\n{}",
        resp.context()
    );
}

#[test]
fn test_serve_local_md_link_resolves() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_local_md_link_resolves", &fixture);

    let resp = fetch(&client(), &server.url("/guide.md"));
    assert_status(&resp, 200);
    assert_header_contains(&resp, "content-type", "text/html");
}

#[test]
fn test_serve_traversal_denied() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_traversal_denied", &fixture);

    let status = raw_http_status(server.port, "/../etc/passwd");
    assert_eq!(status, 404, "expected traversal request to be denied");
}

#[test]
fn test_serve_url_encoded_traversal_denied() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_url_encoded_traversal_denied", &fixture);

    let resp = fetch(&client(), &server.url("/%2e%2e/etc/passwd"));
    assert_status(&resp, 404);
}

#[cfg(unix)]
#[test]
fn test_serve_symlink_escape_denied() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new(FixtureOptions::default());
    let outside = fixture.root.parent().unwrap().join("outside-secret.md");
    fs::write(&outside, "# secret\n").expect("write outside file");
    symlink(&outside, fixture.root.join("escape.md")).expect("create symlink");

    let server = ServerHandle::new("test_serve_symlink_escape_denied", &fixture);
    let resp = fetch(&client(), &server.url("/escape.md"));
    assert_status(&resp, 404);

    let _ = fs::remove_file(outside);
}

#[test]
fn test_serve_extensionless_resolves() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_extensionless_resolves", &fixture);

    let resp = fetch(&client(), &server.url("/guide"));
    assert_status(&resp, 200);
}

#[test]
fn test_serve_directory_readme_resolves() {
    let fixture = Fixture::new(FixtureOptions {
        include_subdir_readme: true,
        include_large_file: false,
        ..Default::default()
    });
    let server = ServerHandle::new("test_serve_directory_readme_resolves", &fixture);

    let resp = fetch(&client(), &server.url("/subdir/"));
    assert_status(&resp, 200);
    assert!(
        resp.body_text().contains("Subdir Readme"),
        "directory README did not resolve\n{}",
        resp.context()
    );
}

#[test]
fn test_serve_directory_index_resolves() {
    let fixture = Fixture::new(FixtureOptions {
        include_subdir_readme: false,
        include_large_file: false,
        ..Default::default()
    });
    let server = ServerHandle::new("test_serve_directory_index_resolves", &fixture);

    let resp = fetch(&client(), &server.url("/subdir/"));
    assert_status(&resp, 200);
    assert!(
        resp.body_text().contains("Subdir Index"),
        "directory index fallback did not resolve\n{}",
        resp.context()
    );
}

#[test]
fn test_serve_static_asset_image() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_static_asset_image", &fixture);

    let resp = fetch(&client(), &server.url("/image.png"));
    assert_status(&resp, 200);
    assert_header_eq(&resp, "content-type", "image/png");
}

#[test]
fn test_serve_nosniff_header() {
    let fixture = Fixture::new(FixtureOptions {
        include_subdir_readme: true,
        include_large_file: true,
        ..Default::default()
    });
    let server = ServerHandle::new("test_serve_nosniff_header", &fixture);

    let ok = fetch(&client(), &server.url("/"));
    assert_status(&ok, 200);
    assert_header_eq(&ok, "x-content-type-options", "nosniff");

    let not_found = fetch(&client(), &server.url("/missing.md"));
    assert_status(&not_found, 404);
    assert_header_eq(&not_found, "x-content-type-options", "nosniff");

    let too_large = fetch(&client(), &server.url("/oversized.md"));
    assert_status(&too_large, 413);
    assert_header_eq(&too_large, "x-content-type-options", "nosniff");
}

#[test]
fn test_serve_etag_present() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_etag_present", &fixture);

    let resp = fetch(&client(), &server.url("/guide"));
    assert_status(&resp, 200);
    let etag = resp
        .header("etag")
        .unwrap_or_else(|| panic!("missing ETag\n{}", resp.context()));
    assert!(
        etag.starts_with('"') && etag.ends_with('"'),
        "invalid ETag '{}'\n{}",
        etag,
        resp.context()
    );
}

#[test]
fn test_serve_304_on_etag_match() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_304_on_etag_match", &fixture);

    let first = fetch(&client(), &server.url("/guide"));
    assert_status(&first, 200);
    let etag = first
        .header("etag")
        .unwrap_or_else(|| panic!("missing ETag\n{}", first.context()));

    let second = fetch_with_headers(
        &client(),
        &server.url("/guide"),
        &[("if-none-match", &etag)],
    );
    assert_status(&second, 304);
    assert!(
        second.body.is_empty(),
        "304 response must have empty body\n{}",
        second.context()
    );
}

#[test]
fn test_serve_200_on_etag_mismatch() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_200_on_etag_mismatch", &fixture);

    let resp = fetch_with_headers(
        &client(),
        &server.url("/guide"),
        &[("if-none-match", "\"definitely-wrong-etag\"")],
    );
    assert_status(&resp, 200);
    assert!(
        !resp.body.is_empty(),
        "ETag mismatch must return full body\n{}",
        resp.context()
    );
}

#[test]
fn test_serve_304_on_modified_since() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_304_on_modified_since", &fixture);

    let future = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(24 * 60 * 60));
    let resp = fetch_with_headers(
        &client(),
        &server.url("/guide"),
        &[("if-modified-since", &future)],
    );
    assert_status(&resp, 304);
    assert!(
        resp.body.is_empty(),
        "304 response must have empty body\n{}",
        resp.context()
    );
}

#[test]
fn test_serve_200_on_modified_since_older() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_200_on_modified_since_older", &fixture);

    let old = "Thu, 01 Jan 1970 00:00:00 GMT";
    let resp = fetch_with_headers(
        &client(),
        &server.url("/guide"),
        &[("if-modified-since", old)],
    );
    assert_status(&resp, 200);
    assert!(
        !resp.body.is_empty(),
        "old If-Modified-Since must return full body\n{}",
        resp.context()
    );
}

#[test]
fn test_serve_compression_gzip() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_compression_gzip", &fixture);

    let resp = fetch_with_headers(
        &client_no_auto_decode(),
        &server.url("/"),
        &[("accept-encoding", "gzip")],
    );
    assert_status(&resp, 200);
    assert_header_eq(&resp, "content-encoding", "gzip");
}

#[test]
fn test_serve_compression_br() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_compression_br", &fixture);

    let resp = fetch_with_headers(
        &client_no_auto_decode(),
        &server.url("/"),
        &[("accept-encoding", "br")],
    );
    assert_status(&resp, 200);
    assert_header_eq(&resp, "content-encoding", "br");
}

#[test]
fn test_serve_file_too_large() {
    let fixture = Fixture::new(FixtureOptions {
        include_subdir_readme: true,
        include_large_file: true,
        ..Default::default()
    });
    let server = ServerHandle::new("test_serve_file_too_large", &fixture);

    let resp = fetch(&client(), &server.url("/oversized.md"));
    assert_status(&resp, 413);
}

#[test]
fn test_serve_script_stripped() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_script_stripped", &fixture);

    let resp = fetch(&client(), &server.url("/"));
    assert_status(&resp, 200);
    let body = resp.body_text();
    assert!(
        !body.contains("<script>alert(1)</script>") && !body.contains("alert(1)"),
        "input script tag leaked into rendered body\n{}",
        resp.context()
    );
}

#[test]
fn test_serve_startup_stdout_format() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_startup_stdout_format", &fixture);
    let port = server.port;

    let _ = fetch(&client(), &server.url("/"));

    let output = server.shutdown_with_sigint();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stdout.lines().collect();

    // Must have at least one line (local URL) and at most two (local + tailscale).
    assert!(
        !lines.is_empty() && lines.len() <= 2,
        "expected 1–2 startup lines, got {}\nstdout:\n{stdout}",
        lines.len()
    );

    // Line 0: bare local URL — matches http://127.0.0.1:{port}/...
    assert!(
        lines[0].starts_with(&format!("http://127.0.0.1:{port}/")),
        "first startup line must be bare local URL http://127.0.0.1:{port}/...\nstdout:\n{stdout}"
    );

    // If a second line exists it must be a bare tailscale URL — no label prefix.
    if let Some(second) = lines.get(1) {
        assert!(
            second.starts_with("http://") && !second.starts_with("url:"),
            "second startup line must be bare http:// URL (no 'url:' prefix)\ngot: {second:?}\nstdout:\n{stdout}"
        );
    }

    // No line may carry any of the old label prefixes.
    let forbidden = ["mdmd serve", "root:", "entry:", "url:", "index:", "backlinks:"];
    for line in &lines {
        for prefix in forbidden {
            assert!(
                !line.starts_with(prefix),
                "startup stdout must not contain label prefix {prefix:?}\noffending line: {line:?}\nstdout:\n{stdout}"
            );
        }
    }

    // Default mode (no --verbose): tailscale diagnostics must not appear on stderr.
    assert!(
        !stderr.contains("[tailscale]"),
        "stderr must not contain [tailscale] diagnostics in default mode\nstderr:\n{stderr}"
    );
}

#[test]
fn test_serve_assets_css() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_assets_css", &fixture);

    let resp = fetch(&client(), &server.url("/assets/mdmd.css"));
    assert_status(&resp, 200);
    assert_header_contains(&resp, "content-type", "text/css");
}

#[test]
fn test_serve_assets_js() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_assets_js", &fixture);

    let resp = fetch(&client(), &server.url("/assets/mdmd.js"));
    assert_status(&resp, 200);
    assert_header_contains(&resp, "content-type", "text/javascript");
}

#[test]
fn test_legacy_cli_tui_path() {
    eprintln!("[TEST] scenario=test_legacy_cli_tui_path port=0");

    let fixture = Fixture::new(FixtureOptions::default());
    let mut child = Command::new(bin_path())
        .arg(&fixture.entry)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn legacy cli process");

    wait_with_timeout(&mut child, Duration::from_millis(800));
    if child.try_wait().expect("try_wait legacy child").is_none() {
        let _ = child.kill();
    }

    let output = child.wait_with_output().expect("collect legacy output");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("[serve]"),
        "legacy path unexpectedly dispatched serve\nstderr:\n{}",
        stderr
    );
}

#[cfg(unix)]
#[test]
fn test_serve_graceful_shutdown() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_graceful_shutdown", &fixture);

    let output = server.shutdown_with_sigint();
    assert!(
        output.status.success(),
        "server should exit cleanly on SIGINT\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// E2E test for directory index rendering policies:
/// - Root index (`GET /`) lists directory contents.
/// - Bare directory (`GET /bare-dir/`) with no README.md/index.md renders index.
/// - Dotfiles are excluded from listings.
/// - Directories appear before files (dirs-first alphabetical sort).
/// - Breadcrumb root link is present.
///
/// Run with: RUST_LOG=debug cargo test --test serve_integration test_serve_directory_index_policies -- --nocapture
#[test]
fn test_serve_directory_index_policies() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let root = tmp.path().to_path_buf();

    // Entry file required for server startup.
    let entry = root.join("README.md");
    fs::write(&entry, "# Root\n").expect("write root README");

    // A bare directory (no README.md / index.md) → should trigger directory listing.
    let bare_dir = root.join("bare-dir");
    fs::create_dir_all(&bare_dir).expect("create bare-dir");

    // Files: mixed-order to validate alphabetical sort within files group.
    fs::write(bare_dir.join("zzz-file.txt"), "zzz").expect("write zzz-file.txt");
    fs::write(bare_dir.join("aaa-file.md"), "# aaa").expect("write aaa-file.md");

    // Subdirectory: must appear before files in listing.
    fs::create_dir_all(bare_dir.join("bbb-subdir")).expect("create bbb-subdir");

    // Dotfile: must be excluded from listing.
    fs::write(bare_dir.join(".hidden-file"), "hidden").expect("write .hidden-file");

    let fixture = Fixture {
        _tmp: tmp,
        root,
        entry,
    };
    let server = ServerHandle::new("test_serve_directory_index_policies", &fixture);

    // --- Root index: GET / always renders directory listing even when README.md exists ---
    let root_resp = fetch(&client(), &server.url("/"));
    assert_status(&root_resp, 200);
    assert_header_contains(&root_resp, "content-type", "text/html");
    let root_body = root_resp.body_text();
    assert!(
        root_body.contains("Index of /"),
        "root index header missing\n{}",
        root_resp.context()
    );
    assert!(
        root_body.contains("README.md") || root_body.contains("bare-dir"),
        "root index missing fixture entries\n{}",
        root_resp.context()
    );
    // Root breadcrumb link must be present.
    assert!(
        root_body.contains("href=\"/\""),
        "root breadcrumb link missing\n{}",
        root_resp.context()
    );
    // README.md raw markdown source must NOT be served for GET /.
    assert!(
        !root_body.contains("# Root"),
        "GET / must not serve raw markdown\n{}",
        root_resp.context()
    );

    // --- Bare directory: GET /bare-dir/ renders directory listing ---
    let dir_resp = fetch(&client(), &server.url("/bare-dir/"));
    assert_status(&dir_resp, 200);
    assert_header_contains(&dir_resp, "content-type", "text/html");
    let dir_body = dir_resp.body_text();

    // Must show directory index header with the directory path.
    assert!(
        dir_body.contains("Index of /bare-dir"),
        "directory index header missing\n{}",
        dir_resp.context()
    );

    // Visible files and dirs must be present.
    assert!(
        dir_body.contains("bbb-subdir"),
        "bbb-subdir missing from listing\n{}",
        dir_resp.context()
    );
    assert!(
        dir_body.contains("aaa-file.md"),
        "aaa-file.md missing from listing\n{}",
        dir_resp.context()
    );
    assert!(
        dir_body.contains("zzz-file.txt"),
        "zzz-file.txt missing from listing\n{}",
        dir_resp.context()
    );

    // Dotfile must be excluded.
    assert!(
        !dir_body.contains(".hidden-file"),
        "dotfile must be excluded from listing\n{}",
        dir_resp.context()
    );

    // Directories must appear before files (dirs-first policy).
    let pos_subdir = dir_body
        .find("bbb-subdir")
        .expect("bbb-subdir not found in body");
    let pos_file = dir_body
        .find("aaa-file.md")
        .expect("aaa-file.md not found in body");
    assert!(
        pos_subdir < pos_file,
        "directories must appear before files (bbb-subdir should precede aaa-file.md)\n{}",
        dir_resp.context()
    );

    // Files must be alphabetically sorted within the files group.
    let pos_aaa = dir_body
        .find("aaa-file.md")
        .expect("aaa-file.md not found in body");
    let pos_zzz = dir_body
        .find("zzz-file.txt")
        .expect("zzz-file.txt not found in body");
    assert!(
        pos_aaa < pos_zzz,
        "files must be alphabetically sorted (aaa-file.md before zzz-file.txt)\n{}",
        dir_resp.context()
    );

    // Breadcrumb root link must be present.
    assert!(
        dir_body.contains("href=\"/\""),
        "breadcrumb root link missing\n{}",
        dir_resp.context()
    );
}

/// Verify that an in-root symlink is included while an out-of-root symlink is
/// excluded from directory listings.
#[cfg(unix)]
#[test]
fn test_serve_directory_index_symlink_policy() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().expect("create tempdir");
    let root = tmp.path().to_path_buf();

    let entry = root.join("README.md");
    fs::write(&entry, "# Root\n").expect("write root README");

    let sym_dir = root.join("sym-test");
    fs::create_dir_all(&sym_dir).expect("create sym-test");

    // Safe target: inside the serve root.
    let safe_target = root.join("safe-target.txt");
    fs::write(&safe_target, "safe").expect("write safe target");
    symlink(&safe_target, sym_dir.join("safe-link.txt")).expect("create in-root symlink");

    // Dangerous target: outside the serve root.
    let outside = std::env::temp_dir()
        .join(format!("mdmd_outside_symtest_{}.txt", std::process::id()));
    fs::write(&outside, "secret").expect("write outside file");
    symlink(&outside, sym_dir.join("escape-link.txt")).expect("create out-of-root symlink");

    let fixture = Fixture {
        _tmp: tmp,
        root,
        entry,
    };
    let server = ServerHandle::new("test_serve_directory_index_symlink_policy", &fixture);

    let resp = fetch(&client(), &server.url("/sym-test/"));
    assert_status(&resp, 200);
    let body = resp.body_text();

    // In-root symlink should be included.
    assert!(
        body.contains("safe-link.txt"),
        "in-root symlink must be included in listing\n{}",
        resp.context()
    );

    // Out-of-root symlink must be excluded.
    assert!(
        !body.contains("escape-link.txt"),
        "out-of-root symlink must be excluded from listing\n{}",
        resp.context()
    );

    let _ = fs::remove_file(&outside);
}

/// E2E test for the rich HTML 404 page (bd-3u5):
/// - Missing path returns 404 with `content-type: text/html`.
/// - Body contains the requested path.
/// - Body contains recovery links: root index, entry document, nearest parent.
/// - Body contains a directory listing for the nearest existing parent.
/// - Security denials (traversal, encoded traversal) continue to return 404.
#[test]
fn test_serve_rich_404_recovery() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let root = tmp.path().to_path_buf();

    let entry = root.join("README.md");
    fs::write(&entry, "# Root\n").expect("write root README");

    // Create a subdirectory with some files.
    let docs = root.join("docs");
    fs::create_dir_all(&docs).expect("create docs dir");
    fs::write(docs.join("intro.md"), "# Intro\n").expect("write intro.md");
    fs::write(docs.join("guide.md"), "# Guide\n").expect("write guide.md");

    let fixture = Fixture {
        _tmp: tmp,
        root,
        entry,
    };
    let server = ServerHandle::new("test_serve_rich_404_recovery", &fixture);

    // --- Missing leaf: /docs/nonexistent.md → parent is /docs/ ---
    let resp = fetch(&client(), &server.url("/docs/nonexistent.md"));
    assert_status(&resp, 404);
    assert_header_contains(&resp, "content-type", "text/html");
    assert_header_eq(&resp, "x-content-type-options", "nosniff");

    let body = resp.body_text();

    // The requested path must appear in the page body.
    assert!(
        body.contains("docs/nonexistent.md"),
        "requested path missing from 404 body\n{}",
        resp.context()
    );

    // Root index recovery link must be present.
    assert!(
        body.contains("href=\"/\""),
        "root index recovery link missing\n{}",
        resp.context()
    );

    // Entry document recovery link must be present.
    assert!(
        body.contains("href=\"/README.md\""),
        "entry document link missing\n{}",
        resp.context()
    );

    // Nearest-parent recovery link must point at /docs/.
    assert!(
        body.contains("href=\"/docs/\""),
        "nearest parent link missing\n{}",
        resp.context()
    );

    // Nearest-parent directory listing must include sibling files.
    assert!(
        body.contains("intro.md"),
        "nearest parent listing missing intro.md\n{}",
        resp.context()
    );
    assert!(
        body.contains("guide.md"),
        "nearest parent listing missing guide.md\n{}",
        resp.context()
    );

    // --- Multi-level miss: /docs/a/b/missing.md → nearest parent is /docs/ ---
    let resp2 = fetch(&client(), &server.url("/docs/a/b/missing.md"));
    assert_status(&resp2, 404);
    assert_header_contains(&resp2, "content-type", "text/html");
    let body2 = resp2.body_text();
    assert!(
        body2.contains("href=\"/docs/\"") || body2.contains("href=\"/\""),
        "multi-level miss recovery link missing\n{}",
        resp2.context()
    );

    // --- Entirely missing path: /gone/missing.md → nearest parent is root / ---
    let resp3 = fetch(&client(), &server.url("/gone/missing.md"));
    assert_status(&resp3, 404);
    assert_header_contains(&resp3, "content-type", "text/html");
    let body3 = resp3.body_text();
    assert!(
        body3.contains("href=\"/\""),
        "root fallback recovery link missing\n{}",
        resp3.context()
    );
}

// ---------------------------------------------------------------------------
// bd-t6w: resolve_candidate fallback order + rewrite_local_links integration
// ---------------------------------------------------------------------------

/// End-to-end validation of the resolve_candidate fallback order via HTTP.
///
/// Pins the four branches (exact, extensionless, readme, index) so a
/// regression in serve.rs fallback logic is detected immediately.
#[test]
fn test_serve_resolve_fallback_order_http() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let root = tmp.path().to_path_buf();

    let entry = root.join("README.md");
    fs::write(&entry, "# Root\n").expect("write root README");

    // Branch 1 – exact: /page.md exists.
    fs::write(root.join("page.md"), "# Page\n").expect("write page.md");

    // Branch 2 – extensionless: /noext → /noext.md.
    fs::write(root.join("noext.md"), "# NoExt\n").expect("write noext.md");

    // Branch 3 – readme: /has-readme/ → README.md.
    let has_readme = root.join("has-readme");
    fs::create_dir_all(&has_readme).expect("create has-readme dir");
    fs::write(has_readme.join("README.md"), "# DirReadme\n").expect("write dir README");

    // Branch 4 – index: /has-index/ → index.md (no README.md present).
    let has_index = root.join("has-index");
    fs::create_dir_all(&has_index).expect("create has-index dir");
    fs::write(has_index.join("index.md"), "# DirIndex\n").expect("write dir index");

    let fixture = Fixture {
        _tmp: tmp,
        root,
        entry,
    };
    let server = ServerHandle::new("test_serve_resolve_fallback_order_http", &fixture);
    let c = client();

    // 1. Exact path.
    let resp = fetch(&c, &server.url("/page.md"));
    assert_status(&resp, 200);
    assert!(
        resp.body_text().contains("Page"),
        "exact branch: expected 'Page' in body\n{}",
        resp.context()
    );

    // 2. Extensionless → .md appended.
    let resp = fetch(&c, &server.url("/noext"));
    assert_status(&resp, 200);
    assert!(
        resp.body_text().contains("NoExt"),
        "extensionless branch: expected 'NoExt' in body\n{}",
        resp.context()
    );

    // 3. Directory → README.md.
    let resp = fetch(&c, &server.url("/has-readme/"));
    assert_status(&resp, 200);
    assert!(
        resp.body_text().contains("DirReadme"),
        "readme branch: expected 'DirReadme' in body\n{}",
        resp.context()
    );

    // 4. Directory → index.md (README.md absent).
    let resp = fetch(&c, &server.url("/has-index/"));
    assert_status(&resp, 200);
    assert!(
        resp.body_text().contains("DirIndex"),
        "index branch: expected 'DirIndex' in body\n{}",
        resp.context()
    );
}

/// Validates rewrite_local_links() behavior when the entry file is nested under
/// a subdirectory (CWD-root scenario introduced by bd-2uj).
///
/// serve_root = CWD = <fixture root>
/// entry_file = <fixture root>/subdir/README.md
///
/// Links in subdir/README.md must rewrite to root-relative hrefs that include
/// the "subdir/" prefix.  If serve_root were still set to entry_file.parent()
/// these assertions would fail, making this a direct regression guard for the
/// CWD-root change.
#[test]
fn test_serve_nested_entry_rewritten_links_root_relative() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let root = tmp.path().to_path_buf();

    let subdir = root.join("subdir");
    fs::create_dir_all(&subdir).expect("create subdir");

    // Entry file with three kinds of relative links.
    fs::write(
        subdir.join("README.md"),
        "# Nested\n\n\
         [sibling](sibling.md)\n\n\
         [parent file](../other.md)\n\n\
         [extensionless](sibling)\n",
    )
    .expect("write nested README");

    fs::write(subdir.join("sibling.md"), "# Sibling\n").expect("write sibling.md");
    fs::write(root.join("other.md"), "# Other\n").expect("write other.md");

    let fixture = Fixture {
        _tmp: tmp,
        root,
        entry: subdir.join("README.md"),
    };
    let server =
        ServerHandle::new("test_serve_nested_entry_rewritten_links_root_relative", &fixture);

    let resp = fetch(&client(), &server.url("/subdir/README.md"));
    assert_status(&resp, 200);
    let body = resp.body_text();

    // Sibling link must be root-relative with the subdir prefix.
    assert!(
        body.contains("href=\"/subdir/sibling.md\""),
        "sibling link must be /subdir/sibling.md\n{}",
        resp.context()
    );

    // Parent-level link (../other.md) must resolve to /other.md.
    assert!(
        body.contains("href=\"/other.md\""),
        "parent link must be /other.md\n{}",
        resp.context()
    );

    // Extensionless link must include the subdir prefix.
    assert!(
        body.contains("href=\"/subdir/sibling\""),
        "extensionless link must be /subdir/sibling\n{}",
        resp.context()
    );
}

/// Validates that rewritten link targets in a nested-entry scenario are all
/// reachable (HTTP 200) via the same server instance.
#[test]
fn test_serve_nested_entry_rewritten_link_targets_reachable() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let root = tmp.path().to_path_buf();

    let subdir = root.join("subdir");
    fs::create_dir_all(&subdir).expect("create subdir");

    fs::write(
        subdir.join("README.md"),
        "# Nested\n\n\
         [sibling](sibling.md)\n\n\
         [parent file](../other.md)\n\n\
         [extensionless](sibling)\n",
    )
    .expect("write nested README");

    fs::write(subdir.join("sibling.md"), "# Sibling\n").expect("write sibling.md");
    fs::write(root.join("other.md"), "# Other\n").expect("write other.md");

    let fixture = Fixture {
        _tmp: tmp,
        root,
        entry: subdir.join("README.md"),
    };
    let server = ServerHandle::new(
        "test_serve_nested_entry_rewritten_link_targets_reachable",
        &fixture,
    );
    let c = client();

    // /subdir/sibling.md — exact path resolve.
    let resp = fetch(&c, &server.url("/subdir/sibling.md"));
    assert_status(&resp, 200);

    // /other.md — root-level file reachable from nested entry.
    let resp = fetch(&c, &server.url("/other.md"));
    assert_status(&resp, 200);

    // /subdir/sibling — extensionless resolve (adds .md).
    let resp = fetch(&c, &server.url("/subdir/sibling"));
    assert_status(&resp, 200);
}

// ---------------------------------------------------------------------------
// bd-3h2: full navigation flow — root-index-flow
// ---------------------------------------------------------------------------

/// End-to-end navigation flow starting at the root directory index.
///
/// Validates that:
/// 1. `GET /` returns a directory listing (not entry markdown source).
/// 2. The listing contains a navigable href to the entry document.
/// 3. Following that href returns rendered markdown HTML (200, text/html).
/// 4. The entry HTML contains a rendered heading, confirming it is not raw source.
///
/// Run with: RUST_LOG=debug cargo test --test serve_integration test_serve_root_index_links_to_entry -- --nocapture
#[test]
fn test_serve_root_index_links_to_entry() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_root_index_links_to_entry", &fixture);
    let c = client();

    // Step 1: Root index must be a directory listing.
    let index_resp = fetch(&c, &server.url("/"));
    assert_status(&index_resp, 200);
    assert_header_contains(&index_resp, "content-type", "text/html");
    let index_body = index_resp.body_text();
    assert!(
        index_body.contains("Index of /"),
        "GET / must return directory index\n{}",
        index_resp.context()
    );

    // Step 2: Listing must contain a navigable href to the entry document.
    // The fixture entry is root/README.md, so the listing must include href="/README.md".
    assert!(
        index_body.contains("href=\"/README.md\""),
        "root index must contain href to entry document /README.md\n{}",
        index_resp.context()
    );

    // Step 3: Following the entry link must return rendered markdown HTML.
    let entry_resp = fetch(&c, &server.url("/README.md"));
    assert_status(&entry_resp, 200);
    assert_header_contains(&entry_resp, "content-type", "text/html");

    // Step 4: Rendered HTML must contain a heading, confirming markdown was processed.
    let entry_body = entry_resp.body_text();
    assert!(
        entry_body.contains("<h1"),
        "entry document must render as HTML with an h1 heading\n{}",
        entry_resp.context()
    );
    // Raw markdown source must not be served.
    assert!(
        !entry_body.contains("# Home"),
        "entry response must not contain raw markdown source\n{}",
        entry_resp.context()
    );
}

/// Verifies that `FixtureOptions::include_dotfiles` creates a dotfile and that
/// it is excluded from directory listings, and that `include_nested_dirs`
/// creates a navigable nested directory structure.
#[test]
fn test_fixture_options_dotfiles_and_nested_dirs() {
    let fixture = Fixture::new(FixtureOptions {
        include_subdir_readme: false,
        include_large_file: false,
        include_dotfiles: true,
        include_nested_dirs: true,
    });
    let server = ServerHandle::new("test_fixture_options_dotfiles_and_nested_dirs", &fixture);
    let c = client();

    // Root index must NOT list the dotfile.
    let root_resp = fetch(&c, &server.url("/"));
    assert_status(&root_resp, 200);
    let root_body = root_resp.body_text();
    assert!(
        !root_body.contains(".hidden-dotfile"),
        "dotfile must be excluded from root listing\n{}",
        root_resp.context()
    );

    // Nested directory must be listed and navigable.
    assert!(
        root_body.contains("nested"),
        "nested/ dir must appear in root listing\n{}",
        root_resp.context()
    );

    // GET /nested/ must list nested-doc.md and inner/.
    let nested_resp = fetch(&c, &server.url("/nested/"));
    assert_status(&nested_resp, 200);
    let nested_body = nested_resp.body_text();
    assert!(
        nested_body.contains("nested-doc.md"),
        "nested-doc.md must appear in /nested/ listing\n{}",
        nested_resp.context()
    );
    assert!(
        nested_body.contains("inner"),
        "inner/ subdir must appear in /nested/ listing\n{}",
        nested_resp.context()
    );

    // Nested markdown doc must be directly reachable and render as HTML.
    let doc_resp = fetch(&c, &server.url("/nested/nested-doc.md"));
    assert_status(&doc_resp, 200);
    assert_header_contains(&doc_resp, "content-type", "text/html");
}

// ---------------------------------------------------------------------------
// bd-2ag: cross-directory link resolution — broad root allows, narrow root blocks
// ---------------------------------------------------------------------------

/// Validates that cross-directory links resolve when both the source and target
/// reside within the selected serve_root (broad root scenario).
///
/// Fixture layout:
///   parent/               ← serve_root (CWD = parent/)
///     docs/
///       a.md              ← entry; links to ../other/b.md
///     other/
///       b.md              ← cross-dir target (inside broad root)
///
/// Expected:
/// - GET /other/b.md → 200 (target is inside broad root)
/// - GET /docs/a.md → 200; body contains href="/other/b.md" (link rewritten)
#[test]
fn test_serve_cross_dir_link_broad_root_allows() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let parent = tmp.path().to_path_buf();

    let docs = parent.join("docs");
    let other = parent.join("other");
    fs::create_dir_all(&docs).expect("create docs dir");
    fs::create_dir_all(&other).expect("create other dir");

    fs::write(
        docs.join("a.md"),
        "# Doc A\n\nSee [Doc B](../other/b.md).\n",
    )
    .expect("write docs/a.md");
    fs::write(other.join("b.md"), "# Doc B\n\nContent of B.\n").expect("write other/b.md");

    // Broad root: CWD = parent (entry docs/a.md is inside parent → serve_root = parent)
    let fixture = Fixture {
        _tmp: tmp,
        root: parent.clone(),
        entry: docs.join("a.md"),
    };
    let server = ServerHandle::new("test_serve_cross_dir_link_broad_root_allows", &fixture);
    let c = client();

    // Cross-dir target /other/b.md is inside broad root → must be accessible.
    let resp = fetch(&c, &server.url("/other/b.md"));
    assert_status(&resp, 200);
    assert!(
        resp.body_text().contains("Doc B"),
        "cross-dir target /other/b.md must be accessible with broad root\n{}",
        resp.context()
    );

    // Entry /docs/a.md must be accessible and its link rewritten to /other/b.md.
    let resp = fetch(&c, &server.url("/docs/a.md"));
    assert_status(&resp, 200);
    let body = resp.body_text();
    assert!(
        body.contains("href=\"/other/b.md\""),
        "cross-dir link must be rewritten to root-relative /other/b.md with broad root\n{}",
        resp.context()
    );
}

/// Validates that cross-directory links are blocked when the target resides
/// outside the selected serve_root (narrow root scenario).
///
/// Fixture layout:
///   parent/
///     docs/               ← serve_root (CWD = parent/docs/)
///       a.md              ← entry; links to ../other/b.md (NOT rewritten: escapes root)
///     other/
///       b.md              ← cross-dir target (OUTSIDE narrow root)
///
/// Expected:
/// - GET /other/b.md → 404 (resolves to docs/other/b.md which does not exist)
/// - GET /a.md → 200; body does NOT contain href="/other/b.md" (link stays unrewritten)
#[test]
fn test_serve_cross_dir_link_narrow_root_blocks() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let parent = tmp.path().to_path_buf();

    let docs = parent.join("docs");
    let other = parent.join("other");
    fs::create_dir_all(&docs).expect("create docs dir");
    fs::create_dir_all(&other).expect("create other dir");

    fs::write(
        docs.join("a.md"),
        "# Doc A\n\nSee [Doc B](../other/b.md).\n",
    )
    .expect("write docs/a.md");
    fs::write(other.join("b.md"), "# Doc B\n\nContent of B.\n").expect("write other/b.md");

    // Narrow root: CWD = docs (entry docs/a.md is inside docs/ → serve_root = docs/)
    let fixture = Fixture {
        _tmp: tmp,
        root: docs.clone(),
        entry: docs.join("a.md"),
    };
    let server = ServerHandle::new("test_serve_cross_dir_link_narrow_root_blocks", &fixture);
    let c = client();

    // Cross-dir target is outside narrow root: /other/b.md resolves to docs/other/b.md
    // which does not exist → must return 404.
    let resp = fetch(&c, &server.url("/other/b.md"));
    assert_status(&resp, 404);

    // Entry /a.md is inside narrow root → accessible.
    let resp = fetch(&c, &server.url("/a.md"));
    assert_status(&resp, 200);
    let body = resp.body_text();

    // Link must NOT be rewritten to /other/b.md (target escapes narrow serve_root).
    // It stays as the original relative ../other/b.md in the rendered output.
    assert!(
        !body.contains("href=\"/other/b.md\""),
        "cross-dir link must NOT be rewritten for narrow root (target escapes root)\n{}",
        resp.context()
    );
    assert!(
        body.contains("href=\"../other/b.md\""),
        "unrewritten cross-dir link must remain as ../other/b.md in narrow root\n{}",
        resp.context()
    );
}

/// Validates that a symlink inside the serve_root pointing to a file outside
/// the root is denied with a 404, and that the server logs the denial with
/// `branch=denied reason=outside-root`.
///
/// This exercises the containment check at serve.rs step 5 (R1) and verifies
/// the log line emitted there for the blocked path.
#[cfg(unix)]
#[test]
fn test_serve_symlink_outside_root_denied_with_outside_root_log() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().expect("create tempdir");
    let parent = tmp.path().to_path_buf();

    let docs = parent.join("docs");
    let other = parent.join("other");
    fs::create_dir_all(&docs).expect("create docs dir");
    fs::create_dir_all(&other).expect("create other dir");

    let entry = docs.join("a.md");
    fs::write(&entry, "# Doc A\n").expect("write docs/a.md");

    let outside_target = other.join("b.md");
    fs::write(&outside_target, "# Doc B\n").expect("write other/b.md");

    // Create a symlink inside the narrow root (docs/) that points to outside (other/b.md).
    let symlink_path = docs.join("escape.md");
    symlink(&outside_target, &symlink_path).expect("create symlink");

    // Narrow root: CWD = docs/
    let fixture = Fixture {
        _tmp: tmp,
        root: docs.clone(),
        entry,
    };
    // Use --verbose so that [resolve] diagnostic lines are emitted.
    let server = ServerHandle::new_verbose(
        "test_serve_symlink_outside_root_denied_with_outside_root_log",
        &fixture,
    );
    let c = client();

    // Symlink target is outside narrow root → must be denied.
    let resp = fetch(&c, &server.url("/escape.md"));
    assert_status(&resp, 404);

    // Shut down and collect server output to verify the denial log.
    let output = server.shutdown_with_sigint();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("branch=denied") && stderr.contains("reason=outside-root"),
        "symlink escape must log branch=denied reason=outside-root\nstderr:\n{}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// bd-26u: backlinks startup stdout/stderr
// ---------------------------------------------------------------------------

/// Verifies that the backlinks startup hint appears in stdout and the scan
/// count line appears in stderr after server startup.
#[test]
fn test_backlinks_startup_stdout() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_backlinks_startup_stdout", &fixture);

    // Trigger at least one request so the server is fully warmed up.
    let _ = fetch(&client(), &server.url("/"));

    let output = server.shutdown_with_sigint();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stdout.contains("backlinks: startup-indexed"),
        "restart reminder missing from stdout\nstdout:\n{stdout}"
    );
    assert!(
        stderr.contains("[backlinks] indexed files="),
        "scan count missing from stderr\nstderr:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// bd-26u.1: backlinks panel rendering, empty state, fragment visibility
// ---------------------------------------------------------------------------

/// Build a fixture with cross-linked pages for backlinks E2E testing.
fn make_backlinks_fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("create backlinks tempdir");
    let root = tmp.path().to_path_buf();

    // a.md links to b.md with a fragment — creates one backlink for b.md.
    fs::write(
        root.join("a.md"),
        "# Page A\n\nSee also [Page B](./b.md#section-1).\n",
    )
    .expect("write a.md");

    // b.md is the target of a.md; has its own section heading.
    fs::write(
        root.join("b.md"),
        "# Page B\n\n## Section 1\n\nContent here.\n",
    )
    .expect("write b.md");

    // empty.md has no inbound links.
    fs::write(root.join("empty.md"), "# Empty\n\nNo links here.\n").expect("write empty.md");

    // c.md and d.md both link to target.md — creates two backlinks for target.md.
    fs::write(root.join("c.md"), "# Page C\n\n[Target](./target.md)\n").expect("write c.md");
    fs::write(root.join("d.md"), "# Page D\n\n[Target](./target.md)\n").expect("write d.md");

    // target.md is referenced by both c.md and d.md.
    fs::write(root.join("target.md"), "# Target\n\nTarget content.\n").expect("write target.md");

    Fixture {
        entry: root.join("a.md"),
        _tmp: tmp,
        root,
    }
}

/// b.md has one inbound backlink (from a.md) — verifies populated panel rendering.
#[test]
fn test_backlinks_panel_populated() {
    let fixture = make_backlinks_fixture();
    let server = ServerHandle::new("test_backlinks_panel_populated", &fixture);
    let c = client();

    let resp = fetch(&c, &server.url("/b.md"));
    assert_status(&resp, 200);
    let body = resp.body_text();

    assert!(
        body.contains("Backlinks (1)"),
        "backlinks count header missing\n{}",
        resp.context()
    );
    assert!(
        body.contains("href=\"/a.md\"") || body.contains("href=\"/a.md#"),
        "source link to a.md missing\n{}",
        resp.context()
    );
    assert!(
        body.contains("Page A"),
        "source display title 'Page A' missing\n{}",
        resp.context()
    );
    assert!(
        body.contains("section-1"),
        "fragment hint for section-1 missing\n{}",
        resp.context()
    );
    assert!(
        body.contains("backlinks-panel"),
        "backlinks-panel section class missing\n{}",
        resp.context()
    );
}

/// empty.md has no inbound backlinks — verifies empty state rendering.
#[test]
fn test_backlinks_panel_empty() {
    let fixture = make_backlinks_fixture();
    let server = ServerHandle::new("test_backlinks_panel_empty", &fixture);
    let c = client();

    let resp = fetch(&c, &server.url("/empty.md"));
    assert_status(&resp, 200);
    let body = resp.body_text();

    assert!(
        !body.contains("No backlinks yet."),
        "empty state must not show 'No backlinks yet.'\n{}",
        resp.context()
    );
    assert!(
        !body.contains("backlinks-empty"),
        "empty state must not render aside\n{}",
        resp.context()
    );
    assert!(
        !body.contains("Backlinks ("),
        "populated header shown for empty state\n{}",
        resp.context()
    );
}

/// target.md has two inbound backlinks (c.md, d.md) — verifies count accuracy.
#[test]
fn test_backlinks_count_accuracy() {
    let fixture = make_backlinks_fixture();
    let server = ServerHandle::new("test_backlinks_count_accuracy", &fixture);
    let c = client();

    let resp = fetch(&c, &server.url("/target.md"));
    assert_status(&resp, 200);
    let body = resp.body_text();

    assert!(
        body.contains("Backlinks (2)"),
        "backlinks count must be 2\n{}",
        resp.context()
    );
    assert!(
        body.contains("Page C"),
        "source display 'Page C' missing\n{}",
        resp.context()
    );
    assert!(
        body.contains("Page D"),
        "source display 'Page D' missing\n{}",
        resp.context()
    );
}

/// Every rendered markdown page must include the change-notice div with hidden attribute.
#[test]
fn test_change_notice_div_present_and_hidden() {
    let fixture = make_backlinks_fixture();
    let server = ServerHandle::new("test_change_notice_div_present_and_hidden", &fixture);
    let c = client();

    let resp = fetch(&c, &server.url("/b.md"));
    assert_status(&resp, 200);
    let body = resp.body_text();

    let notice_pos = body.find("id=\"mdmd-change-notice\"");
    assert!(
        notice_pos.is_some(),
        "id=\"mdmd-change-notice\" missing from page\n{}",
        resp.context()
    );
    let context_slice = &body[notice_pos.unwrap()..notice_pos.unwrap() + 100];
    assert!(
        context_slice.contains("hidden"),
        "change-notice div must carry the 'hidden' attribute\ncontext: {context_slice}"
    );
}

/// /assets/mdmd.css must contain scroll-margin-top for heading anchor navigation.
#[test]
fn test_scroll_margin_top_in_css() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_scroll_margin_top_in_css", &fixture);
    let c = client();

    let resp = fetch(&c, &server.url("/assets/mdmd.css"));
    assert_status(&resp, 200);
    assert!(
        resp.body_text().contains("scroll-margin-top"),
        "heading anchor scroll-margin missing from CSS\n{}",
        resp.context()
    );
}

/// No anchor element in a rendered page should have 'back to' as visible link text.
#[test]
fn test_no_back_to_links() {
    let fixture = make_backlinks_fixture();
    let server = ServerHandle::new("test_no_back_to_links", &fixture);
    let c = client();

    let resp = fetch(&c, &server.url("/b.md"));
    assert_status(&resp, 200);
    let body_lower = resp.body_text().to_lowercase();

    // Check that ">back to" does not appear — this pattern catches link text
    // like <a ...>back to ...</a> while ignoring code comments.
    assert!(
        !body_lower.contains(">back to"),
        "found 'back to' as anchor link text\n{}",
        resp.context()
    );
}

// ---------------------------------------------------------------------------
// bd-26u.2: file-change notice reload flow
// ---------------------------------------------------------------------------

/// Build a minimal fixture with a single fixture.md entry for freshness tests.
fn make_freshness_fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("create freshness tempdir");
    let root = tmp.path().to_path_buf();
    let entry = root.join("fixture.md");
    fs::write(&entry, "# Test\n\nContent.\n").expect("write fixture.md");

    Fixture {
        entry: entry.clone(),
        _tmp: tmp,
        root,
    }
}

/// GET /_mdmd/freshness?path=fixture.md must return 200 with a positive mtime.
#[test]
fn test_freshness_endpoint_returns_mtime() {
    let fixture = make_freshness_fixture();
    let server = ServerHandle::new("test_freshness_endpoint_returns_mtime", &fixture);
    let c = client();

    let resp = fetch(&c, &server.url("/_mdmd/freshness?path=fixture.md"));
    assert_status(&resp, 200);

    let body = resp.body_text();
    let json: serde_json::Value =
        serde_json::from_str(&body).expect("freshness response must be valid JSON");
    let mtime = json["mtime"].as_u64().expect("mtime must be a u64");
    assert!(mtime > 0, "mtime must be positive, got {mtime}");
}

/// After modifying fixture.md the freshness endpoint must return a newer mtime.
#[test]
fn test_freshness_detects_file_change() {
    let fixture = make_freshness_fixture();
    let server = ServerHandle::new("test_freshness_detects_file_change", &fixture);
    let c = client();

    // Snapshot the initial mtime.
    let resp1 = fetch(&c, &server.url("/_mdmd/freshness?path=fixture.md"));
    assert_status(&resp1, 200);
    let mtime1 = serde_json::from_str::<serde_json::Value>(&resp1.body_text())
        .expect("initial freshness JSON")["mtime"]
        .as_u64()
        .expect("initial mtime u64");

    // Wait at least one second so the filesystem mtime advances.
    thread::sleep(Duration::from_secs(1));

    // Mutate the file.
    fs::write(
        fixture.root.join("fixture.md"),
        "# Test\n\nUpdated content.\n",
    )
    .expect("mutate fixture.md");

    // Poll until the mtime changes (up to STARTUP_TIMEOUT) to absorb any
    // filesystem granularity delay.
    let start = std::time::Instant::now();
    let mtime2 = loop {
        let resp = fetch(&c, &server.url("/_mdmd/freshness?path=fixture.md"));
        let m = serde_json::from_str::<serde_json::Value>(&resp.body_text())
            .expect("mutated freshness JSON")["mtime"]
            .as_u64()
            .expect("mutated mtime u64");
        if m > mtime1 {
            break m;
        }
        if start.elapsed() > STARTUP_TIMEOUT {
            break m;
        }
        thread::sleep(Duration::from_millis(200));
    };

    assert!(
        mtime2 > mtime1,
        "freshness endpoint must return newer mtime after file change; mtime1={mtime1} mtime2={mtime2}"
    );
}

/// Two consecutive freshness requests on an unmodified file return the same mtime.
#[test]
fn test_freshness_unchanged_file() {
    let fixture = make_freshness_fixture();
    let server = ServerHandle::new("test_freshness_unchanged_file", &fixture);
    let c = client();

    let resp1 = fetch(&c, &server.url("/_mdmd/freshness?path=fixture.md"));
    let resp2 = fetch(&c, &server.url("/_mdmd/freshness?path=fixture.md"));
    assert_status(&resp1, 200);
    assert_status(&resp2, 200);

    let mtime1 = serde_json::from_str::<serde_json::Value>(&resp1.body_text())
        .expect("first freshness JSON")["mtime"]
        .as_u64()
        .expect("first mtime u64");
    let mtime2 = serde_json::from_str::<serde_json::Value>(&resp2.body_text())
        .expect("second freshness JSON")["mtime"]
        .as_u64()
        .expect("second mtime u64");

    assert_eq!(
        mtime1, mtime2,
        "mtime must be stable for unmodified file; mtime1={mtime1} mtime2={mtime2}"
    );
}

/// Path traversal via ../../ must return 404 from the freshness endpoint.
#[test]
fn test_freshness_path_traversal_blocked() {
    let fixture = make_freshness_fixture();
    let server = ServerHandle::new("test_freshness_path_traversal_blocked", &fixture);
    let c = client();

    let resp = fetch(
        &c,
        &server.url("/_mdmd/freshness?path=../../etc/passwd"),
    );
    assert_status(&resp, 404);
}

/// A rendered markdown page must include the mdmd-mtime and mdmd-path meta tags.
#[test]
fn test_page_has_mtime_meta_tag() {
    let fixture = make_freshness_fixture();
    let server = ServerHandle::new("test_page_has_mtime_meta_tag", &fixture);
    let c = client();

    let resp = fetch(&c, &server.url("/fixture.md"));
    assert_status(&resp, 200);
    let body = resp.body_text();

    assert!(
        body.contains("name=\"mdmd-mtime\""),
        "mdmd-mtime meta tag missing from page\n{}",
        resp.context()
    );
    assert!(
        body.contains("name=\"mdmd-path\""),
        "mdmd-path meta tag missing from page\n{}",
        resp.context()
    );
    assert!(
        body.contains("content=\"fixture.md\""),
        "mdmd-path meta content must equal 'fixture.md'\n{}",
        resp.context()
    );
}

/// A rendered markdown page must contain the change-notice div in the hidden state.
#[test]
fn test_page_has_change_notice_div() {
    let fixture = make_freshness_fixture();
    let server = ServerHandle::new("test_page_has_change_notice_div", &fixture);
    let c = client();

    let resp = fetch(&c, &server.url("/fixture.md"));
    assert_status(&resp, 200);
    let body = resp.body_text();

    assert!(
        body.contains("id=\"mdmd-change-notice\""),
        "change-notice div id missing\n{}",
        resp.context()
    );
    assert!(
        body.contains("Load latest"),
        "Load latest button text missing from change-notice div\n{}",
        resp.context()
    );
}

// ---------------------------------------------------------------------------
// bd-26u.3: out-of-cwd warning modes and cross-directory allow/block matrix
// ---------------------------------------------------------------------------

/// Wait for a server to start accepting TCP connections on the given port.
fn wait_for_port(port: u16) {
    let start = std::time::Instant::now();
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if start.elapsed() > STARTUP_TIMEOUT {
            panic!(
                "server did not become ready on port {} within {:?}",
                port, STARTUP_TIMEOUT
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// When stdin is /dev/null (non-interactive) the server auto-proceeds past the
/// out-of-cwd warning without any prompt.
#[cfg(unix)]
#[test]
fn test_out_of_cwd_non_interactive_proceeds() {
    let tmp = tempfile::tempdir().expect("create ooc tempdir");
    let entry = tmp.path().join("doc.md");
    fs::write(&entry, "# Doc\n").expect("write doc.md");

    let port = free_port();
    let mut child = Command::new(bin_path())
        .arg("serve")
        .arg("--bind")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg(&entry)
        // current_dir("/") ensures the entry is outside CWD.
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn out-of-cwd server");

    // Server should start and serve requests.
    wait_for_port(port);

    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "server process must still be running after startup"
    );

    // Shut down cleanly.
    send_sigint(child.id());
    wait_with_timeout(&mut child, Duration::from_secs(5));
    let output = child.wait_with_output().expect("collect output");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("WARNING"),
        "out-of-cwd WARNING missing from stderr\nstderr:\n{stderr}"
    );
}

/// When the user writes 'y' to piped stdin the server proceeds past the warning.
#[cfg(unix)]
#[test]
fn test_out_of_cwd_interactive_confirm_proceeds() {
    let tmp = tempfile::tempdir().expect("create ooc confirm tempdir");
    let entry = tmp.path().join("doc.md");
    fs::write(&entry, "# Doc\n").expect("write doc.md");

    let port = free_port();
    let mut child = Command::new(bin_path())
        .arg("serve")
        .arg("--bind")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg(&entry)
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn out-of-cwd server (confirm)");

    // Provide 'y' confirmation before the server reads stdin.
    if let Some(mut stdin_handle) = child.stdin.take() {
        stdin_handle.write_all(b"y\n").expect("write y to stdin");
    }

    wait_for_port(port);

    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "server process must be running after 'y' confirmation"
    );

    send_sigint(child.id());
    wait_with_timeout(&mut child, Duration::from_secs(5));
    let output = child.wait_with_output().expect("collect output");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("WARNING"),
        "out-of-cwd WARNING missing from stderr\nstderr:\n{stderr}"
    );
}

/// When the user writes 'n' to piped stdin the server aborts with a non-zero exit.
#[cfg(unix)]
#[test]
fn test_out_of_cwd_interactive_decline_exits() {
    let tmp = tempfile::tempdir().expect("create ooc decline tempdir");
    let entry = tmp.path().join("doc.md");
    fs::write(&entry, "# Doc\n").expect("write doc.md");

    let port = free_port();
    let mut child = Command::new(bin_path())
        .arg("serve")
        .arg("--bind")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg(&entry)
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn out-of-cwd server (decline)");

    // Provide 'n' to abort.
    if let Some(mut stdin_handle) = child.stdin.take() {
        stdin_handle.write_all(b"n\n").expect("write n to stdin");
    }

    // Poll for child exit (process should exit quickly after reading 'n').
    wait_with_timeout(&mut child, Duration::from_secs(5));
    let output = child.wait_with_output().expect("collect output");

    assert!(
        !output.status.success(),
        "process must exit non-zero after declining the out-of-cwd prompt"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Aborted"),
        "Aborted message missing from stderr\nstderr:\n{stderr}"
    );
}

/// When the entry is inside CWD the server must NOT emit the out-of-cwd WARNING.
#[cfg(unix)]
#[test]
fn test_in_cwd_no_warning() {
    // Create the fixture temp dir inside the test process's CWD so that the
    // entry is inside the server's CWD.
    let cwd = std::env::current_dir().expect("get cwd");
    let tmp = tempfile::Builder::new()
        .tempdir_in(&cwd)
        .expect("create tempdir inside CWD");
    let entry = tmp.path().join("doc.md");
    fs::write(&entry, "# Doc\n").expect("write doc.md");

    let fixture = Fixture {
        _tmp: tmp,
        root: entry.parent().unwrap().to_path_buf(),
        entry,
    };

    let server = ServerHandle::new("test_in_cwd_no_warning", &fixture);
    let _ = fetch(&client(), &server.url("/"));

    let output = server.shutdown_with_sigint();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stderr.contains("WARNING"),
        "in-cwd server must not emit WARNING\nstderr:\n{stderr}"
    );
}

/// When the serve_root is the parent directory both sibling sub-dirs are accessible.
#[cfg(unix)]
#[test]
fn test_cross_dir_allow_broad_root() {
    let tmp = tempfile::tempdir().expect("create cross-dir tempdir");
    let base = tmp.path().to_path_buf();

    // Layout: base/docs/a.md  base/other/b.md  base/README.md (entry for directory serve)
    fs::create_dir_all(base.join("docs")).expect("create docs/");
    fs::create_dir_all(base.join("other")).expect("create other/");
    fs::write(base.join("README.md"), "# Base\n").expect("write README.md");
    fs::write(
        base.join("docs").join("a.md"),
        "# Page A\n\n[Page B](../other/b.md)\n",
    )
    .expect("write docs/a.md");
    fs::write(base.join("other").join("b.md"), "# Page B\n\nContent.\n")
        .expect("write other/b.md");

    let port = free_port();
    let mut child = Command::new(bin_path())
        .arg("serve")
        .arg("--bind")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        // Pass the parent directory as entry: serve_root = base/
        .arg(&base)
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn broad-root server");

    wait_for_port(port);

    let c = client();
    let base_url = format!("http://127.0.0.1:{port}");

    // Both sibling paths must be accessible under broad root.
    let resp_a = fetch(&c, &format!("{base_url}/docs/a.md"));
    assert_status(&resp_a, 200);

    let resp_b = fetch(&c, &format!("{base_url}/other/b.md"));
    assert_status(
        &resp_b,
        200,
    );

    send_sigint(child.id());
    wait_with_timeout(&mut child, Duration::from_secs(5));
    let _ = child.wait_with_output();
}

/// When the serve_root is a narrow subdirectory a cross-dir path returns 404.
#[test]
fn test_cross_dir_block_narrow_root() {
    let tmp = tempfile::tempdir().expect("create cross-dir narrow tempdir");
    let base = tmp.path().to_path_buf();

    fs::create_dir_all(base.join("docs")).expect("create docs/");
    fs::create_dir_all(base.join("other")).expect("create other/");
    fs::write(
        base.join("docs").join("a.md"),
        "# Page A\n\n[Page B](../other/b.md)\n",
    )
    .expect("write docs/a.md");
    fs::write(base.join("other").join("b.md"), "# Page B\n\nContent.\n")
        .expect("write other/b.md");

    // Serve entry is docs/a.md: serve_root = docs/; other/ is outside root.
    let fixture = Fixture {
        entry: base.join("docs").join("a.md"),
        _tmp: tmp,
        root: base.join("docs"),
    };
    let server = ServerHandle::new("test_cross_dir_block_narrow_root", &fixture);
    let c = client();

    // /other/b.md resolves to docs/other/b.md which does not exist → 404.
    let resp = fetch(&c, &server.url("/other/b.md"));
    assert_status(
        &resp,
        404,
    );
}

// ---------------------------------------------------------------------------
// bd-1zw: verbose-gated diagnostic helper — E2E coverage
// ---------------------------------------------------------------------------

/// Default startup (no --verbose) must emit zero informational stderr lines
/// in the `[serve]` and `[bind]` categories.
#[cfg(unix)]
#[test]
fn test_default_startup_no_informational_stderr() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_default_startup_no_informational_stderr", &fixture);

    // Make one request to ensure the server processed at least one event.
    let _ = fetch(&client(), &server.url("/"));

    let output = server.shutdown_with_sigint();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stderr.contains("[serve]"),
        "[serve] must be absent from stderr without --verbose\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("[bind]"),
        "[bind] must be absent from stderr without --verbose\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("[request]"),
        "[request] must be absent from stderr without --verbose\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("[shutdown]"),
        "[shutdown] must be absent from stderr without --verbose\nstderr:\n{stderr}"
    );
}

/// With --verbose, startup diagnostics in `[serve]` and `[bind]` categories
/// must appear in stderr.
#[cfg(unix)]
#[test]
fn test_verbose_startup_diagnostics_emitted() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server =
        ServerHandle::new_verbose("test_verbose_startup_diagnostics_emitted", &fixture);

    // Make one request to ensure the server processed at least one event.
    let _ = fetch(&client(), &server.url("/"));

    let output = server.shutdown_with_sigint();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("[serve]"),
        "[serve] must appear in stderr with --verbose\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("[bind]"),
        "[bind] must appear in stderr with --verbose\nstderr:\n{stderr}"
    );
}
