# YAML Frontmatter Draft B

## Overview

Add first-class YAML frontmatter rendering to the `mdmd serve` web UI using a table-based layout with collapsible display and client-side toggle state. Like Draft A, the feature is web-UI-only and server-side rendered, but Draft B takes a different approach to the data model, rendering shape, and user interaction:

- **Flat key-value table** instead of a definition-list panel. Frontmatter is rendered as a compact `<table>` with two columns (key, value), making metadata scannable at a glance without consuming excessive vertical space.
- **Collapsible by default** for documents with more than 4 frontmatter fields. A disclosure toggle lets readers expand the full metadata when they want it and collapse it when they don't. The collapsed/expanded preference is persisted in `localStorage`.
- **Unified extraction function** that operates on raw bytes before any markdown parsing. The extractor returns an opaque `FrontmatterBlock` that carries both the parsed mapping and the original byte range, allowing the page shell to decide how to render while keeping `render_markdown` completely unaware of frontmatter.
- **No new crate-level dependency**. Instead of adding `serde_yaml`, use the `serde_yml` crate (the maintained successor to `serde_yaml`, which has been deprecated). Parse the delimited slice into `serde_yml::Value`, convert to the internal model, and discard the `Value` immediately.
- **Graceful degradation** preserved: malformed YAML, missing closing delimiter, non-mapping root, or empty frontmatter all fall back to current behavior with zero visual change.

Design principles:

- The extraction boundary sits in `serve.rs` (the call site), not inside `html.rs`. This keeps `html.rs` a pure markdown→HTML converter with no YAML knowledge.
- The table is rendered outside the `<main class="content">` element to avoid polluting the heading observer's scope entirely, rather than relying on "no heading tags inside the panel" as a convention.
- `?raw=1` behavior is unchanged.
- The TUI path (`parse.rs`, `render.rs`) is not touched.

## Workflow Diagram

```text
GET /path/to/file.md
  │
  ├─ read file to String
  │
  ├─ ?raw=1 ──────────────> return original source as text/plain
  │
  ├─ frontmatter::extract(source: &str)
  │    │
  │    ├─ file does not start with "---\n"
  │    │    └─> ExtractResult { body: <original>, meta: None }
  │    │
  │    ├─ no closing "---" or "..." found
  │    │    └─> ExtractResult { body: <original>, meta: None }
  │    │
  │    ├─ YAML slice parses but root is not a mapping
  │    │    └─> ExtractResult { body: <original>, meta: None }
  │    │
  │    └─ valid YAML mapping
  │         └─> ExtractResult {
  │               body: <source after closing delimiter>,
  │               meta: Some(FrontmatterMeta { fields, title })
  │             }
  │
  ├─ html::render_markdown(body, file_path, serve_root)
  │    └─> (html_body, headings)
  │
  ├─ html::build_page_shell(html_body, headings, ctx)
  │    │
  │    │  ctx now includes: frontmatter: Option<&FrontmatterMeta>
  │    │
  │    ├─ <title> = ctx.frontmatter.title || first H1 || file stem
  │    │
  │    ├─ <header> (existing controls)
  │    │
  │    ├─ <section class="frontmatter-table"> (if meta present)
  │    │    ├─ disclosure toggle (if > 4 fields)
  │    │    └─ <table> with key/value rows
  │    │
  │    ├─ <div class="page-body">
  │    │    ├─ <nav class="toc-sidebar"> (headings only)
  │    │    └─ <main class="content"> (html_body)
  │    │
  │    └─ <section class="backlinks-panel"> (if any)
  │
  └─ return text/html with ETag + Last-Modified
```

## Module Boundaries

Draft B introduces a new `src/frontmatter.rs` module rather than adding extraction logic to `html.rs`:

| Module | Responsibility |
|---|---|
| `src/frontmatter.rs` (new) | `extract()`: byte-level delimiter detection, YAML parsing, model conversion. No HTML knowledge. |
| `src/serve.rs` | Calls `frontmatter::extract()` before `html::render_markdown()`. Passes the stripped body to the renderer and the parsed metadata to the shell builder. |
| `src/html.rs` | Receives `Option<&FrontmatterMeta>` in `PageShellContext`. Renders the metadata table in `build_page_shell`. Never sees raw YAML. |
| `src/assets/mdmd.css` | Styles for `.frontmatter-table`, collapse toggle, key/value cells. |
| `src/assets/mdmd.js` | Disclosure toggle persistence in `localStorage` (`mdmd-fm-collapsed`). |

## Data Model

```rust
// src/frontmatter.rs

/// Result of attempting frontmatter extraction.
pub struct ExtractResult<'a> {
    /// Markdown body with frontmatter stripped (or original source if none found).
    pub body: &'a str,
    /// Parsed metadata, present only when a valid YAML mapping was extracted.
    pub meta: Option<FrontmatterMeta>,
}

/// Parsed frontmatter metadata ready for rendering.
pub struct FrontmatterMeta {
    /// Ordered key-value pairs preserving YAML source order.
    pub fields: Vec<(String, MetaValue)>,
    /// Cached extraction of the `title` field if present and scalar.
    pub title: Option<String>,
}

/// A frontmatter value normalized for display.
pub enum MetaValue {
    /// Scalar: string, number, boolean, date, or null — all kept as their YAML string form.
    Scalar(String),
    /// Null value (explicit `~` or `null` keyword).
    Null,
    /// Flat list of scalars, rendered as comma-separated pills.
    List(Vec<String>),
    /// Nested mapping, rendered as an indented sub-table.
    Nested(Vec<(String, MetaValue)>),
}
```

The model intentionally avoids `serde::Deserialize` — it converts from `serde_yml::Value` via a simple recursive function, which means the frontmatter schema is never constrained and any valid YAML mapping is displayable.

## Rendering Shape

```html
<!-- Placed BEFORE .page-body, OUTSIDE <main class="content"> -->
<section class="frontmatter-table" aria-label="Document metadata">
  <div class="fm-header">
    <span class="fm-label">Metadata</span>
    <!-- Only present when field count > 4 -->
    <button class="fm-toggle" aria-expanded="true">collapse</button>
  </div>
  <table class="fm-fields">
    <tbody>
      <tr>
        <td class="fm-key">title</td>
        <td class="fm-val">Example document</td>
      </tr>
      <tr>
        <td class="fm-key">tags</td>
        <td class="fm-val">
          <span class="fm-pill">rust</span>
          <span class="fm-pill">markdown</span>
        </td>
      </tr>
      <tr>
        <td class="fm-key">draft</td>
        <td class="fm-val fm-null">null</td>
      </tr>
    </tbody>
  </table>
</section>
```

Key rendering rules:

- Scalars: plain text, HTML-escaped.
- `Null`: muted italic `null` label with `.fm-null` class.
- `List`: inline `<span class="fm-pill">` elements separated by spaces.
- `Nested`: recursive `<table>` inside the `<td>`, indented with a left border. Nesting depth capped at 3 levels; deeper structures render as a YAML-formatted `<pre>` block.
- Boolean/number/date values are not coerced — they display exactly as written in the YAML source.

## Implementation Steps

1. **Add `serde_yml` dependency** to `Cargo.toml`.
   - `serde_yml = "0.0.12"` (or latest stable).
   - No other new dependencies.

2. **Create `src/frontmatter.rs`** with:
   - `extract(source: &str) -> ExtractResult` — delimiter detection + parse + model conversion.
   - `fn yaml_value_to_meta(value: serde_yml::Value) -> Option<FrontmatterMeta>` — recursive converter.
   - Unit tests for: no frontmatter, unterminated block, malformed YAML, non-mapping root, valid mapping with scalars/lists/nested, `title` extraction, empty frontmatter block (`---\n---`).

3. **Wire extraction into `src/serve.rs`** (around line 1316):
   - Call `frontmatter::extract(&content)` before `render_markdown`.
   - Pass `result.body` to `render_markdown` instead of `&content`.
   - Pass `result.meta.as_ref()` into `PageShellContext`.

4. **Extend `PageShellContext` and `build_page_shell` in `src/html.rs`**:
   - Add `pub frontmatter: Option<&'a FrontmatterMeta>` to `PageShellContext`.
   - Update title selection: `ctx.frontmatter.and_then(|f| f.title.as_deref()) || first_h1 || file_stem`.
   - Emit `<section class="frontmatter-table">` before `.page-body` div when metadata is present.
   - Recursive HTML builder for `MetaValue` with nesting-depth guard.

5. **Add CSS for the metadata table** in `src/assets/mdmd.css`:
   - `.frontmatter-table`: full-width block above `.page-body`, `margin-bottom: 1rem`, `border-bottom` separator.
   - `.fm-fields`: borderless two-column table with compact padding.
   - `.fm-key`: `font-family: monospace`, `white-space: nowrap`, `color: var(--color-text-muted)`, right-aligned.
   - `.fm-val`: normal text, wrapping allowed.
   - `.fm-pill`: inline-block, rounded, `background: var(--color-surface)`, small padding.
   - `.fm-null`: italic, `color: var(--color-text-subtle)`.
   - Collapse state: `.frontmatter-table.collapsed .fm-fields { display: none; }`.
   - Dark mode inherits from existing CSS custom properties — no new color tokens needed.

6. **Add JS for collapse toggle** in `src/assets/mdmd.js`:
   - Read `localStorage.getItem('mdmd-fm-collapsed')`.
   - On toggle click: flip class, update `aria-expanded`, write to localStorage.
   - ~15 lines of JS inside the existing IIFE.

7. **Add integration tests** in `tests/serve_integration.rs`:
   - Document with valid frontmatter: metadata table present, YAML block absent from `<main>`.
   - Document with valid frontmatter + `?raw=1`: original source returned verbatim.
   - Document without frontmatter: no `.frontmatter-table` section in output.
   - Document with malformed frontmatter: page renders normally, no metadata table.

## Differences from Draft A

| Aspect | Draft A | Draft B |
|---|---|---|
| **Rendering element** | `<dl>` definition list | `<table>` with key/value columns |
| **Placement** | Inside `<main class="content">` | Before `.page-body`, outside `<main>` |
| **Extraction location** | `html.rs` | New `frontmatter.rs` module |
| **Collapsibility** | Not addressed | Collapsible with localStorage persistence |
| **YAML crate** | `serde_yaml` (deprecated) | `serde_yml` (maintained successor) |
| **List rendering** | "Compact pills or stacked inline items" | Explicit pill `<span>` elements |
| **Nesting depth** | Unlimited recursive rendering | Capped at 3 levels; deeper → `<pre>` fallback |
| **JS changes** | None | ~15 lines for collapse toggle |

## Risks and Guardrails

- **Frontmatter parse failures are invisible**: if extraction returns `None`, the page renders exactly as today. No error banner, no console warning. This is intentional — users editing markdown should not be distracted by YAML parse noise.
- **Placing the panel outside `<main class="content">`** means it is structurally excluded from the heading observer's `querySelectorAll('main.content h1, ...')` scope. This is stronger than Draft A's "avoid heading tags" convention.
- **Nesting-depth cap** prevents pathological YAML from generating deeply nested tables. The `<pre>` fallback for depth > 3 ensures the page remains usable.
- **`serde_yml` is a newer crate** with less ecosystem adoption than `serde_yaml`. However, `serde_yaml` is officially deprecated and `serde_yml` is API-compatible. Pin to a specific version to avoid surprises.
- **localStorage key collision**: the key `mdmd-fm-collapsed` is namespaced to mdmd and unlikely to collide. The toggle is a progressive enhancement — if localStorage is unavailable, the panel defaults to expanded.
- **No schema-specific rendering**: the feature renders arbitrary YAML mappings. It does not interpret fields like `date`, `author`, or `tags` semantically. This keeps the implementation simple and avoids opinionated formatting.

## Verification

- Unit: `extract("")` returns body unchanged, meta `None`.
- Unit: `extract("---\ntitle: Hello\n---\n# Body")` returns stripped body starting at `# Body`, meta with title `"Hello"`.
- Unit: `extract("---\n[1,2,3]\n---\n")` returns original source (non-mapping root), meta `None`.
- Unit: `extract("---\ntitle: Hello\n")` returns original source (unterminated), meta `None`.
- Unit: `extract("---\n: bad yaml {{{\n---\n")` returns original source (parse error), meta `None`.
- Unit: `MetaValue::Null` renders as `<td class="fm-val fm-null">null</td>`.
- Unit: `MetaValue::List(["a","b"])` renders as pill spans.
- Unit: nested mapping at depth 4 renders as `<pre>` YAML block.
- Unit: title precedence — frontmatter `title` field wins over first H1.
- Integration: served page with frontmatter contains `<section class="frontmatter-table">` and no raw `---` delimiters inside `<main>`.
- Integration: `?raw=1` returns the original file contents including the YAML block.
- Integration: served page without frontmatter has no `.frontmatter-table` element.
- Manual: confirm table renders correctly in light and dark themes, collapses/expands, and persists state across page loads.
