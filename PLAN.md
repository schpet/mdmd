# Plan: `mdmd html`

## Goal

Add `mdmd html <file> [-o output.html]` that writes a single HTML file to disk.

The exported page should match the served page as closely as possible, but it must be valid as a standalone `file://` document:

- no `/assets/...` references
- no dependency on the mdmd HTTP server
- no external references except ones we already intentionally rely on in serve mode

For v1, that means:

- inline mdmd CSS and mdmd JS
- keep the existing pinned Mermaid CDN script as the only allowed external reference
- avoid shipping serve-only UI that would be broken or misleading in a local file

## What the code does today

- `src/html.rs`
  - `render_markdown()` parses markdown, rewrites local links for HTTP serving, and returns `(body_html, headings)`
  - `build_page_shell()` wraps that body in the full served HTML page
- `src/web_assets.rs`
  - mdmd CSS and JS are already embedded at compile time via `include_str!`
- `src/serve.rs`
  - serve mode extracts frontmatter, renders markdown, builds the page shell, and serves `/assets/mdmd.css` and `/assets/mdmd.js`
- `src/main.rs`
  - CLI dispatch is centralized here; there is no export subcommand yet

## Corrections To The Current Draft

- The export problem is not only an asset problem.
  - `render_markdown()` currently rewrites relative links and image URLs into root-relative HTTP paths like `/docs/page.md`.
  - That is correct for `serve`, but wrong for an exported file opened directly from disk.
- `build_page_shell()` already lives in `src/html.rs`.
  - The plan should keep `src/serve.rs` thin and avoid duplicating shell generation there.
- Backlinks are not “already handled” for export.
  - The current backlinks HTML emits root-relative hrefs, which would be broken in a standalone exported file.
  - Export must either omit backlinks in v1 or add a separate file-relative backlink href policy.
- Serve-only controls should not be carried into export unchanged.
  - `?raw=1`
  - change notice / reload button
  - freshness meta tags used by the polling JS

## Proposed Design

### CLI

```bash
mdmd html <file> [-o <output>]
```

- `<file>`: markdown file, validated with the same extension rules as other file-based commands
- `-o`, `--output`: destination path
- default output: `<input-stem>.html` next to the source file
- stdout: print the written path

### Rendering Model

Do not fork the rendering pipeline into separate serve/export implementations.

Instead, introduce explicit render targets and thread them through the existing HTML path:

- `RenderTarget::Serve`
- `RenderTarget::Html`

This target should control two things:

1. Link policy in `render_markdown()`
2. Asset/shell policy in `build_page_shell()`

That keeps one source of truth for markdown rendering and one source of truth for page shell structure.

### Link Policy

This is the key missing piece in the current draft.

`render_markdown()` currently rewrites local relative links and images into root-relative HTTP paths for serve mode. Export cannot reuse that behavior.

Proposed behavior:

- `RenderTarget::Serve`
  - keep the current root-relative rewrite behavior
- `RenderTarget::Html`
  - preserve authored relative URLs as-is
  - keep external URLs, absolute URLs, fragment-only links, and `mailto:` unchanged as today

Rationale:

- preserving relative paths makes an exported page behave sensibly when opened from disk
- rewriting to `.html` should be deferred until there is a real multi-file export feature
- this avoids inventing partially correct file-path rewrite rules now

Implication for v1:

- links to other markdown files will still point to `.md` files, not exported `.html` siblings
- that is acceptable for single-file export

### Shell / Asset Policy

`build_page_shell()` should become target-aware.

For `RenderTarget::Html`:

- inline `web_assets::CSS` as `<style>...</style>`
- inline `web_assets::JS` as `<script>...</script>`
- keep the tiny theme/full-width/indent init scripts inline as they already are
- keep the pinned Mermaid CDN script as-is for v1

Serve-only elements to remove from export:

- raw-source link (`?raw=1`)
- change-notice banner and reload button
- `mdmd-mtime` meta tag
- `mdmd-path` meta tag
- page title suffix `· mdmd serve`

Export title should be neutral:

- `<title>{title} · mdmd</title>`

### Backlinks

Do not claim full served-page parity here yet.

Backlinks should be omitted in export v1.

Reason:

- current backlink hrefs are root-relative serve URLs
- emitting them unchanged in an exported file would create obviously broken links
- adding file-relative backlink href generation is a separate design problem

Implementation approach:

- pass `backlinks: &[]` in export mode
- leave multi-file/export-corpus backlink support for a follow-up

### File Organization

Expected code changes:

1. `src/main.rs`
   - add `Commands::Html`
   - add `DispatchMode::Html`
   - wire dispatch to `run_html(...)`

2. `src/html.rs`
   - add a target enum such as `RenderTarget`
   - make `render_markdown()` target-aware for link rewriting
   - make `build_page_shell()` target-aware for linked-vs-inlined assets and serve-only UI

3. `src/html_export.rs` (new)
   - read the source file
   - extract frontmatter
   - call `render_markdown(..., RenderTarget::Html, ...)`
   - call `build_page_shell(..., RenderTarget::Html, ...)`
   - write the HTML file

4. `src/main.rs`
   - add `mod html_export;`

No new dependency is needed for v1.

## Suggested API Shape

Keep the target explicit rather than using a boolean.

Example:

```rust
pub enum RenderTarget {
    Serve,
    Export,
}
```

Likewise, avoid `inline_assets: bool`.

The shell differences are not just asset inlining; export also removes serve-only controls and meta tags. An enum keeps that honest.

## Testing

Add focused tests for both target modes.

### `src/html.rs` unit tests

- serve mode still emits:
  - `/assets/mdmd.css`
  - `/assets/mdmd.js`
  - Mermaid CDN script
  - raw-source link
  - change notice
  - freshness meta tags when present
- export mode emits:
  - inline `<style>` containing mdmd CSS
  - inline `<script>` containing mdmd JS
  - Mermaid CDN script
  - no `/assets/mdmd.css`
  - no `/assets/mdmd.js`
  - no `?raw=1`
  - no change notice
  - no `mdmd-mtime`
  - no `mdmd-path`
  - title suffix `· mdmd`
- link rewriting:
  - serve mode rewrites local relative links to root-relative URLs
  - export mode preserves local relative links and image paths

### CLI / integration tests

- `mdmd html README.md`
  - exits successfully
  - writes `README.html` by default
  - output contains full HTML document
- `mdmd html README.md -o out/custom.html`
  - writes to the requested path
- output HTML for a page with Mermaid
  - still includes the pinned Mermaid CDN script

## Implementation Order

1. Add `RenderTarget` in `src/html.rs`
2. Update `render_markdown()` to make link rewriting target-aware
3. Update `build_page_shell()` to make asset/UI/meta behavior target-aware
4. Add `src/html_export.rs` and implement `run_html()`
5. Add CLI parsing and dispatch in `src/main.rs`
6. Add tests for export output and preserve existing serve behavior

## Non-Goals For V1

- multi-file export
- rewriting markdown links to sibling `.html` files
- offline Mermaid without the existing CDN dependency
- backlinks in exported files
- a generic asset bundling system

## Follow-Ups

- multi-file export that rewrites local `.md` links to exported `.html` files
- optional fully offline Mermaid bundling
- export-time backlinks with file-relative hrefs
