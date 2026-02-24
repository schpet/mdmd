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
  1) decode + normalize request path, reject traversal/out-of-root;
  2) try exact path, then `path + ".md"` when extensionless;
  3) if directory, try `README.md` then `index.md`.
- If resolved file is Markdown, render as page; otherwise serve as static asset.
- This preserves relative links across nested docs while maintaining root containment checks.

7. Port selection policy
- Default start port: `3333`.
- Increment by 1 until bind succeeds, with max attempts (`100`).
- Fail with clear error after max attempts (no unbounded loop).

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
2. Server binds on preferred port with bounded retries.
3. Startup URL is printed with Tailscale-aware host detection.
4. `GET /` (or routed Markdown path) reads and renders Markdown server-side.
5. HTML shell includes persistent TOC + embedded/served CSS/JS assets.
6. Client JS performs TOC active-heading highlighting and Mermaid hydration.
7. Responses include cache validators (`ETag`, `Last-Modified`) and compression where supported.

## Concurrency and Performance

- Use immutable shared app state via `Arc` (serve root, initial file, config).
- Keep per-request behavior simple, but add low-risk web performance defaults in phase 1:
  - cache validators (`ETag`/`Last-Modified`) to enable fast 304 revalidation;
  - response compression (gzip/brotli) via middleware.
- Keep main content SSR; JS is limited to progressive enhancement.

## Graceful Shutdown

- Use `tokio::signal::ctrl_c()` for clean shutdown on SIGINT.
- Ensure listener/task exits cleanly without panics on Ctrl+C.

## Testing Strategy

1. Unit tests
- Port selection retry logic (success, retry, max-attempt failure).
- Tailscale JSON parsing (valid name, trailing dot, empty, malformed).
- Path containment checks for local link security.
- Markdown link rewrite rules for `.md` links.

2. Integration tests
- `mdmd serve <file>` serves HTML with TOC present.
- Local `.md` link request resolves inside root and is blocked outside root.
- Extensionless and directory markdown route resolution behaves as specified.
- Non-markdown local assets are served from within `serve_root`.
- Mermaid block emits expected placeholder markup.
- Legacy `mdmd <file>` invocation still works.

## Bead Decomposition (Gate A)

Top-level feature:
- `bd-39c` - Serve5: Implement `mdmd serve` phase-1 web server

Dependency execution order (blocks):
1. `bd-1jy` -> CLI compatibility foundation
2. `bd-23e` -> server lifecycle and bind policy
3. `bd-3to` -> tailscale-aware startup URLs
4. `bd-281` -> SSR comrak rendering + heading extraction
5. `bd-58o` -> HTML shell + TOC + assets + `?raw=1`
6. `bd-9ty` -> secure serve-root path resolver
7. `bd-2vq` -> markdown local-link rewriting
8. `bd-2f4` -> Mermaid placeholders + hydration
9. `bd-26h` -> cache validators + compression
10. `bd-ljy` -> startup/page-context UX polish
11. `bd-1pl` -> unit test coverage
12. `bd-3hj` -> integration/e2e verification

Beads (self-contained implementation contracts):

### `bd-1jy` - Serve5-01: CLI subcommands with legacy compatibility

- Objective and Scope:
  - Preserve `mdmd <file>` TUI behavior while adding explicit `mdmd serve <file>` and `mdmd view <file>`.
- Implementation Details:
  - Refactor clap to support subcommands plus legacy positional compatibility.
  - Ensure `view` maps to legacy TUI path and `serve` dispatches new web server path.
- Dependency Links:
  - Parent: `bd-39c`.
  - Blocks: `bd-23e`, `bd-281`.
- Acceptance Criteria:
  - Legacy invocation remains unchanged.
  - `view`/`serve` routes dispatch correctly and help text reflects behavior.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit parse matrix for legacy/subcommand modes.
  - Integration smoke asserts command dispatch.
  - Startup diagnostics indicate selected mode.

### `bd-23e` - Serve5-02: Server bootstrap, bounded port retry, graceful shutdown

- Objective and Scope:
  - Implement axum/tokio server startup, default port `3333`, bounded retry (`100` attempts), and Ctrl+C shutdown.
- Implementation Details:
  - Bind loop increments by 1 on failure and exits with explicit error when exhausted.
  - Keep default bind on loopback unless explicit `--bind` override.
  - Add `tokio::signal::ctrl_c()` shutdown path.
- Dependency Links:
  - Parent: `bd-39c`.
  - Depends on: `bd-1jy`.
  - Blocks: `bd-3to`, `bd-9ty`, `bd-26h`, `bd-ljy`, test beads.
- Acceptance Criteria:
  - Port policy works exactly as specified.
  - Clean shutdown on SIGINT without panic.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit tests for retry success/retry/failure.
  - Integration test with occupied 3333 verifies increment.
  - Logs include bind attempts, final port, shutdown completion.

### `bd-3to` - Serve5-03: Tailscale-aware startup URL output

- Objective and Scope:
  - Always print localhost URL; conditionally print tailscale URL from `.Self.DNSName`.
- Implementation Details:
  - Parse `tailscale status --json` safely.
  - Trim trailing `.` from DNSName; ignore empty/malformed values.
- Dependency Links:
  - Parent: `bd-39c`.
  - Depends on: `bd-23e`.
- Acceptance Criteria:
  - Startup never fails on tailscale parsing errors.
  - URL output policy is deterministic and complete.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit tests: valid, trailing-dot, empty, malformed inputs.
  - Integration startup-output assertions with stubbed tailscale responses.
  - Logs indicate detection/skipped state.

### `bd-281` - Serve5-04: Comrak SSR renderer with heading extraction

- Objective and Scope:
  - Implement serve-only markdown renderer using comrak with GFM features and heading extraction.
- Implementation Details:
  - Enable tables/task lists/autolinks/strikethrough/fenced code.
  - Extract headings and anchor IDs from AST during render.
  - Keep TUI parse/render path untouched.
- Dependency Links:
  - Parent: `bd-39c`.
  - Depends on: `bd-1jy`.
  - Blocks: `bd-58o`, `bd-2vq`, `bd-2f4`, `bd-26h`, test beads.
- Acceptance Criteria:
  - HTML covers phase-1 markdown feature matrix.
  - TOC-compatible heading metadata is produced.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit tests for GFM outputs and anchor determinism.
  - Render debug logs include file path and heading count.

### `bd-58o` - Serve5-05: HTML shell, sticky TOC UI, assets, and raw mode

- Objective and Scope:
  - Build page shell with persistent TOC, active heading highlight, static assets, and `?raw=1`.
- Implementation Details:
  - Provide header context with current file path.
  - Serve `/assets/mdmd.css` and `/assets/mdmd.js`.
  - Add IntersectionObserver logic for active section and Mermaid init entrypoint.
- Dependency Links:
  - Parent: `bd-39c`.
  - Depends on: `bd-281`.
  - Blocks: `bd-2f4`, `bd-ljy`, `bd-3hj`.
- Acceptance Criteria:
  - TOC visible and active section updates while scrolling.
  - Raw mode returns markdown source.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Integration tests for TOC/assets/raw-mode response behavior.
  - Browser e2e verifies active-heading class changes.
  - Request logs include raw/rendered mode.

### `bd-9ty` - Serve5-06: Secure serve_root path resolution for markdown and assets

- Objective and Scope:
  - Enforce canonical `serve_root` containment and robust local path resolution.
- Implementation Details:
  - Decode + normalize request path; block traversal/out-of-root.
  - Resolve exact path, extensionless `.md`, and directory `README.md`/`index.md`.
  - Render markdown; serve non-markdown files as static responses.
- Dependency Links:
  - Parent: `bd-39c`.
  - Depends on: `bd-23e`.
  - Blocks: `bd-2vq`, `bd-26h`, test beads.
- Acceptance Criteria:
  - No out-of-root access; valid in-root resolution works across fallback cases.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit tests for normalization/containment edge cases and fallback ordering.
  - Integration tests for in-root success and traversal denial.
  - Logs include normalized path and resolution branch/deny reason.

### `bd-2vq` - Serve5-07: Local markdown link rewriting for web navigation

- Objective and Scope:
  - Rewrite local markdown links for served web navigation while preserving external URLs.
- Implementation Details:
  - Rewrite relative `.md` and extensionless targets to resolver-compatible routes.
  - Preserve query strings and fragments.
- Dependency Links:
  - Parent: `bd-39c`.
  - Depends on: `bd-281`, `bd-9ty`.
  - Blocks: `bd-1pl`, `bd-3hj`.
- Acceptance Criteria:
  - Internal doc links navigate correctly across nested files.
  - No unsafe escaping via rewritten links.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit rewrite matrix (relative/nested/fragment/query/external).
  - Integration navigation tests across fixtures.
  - Render logs include rewrite counts and skip reasons.

### `bd-2f4` - Serve5-08: Mermaid placeholder SSR and client hydration

- Objective and Scope:
  - Support Mermaid in phase 1 with SSR placeholders and client hydration only.
- Implementation Details:
  - Emit mermaid placeholder markup in SSR output.
  - Initialize Mermaid in shipped JS without node/headless-browser dependency.
- Dependency Links:
  - Parent: `bd-39c`.
  - Depends on: `bd-281`, `bd-58o`.
  - Blocks: `bd-3hj`.
- Acceptance Criteria:
  - Mermaid fences render placeholder markup and hydrate client-side.
  - Non-Mermaid fences are unchanged.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit tests for fence classification/markup output.
  - Integration response checks for placeholder presence.
  - Browser e2e validates hydration and no console errors.

### `bd-26h` - Serve5-09: Cache validators and compression middleware

- Objective and Scope:
  - Add `ETag`/`Last-Modified` and compression middleware to improve serve performance.
- Implementation Details:
  - Implement conditional request handling and 304 responses.
  - Enable gzip/brotli for eligible responses.
- Dependency Links:
  - Parent: `bd-39c`.
  - Depends on: `bd-23e`, `bd-9ty`, `bd-281`.
  - Blocks: `bd-3hj`.
- Acceptance Criteria:
  - Validators are present and conditional revalidation works.
  - Compression is applied when negotiated.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Unit tests for validator comparison logic.
  - Integration tests for `If-None-Match`/`If-Modified-Since` and encoding negotiation.
  - Access logs include cache/compression outcomes.

### `bd-ljy` - Serve5-12: Startup and page-context UX polish

- Objective and Scope:
  - Finalize startup/page context details: `serve_root`, entry file, URLs, and current page path.
- Implementation Details:
  - Ensure startup output format is stable and machine-assertable.
  - Ensure page header displays current file context.
- Dependency Links:
  - Parent: `bd-39c`.
  - Depends on: `bd-23e`, `bd-58o`.
  - Blocks: `bd-3hj`.
- Acceptance Criteria:
  - Startup output and page header include required context fields.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Integration assertions for startup lines and header context.
  - Logs remain structured for bind/path troubleshooting.

### `bd-1pl` - Serve5-10: Unit test coverage for serve internals

- Objective and Scope:
  - Consolidate deterministic unit coverage for serve internals.
- Implementation Details:
  - Cover retry logic, tailscale parsing, heading extraction, path safety, and link rewriting.
- Dependency Links:
  - Parent: `bd-39c`.
  - Depends on: `bd-23e`, `bd-3to`, `bd-281`, `bd-9ty`, `bd-2vq`.
- Acceptance Criteria:
  - All critical branch logic has direct unit tests and passes in CI.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Run unit test targets.
  - Assertions expose scenario-specific failure context.

### `bd-3hj` - Serve5-11: Integration and e2e test suite for mdmd serve

- Objective and Scope:
  - Provide end-to-end coverage for serve behavior, security, rendering, navigation, and compatibility.
- Implementation Details:
  - Validate TOC rendering, markdown navigation, traversal denial, asset serving, mermaid placeholders, raw mode, cache semantics, and legacy CLI behavior.
  - Include startup output assertions for localhost and optional tailscale URL.
- Dependency Links:
  - Parent: `bd-39c`.
  - Depends on: `bd-23e`, `bd-3to`, `bd-58o`, `bd-9ty`, `bd-2vq`, `bd-2f4`, `bd-26h`, `bd-ljy`.
- Acceptance Criteria:
  - Integration/e2e suite validates all phase-1 acceptance criteria.
- Verification Steps (unit and e2e requirements, with logging expectations):
  - Run full integration/e2e targets and confirm pass.
  - Ensure tests assert logs/output for startup, routing decisions, and denial cases.

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

---

## Acceptance Criteria

- `mdmd <file>` continues to launch existing TUI behavior.
- `mdmd serve README.md` starts server and prints a usable URL.
- If port `3333` is occupied, server retries with incrementing ports and stops after 100 attempts with clear error.
- Startup output always includes localhost URL, and includes Tailscale URL when available.
- Rendered HTML includes phase-1 feature matrix items (including tables and Mermaid placeholders).
- TOC is persistently visible and active heading updates while scrolling.
- Local Markdown links resolve only within `serve_root`; traversal attempts are denied.
- Static asset requests resolve only within `serve_root`; traversal attempts are denied.
- HTTP responses support cache validation and compression middleware.
- Server shuts down cleanly on Ctrl+C.
