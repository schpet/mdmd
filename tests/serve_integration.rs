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
}

impl Default for FixtureOptions {
    fn default() -> Self {
        Self {
            include_subdir_readme: true,
            include_large_file: false,
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

    let resp = fetch(&client(), &server.url("/"));
    assert_status(&resp, 200);
    assert_header_contains(&resp, "content-type", "text/html");
}

#[test]
fn test_serve_toc_present() {
    let fixture = Fixture::new(FixtureOptions::default());
    let server = ServerHandle::new("test_serve_toc_present", &fixture);

    let resp = fetch(&client(), &server.url("/"));
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

    let resp = fetch(&client(), &server.url("/?raw=1"));
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

    let resp = fetch(&client(), &server.url("/"));
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

    let resp = fetch(&client(), &server.url("/"));
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

    let resp = fetch(&client(), &server.url("/"));
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

    let resp = fetch(&client(), &server.url("/"));
    assert_status(&resp, 200);
    assert!(
        resp.body_text()
            .contains("https://cdn.jsdelivr.net/npm/mermaid@10.9.3/dist/mermaid.min.js"),
        "pinned mermaid CDN script missing\n{}",
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

    let _ = fetch(&client(), &server.url("/"));

    let output = server.shutdown_with_sigint();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();

    assert!(
        !lines.is_empty(),
        "startup stdout is empty\nstdout:\n{stdout}"
    );
    assert_eq!(
        lines[0], "mdmd serve",
        "first startup line must be exact banner\nstdout:\n{stdout}"
    );

    let root_idx = lines
        .iter()
        .position(|l| l.starts_with("root:  "))
        .unwrap_or_else(|| panic!("missing root line\nstdout:\n{stdout}"));
    let entry_idx = lines
        .iter()
        .position(|l| l.starts_with("entry: "))
        .unwrap_or_else(|| panic!("missing entry line\nstdout:\n{stdout}"));
    let url_idx = lines
        .iter()
        .position(|l| l.starts_with("url:   http://"))
        .unwrap_or_else(|| panic!("missing url line\nstdout:\n{stdout}"));

    assert!(
        root_idx > 0,
        "root line must follow banner\nstdout:\n{stdout}"
    );
    assert!(
        entry_idx > root_idx,
        "entry line must appear after root line\nstdout:\n{stdout}"
    );
    assert!(
        url_idx > entry_idx,
        "url line must appear after entry line\nstdout:\n{stdout}"
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
        stderr.contains("[legacy] TUI viewer dispatched"),
        "legacy path did not dispatch TUI\nstderr:\n{}",
        stderr
    );
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
