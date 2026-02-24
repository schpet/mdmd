## Overview

Implement `mdmd serve` as a built-in command that serves Markdown as a web page with strong server-side rendering, safe local-link resolution, and GitHub-style features in scope for phase 1.

## Gate A Revision Disposition (ChatGPT Suggestions)

Adopted for phase 1 UX/perf:
- Default port changed to `3333` with bounded auto-increment.
- Always print localhost URL; additionally print Tailscale DNS URL when available.
- Use `comrak` for web rendering to improve GFM parity and AST-based TOC/link handling.
- Keep sticky TOC + active heading scrollspy behavior.
- Support robust local path resolution (exact, extensionless `.md`, directory `README.md`/`index.md`).
- Serve non-Markdown local assets (for example images) inside `serve_root`.
- Include response cache validators (`ETag`, `Last-Modified`) and compression middleware.
- Include startup/page polish (`serve_root`, entry file, current file path in header, `?raw=1`).

Deferred or kept out of phase 1:
- `--open` browser launch is optional and deferred.
- Live reload (`--watch`) remains out of scope for phase 1.
- Mermaid server-side rendering via `mmdc` is deferred; client hydration remains default.
- HTML sanitization flag is optional follow-up, not required for phase 1.
- Theme toggle, directory index browsing, TOC search remain optional follow-ons.

## Key Decisions (Resolved)

1. HTTP stack
- Use `axum` + `tokio` for the server.
- Rationale: minimal routing overhead, clear async story, easy graceful shutdown.

2. CLI compatibility
- Preserve existing `mdmd <file>` behavior (current TUI viewer).
- Add explicit subcommands without breaking existing usage:
  - `mdmd serve <file>`
  - `mdmd view <file>` (explicit equivalent of legacy mode)
- Clap shape: a `Subcommand` enum plus compatibility parsing for legacy positional file input.

3. HTML rendering strategy
- Create a new HTML pipeline separate from current ratatui rendering.
- Use `comrak` for the `serve` pipeline to improve GitHub-style feature parity (GFM tables/task lists/autolinks/strikethrough) and make heading/TOC/link handling easier via AST traversal.
- Render Markdown server-side in Rust into full page HTML shell.
- Extract headings and IDs during render for TOC construction.

4. Mermaid strategy
- Phase 1 uses SSR placeholders (`<pre class="mermaid">...</pre>`) and client-side Mermaid hydration via JS.
- No Node/headless browser dependency in phase 1.

5. Styling and assets
- Ship embedded assets with `include_str!`:
  - `mdmd.css` for layout/typography/table/code/TOC styles
  - `mdmd.js` for TOC active-heading sync and Mermaid init
- Serve assets from static routes (for example `/assets/mdmd.css`, `/assets/mdmd.js`).

6. Security boundary for local links
- Define `serve_root` as canonical parent directory of the initial file.
- Route all content through a safe resolver rooted at `serve_root`:
  1) decode percent-encoding FIRST, then normalize request path, reject traversal/out-of-root;
  2) try exact path, then `path + ".md"` when extensionless;
  3) if directory, try `README.md` then `index.md`.
- After resolving a candidate path, apply `std::fs::canonicalize` (follows symlinks) and re-verify containment against canonicalized `serve_root` (R1 symlink escape mitigation).
- If resolved file is Markdown, render as page; otherwise serve as static asset.
- This preserves relative links across nested docs while maintaining root containment checks.

7. Port selection policy
- Default start port: `3333`.
- Increment by 1 on `EADDRINUSE` only; max attempts (`100`).
- Fail immediately (no retry) for any other bind error (permission denied, interface not found, etc.).
- Fail with clear error after max `EADDRINUSE` retries (no unbounded loop).

8. Host URL printing
- Try `tailscale status --json`; use `.Self.DNSName` only when non-empty.
- Trim trailing dot from DNS name.
- Always print local URL: `http://127.0.0.1:{port}`.
- When Tailscale DNS is available, also print `http://{tailscale_dns}:{port}`.
- Default bind remains loopback for safety; `--bind` can be set explicitly for LAN/Tailscale reachability.

9. TOC active-section behavior
- Use `IntersectionObserver` in minimal client JS to highlight the current heading.
- TOC markup and heading anchors are server-rendered.

10. Live reload
- Explicitly out of scope for phase 1.

11. UX polish in startup/page output
- Print resolved `serve_root` and entry markdown file on startup.
- Include current file path in page header.
- Add `?raw=1` response mode for debugging/raw markdown inspection.
- Startup output format (stable and machine-assertable):
  ```
  mdmd serve
  root:  /absolute/path/to/serve_root
  entry: /absolute/path/to/entry.md
  url:   http://127.0.0.1:3333
  url:   http://hostname.ts.net:3333   [only when tailscale available]
  ```

## Phase-1 Feature Matrix (GitHub-Style Support)

In scope:
- ATX headings
- Paragraphs, emphasis, strong, inline code
- Fenced code blocks
- Tables
- Task lists
- Strikethrough
- Blockquotes
- Ordered and unordered lists
- Autolinks and normal links
- Mermaid fenced blocks (client-side hydrated)
- Relative links to local Markdown pages (including extensionless paths)
- Non-Markdown local assets (for example images) served as static files

Out of scope (phase 1):
- Full GitHub CSS parity pixel-match
- Footnotes if not already supported by current parser options
- Math rendering
- Live reload/file watching

## Architecture and Modules

New modules (planned):
- `src/serve.rs`: server bootstrap, routing, bind/retry, shutdown
- `src/html.rs`: Markdown -> HTML render, heading extraction, local-link rewrite
- `src/web_assets.rs`: embedded CSS/JS constants and helpers

Existing modules remain:
- `src/parse.rs`, `src/render.rs` continue to back the TUI path.

## Request Flow

1. `mdmd serve <file>` validates input and computes canonical `serve_root`.
2. Server binds on preferred port with bounded retries (only on EADDRINUSE).
3. Startup URL is printed with Tailscale-aware host detection.
4. `GET /` (or routed Markdown path) reads and renders Markdown server-side.
5. HTML shell includes persistent TOC + embedded/served CSS/JS assets.
6. Client JS performs TOC active-heading highlighting and Mermaid hydration.
7. Responses include cache validators (`ETag`, `Last-Modified`) and compression where supported.

## Concurrency and Performance

- Use immutable shared app state via `Arc` (serve root, initial file, config).
- Keep per-request behavior simple, but add low-risk web performance defaults in phase 1:
  - cache validators (`ETag`/`Last-Modified`) to enable fast 304 revalidation;
  - response compression (gzip/brotli) via `tower-http` `CompressionLayer`.
- Keep main content SSR; JS is limited to progressive enhancement.

## Graceful Shutdown

- Use `tokio::signal::ctrl_c()` for clean shutdown on SIGINT.
- Ensure listener/task exits cleanly without panics on Ctrl+C.

## Testing Strategy

1. Unit tests
- Port selection retry logic (EADDRINUSE success/retry/max-failure; non-EADDRINUSE immediate failure).
- Tailscale JSON parsing (valid name, trailing dot, empty, malformed).
- Path containment checks (traversal, URL-encoded traversal `%2e%2e`, symlink escape, in-root success).
- Path resolution fallbacks (extensionless .md, directory README.md, directory index.md).
- Anchor ID deduplication (same heading text repeated, same text at different nesting levels).
- HTML injection: `<script>` block stripped with `unsafe_ = false`.
- Markdown link rewrite rules (relative .md, ../nested.md, external unchanged, fragment, query string).
- MIME type lookup for common extensions; `application/octet-stream` fallback.
- File size limit: oversized content yields 413 before read.

2. Integration tests
- `mdmd serve <file>` serves HTML with TOC present.
- Local `.md` link request resolves inside root and is blocked outside root.
- Extensionless and directory markdown route resolution behaves as specified.
- Non-markdown local assets are served from within `serve_root`.
- Mermaid block emits expected placeholder markup.
- Legacy `mdmd <file>` invocation still works.
- Cache headers present; `If-None-Match` returns 304.
- Compression negotiated when `Accept-Encoding: gzip`.
- `X-Content-Type-Options: nosniff` present on all responses.
- 413 returned for oversized file.

## Bead Decomposition (Gate A)

Top-level feature:
- `bd-2f2` - Serve5: Implement `mdmd serve` phase-1 web server

Dependency execution order (blocks):
1. `bd-p7i` -> CLI compatibility foundation
2. `bd-1mz` -> server lifecycle and bind policy
3. `bd-3kq` -> tailscale-aware startup URLs
4. `bd-mzl` -> SSR comrak rendering + heading extraction
5. `bd-2n4` -> HTML shell + TOC + assets + `?raw=1`
6. `bd-ezg` -> secure serve-root path resolver
7. `bd-1p6` -> markdown local-link rewriting
8. `bd-2se` -> Mermaid placeholders + hydration
9. `bd-22o` -> cache validators + compression
10. `bd-30z` -> startup/page-context UX polish
11. `bd-2l9` -> unit test coverage
12. `bd-39z` -> integration/e2e verification

Beads (self-contained implementation contracts):

### `bd-p7i` - Serve5-01: CLI subcommands with legacy compatibility

- Objective and Scope:
  - Preserve `mdmd <file>` TUI behavior while adding explicit `mdmd serve <file>` and `mdmd view <file>`.
- Implementation Details:
  - Refactor clap to support a `Subcommand` enum (`Serve { file: String, bind: String, port: u16 }`, `View { file: String }`) plus legacy positional compatibility.
  - For the legacy path: if no subcommand is matched, re-parse the first positional arg as a file path and dispatch to the TUI.
  - Use `#[command(flatten)]` or a manual `Args` compatibility shim; do not break existing `mdmd <file>` parsing.
  - `view` maps to the existing TUI dispatch path; `serve` dispatches to the new web server path.
  - The `serve` subcommand must include both `--bind <ADDR>` (default `127.0.0.1`) and `--port <N>` (default `3333`) as separate flags. `--bind` is the interface address; `--port` is the starting port number passed to bd-1mz's retry loop. Integration tests in bd-39z use `--port <free_port>` with a randomly allocated free port to avoid conflicts.
  - `--help` output should clearly describe all three invocation forms.
  - Startup logs must indicate which mode was dispatched (`[serve]`, `[view]`, `[legacy]`).
  - R9 mitigation: the refactored clap must not break bare positional file argument parsing.
- Dependency Links:
  - Parent: `bd-2f2`.
  - Blocks: `bd-1mz`, `bd-mzl`.
- Acceptance Criteria:
  - `mdmd README.md` still launches TUI (no regression).
  - `mdmd view README.md` launches TUI identically.
  - `mdmd serve README.md` dispatches to web server path.
  - `mdmd serve --bind 0.0.0.0 --port 8080 README.md` binds to specified interface and port without error.
  - Help text accurately describes all three invocation forms.
  - Startup diagnostic log line indicates selected mode.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit parse matrix covering: bare file, `view <file>`, `serve <file>`, `serve --bind 127.0.0.1 <file>`, `serve --port 8080 <file>`, `serve --bind 0.0.0.0 --port 3334 <file>`, missing file, `--help`.
  - Integration: spawn binary with bare `README.md` arg, assert TUI mode dispatched (not serve).
  - Integration: spawn binary with `serve README.md`, assert serve mode dispatched.
  - Log line `[legacy]`/`[view]`/`[serve]` must appear in startup output.
  - Verify `--bind` and `--port` are each accepted without error by the clap parser and both values are propagated to server bootstrap (bd-1mz).

### `bd-1mz` - Serve5-02: Server bootstrap, bounded port retry, graceful shutdown

- Objective and Scope:
  - Implement axum/tokio server startup, default port `3333`, bounded retry (`100` attempts on EADDRINUSE only), and Ctrl+C shutdown.
- Implementation Details:
  - Bind loop: on `EADDRINUSE` errors, increment port by 1 and retry up to 100 times; exit with clear error when max attempts exhausted.
  - R8 mitigation: distinguish `EADDRINUSE` from other OS errors. For any non-EADDRINUSE bind error (e.g. `EACCES`, `ENODEV`), fail immediately with the specific OS error message — do NOT retry.
  - Keep default bind address on loopback (`127.0.0.1`) unless explicit `--bind` flag is provided.
  - Add `tokio::signal::ctrl_c()` shutdown path; ensure all listener tasks exit cleanly.
  - Log each bind attempt (port tried, outcome), the final bound port, and shutdown completion.
  - App state shared via `Arc<AppState>` containing `serve_root`, `entry_file`, and config.
- Dependency Links:
  - Parent: `bd-2f2`.
  - Depends on: `bd-p7i`.
  - Blocks: `bd-3kq`, `bd-ezg`, `bd-22o`, `bd-30z`, test beads.
- Acceptance Criteria:
  - Server binds on `3333` when free; retries on EADDRINUSE up to 100 times.
  - Non-EADDRINUSE errors cause immediate failure with OS error detail (not retry).
  - Clean shutdown on SIGINT without panic; exit code 0.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit: `bind_with_retry` returns correct port after N EADDRINUSE failures.
  - Unit: non-EADDRINUSE error produces immediate `Err`, never retries.
  - Unit: attempt counter stops at 100 and returns descriptive error.
  - Integration: occupy port 3333, assert server binds on 3334.
  - Integration: occupy ports 3333–3432 (all 100 candidate ports), assert server fails with clear error message — must not panic.
  - Log lines: `[bind] trying port=3333`, `[bind] EADDRINUSE, trying 3334`, `[bind] success port=3334`, `[shutdown] complete`.

### `bd-3kq` - Serve5-03: Tailscale-aware startup URL output

- Objective and Scope:
  - Always print localhost URL; conditionally print tailscale URL from `.Self.DNSName`.
- Implementation Details:
  - Spawn `tailscale status --json` as subprocess; capture stdout.
  - R7 mitigation: parse JSON via `serde_json` using `?`/`Result` propagation only — zero `unwrap()`/`expect()` on tailscale parsing paths. Any parse failure, empty output, or subprocess error is silently treated as "no Tailscale available"; log at `debug` level why it was skipped.
  - Trim trailing `.` from DNSName before printing.
  - Only print the Tailscale URL when DNSName is non-empty after trimming.
  - Startup output always includes `url:   http://127.0.0.1:{port}`.
  - When Tailscale is available: also print `url:   http://{tailscale_dns}:{port}`.
- Dependency Links:
  - Parent: `bd-2f2`.
  - Depends on: `bd-1mz`.
- Acceptance Criteria:
  - Startup never panics or fails due to tailscale errors.
  - URL output policy is deterministic: always localhost, optionally tailscale.
  - All tailscale parse paths use `?`/`Result`; no unwrap.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit: valid JSON with DNSName `"hostname.ts.net."` → trimmed URL `http://hostname.ts.net:{port}`.
  - Unit: trailing dot only `"."` → treated as empty, no tailscale URL.
  - Unit: empty JSON `{}` or missing `Self` → no tailscale URL, no panic.
  - Unit: malformed JSON → no tailscale URL, no panic.
  - Unit: subprocess failure (command not found) → no tailscale URL, no panic.
  - Integration: start server with environment stub returning valid Tailscale JSON; assert tailscale URL appears in startup output.
  - Integration: start server with environment stub returning invalid JSON; assert only localhost URL appears (no tailscale URL, no crash).
  - Log: `[tailscale] skipped reason=<detail>` at debug level when unavailable.

### `bd-mzl` - Serve5-04: Comrak SSR renderer with heading extraction

- Objective and Scope:
  - Implement serve-only markdown renderer using comrak with GFM features, heading extraction, and security-safe defaults.
- Implementation Details:
  - Enable comrak GFM extensions: tables, task lists, autolinks, strikethrough, fenced code blocks.
  - R3 mitigation: set `ComrakOptions::render.unsafe_ = false` (default). Raw HTML blocks in Markdown are stripped, not passed through. Add unit test asserting `<script>alert(1)</script>` does not appear in rendered HTML output.
  - R4 mitigation: implement anchor ID deduplication counter during heading extraction. The first occurrence of a heading text slug gets its bare slug (e.g. `my-heading`); subsequent occurrences get a numeric suffix (`my-heading-1`, `my-heading-2`). Counter is per-document. Add unit tests with repeated headings at same and different nesting levels.
  - Extract headings (level, text, anchor ID) from the AST before rendering to a `Vec<HeadingEntry>` for TOC construction.
  - Keep TUI parse/render path (`src/parse.rs`, `src/render.rs`) completely untouched.
  - Render into `src/html.rs` as a pure function: `fn render_markdown(input: &str, file_path: &Path, serve_root: &Path) -> (String, Vec<HeadingEntry>)`. The `file_path` and `serve_root` parameters are required by bd-1p6's relative link resolution during the same AST traversal pass.
  - Log rendered file path and extracted heading count at `info` level.
- Dependency Links:
  - Parent: `bd-2f2`.
  - Depends on: `bd-p7i`.
  - Blocks: `bd-2n4`, `bd-1p6`, `bd-2se`, `bd-22o`, test beads.
- Acceptance Criteria:
  - HTML covers phase-1 markdown feature matrix.
  - `<script>` in input does not appear in rendered HTML output.
  - Duplicate headings produce unique, sequenced anchor IDs.
  - TOC-compatible heading metadata (`Vec<HeadingEntry>`) is produced.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit: GFM tables render `<table>` with `<th>` and `<td>`.
  - Unit: task lists render `<input type="checkbox">`.
  - Unit: strikethrough renders `<del>`.
  - Unit: fenced code block with info string `rust` renders `<pre><code class="language-rust">` (or equivalent) — validates language hint is preserved in HTML output.
  - Unit: autolinks: bare URL `https://example.com` in Markdown body → rendered as `<a href="https://example.com">` (GFM autolink extension active).
  - Unit: blockquotes: `> quoted text` → renders `<blockquote>` wrapper.
  - Unit: ordered list `1. Item` → renders `<ol><li>`.
  - Unit: unordered list `- Item` → renders `<ul><li>`.
  - Unit: `<script>alert(1)</script>` in input → absent from HTML output.
  - Unit: headings `## Foo`, `## Foo`, `## Foo` → anchors `foo`, `foo-1`, `foo-2`.
  - Unit: headings `## Foo`, `### Foo` → anchors `foo`, `foo-1` (no collision sharing).
  - Unit: anchor IDs are stable across two renders of the same document.
  - Log: `[render] path=<file> headings=<count>` at info level.

### `bd-2n4` - Serve5-05: HTML shell, sticky TOC UI, assets, and raw mode

- Objective and Scope:
  - Build page shell with persistent sticky TOC, active heading highlight via IntersectionObserver, embedded static assets, and `?raw=1` raw mode.
- Implementation Details:
  - HTML shell template in `src/html.rs` (or a const template string): full `<!DOCTYPE html>` with `<head>` (charset, viewport, CSS link) and `<body>` (header, TOC sidebar, content area).
  - Header displays: `mdmd serve` branding and current file path relative to `serve_root`.
  - TOC sidebar: sticky, left-aligned, generated from `Vec<HeadingEntry>`; links use `#anchor-id` fragments.
  - Active heading: `mdmd.js` uses `IntersectionObserver` with `rootMargin: "0px 0px -80% 0px"` to detect topmost visible heading; applies `.active` CSS class to corresponding TOC link.
  - Mermaid init entrypoint in `mdmd.js`: call `mermaid.initialize({ startOnLoad: true })` after DOM ready.
  - Serve `/assets/mdmd.css` and `/assets/mdmd.js` from `include_str!` embedded constants in `src/web_assets.rs`.
  - `?raw=1` query param: return Markdown source as `text/plain; charset=utf-8`, bypassing HTML rendering.
  - CSS: responsive two-column layout (TOC + content), GitHub-inspired code block and table styles, sticky TOC behavior via `position: sticky`.
  - Request log includes `mode=rendered` or `mode=raw` per request.
- Dependency Links:
  - Parent: `bd-2f2`.
  - Depends on: `bd-mzl`.
  - Blocks: `bd-2se`, `bd-30z`, `bd-39z`.
- Acceptance Criteria:
  - TOC is visible and active section updates while scrolling (IntersectionObserver).
  - `?raw=1` returns Markdown source with `Content-Type: text/plain`.
  - `/assets/mdmd.css` and `/assets/mdmd.js` serve with correct MIME types.
  - Header includes current file path context.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Integration: GET `/` returns HTML with `<nav>` containing TOC links.
  - Integration: GET `/?raw=1` returns `Content-Type: text/plain` and Markdown source text.
  - Integration: GET `/assets/mdmd.css` returns `Content-Type: text/css`.
  - Integration: GET `/assets/mdmd.js` returns `Content-Type: text/javascript`.
  - Integration: rendered page HTML contains `<script>` reference to Mermaid CDN or local init.
  - Browser e2e (optional manual): scroll page, verify `.active` class moves between TOC entries.
  - Log: `[request] path=/ mode=rendered` and `[request] path=/ mode=raw` per request type.

### `bd-ezg` - Serve5-06: Secure serve_root path resolution for markdown and assets

- Objective and Scope:
  - Enforce canonical `serve_root` containment with robust local path resolution, symlink safety, URL-encoding safety, file size limits, and correct MIME types.
- Implementation Details:
  - **Step 1 – Decode**: percent-decode the request path FIRST (before any normalization).
  - **Step 2 – Normalize**: strip `..` components using `Path` component iteration; reject requests still containing `..` after decode.
  - **Step 3 – Construct**: join `serve_root` + normalized path to get a candidate absolute path.
  - **Step 4 – Fallback resolution**: try candidate as exact path; if not found, try `candidate + ".md"`; if candidate is a directory, try `candidate/README.md` then `candidate/index.md`.
  - **Step 5 – Canonicalize + re-check (R1)**: call `std::fs::canonicalize` on the resolved path (follows symlinks); re-verify the canonicalized path starts with canonicalized `serve_root`. Reject if outside root even via symlink.
  - **Step 6 – File size guard (R5)**: stat the file before reading; if size exceeds 16 MB (configurable), return `413 Content Too Large` with log of path and size.
  - **Step 7 – Dispatch**: if resolved file is `.md` (case-insensitive), render as HTML page; otherwise serve as static asset.
  - **MIME type (R6)**: derive `Content-Type` from file extension using a static lookup table: `.md`→`text/html`, `.html`→`text/html`, `.css`→`text/css`, `.js`→`text/javascript`, `.png`→`image/png`, `.jpg`/`.jpeg`→`image/jpeg`, `.svg`→`image/svg+xml`, `.gif`→`image/gif`, `.ico`→`image/x-icon`, `.woff2`→`font/woff2`, `.pdf`→`application/pdf`. Unknown extensions → `application/octet-stream`. Never rely on browser MIME sniffing.
  - **R2 mitigation (URL-encoded traversal)**: unit tests must cover `%2e%2e`, `%2F`, `%2e%2e%2f`, mixed-case `%2E%2E`. All must return 404, never file content outside root.
  - Set `X-Content-Type-Options: nosniff` on all responses (including 404/413/asset).
  - Log: normalized path, resolution branch taken (exact/extensionless/readme/index), deny reason when applicable.
- Dependency Links:
  - Parent: `bd-2f2`.
  - Depends on: `bd-1mz`.
  - Blocks: `bd-1p6`, `bd-22o`, test beads.
- Acceptance Criteria:
  - No out-of-root file access via traversal, URL-encoding, or symlink.
  - All fallback resolution cases (extensionless, README.md, index.md) work correctly.
  - File size limit enforced at 16 MB; 413 returned for oversized files.
  - All responses include correct `Content-Type` and `X-Content-Type-Options: nosniff`.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit: request `/../etc/passwd` → 404 (after decode+normalize).
  - Unit: request `/%2e%2e/etc/passwd` → 404.
  - Unit: request `/%2e%2e%2fetc%2fpasswd` → 404.
  - Unit: in-tree symlink pointing to `/tmp/secret.txt` → 404 (canonicalize re-check).
  - Unit: extensionless request `GET /docs/guide` resolves to `docs/guide.md` when exists.
  - Unit: directory request `GET /docs/` resolves to `docs/README.md` then `docs/index.md`.
  - Unit: file > 16 MB → 413 before any content read.
  - Unit: `.png` extension → `Content-Type: image/png`.
  - Unit: `.svg` extension → `Content-Type: image/svg+xml`.
  - Unit: unknown `.xyz` extension → `Content-Type: application/octet-stream`.
  - Integration: traverse attempt `GET /../../etc/passwd` → 404.
  - Integration: in-root file `GET /image.png` → 200 with correct MIME.
  - Integration: all responses include `X-Content-Type-Options: nosniff`.
  - Log: `[resolve] path=<norm> branch=<exact|extensionless|readme|index|denied> reason=<detail>`.

### `bd-1p6` - Serve5-07: Local markdown link rewriting for web navigation

- Objective and Scope:
  - Rewrite local markdown links in the AST before HTML rendering for correct web navigation; preserve external URLs.
- Implementation Details:
  - Perform link rewriting via comrak AST traversal (not post-render string replacement) to avoid spurious matches in code blocks, inline HTML, and titles.
  - Rewrite targets that are relative paths ending in `.md` or extensionless local paths (not starting with `http://`, `https://`, `//`, `mailto:`, or `#`):
    - `.md` suffix: leave as-is (the serve resolver handles `.md` paths).
    - Extensionless relative path: leave as-is (resolver will try `.md` fallback).
    - Absolute and external URLs: never rewritten.
  - Preserve query strings (`?`) and fragments (`#`) — append after the rewritten base path.
  - Directory-relative links: resolve from the current file's directory within `serve_root`.
  - Log: count of links rewritten vs. skipped per document at `debug` level.
- Dependency Links:
  - Parent: `bd-2f2`.
  - Depends on: `bd-mzl`, `bd-ezg`.
  - Blocks: `bd-2l9`, `bd-39z`.
- Acceptance Criteria:
  - Internal doc links navigate correctly across nested file hierarchies.
  - External links are unchanged.
  - No unsafe escaping via rewritten links.
  - Fragments and query strings are preserved.
  - Relative links that would escape `serve_root` (e.g. `../../outside.md`) are sanitized and never produce a URL pointing outside the root.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit: `[text](other.md)` → href remains `/other.md` (resolver-compatible).
  - Unit: `[text](../parent.md)` → href resolves relative to current file directory; result is root-relative and within `serve_root`.
  - Unit: `[text](subdir/doc)` → extensionless href passed through (resolver will add .md).
  - Unit: `[text](https://example.com/page.md)` → href unchanged.
  - Unit: `[text](#section)` → href unchanged.
  - Unit: `[text](doc.md#section?query=1)` → fragment and query preserved.
  - Unit: `[text](../../outside.md)` from a file in `docs/subdir/` → resulting href does not escape `serve_root`; either clamped to root or rendered as-is for the path resolver to reject at request time with 404.
  - Unit: link in fenced code block → NOT rewritten.
  - Integration: navigate from `index.md` → `subdir/page.md` via link → page renders.
  - Log: `[rewrite] file=<path> rewritten=<N> skipped=<M>` at debug level.

### `bd-2se` - Serve5-08: Mermaid placeholder SSR and client hydration

- Objective and Scope:
  - Support Mermaid in phase 1 with SSR placeholders and client hydration only; no Node/headless-browser dependency.
- Implementation Details:
  - During comrak AST traversal, detect fenced code blocks with `language = "mermaid"` (case-insensitive).
  - Replace mermaid fences with `<pre class="mermaid">...</pre>` where `...` is the HTML-escaped diagram source (do not inject raw Mermaid source into HTML without escaping).
  - Non-mermaid fenced code blocks are rendered via standard comrak code block output (unchanged).
  - In `mdmd.js`, initialize Mermaid client-side: load Mermaid from CDN (version-pinned URL) and call `mermaid.initialize({ startOnLoad: true, theme: 'default' })`.
  - Mermaid CDN script tag is included in the HTML shell unconditionally (small overhead, avoids conditional complexity).
- Dependency Links:
  - Parent: `bd-2f2`.
  - Depends on: `bd-mzl`, `bd-2n4`.
  - Blocks: `bd-39z`.
- Acceptance Criteria:
  - Mermaid fences render `<pre class="mermaid">` placeholder with escaped diagram source.
  - Non-Mermaid fences are unchanged by the Mermaid detection pass.
  - Client-side hydration initializes without console errors on a page with a Mermaid diagram.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit: fenced block with language `mermaid` → output contains `<pre class="mermaid">`.
  - Unit: diagram source containing `<>` → HTML-escaped in output (`&lt;&gt;`).
  - Unit: fenced block with language `rust` → rendered as code block, NOT as mermaid placeholder.
  - Unit: fenced block with language `MERMAID` (uppercase) → also detected as mermaid.
  - Integration: GET page with mermaid diagram → HTML response contains `class="mermaid"`.
  - Integration: Mermaid CDN `<script>` tag present in HTML shell.
  - Browser e2e (optional manual): mermaid diagram renders as SVG, no console errors.

### `bd-22o` - Serve5-09: Cache validators and compression middleware

- Objective and Scope:
  - Add `ETag`/`Last-Modified` response headers and response compression to improve serve performance; handle conditional requests with 304 responses.
- Implementation Details:
  - ETag: compute as hex-encoded FNV-1a (fast; sufficient for cache validation — not a cryptographic use) of response body bytes; set as strong ETag `"<hash>"`. Document the chosen algorithm in a code comment so it can be changed consistently. SHA-256 is acceptable if FNV-1a is unavailable, but prefer the faster hash.
  - Last-Modified: derive from file `mtime` (`std::fs::metadata().modified()`); format as HTTP date string per RFC 7231.
  - Conditional request handling:
    - `If-None-Match`: compare request ETag against response ETag; return `304 Not Modified` with no body when equal.
    - `If-Modified-Since`: compare request date against `Last-Modified`; return `304 Not Modified` when file is not newer.
  - Compression: use `tower-http` `CompressionLayer` with gzip and brotli enabled; apply to all text responses (HTML, CSS, JS, plain text).
  - Static assets (`/assets/mdmd.css`, `/assets/mdmd.js`) also include ETag/Last-Modified based on embedded content hash/build time.
  - Log: `[cache] path=<p> etag=<hash> status=<200|304>` and `[compression] encoding=<gzip|br|none>` per request.
- Dependency Links:
  - Parent: `bd-2f2`.
  - Depends on: `bd-1mz`, `bd-ezg`, `bd-mzl`.
  - Blocks: `bd-39z`.
- Acceptance Criteria:
  - All responses include `ETag` and `Last-Modified` headers.
  - `If-None-Match` with matching ETag returns 304.
  - `If-Modified-Since` with unmodified file returns 304.
  - Compression applied for gzip/brotli when negotiated.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit: same content bytes produce identical ETag; different bytes produce different ETag.
  - Unit: `Last-Modified` format parses as valid RFC 7231 HTTP date.
  - Integration: GET page → response includes `ETag` and `Last-Modified`.
  - Integration: GET with `If-None-Match: <etag>` → 304, empty body.
  - Integration: GET with `If-Modified-Since: <date newer than file mtime>` → 304, empty body (file not modified since that date).
  - Integration: GET with `Accept-Encoding: gzip` → `Content-Encoding: gzip`.
  - Integration: GET with `Accept-Encoding: br` → `Content-Encoding: br`.
  - Log: `[cache] path=/ etag=abc123 status=304` when conditional match.

### `bd-30z` - Serve5-12: Startup and page-context UX polish

- Objective and Scope:
  - Finalize startup/page context details: print `serve_root`, entry file, and URL(s) in a stable machine-assertable format; display current file path in page header.
- Implementation Details:
  - Startup output (written to stdout, stable format):
    ```
    mdmd serve
    root:  /absolute/path/to/serve_root
    entry: /absolute/path/to/entry.md
    url:   http://127.0.0.1:3333
    url:   http://hostname.ts.net:3333   (only when tailscale available)
    ```
  - Each line uses a consistent `key:   value` format with aligned colons for readability.
  - Page header HTML element: `<header><span class="serve-root">{serve_root}</span> / <span class="current-file">{relative_file_path}</span></header>`.
  - `relative_file_path` is the current request's resolved file path relative to `serve_root`.
  - All startup output must be assertable by integration tests via stdout line matching.
- Dependency Links:
  - Parent: `bd-2f2`.
  - Depends on: `bd-1mz`, `bd-2n4`.
  - Blocks: `bd-39z`.
- Acceptance Criteria:
  - Startup stdout contains `root:`, `entry:`, and at least one `url:` line in stable format.
  - Page header shows current file relative path.
  - Output remains consistent regardless of Tailscale availability (localhost URL always present).
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Integration: capture startup stdout, assert lines match expected format with regex.
  - Integration: assert `root:  /abs/path` line present.
  - Integration: assert `entry: /abs/path` line present.
  - Integration: assert `url:   http://127.0.0.1:<port>` line present.
  - Integration: render a page, assert `<header>` contains relative file path.
  - Log: startup diagnostics remain structured for bind/path troubleshooting.

### `bd-2l9` - Serve5-10: Unit test coverage for serve internals

- Objective and Scope:
  - Provide comprehensive deterministic unit test coverage for all serve internals, including all R1–R10 risk mitigations.
- Implementation Details:
  - Tests live in `#[cfg(test)]` modules within each source file (`src/serve.rs`, `src/html.rs`, `src/web_assets.rs`).
  - Each test must include a descriptive name explaining the exact scenario under test.
  - All tests must log scenario-specific failure context (via `println!` in test body or assertion messages).
  - **Port retry (R8)**:
    - `test_bind_retry_eaddrinuse_success`: N ports occupied, next free → success.
    - `test_bind_retry_max_failure`: 100 ports all EADDRINUSE → descriptive error.
    - `test_bind_no_retry_eacces`: EACCES error → immediate failure, attempt count = 1.
  - **Tailscale parsing (R7)**:
    - `test_tailscale_valid_dns_name`: `"hostname.ts.net."` → trimmed URL.
    - `test_tailscale_trailing_dot_only`: `"."` → no tailscale URL.
    - `test_tailscale_empty_dns_name`: `""` → no tailscale URL.
    - `test_tailscale_missing_self_key`: JSON `{}` → no tailscale URL.
    - `test_tailscale_malformed_json`: `"not json"` → no tailscale URL, no panic.
    - `test_tailscale_subprocess_error`: command not found → no tailscale URL, no panic.
  - **Path safety (R1, R2)**:
    - `test_path_traversal_dotdot`: `/../etc/passwd` → denied.
    - `test_path_traversal_url_encoded_dotdot`: `/%2e%2e/etc/passwd` → denied.
    - `test_path_traversal_slash_encoded`: `/%2e%2e%2fetc%2fpasswd` → denied (slash also URL-encoded; must not bypass normalization).
    - `test_path_traversal_double_encoded`: `/%252e%252e/` → denied.
    - `test_path_traversal_mixed_case_encoding`: `/%2E%2E/` → denied.
    - `test_path_symlink_escape`: in-tree symlink → `/tmp/` → denied (canonicalize re-check).
    - `test_path_in_root_exact`: valid in-root path → resolved.
  - **Path fallbacks**:
    - `test_path_extensionless_md_fallback`: `GET /guide` → resolves `guide.md`.
    - `test_path_directory_readme`: `GET /docs/` → resolves `docs/README.md`.
    - `test_path_directory_index`: `GET /docs/` (no README.md) → resolves `docs/index.md`.
  - **File size limit (R5)**:
    - `test_file_size_limit_exceeded`: file > 16 MB → 413 result.
    - `test_file_size_limit_at_boundary`: file = 16 MB exactly → allowed.
  - **MIME types (R6)**:
    - `test_mime_png`, `test_mime_svg`, `test_mime_jpg`, `test_mime_gif`, `test_mime_ico`.
    - `test_mime_css`, `test_mime_js`, `test_mime_woff2`, `test_mime_pdf`.
    - `test_mime_unknown_extension`: `.xyz` → `application/octet-stream`.
  - **HTML injection (R3)**:
    - `test_script_tag_stripped`: Markdown with `<script>` → not in HTML output.
    - `test_style_tag_stripped`: Markdown with `<style>` → not in HTML output.
  - **Renderer feature tests**:
    - `test_render_fenced_code_language_class`: fenced block with info string `rust` → output contains `class="language-rust"` (or equivalent); validates language hint preserved.
    - `test_render_autolinks`: bare URL `https://example.com` in Markdown body → output contains `<a href="https://example.com">` (GFM autolink extension active).
    - `test_render_blockquote`: `> quoted text` → output contains `<blockquote>`.
    - `test_render_ordered_list`: `1. Item` → output contains `<ol>` and `<li>`.
    - `test_render_unordered_list`: `- Item` → output contains `<ul>` and `<li>`.
  - **Anchor deduplication (R4)**:
    - `test_anchor_dedup_same_level`: three `## Foo` → `foo`, `foo-1`, `foo-2`.
    - `test_anchor_dedup_mixed_levels`: `## Foo` then `### Foo` → `foo`, `foo-1`.
    - `test_anchor_no_collision_different_text`: `## Foo` then `## Bar` → `foo`, `bar`.
  - **Link rewriting**:
    - `test_link_rewrite_relative_md`: `[t](other.md)` → href passes through unchanged.
    - `test_link_rewrite_nested_relative`: `[t](../doc.md)` → resolved correctly relative to current file directory; result is root-relative.
    - `test_link_rewrite_external_unchanged`: `https://example.com/page.md` → unchanged.
    - `test_link_rewrite_fragment_only`: `#section` → unchanged.
    - `test_link_rewrite_fragment_preserved`: `doc.md#section` → fragment preserved.
    - `test_link_in_code_block_not_rewritten`: link in fenced code block → unchanged.
    - `test_link_rewrite_above_root_does_not_escape`: `[t](../../outside.md)` from `docs/subdir/page.md` → resulting href does not reference content above `serve_root`; either clamped to root or passed as-is for the path resolver to reject with 404 at request time.
  - **ETag (bd-22o)**:
    - `test_etag_deterministic`: same bytes → same ETag.
    - `test_etag_differs_on_change`: different bytes → different ETag.
- Dependency Links:
  - Parent: `bd-2f2`.
  - Depends on: `bd-1mz`, `bd-3kq`, `bd-mzl`, `bd-ezg`, `bd-1p6`.
- Acceptance Criteria:
  - All unit tests pass in CI (`cargo test`).
  - No test passes vacuously (each test has meaningful assertions).
  - All scenario names are descriptive enough to diagnose failure without reading the test body.
- Verification Steps:
  - Run `cargo test` in full; all listed test functions must exist and pass.
  - `cargo test 2>&1 | grep -E "test .* ok|FAILED"` must show all listed tests passing.

### `bd-39z` - Serve5-11: Integration and e2e test suite for mdmd serve

- Objective and Scope:
  - Provide comprehensive end-to-end coverage for serve behavior, security, rendering, navigation, cache semantics, MIME correctness, and legacy CLI compatibility.
- Implementation Details:
  - Integration tests live in `tests/serve_integration.rs` using `reqwest` (blocking) as HTTP client and `std::process::Command` to spawn the `mdmd` binary.
  - Each test function spawns the server on a random available port (to avoid conflicts), performs HTTP requests, and asserts on response status, headers, and body content.
  - Fixtures: create a temp dir with test Markdown files (table, task list, mermaid block, relative links, oversized stub, symlink pointing outside root) before each test; clean up after.
  - Detailed logging: each test prints `[TEST] scenario=<name> port=<port>` before making requests. Failed assertions must include the full response body and headers in the failure message.
  - **Coverage map** (each scenario is a separate `#[test]` function):
    - `test_serve_basic_html`: GET `/` returns 200, HTML body, `Content-Type: text/html`.
    - `test_serve_toc_present`: HTML response contains `<nav>` with at least one TOC `<a>` link.
    - `test_serve_raw_mode`: GET `/?raw=1` returns `Content-Type: text/plain` and Markdown source.
    - `test_serve_table_rendered`: HTML response contains `<table>` for a fixture with a GFM table.
    - `test_serve_task_list_rendered`: HTML response contains `<input type="checkbox">`.
    - `test_serve_mermaid_placeholder`: HTML response contains `class="mermaid"` for mermaid fixture.
    - `test_serve_mermaid_cdn_script`: HTML contains Mermaid CDN `<script>` tag.
    - `test_serve_local_md_link_resolves`: GET link target → 200.
    - `test_serve_traversal_denied`: GET `/../etc/passwd` → 404.
    - `test_serve_url_encoded_traversal_denied`: GET `/%2e%2e/etc/passwd` → 404.
    - `test_serve_symlink_escape_denied`: symlink in root → outside path → 404.
    - `test_serve_extensionless_resolves`: GET `/guide` (no extension) → 200 (resolves `guide.md`).
    - `test_serve_directory_readme_resolves`: GET `/subdir/` → 200 (resolves `subdir/README.md`).
    - `test_serve_directory_index_resolves`: GET `/subdir/` when `README.md` absent but `index.md` present → 200 (resolves `subdir/index.md`).
    - `test_serve_static_asset_image`: GET `/image.png` → 200, `Content-Type: image/png`.
    - `test_serve_nosniff_header`: all responses include `X-Content-Type-Options: nosniff`.
    - `test_serve_etag_present`: response includes `ETag` header.
    - `test_serve_304_on_etag_match`: GET with `If-None-Match: <etag>` → 304, empty body.
    - `test_serve_304_on_modified_since`: GET with `If-Modified-Since: <date newer than file mtime>` → 304, empty body.
    - `test_serve_compression_gzip`: GET with `Accept-Encoding: gzip` → `Content-Encoding: gzip`.
    - `test_serve_file_too_large`: oversized file (> 16 MB) → 413.
    - `test_serve_script_stripped`: page with `<script>` in Markdown → not in HTML response.
    - `test_serve_startup_stdout_format`: capture stdout, assert `root:`, `entry:`, `url:` lines.
    - `test_serve_assets_css`: GET `/assets/mdmd.css` → 200, `Content-Type: text/css`.
    - `test_serve_assets_js`: GET `/assets/mdmd.js` → 200, `Content-Type: text/javascript`.
    - `test_legacy_cli_tui_path`: spawn `mdmd <file>` (no subcommand) → exits without error (TUI mode dispatched, not serve mode). May require a non-interactive assertion (check process behavior, not terminal output).
    - `test_serve_graceful_shutdown`: send SIGINT to server process → exits with code 0.
  - Port selection: use `TcpListener::bind("127.0.0.1:0")` to get a free port before spawning the binary with `--port <free_port>` (and `--bind 127.0.0.1` to restrict to loopback).
  - Use `std::thread::sleep` with a reasonable startup wait (or retry loop with timeout) before making requests.
- Dependency Links:
  - Parent: `bd-2f2`.
  - Depends on: `bd-1mz`, `bd-3kq`, `bd-2n4`, `bd-ezg`, `bd-1p6`, `bd-2se`, `bd-22o`, `bd-30z`.
- Acceptance Criteria:
  - All integration/e2e test functions pass with `cargo test --test serve_integration`.
  - Traversal and symlink denial tests confirm no file content is leaked outside root.
  - Legacy CLI test confirms TUI path is not regressed.
  - Pre-Merge Operational Checklist items are covered by corresponding test functions.
- Verification Steps:
  - Run `cargo test --test serve_integration 2>&1`; all listed test names must appear as `ok`.
  - Failed tests must print `[TEST] scenario=<name>` plus full response context for diagnosis.
  - Run checklist: `cargo build --release`, `cargo clippy -- -D warnings`, `cargo fmt --check`, `cargo test`.

## Risks and Mitigations

### R1 – Symlink escape from serve_root
**Risk**: A symlink inside `serve_root` can point outside it. Canonical path normalization alone may not catch this if the symlink is followed after the containment check.
**Mitigation**: Resolve the final path with `std::fs::canonicalize` (which follows symlinks) and re-check containment against the canonicalized `serve_root`. Both the symlink target and `serve_root` must be canonicalized before comparison. Add a unit test with an in-tree symlink pointing to `/tmp`.

### R2 – URL-encoded traversal bypass
**Risk**: A request path like `/%2e%2e/etc/passwd` may survive naive normalization if decoding and normalization are applied out of order.
**Mitigation**: Decode percent-encoding first, then normalize with `Path::new` component stripping. Add dedicated unit tests for `%2e%2e`, `%2F`, and mixed-case encodings. Reject requests that still contain `..` components after decoding.

### R3 – Comrak HTML injection via embedded HTML blocks
**Risk**: By default comrak allows raw HTML passthrough in Markdown. Documents containing `<script>` or `<style>` tags will inject unescaped HTML into the page.
**Mitigation**: Enable `comrak::ComrakOptions::render.unsafe_` only intentionally. For phase 1, default to `unsafe_ = false` (raw HTML blocks stripped). Document the setting and add a test asserting `<script>` is stripped from rendered output.

### R4 – Anchor ID collision for duplicate headings
**Risk**: Multiple headings with identical text produce duplicate anchor IDs, breaking TOC links and `#fragment` navigation.
**Mitigation**: Implement a deduplication counter during heading extraction (e.g., `my-heading`, `my-heading-1`, `my-heading-2`). Add unit tests with repeated heading strings at same and different nesting levels.

### R5 – Unbounded file size in single-request read
**Risk**: Serving a multi-MB Markdown or binary file reads the entire file into memory per request with no size guard.
**Mitigation**: Add a configurable max file size (default 16 MB). Return `413 Content Too Large` for oversized files. Log the attempted path and size. Add integration test asserting 413 for oversized content.

### R6 – MIME type sniffing for non-Markdown static assets
**Risk**: Without explicit `Content-Type` headers, browsers may sniff MIME types incorrectly, enabling content-type confusion attacks.
**Mitigation**: Derive `Content-Type` from file extension using a static lookup (e.g., `.png` → `image/png`, `.svg` → `image/svg+xml`). Fall back to `application/octet-stream` for unknown extensions. Never let browsers sniff; set `X-Content-Type-Options: nosniff`. Add tests asserting correct MIME for common asset types.

### R7 – Tailscale subprocess output injection
**Risk**: Although `tailscale status --json` is a local privileged command, a corrupted or adversarial JSON payload could cause `serde_json` parsing errors that surface as panics if unwrap is used.
**Mitigation**: All tailscale JSON parsing must use `?`/`Result` propagation and treat parse failure as "no Tailscale" (silently skip, log at debug level). Add unit test for malformed JSON and empty output. No `unwrap()`/`expect()` on tailscale parsing paths.

### R8 – Port exhaust fallback fails silently under certain OS configurations
**Risk**: On some Linux configurations, non-root processes cannot bind ports below 1024, and OS errors other than EADDRINUSE may occur. The current plan only handles EADDRINUSE-style retry.
**Mitigation**: Only retry on `EADDRINUSE` (address already in use). For all other bind errors (permission denied, interface not found, etc.) fail immediately with the specific OS error. Add a unit test asserting non-EADDRINUSE errors cause immediate failure, not retry.

### R9 – Regression in legacy TUI path
**Risk**: Refactoring clap to support subcommands may silently break `mdmd <file>` positional argument parsing.
**Mitigation**: Integration test suite must include a specific `mdmd <file>` invocation that verifies the TUI path is dispatched (not serve). Include a CI step asserting the binary compiled from the feature branch still launches TUI mode for a bare file argument.

### R10 – New dependency `comrak` increases binary size and compile time
**Risk**: Adding `comrak` with full GFM feature flags may increase binary size significantly or introduce transitive dependencies with security issues.
**Mitigation**: Audit `comrak`'s feature flags and enable only needed ones (`gfm`, not experimental extensions). Pin to a specific version in `Cargo.toml`. After implementation, verify binary size delta is acceptable (target: ≤500 KB increase).

---

## Pre-Merge Operational Checklist

Before any bead is marked closed and before the feature branch is merged, the following checks must pass:

### Build and Static Analysis
- [ ] `cargo build` (debug) exits 0 with zero errors and zero warnings
- [ ] `cargo build --release` exits 0
- [ ] `cargo clippy -- -D warnings` exits 0 (no clippy lints suppressed without explicit reason)
- [ ] `cargo fmt --check` exits 0 (code is formatted)

### Test Gates
- [ ] `cargo test` passes in full (all unit and integration targets)
- [ ] Path traversal unit tests pass: URL-encoded `%2e%2e`, symlink escape, out-of-root absolute paths
- [ ] Legacy `mdmd <file>` smoke test passes (TUI path not regressed)
- [ ] Port retry unit tests pass for success/retry/max-failure cases
- [ ] Tailscale parsing unit tests pass for all four cases: valid, trailing-dot, empty, malformed
- [ ] HTML injection unit test passes: `<script>` stripped with `unsafe_ = false`
- [ ] Anchor deduplication unit tests pass
- [ ] File size limit (413) test passes
- [ ] MIME type unit tests pass for all listed extensions

### Runtime Smoke Tests (manual)
- [ ] `mdmd serve README.md` starts on port 3333 (or next available) and prints localhost URL
- [ ] Opening the URL in a browser renders a page with: visible TOC, code blocks, tables
- [ ] Scrolling the page highlights the active heading in the TOC
- [ ] Clicking a local `.md` link navigates to that page within the browser tab
- [ ] Requesting `/?raw=1` returns plaintext Markdown source
- [ ] `curl -I` response includes `ETag` and `Content-Encoding` (when compression supported)
- [ ] `curl -H 'If-None-Match: <etag>'` returns `304 Not Modified`
- [ ] Requesting `/../etc/passwd` returns 404 (not a path traversal)
- [ ] Ctrl+C in the terminal shuts down the server cleanly (no panic, clean exit)

### Security Spot-Check
- [ ] A Markdown file containing `<script>alert(1)</script>` is served with the script tag stripped or escaped
- [ ] A request for a path outside `serve_root` (via symlink) returns 404
- [ ] Static asset response includes `X-Content-Type-Options: nosniff` header
- [ ] File exceeding 16 MB returns 413

---

## Acceptance Criteria

- `mdmd <file>` continues to launch existing TUI behavior.
- `mdmd serve README.md` starts server and prints a usable URL.
- If port `3333` is occupied, server retries with incrementing ports and stops after 100 attempts with clear error.
- Non-EADDRINUSE bind errors cause immediate failure with OS error detail.
- Startup output always includes localhost URL, and includes Tailscale URL when available.
- Rendered HTML includes phase-1 feature matrix items (including tables and Mermaid placeholders).
- TOC is persistently visible and active heading updates while scrolling.
- Local Markdown links resolve only within `serve_root`; traversal attempts are denied.
- URL-encoded traversal (`%2e%2e`) attempts are denied.
- Symlink-based escape from `serve_root` is denied via post-canonicalize re-check.
- Static asset requests resolve only within `serve_root`; traversal attempts are denied.
- Files exceeding 16 MB return 413.
- All responses include `X-Content-Type-Options: nosniff` and correct `Content-Type`.
- HTTP responses support cache validation and compression middleware.
- Server shuts down cleanly on Ctrl+C.
