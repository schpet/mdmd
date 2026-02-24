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

## Implementation Steps

1. AppState and startup URL
- Add `entry_url_path: String` to `AppState`.
- Compute `entry_url_path` at startup from `entry_file` relative to `canonical_root` with percent-encoding.
- Update startup output to include both primary entry URL and root index URL.

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
- Update current tests that expect `GET /` to return entry content.
- Add assertions that `GET /` returns root directory index content.
- Add/extend integration tests for:
  - navigating from `/` root index to entry document link reaches entry content
  - nested relative Markdown links
  - directory index output (breadcrumbs + expected links)
  - hidden entries excluded
  - rich 404 response with nearest-parent links/listing
- Add targeted unit tests for entry URL path computation (including spaces/unicode percent-encoding).

## Explicitly Out Of Scope

- Adding a `--root` CLI override.
- Changing containment/security model beyond current local-tool checks.

## Risks and Compatibility Notes

- `/` semantics change from direct entry rendering to root directory index; this can break existing clients/tests that assumed entry content at `/`.
- Keeping startup output pathful URL primary preserves canonical entry navigation while root remains browsable.
- Directory index omission rules (hidden files and out-of-root symlinks) should be documented in help/docs after implementation.
