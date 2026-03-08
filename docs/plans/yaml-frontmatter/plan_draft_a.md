# YAML Frontmatter Draft A

## Overview

Add first-class YAML frontmatter rendering to the `mdmd serve` web UI. Today the serve path passes the full markdown source directly into `html::render_markdown`, so a leading frontmatter block is rendered as ordinary content instead of document metadata. Draft A keeps the change server-side and focused:

- Detect a document-start YAML frontmatter block before comrak rendering.
- Parse it with `serde_yaml` only when the block is well-formed and closed.
- Strip the frontmatter from the markdown body before HTML rendering.
- Render the parsed fields in a dedicated metadata panel above the article body.
- Keep `?raw=1` behavior unchanged so raw responses still return the original file contents.
- Limit scope to the web UI; the TUI parser and renderer stay unchanged.

Behavioral rules:

- Only treat frontmatter as special when the file starts with `---` on line 1 and the block has a closing `---` or `...`.
- Require the parsed YAML root to be a mapping for first-class rendering. Non-mapping YAML, malformed YAML, or unterminated blocks fall back to current behavior with no stripping.
- Avoid heading tags inside the metadata panel so the TOC and active-heading observer continue to track markdown headings only.
- Prefer a string `title` field from frontmatter for the browser `<title>` when present; otherwise keep the current H1/file-stem fallback.
- Keep the touched code surface small: `src/html.rs`, `src/serve.rs`, `src/assets/mdmd.css`, and focused tests.

## Workflow Diagram

```text
serve_handler reads markdown file
  |
  +--> ?raw=1
  |      |
  |      +--> return original file unchanged
  |
  +--> extract_yaml_frontmatter(source)
         |
         +--> no valid frontmatter
         |      |
         |      +--> render_markdown(original source)
         |
         +--> valid YAML mapping
                |
                +--> strip frontmatter from markdown body
                +--> render_markdown(body only)
                +--> build_page_shell(
                       metadata panel from parsed YAML,
                       TOC from markdown headings only,
                       browser title preferring frontmatter.title
                    )
```

## Current State

- `src/serve.rs` reads the markdown file and passes the full source string into `html::render_markdown`.
- `src/html.rs` uses comrak with GFM extensions but has no frontmatter extraction or metadata model.
- `build_page_shell` has no reserved slot for document metadata, so the page shell can only render TOC, body HTML, and backlinks.
- Because raw mode short-circuits before HTML rendering, it already has the right behavior and should remain untouched.

## Proposed Shape

Use a small internal metadata model instead of passing raw `serde_yaml::Value` through the page shell:

- `Frontmatter { fields: Vec<FrontmatterField>, title: Option<String> }`
- `FrontmatterField { key: String, value: FrontmatterValue }`
- `FrontmatterValue`
  - `Scalar(String)`
  - `Sequence(Vec<FrontmatterValue>)`
  - `Mapping(Vec<FrontmatterField>)`

Rendering rules:

- Scalars render as plain text.
- Sequences of scalars render as compact pills or stacked inline items.
- Nested mappings render as nested definition-list rows.
- Null values render as an explicit muted `null` label instead of disappearing.
- Booleans, numbers, and dates stay as YAML string representations; do not coerce or reformat them.

Recommended HTML shape:

```html
<section class="frontmatter-panel" aria-label="Document metadata">
  <div class="frontmatter-row">
    <dt>title</dt>
    <dd>Example doc</dd>
  </div>
</section>
```

Implementation note: keep this panel inside `main.content` so it inherits page width and typography, but do not use `<h1>`-`<h6>` inside it.

## Implementation Steps

1. Add a frontmatter extraction helper in `src/html.rs`.
   - Input: full markdown source.
   - Output: `{ body_markdown, frontmatter: Option<Frontmatter> }`.
   - Treat parse failures as soft failures and return the original source unchanged.

2. Add YAML parsing and normalization.
   - Introduce `serde_yaml` as the only new dependency.
   - Parse only the delimited frontmatter slice.
   - Accept only a top-level mapping for first-class rendering.

3. Extend the page-shell inputs.
   - Add `frontmatter: Option<&Frontmatter>` to `PageShellContext`.
   - Update title selection to prefer `frontmatter.title` before H1/file stem.
   - Render the metadata panel above the body HTML and below the fixed controls.

4. Add focused CSS.
   - Style the panel as a low-noise metadata card that fits the current serve theme.
   - Ensure long scalar values, lists, and nested mappings wrap without forcing page overflow.
   - Keep mobile behavior simple: stack rows vertically under the existing narrow-screen layout.

5. Add tests.
   - Unit tests in `src/html.rs` for extraction, parse fallback, normalization, title precedence, and metadata HTML rendering.
   - Integration coverage in `tests/serve_integration.rs` confirming rendered pages show the metadata panel while `?raw=1` still returns the original frontmatter block.

## Risks and Guardrails

- Frontmatter parse failures must never hide document content. If parsing fails, render the page exactly as it works today.
- The metadata panel must not introduce headings, or it will pollute the TOC sidebar and active-heading logic in `mdmd.js`.
- Frontmatter stripping must happen before comrak rendering so the YAML block does not produce stray `<hr>` or paragraph nodes in the body.
- Keep the implementation SSR-only; there is no product need for client-side hydration or JS state for this feature.
- Nested or unusual YAML should degrade to readable text instead of requiring schema-specific support.

## Verification

- Unit: opening delimiter without a closing delimiter leaves the document unchanged.
- Unit: malformed YAML leaves the document unchanged.
- Unit: valid mapping frontmatter is removed from the markdown body and rendered as metadata rows.
- Unit: non-mapping YAML frontmatter falls back to current behavior.
- Unit: `title` in frontmatter overrides H1 for the HTML `<title>`.
- Integration: a served document with frontmatter renders metadata above the body and excludes raw YAML lines from the main article content.
- Integration: the same document served with `?raw=1` returns the original source including the frontmatter block.
- Manual browser check: confirm the panel does not appear in the TOC, wraps cleanly on a narrow viewport, and still looks correct in both light and dark themes.
