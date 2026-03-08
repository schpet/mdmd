# YAML Frontmatter

## Overview

Add first-class YAML frontmatter rendering to the `mdmd serve` web UI without changing the TUI path or raw responses. The best version of this change keeps the extraction boundary outside `html::render_markdown`, but keeps the UI itself simple: recognize a document-start frontmatter block, strip it before comrak runs, normalize it into a small metadata model, and render a compact server-side metadata panel above the article body.

Primary goals:

- render frontmatter as document metadata instead of visible YAML lines
- keep malformed or unsupported blocks from hiding document content
- preserve `?raw=1` exactly as it works today
- prefer a frontmatter `title` for the browser `<title>` when it is a plain string
- keep the change focused on the serve pipeline and current page shell

This plan deliberately keeps the stronger ideas from both draft directions:

- use a dedicated frontmatter module so markdown rendering stays markdown-only
- keep the feature SSR-only in v1; do not add collapse state, `localStorage`, or new `mdmd.js` behavior yet
- use a compact row-based metadata panel rather than a schema-heavy or app-like UI
- preserve graceful fallback rules so frontmatter support never becomes content loss

Non-goals for this change:

- no changes to the TUI parser or renderer
- no schema-specific behavior for fields like `date`, `author`, or `tags`
- no synthetic visible H1 generated from frontmatter
- no frontmatter editing UI

## Workflow Diagram

```text
request markdown file
  |
  +--> query contains raw=1
  |      |
  |      +--> return original file contents unchanged
  |
  +--> frontmatter::extract(source)
         |
         +--> no recognized / malformed / unsupported block
         |      |
         |      +--> body = original source
         |      +--> meta = None
         |
         +--> valid empty block
         |      |
         |      +--> body = source after closing delimiter
         |      +--> meta = None
         |
         +--> valid mapping
                |
                +--> body = source after closing delimiter
                +--> meta = Some(FrontmatterMeta)
  |
  +--> html::render_markdown(body, ...)
  |
  +--> html::build_page_shell(..., ctx.frontmatter)
         |
         +--> <title> prefers frontmatter.title, then first H1, then file stem
         +--> frontmatter panel renders before markdown body HTML
         +--> TOC and active-heading logic still track markdown headings only
  |
  +--> return text/html response with existing cache behavior
```

## Current State

As of input commit `d9742cb931f6d94f9d7126357e08ccd0b89bc957`:

- `src/serve.rs` reads the full markdown file and passes the entire source into `html::render_markdown`.
- `?raw=1` already short-circuits before HTML rendering and returns the original source.
- `src/html.rs` computes the page title from the first H1 or file stem and has no frontmatter slot in `PageShellContext`.
- `src/assets/mdmd.js` scopes heading observation to `main.content h1` through `h6`, so the metadata panel must not emit heading tags if it lives inside `main.content`.

That means the cleanest implementation is to extract frontmatter in the serve path, keep `render_markdown()` unaware of YAML, and only teach the page shell how to display normalized metadata.

## Decisions

### 1. Module boundary

Add a new `src/frontmatter.rs` module and wire it from `src/serve.rs`.

Module responsibilities:

| Module | Responsibility |
|---|---|
| `src/frontmatter.rs` | Detect, parse, and normalize document-start YAML frontmatter. No HTML rendering. |
| `src/serve.rs` | Keep raw mode unchanged; call `frontmatter::extract()` before `html::render_markdown()`. |
| `src/html.rs` | Receive normalized metadata through `PageShellContext`, render the panel, and apply title precedence. |
| `src/assets/mdmd.css` | Style the metadata panel so it fits the existing serve UI. |

This keeps the YAML-specific logic isolated without turning `html.rs` into a mixed parser/renderer module.

### 2. Recognition and fallback rules

Frontmatter should be treated as special only when all of the following are true:

- the file starts with `---` on the first logical line
- the block is closed by a line containing only `---` or `...`
- delimiter scanning works on logical lines so both LF and CRLF files are accepted
- the parsed YAML is either an empty document or a top-level mapping with renderable keys

Behavioral rules:

- valid mapping: strip the block and render metadata
- valid empty block (`---` immediately followed by a closing delimiter): strip it and render no panel
- malformed YAML: leave the source unchanged and render exactly as today
- unterminated block: leave the source unchanged
- valid non-mapping YAML: leave the source unchanged
- mappings with unsupported key shapes: leave the source unchanged

The fallback rule is strict: if there is any doubt that the block can be rendered safely and predictably, do not strip it.

### 3. Data model

Use a small internal model instead of passing parser values into the page shell:

```rust
pub struct ExtractResult<'a> {
    pub body: &'a str,
    pub meta: Option<FrontmatterMeta>,
}

pub struct FrontmatterMeta {
    pub fields: Vec<FrontmatterField>,
    pub title: Option<String>,
}

pub struct FrontmatterField {
    pub key: String,
    pub value: MetaValue,
}

pub enum MetaValue {
    Scalar(String),
    Null,
    Sequence(Vec<MetaValue>),
    Mapping(Vec<FrontmatterField>),
}
```

Normalization rules:

- parse with `serde_yml` into a generic value, then immediately convert into the internal model
- preserve field order by converting mappings directly into `Vec<...>` in parser iteration order
- only cache `title` when the frontmatter field is a plain string scalar
- do not coerce numbers, booleans, or dates into custom display formats; keep their YAML text

### 4. Rendering shape

Render metadata as a compact row-based panel inside `main.content`, immediately before the markdown body HTML and before backlinks. This keeps layout churn low because the current width, padding, and typography already live there.

Recommended HTML shape:

```html
<section class="frontmatter-panel" aria-label="Document metadata">
  <dl class="frontmatter-fields">
    <div class="frontmatter-row">
      <dt>title</dt>
      <dd>Example document</dd>
    </div>
  </dl>
</section>
```

Rendering rules:

- the panel must never emit `<h1>` through `<h6>`
- scalar values render as escaped text
- null renders as an explicit muted `null` label
- sequences of simple scalars render as compact pills or inline items
- nested mappings and complex sequences render as stacked nested rows
- cap recursive HTML rendering depth at a small bound such as 3; deeper structures fall back to escaped YAML text so pathological YAML cannot explode the DOM
- if normalization yields no visible fields, omit the panel entirely

This keeps the UI readable without requiring new JS or a more invasive layout wrapper.

### 5. Scope discipline

Keep v1 intentionally narrow:

- no changes to `src/assets/mdmd.js`
- no persistence for collapsed/expanded state
- no new interactive controls
- no special treatment for specific keys beyond browser-title preference

If the panel later proves too noisy for metadata-heavy files, collapse behavior can be a follow-up once the basic rendering contract is stable.

## Implementation Steps

1. Add the parser dependency and module wiring.
- Add `serde_yml` to `Cargo.toml`.
- Add `mod frontmatter;` in `src/main.rs`.

2. Implement `src/frontmatter.rs`.
- Add `extract(source: &str) -> ExtractResult`.
- Detect opening and closing delimiters on logical lines.
- Parse only the delimited slice.
- Convert supported YAML into `FrontmatterMeta`.
- Treat parse failures and unsupported shapes as soft failures that return the original source unchanged.

3. Wire extraction into `src/serve.rs`.
- Keep the existing raw-mode early return exactly where it is.
- For normal markdown rendering, call `frontmatter::extract(&content)` before `html::render_markdown()`.
- Pass the stripped body into `render_markdown()`.
- Pass `result.meta.as_ref()` into `PageShellContext`.

4. Extend `src/html.rs`.
- Add `frontmatter: Option<&FrontmatterMeta>` to `PageShellContext`.
- Update title selection to prefer `frontmatter.title`, then first H1, then file stem.
- Add a small `render_frontmatter_html()` helper used by `build_page_shell()`.
- Insert the metadata panel before the rendered markdown body and keep backlinks after the body as they are today.

5. Add focused CSS in `src/assets/mdmd.css`.
- Style `.frontmatter-panel` as a low-noise metadata card.
- Lay out `.frontmatter-row` as a compact two-column grid that collapses cleanly on narrow screens.
- Ensure long values wrap and do not force horizontal overflow.
- Reuse existing color tokens instead of adding a new theme system.

6. Add tests.
- Unit tests in `src/frontmatter.rs` for extraction, fallback behavior, CRLF handling, empty blocks, key-order preservation, and title extraction.
- Unit tests in `src/html.rs` for title precedence, panel escaping, and panel omission when metadata is absent.
- Integration tests in `tests/serve_integration.rs` for rendered frontmatter, malformed frontmatter fallback, and unchanged `?raw=1`.

## Risks and Guardrails

- Frontmatter support must never hide the markdown body on parse failure.
- Stripping must happen before comrak so the YAML block does not become `<hr>` or paragraph noise in the rendered article.
- Every key and value must be HTML-escaped before insertion into the page shell.
- Keeping the panel inside `main.content` is acceptable only because the renderer will never emit heading tags there; tests should lock that contract in.
- The touched code surface should stay limited to `Cargo.toml`, `src/main.rs`, `src/frontmatter.rs`, `src/serve.rs`, `src/html.rs`, `src/assets/mdmd.css`, and focused tests.

## Verification

- Unit: no frontmatter returns the original source and `meta = None`.
- Unit: valid mapping frontmatter strips cleanly and returns structured metadata.
- Unit: empty frontmatter block strips cleanly and returns `meta = None`.
- Unit: malformed YAML, unterminated delimiters, non-mapping roots, and unsupported keys leave the source unchanged.
- Unit: CRLF-delimited frontmatter is recognized.
- Unit: frontmatter `title` overrides the first H1 for the HTML `<title>`, but does not synthesize a visible H1.
- Unit: rendered panel HTML contains no heading tags and escapes user content.
- Integration: served HTML contains the metadata panel and does not show raw YAML lines in the main article content.
- Integration: `?raw=1` still returns the original file contents including the frontmatter block.
- Integration: pages without valid frontmatter render exactly as they do today.
- Manual: run `cargo test` and open a sample document in `mdmd serve` to confirm the panel looks correct in both light and dark themes and wraps cleanly on a narrow viewport.
