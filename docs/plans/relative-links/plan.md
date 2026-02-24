## Overview

Improve `mdmd serve <path>` so links in served Markdown resolve like local docs browsing instead of breaking when the entry page is rooted at `/`.

Primary goals:
- Keep relative links working for Markdown pages and local assets.
- Make the default URL include the entry path (for example `/playground/README.md`) rather than only a bare root URL.
- Provide a directory index page for navigable folders (for example `/playground/`).
- Return a useful error page for broken links, with context and recovery links.

Scope note:
- Relative link rewriting and fallback candidate resolution already exist (`rewrite_local_links()` in `html.rs`, `resolve_candidate()` in `serve.rs`). This plan focuses on validating and integrating that behavior with routing, startup URL UX, directory index, and error-page UX rather than re-implementing those resolvers.

## Decisions

1. Root behavior and startup URL
- `GET /` will render the `serve_root` directory index (same index behavior as other directory paths).
- Rationale: `serve_root` browsing should start at root without an extra redirect hop, and aligns with directory-first navigation.
- Startup output will print both:
  - `http://127.0.0.1:3333/playground/README.md` (primary)
  - `http://127.0.0.1:3333/` (root index)

2. Entry URL path computation
- Compute `entry_url_path` once at startup and store it in `AppState`.
- Algorithm:
  - canonicalize entry file path
  - strip `canonical_root` prefix
  - convert to forward-slash URL path
  - percent-encode each path segment
  - prepend `/`
- Entry outside `serve_root` remains rejected at startup (existing behavior).
- If user passes a directory entry, resolve it first (existing fallback rules), then compute URL from the resolved file (for example `/playground/README.md`).

3. Path model
- `serve_root` is current working directory for now (no `--root` flag in this change).
- Keep containment checks so requests cannot escape `serve_root`.
- Local-tool usability is prioritized, with basic traversal safety retained.

4. Directory index policy
- If request path is a directory and no preferred Markdown file exists, render directory index HTML.
- Sorting: directories first, then files; each group case-insensitive alphabetical.
- Hidden entries: exclude dotfiles/dotdirs by default.
- Symlinks: show if the resolved target remains under `serve_root`; omit otherwise.

5. Broken link page
- Missing path returns HTML 404 page.
- 404 content includes:
  - requested path
  - nearest existing parent directory
  - quick links: entry doc, root index, parent directory index
  - listing for nearest existing parent when available.

6. Assets and embedded resources
- Preserve existing embedded asset handling for `/assets/mdmd.css` and `/assets/mdmd.js` before document/file resolution logic.

## Workflow Diagram

```text
mdmd serve <entry_path>
  |
  +--> canonicalize cwd as serve_root
  |
  +--> canonicalize/resolve entry_path under serve_root
  |
  +--> compute + store entry_url_path in AppState
  |
  +--> start server
         |
         +--> GET / ------------------------------> render serve_root directory index
         |
         +--> GET /assets/mdmd.css|mdmd.js ------> serve embedded asset
         |
         +--> GET /<requested_path>
                |
                +--> sanitize + normalize path under serve_root
                |
                +--> resolve candidate in order (existing behavior):
                |      1) exact
                |      2) +.md (if extensionless)
                |      3) dir/README.md
                |      4) dir/index.md
                |
                +--> resolved markdown file? ---- yes --> render markdown HTML
                |                              |
                |                              no
                |
                +--> resolved non-markdown file? yes --> serve static asset
                |                                  |
                |                                  no
                |
                +--> is directory? -------------- yes --> render directory index page
                |                              |
                |                              no
                |
                +--> render rich 404 page with nearest-parent listing
```

## Current State (as of input commit)

The following items from this plan are **already implemented** and must not be re-implemented:

- `entry_url_path: String` field in `AppState` (`serve.rs:87-105`)
- `derive_entry_url_path()` at startup, including percent-encoding via `percent_encode_segment()` (`serve.rs:326-357`)
- `percent_encode_segment()` and `percent_decode()` utilities (`serve.rs:231-324`)
- Startup banner printing both primary entry URL (`url:`) and root index (`index:`) lines (`serve.rs:867-883`)
- `resolve_candidate()` with exact / extensionless / readme / index fallbacks (`serve.rs:359-408`)
- `rewrite_local_links()` rewriting AST link nodes to root-relative hrefs (`html.rs:278-307`)

**What remains to implement** (actual work for this plan):

- `GET /` currently serves entry content, not a directory index — the route handler must change.
- Directory index renderer (new function, new HTML template).
- Rich 404 renderer (new function, new HTML template).
- Test migrations and new test cases (see Step 6).

## Implementation Steps

1. ~~AppState and startup URL~~ — already done. Verify only.
- Confirm `AppState.entry_url_path` is set correctly and startup banner prints `url:` and `index:` lines.
- Run `cargo test serve_startup_stdout` to validate. If the test passes, proceed; do not touch this code.

2. Root route behavior
- Change `/` handler from serving entry content directly to rendering the `serve_root` directory index.
- Mark this as a behavioral change and update affected integration tests.

3. Resolver and link behavior validation (no re-implementation)
- Keep existing `resolve_candidate()` and `rewrite_local_links()` logic.
- Add/adjust tests to assert existing fallback and link rewrite behavior still holds with new routing.

4. Directory index renderer
- Add directory index HTML with breadcrumbs and policy above (sort, hidden filtering, symlink handling).
- Ensure href generation uses URL-safe percent-encoded segments.

5. 404 renderer
- Add HTML 404 template with nearest-parent recovery links and optional listing.

6. Test updates
- **Migrate** existing tests that assert `GET /` returns entry content:
  - `test_serve_basic_html` (line 366): update expected body to match directory index HTML, not markdown-rendered entry.
  - `test_serve_toc_present` (line 376): update or split so TOC assertion targets `GET <entry_url_path>`, not `GET /`.
  - Any other test hitting `GET /` and asserting markdown prose or TOC elements.
- **Add** assertions that `GET /` returns root directory index (status 200, `Content-Type: text/html`, contains listing of top-level entries).
- **Add/extend** integration tests for:
  - `GET <entry_url_path>` reaches entry content (status 200, rendered markdown).
  - Navigating from `/` root index link to entry document reaches entry content.
  - Nested relative Markdown links: a page at `/playground/sub/page.md` with `[x](../other.md)` resolves to `/playground/other.md`.
  - Directory index: status 200, breadcrumb contains correct segments, expected file/dir links present.
  - Hidden entries excluded from directory index (dotfile/dotdir not listed).
  - Symlink out of `serve_root` omitted from directory index.
  - Rich 404 response: status 404, nearest-parent link present, no crash on missing path.
- **Add** unit tests for `derive_entry_url_path()` covering: ASCII-only path, spaces in segment, Unicode in segment, path with single component, path equal to `serve_root` (edge: empty relative path).

## Explicitly Out Of Scope

- Adding a `--root` CLI override.
- Changing containment/security model beyond current local-tool checks.

## Risks and Compatibility Notes

### Breaking changes
- **`GET /` behavior regression** (high probability): At least `test_serve_basic_html` and `test_serve_toc_present` assert that `GET /` returns rendered markdown/TOC. These will fail immediately after Step 2. The test migration in Step 6 **must** be done in the same commit as Step 2 or the build will be in a broken-test state.
- **Startup banner line format**: `test_serve_startup_stdout_format` validates exact `url:` and `index:` line patterns. Do not change startup output format in Steps 2–5 without updating that test.

### HTML safety in generated pages
- **XSS in directory index and 404 page**: File names containing `<`, `>`, `"`, `&` must be HTML-escaped in display text and `href` attributes. Use a helper function for HTML escaping; do not use string interpolation unescaped. Failure to escape means a file named `<script>alert(1)</script>.md` could inject script tags into the index page.
- **href encoding in directory index**: Each path segment in generated hrefs must pass through `percent_encode_segment()` (already available). A filename containing `#`, `?`, `%`, or space breaks browser navigation if un-encoded.

### File-system edge cases
- **Directory read failure**: `tokio::fs::read_dir()` returns `io::Error` on permission-denied or I/O error. The directory index renderer must return a 500 error response with a plain message, not panic or hang.
- **Empty directory**: A directory with no entries (after dotfile filtering) must render a valid index page with an appropriate "empty" message rather than malformed HTML.
- **Very large directories**: No pagination is planned. Directories with thousands of visible entries will produce a large HTML response. This is acceptable for a local tool but should not cause a timeout or OOM; use streaming or collect the full entry list in memory with a reasonable cap (e.g., warn and truncate after 10 000 entries).
- **Nearest-parent walk for 404**: The ancestor-search loop must stop at `serve_root` and not ascend above it. Must handle the degenerate case where the requested path decodes to exactly `serve_root` or an empty relative path.

### Symlink handling
- **Containment re-check after canonicalize**: When listing a directory, each symlink entry must have its resolved target checked against `canonical_root` before inclusion. The existing `canonical_root`-prefix containment pattern (already used in the request handler) should be reused.
- **Symlink cycle safety**: `tokio::fs::canonicalize()` resolves the full chain and will surface an `io::Error` on OS-level cycle detection. Treat that error as "omit entry from listing" rather than propagate it.

### Behavioral edge cases
- **Entry at `serve_root` root**: If a user runs `mdmd serve README.md` from the same directory containing `README.md`, `entry_url_path` is `/README.md` and `GET /` renders the top-level directory index (not the README). This is intentional but differs from the pre-change behavior where `GET /` showed the README. Verify the startup `url:` line correctly prints `/README.md` in this case.
- **Non-markdown static files at explicit paths**: `GET /image.png` should still serve the raw bytes via the existing static asset fallback. The directory index path must not intercept non-directory paths.

## Validation Strategy

Run after each implementation step before moving to the next:

| After step | Command | Expected outcome |
|---|---|---|
| 1 (verify) | `cargo test serve_startup_stdout` | Test passes unchanged |
| 2 | `cargo build` | Compiles without errors or warnings |
| 2 + 6 (migrate tests) | `cargo test` | No test failures |
| 4 | `cargo test serve_dir` (or new test) | Directory index returns 200 with HTML listing |
| 5 | `cargo test serve_404` (or new test) | 404 response contains nearest-parent link |
| All | `cargo test` | Full suite green |
| All | `cargo run -- serve playground/README.md` (manual) | Browser opens; relative links navigate correctly; `/` shows dir index; broken link shows 404 page |

## Operational Checks Before Starting

1. Run `cargo test` on the input commit and record the passing test count as baseline.
2. Confirm `entry_url_path` is set: add a temporary `eprintln!` in a test or inspect the `test_serve_startup_stdout_format` assertions to verify the field is correct.
3. Confirm `GET /` currently returns entry content (not a directory index) so the behavioral gap is understood before changing it.
