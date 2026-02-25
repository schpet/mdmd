//! HTML rendering module for serve mode.
//!
//! Converts markdown text to HTML using comrak with GFM extensions.
//! Heading metadata (level, text, anchor ID) is extracted for TOC construction.
//!
//! The TUI parse/render path (`parse.rs`, `render.rs`) is not touched here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::backlinks::BacklinkRef;

use comrak::{
    arena_tree::NodeEdge,
    format_html,
    nodes::{AstNode, NodeValue},
    parse_document, Arena, Options,
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A heading extracted from the document for TOC construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingEntry {
    /// Heading level (1–6).
    pub level: u8,
    /// Plain-text content of the heading.
    pub text: String,
    /// URL-safe anchor ID, deduplicated within the document.
    ///
    /// The first occurrence of a heading slug is bare (e.g. `my-heading`);
    /// subsequent occurrences receive a numeric suffix (`my-heading-1`, `my-heading-2`).
    pub anchor_id: String,
}

/// Context passed to [`build_page_shell`] to avoid repeated signature churn as
/// new per-page metadata fields are added.
///
/// Fields that are not yet wired up (e.g. `file_mtime_secs`, `page_url_path`
/// from bd-38z) default to `None`; callers that do not have those values should
/// pass `None` until the relevant subsystem is implemented.
// `file_mtime_secs` and `page_url_path` are reserved for bd-38z and are read
// by that subsystem once it is wired in.
#[allow(dead_code)]
pub struct PageShellContext<'a> {
    /// Inbound backlinks for this page from the startup index.
    /// Pass `&[]` for non-markdown pages, static assets, and error responses.
    pub backlinks: &'a [BacklinkRef],
    /// Unix timestamp (seconds) of the file's last modification, for freshness
    /// polling (bd-38z).  `None` disables change detection on this page.
    pub file_mtime_secs: Option<u64>,
    /// Root-relative URL path for this page (e.g. `/docs/guide.md`), used to
    /// emit a `<meta>` tag for the JS freshness check (bd-38z).  `None` omits
    /// the tag.
    pub page_url_path: Option<&'a str>,
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Build comrak render options with GFM extensions and secure defaults.
///
/// - GFM extensions: strikethrough, tables, autolinks, task lists.
/// - R3 mitigation: `render.unsafe_ = false` (default) — raw HTML from input is
///   stripped and replaced with `<!-- raw HTML omitted -->`.
fn make_options() -> Options<'static> {
    let mut options = Options::default();
    // GFM extensions — only what is required (R10)
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    // Explicit: raw HTML is unsafe — do not pass through (R3).
    // This is already the default (false), but stated clearly for auditability.
    options.render.unsafe_ = false;
    options
}

/// Convert heading text to a URL-safe anchor slug.
///
/// Algorithm: lowercase the text, map spaces/hyphens/underscores to `-`,
/// strip all other non-alphanumeric characters, collapse consecutive hyphens,
/// and trim leading/trailing hyphens.
fn slugify(text: &str) -> String {
    let mut slug = String::new();
    for c in text.to_lowercase().chars() {
        if c.is_alphanumeric() {
            slug.push(c);
        } else if c == ' ' || c == '-' || c == '_' {
            if !slug.ends_with('-') {
                slug.push('-');
            }
        }
        // all other characters are dropped
    }
    slug.trim_matches('-').to_owned()
}

/// Recursively collect plain-text content of a heading AST node.
fn collect_heading_text<'a>(node: &'a AstNode<'a>) -> String {
    let mut text = String::new();
    for child in node.children() {
        match &child.data.borrow().value {
            NodeValue::Text(s) => text.push_str(s),
            NodeValue::Code(c) => text.push_str(&c.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => text.push(' '),
            _ => text.push_str(&collect_heading_text(child)),
        }
    }
    text
}

// ---------------------------------------------------------------------------
// Private HTML helpers
// ---------------------------------------------------------------------------

/// Minimal HTML entity escaping for text content and attribute values.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Inject `id` attributes into heading elements in the rendered HTML fragment.
///
/// Performs sequential first-occurrence replacements: `<hN>` → `<hN id="...">`.
/// Because `render.unsafe_ = false` is set, comrak will never emit bare `<hN>`
/// tags from raw-HTML inputs in the markdown source, so replacements only hit
/// genuine heading elements generated from markdown headings.
fn inject_heading_ids(html: &str, headings: &[HeadingEntry]) -> String {
    let mut result = html.to_owned();
    for heading in headings {
        let tag = format!("<h{}>", heading.level);
        let with_id = format!("<h{} id=\"{}\">", heading.level, heading.anchor_id);
        result = result.replacen(&tag, &with_id, 1);
    }
    result
}

/// Build the `<ul>…</ul>` HTML for the TOC sidebar.
///
/// Returns an empty string when `headings` is empty (the sidebar will still be
/// rendered in the page shell but will contain nothing).
fn build_toc_html(headings: &[HeadingEntry]) -> String {
    if headings.is_empty() {
        return String::new();
    }
    let mut html = String::from("<ul>\n");
    for heading in headings {
        let class = format!("toc-h{}", heading.level);
        let anchor = heading.anchor_id.as_str(); // anchor_id is already a URL-safe slug
        let text = html_escape(&heading.text);
        html.push_str(&format!(
            "<li class=\"{class}\"><a href=\"#{anchor}\">{text}</a></li>\n",
        ));
    }
    html.push_str("</ul>\n");
    html
}

/// Returns true when a fenced code block info string denotes Mermaid.
///
/// Matching is case-insensitive and based on the first whitespace-delimited
/// token of the info string (for example, `mermaid` in `mermaid title=...`).
fn is_mermaid_info(info: &str) -> bool {
    info.split_whitespace()
        .next()
        .map(|lang| lang.eq_ignore_ascii_case("mermaid"))
        .unwrap_or(false)
}

/// Rewrite Mermaid fenced code blocks into SSR placeholders:
/// `<pre class="mermaid">...</pre>`.
///
/// Mermaid source is HTML-escaped before insertion so diagram text is never
/// injected as raw HTML.
fn rewrite_mermaid_code_blocks<'a>(root: &'a AstNode<'a>) -> usize {
    let mut rewritten = 0usize;

    for node in root.descendants() {
        let replacement = {
            let data = node.data.borrow();
            match &data.value {
                NodeValue::CodeBlock(ncb) if ncb.fenced && is_mermaid_info(&ncb.info) => {
                    Some(format!(
                        "<pre class=\"mermaid\">{}</pre>\n",
                        html_escape(&ncb.literal)
                    ))
                }
                _ => None,
            }
        };

        if let Some(raw_html) = replacement {
            node.data.borrow_mut().value = NodeValue::Raw(raw_html);
            rewritten += 1;
        }
    }

    rewritten
}

// ---------------------------------------------------------------------------
// Local link rewriting (bd-1p6)
// ---------------------------------------------------------------------------

/// Split a URL into its base path and trailing suffix (query string and/or fragment).
///
/// The suffix starts at the first `?` or `#` character (whichever comes first).
/// Returns `(base, suffix)` where `suffix` may be empty.
fn split_url_suffix(url: &str) -> (&str, &str) {
    match url.find(|c| c == '?' || c == '#') {
        Some(pos) => (&url[..pos], &url[pos..]),
        None => (url, ""),
    }
}

/// Resolve a relative URL path against `file_dir`, producing an absolute `PathBuf`.
///
/// Processes each `/`-separated component of `rel`:
/// - `""` and `"."` are ignored.
/// - `".."` pops the last component (clamped at root; will not go above filesystem root).
/// - All other components are pushed.
fn resolve_relative_path(file_dir: &Path, rel: &str) -> PathBuf {
    let mut resolved = file_dir.to_path_buf();
    for component in rel.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                // pop() returns false at filesystem root — path stays clamped.
                resolved.pop();
            }
            part => resolved.push(part),
        }
    }
    resolved
}

/// Rewrite a single link URL to a root-relative href suitable for web navigation.
///
/// # Returns
/// - `None`: the URL is external, absolute, fragment-only, or cannot be made
///   root-relative (resolved path escapes `serve_root`). Leave as-is.
/// - `Some(new_url)`: the rewritten root-relative URL (e.g. `/docs/page.md`),
///   with any original query string and fragment preserved.
fn rewrite_url(url: &str, file_dir: &Path, serve_root: &Path) -> Option<String> {
    // Never rewrite external, protocol-relative, absolute, or fragment-only URLs.
    if url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("//")
        || url.starts_with("mailto:")
        || url.starts_with('#')
        || url.starts_with('/')
    {
        return None;
    }

    // Separate the base path from any query string / fragment suffix.
    let (base, suffix) = split_url_suffix(url);

    // If the base is empty (e.g. url is "?q=1" without a path), leave as-is.
    if base.is_empty() {
        return None;
    }

    // Resolve the relative base path from the current file's directory.
    let resolved = resolve_relative_path(file_dir, base);

    // Make root-relative by stripping the serve_root prefix.
    // If strip_prefix fails the resolved path escaped serve_root; leave url unchanged
    // so the server's path resolver will reject it with 404 at request time.
    match resolved.strip_prefix(serve_root) {
        Ok(rel) => {
            let rel_str = rel.to_string_lossy();
            Some(format!("/{}{}", rel_str, suffix))
        }
        Err(_) => None,
    }
}

/// Traverse the comrak AST and rewrite local relative link (and image) URLs to
/// root-relative hrefs suitable for web navigation.
///
/// Mutates matching `NodeValue::Link` and `NodeValue::Image` nodes in-place.
/// Links inside fenced code blocks are not visited (they are `NodeValue::Code`
/// or `NodeValue::CodeBlock`, not `Link` nodes, so they are naturally skipped).
///
/// # Returns
/// `(rewritten, skipped)` — counts of links rewritten and left unchanged.
fn rewrite_local_links<'a>(
    root: &'a AstNode<'a>,
    file_path: &Path,
    serve_root: &Path,
) -> (usize, usize) {
    let file_dir = file_path.parent().unwrap_or(Path::new(""));
    let mut rewritten = 0usize;
    let mut skipped = 0usize;

    for node in root.descendants() {
        let mut data = node.data.borrow_mut();
        let url = match &mut data.value {
            NodeValue::Link(nl) => &mut nl.url,
            NodeValue::Image(ni) => &mut ni.url,
            _ => continue,
        };

        match rewrite_url(url, file_dir, serve_root) {
            Some(new_url) => {
                *url = new_url;
                rewritten += 1;
            }
            None => {
                skipped += 1;
            }
        }
    }

    (rewritten, skipped)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render a markdown string to HTML and extract heading metadata.
///
/// # Parameters
/// - `input`: raw markdown source.
/// - `file_path`: absolute path of the source file (used for link rewriting
///   and logging).
/// - `serve_root`: canonicalized root directory of the serve tree. Local
///   relative links are rewritten to root-relative hrefs using this value.
///
/// # Returns
/// `(html, headings)` where `html` is the full HTML string and `headings` is
/// the ordered list of [`HeadingEntry`] values for TOC construction.
///
/// Logs `[render] path=<file> headings=<count>` and
/// `[rewrite] file=<path> rewritten=<N> skipped=<M>` at info/debug level.
pub fn render_markdown(
    input: &str,
    file_path: &Path,
    serve_root: &Path,
) -> (String, Vec<HeadingEntry>) {
    let arena = Arena::new();
    let options = make_options();
    let root = parse_document(&arena, input, &options);

    // --- Mermaid fenced blocks: SSR placeholders for client hydration (bd-2se) ---
    let mermaid_rewritten = rewrite_mermaid_code_blocks(root);
    eprintln!(
        "[mermaid] file={} rewritten={}",
        file_path.display(),
        mermaid_rewritten
    );

    // --- Rewrite local relative links to root-relative hrefs (bd-1p6) ---
    let (rewritten, skipped) = rewrite_local_links(root, file_path, serve_root);
    eprintln!(
        "[rewrite] file={} rewritten={} skipped={}",
        file_path.display(),
        rewritten,
        skipped
    );

    // --- Extract headings with per-document slug deduplication (R4) ---
    let mut entries: Vec<HeadingEntry> = Vec::new();
    // Maps base slug → number of times it has been seen so far.
    let mut slug_counter: HashMap<String, usize> = HashMap::new();

    for edge in root.traverse() {
        if let NodeEdge::Start(node) = edge {
            if let NodeValue::Heading(nh) = &node.data.borrow().value {
                let level = nh.level;
                let text = collect_heading_text(node);
                let base_slug = slugify(&text);

                let count = slug_counter.entry(base_slug.clone()).or_insert(0);
                let anchor_id = if *count == 0 {
                    // First occurrence: bare slug.
                    *count = 1;
                    base_slug.clone()
                } else {
                    // Subsequent occurrences: slug-N where N starts at 1.
                    let n = *count;
                    *count += 1;
                    format!("{}-{}", base_slug, n)
                };

                entries.push(HeadingEntry {
                    level,
                    text,
                    anchor_id,
                });
            }
        }
    }

    // --- Render to HTML ---
    let mut html_bytes = Vec::new();
    format_html(root, &options, &mut html_bytes).expect("comrak HTML formatting should not fail");
    let html = String::from_utf8(html_bytes).expect("comrak output must be valid UTF-8");

    eprintln!(
        "[render] path={} headings={}",
        file_path.display(),
        entries.len()
    );

    (html, entries)
}

/// Build the full HTML page shell: `<!DOCTYPE html>` with header, sticky TOC
/// sidebar, rendered content area, and backlinks panel.
///
/// # Parameters
/// - `body_html`: the raw HTML fragment produced by `render_markdown`.
/// - `headings`: ordered heading entries for the TOC (from `render_markdown`).
/// - `file_path`: absolute path to the source `.md` file (for display).
/// - `serve_root`: root directory of the serve tree (used to compute the
///   relative display path shown in the header).
/// - `ctx`: per-page metadata including backlinks, mtime, and URL path.
///   Pass `&PageShellContext { backlinks: &[], .. }` for pages without backlinks.
///
/// # Returns
/// A complete `text/html` document ready to send to the browser.
pub fn build_page_shell(
    body_html: &str,
    headings: &[HeadingEntry],
    file_path: &Path,
    serve_root: &Path,
    ctx: &PageShellContext,
) -> String {
    // Page title: first H1 text, then file stem, then a safe default.
    let title_raw = headings
        .iter()
        .find(|h| h.level == 1)
        .map(|h| h.text.as_str())
        .or_else(|| file_path.file_stem().and_then(|s| s.to_str()))
        .unwrap_or("Document");

    let title = html_escape(title_raw);
    let content_html = inject_heading_ids(body_html, headings);
    let toc_html = build_toc_html(headings);
    let backlinks_html = build_backlinks_html(ctx.backlinks);

    // Emit freshness meta tags when mtime / url path are available (bd-38z).
    let mtime_meta = match ctx.file_mtime_secs {
        Some(secs) => format!("<meta name=\"mdmd-mtime\" content=\"{secs}\">\n"),
        None => String::new(),
    };
    let path_meta = match ctx.page_url_path {
        Some(p) => format!(
            "<meta name=\"mdmd-path\" content=\"{}\">\n",
            html_escape(p)
        ),
        None => String::new(),
    };

    // Mermaid is loaded unconditionally to keep shell logic simple.
    // Version is pinned (not @latest) for reproducibility and to avoid silent
    // breakage from upstream CDN updates.
    const MERMAID_CDN_URL: &str = "https://cdn.jsdelivr.net/npm/mermaid@10.9.3/dist/mermaid.min.js";

    // Inline FOUC-prevention script: reads localStorage before CSS paints.
    const THEME_INIT_SCRIPT: &str = "\
<script>(function(){\
var s=localStorage.getItem('mdmd-theme');\
var dark=s==='dark'||(!s&&window.matchMedia('(prefers-color-scheme:dark)').matches);\
if(dark)document.documentElement.setAttribute('data-theme','dark');\
}());</script>";

    // SVG icons for the theme toggle button.
    const ICON_MOON: &str = r#"<svg class="icon-moon" xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>"#;
    const ICON_SUN: &str = r#"<svg class="icon-sun" xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>"#;

    format!(
        "<!DOCTYPE html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{title} · mdmd serve</title>\n\
{mtime_meta}\
{path_meta}\
{THEME_INIT_SCRIPT}\n\
<link rel=\"stylesheet\" href=\"/assets/mdmd.css\">\n\
</head>\n\
<body>\n\
<button id=\"theme-toggle\" class=\"theme-toggle\" aria-label=\"Toggle dark mode\">{ICON_MOON}{ICON_SUN}</button>\n\
<div id=\"mdmd-change-notice\" class=\"change-notice\" hidden>\n\
This file has changed on disk.\n\
<button class=\"change-notice-reload\" onclick=\"location.reload()\">Load latest</button>\n\
</div>\n\
<div class=\"layout\">\n\
<nav class=\"toc-sidebar\">\n\
{toc_html}</nav>\n\
<main class=\"content\">\n\
{content_html}\
{backlinks_html}</main>\n\
</div>\n\
<script src=\"{MERMAID_CDN_URL}\"></script>\n\
<script src=\"/assets/mdmd.js\"></script>\n\
</body>\n\
</html>\n"
    )
}

/// Build the HTML fragment for the backlinks section.
///
/// Returns an empty string when there are no backlinks (section is omitted).
/// Otherwise renders a bordered footnote-style section below the document
/// content with one entry per source document and a count in the header.
fn build_backlinks_html(backlinks: &[BacklinkRef]) -> String {
    if backlinks.is_empty() {
        return String::new();
    }

    let count = backlinks.len();
    let mut html = format!(
        "<section class=\"backlinks-panel\" aria-label=\"Backlinks\">\n\
<h2 class=\"backlinks-header\">Backlinks ({count})</h2>\n\
<ul class=\"backlinks-list\">\n",
    );
    for bl in backlinks {
        let base_href = html_escape(&bl.source_url_path);
        let href = match &bl.target_fragment {
            Some(frag) => format!("{}#{}", base_href, html_escape(frag)),
            None => base_href,
        };
        let label = html_escape(&bl.source_display);
        let snippet = html_escape(&bl.snippet);
        let fragment_span = match &bl.target_fragment {
            Some(frag) => format!(
                "<span class=\"backlinks-fragment\"> \u{00a7} {}</span>",
                html_escape(frag)
            ),
            None => String::new(),
        };
        html.push_str(&format!(
            "<li class=\"backlinks-item\">\n\
<a class=\"backlinks-source\" href=\"{href}\">{label}</a>{fragment_span}\n\
<p class=\"backlinks-snippet\">{snippet}</p>\n\
</li>\n"
        ));
    }
    html.push_str("</ul>\n</section>\n");
    html
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience wrapper: render with dummy paths.
    fn render(input: &str) -> (String, Vec<HeadingEntry>) {
        render_markdown(input, Path::new("test.md"), Path::new("."))
    }

    // --- Phase-1 markdown feature matrix ---

    #[test]
    fn paragraph_renders() {
        let (html, _) = render("hello world\n");
        assert!(html.contains("<p>"), "expected <p> tag");
    }

    #[test]
    fn emphasis_renders() {
        let (html, _) = render("*text*\n");
        assert!(html.contains("<em>"), "expected <em> tag");
    }

    #[test]
    fn strong_renders() {
        let (html, _) = render("**text**\n");
        assert!(html.contains("<strong>"), "expected <strong> tag");
    }

    #[test]
    fn inline_code_renders() {
        let (html, _) = render("`inline code`\n");
        assert!(html.contains("<code>"), "expected <code> tag");
    }

    #[test]
    fn link_renders() {
        let (html, _) = render("[text](https://example.com)\n");
        assert!(
            html.contains("href=\"https://example.com\""),
            "expected href attribute"
        );
        assert!(html.contains("<a "), "expected anchor tag");
    }

    #[test]
    fn gfm_table_renders() {
        let (html, _) = render("| A | B |\n|---|---|\n| 1 | 2 |\n");
        assert!(html.contains("<table>"), "expected <table>");
        assert!(html.contains("<th>"), "expected <th>");
        assert!(html.contains("<td>"), "expected <td>");
    }

    #[test]
    fn task_list_renders() {
        let (html, _) = render("- [ ] todo\n- [x] done\n");
        assert!(
            html.contains("<input") && html.contains("checkbox"),
            "expected checkbox input"
        );
    }

    #[test]
    fn strikethrough_renders() {
        let (html, _) = render("~~deleted~~\n");
        assert!(html.contains("<del>"), "expected <del> tag");
    }

    #[test]
    fn fenced_code_block_with_language() {
        let (html, _) = render("```rust\nfn main() {}\n```\n");
        assert!(html.contains("<pre>"), "expected <pre>");
        assert!(html.contains("<code"), "expected <code>");
        // CommonMark specifies language class on the <code> element.
        assert!(
            html.contains("language-rust") || html.contains("rust"),
            "expected language hint"
        );
    }

    #[test]
    fn mermaid_fence_renders_pre_placeholder() {
        let (html, _) = render("```mermaid\ngraph TD;\nA-->B;\n```\n");
        assert!(
            html.contains("<pre class=\"mermaid\">"),
            "expected Mermaid SSR placeholder, got: {html}"
        );
        assert!(
            !html.contains("language-mermaid"),
            "must not render mermaid as a normal code block, got: {html}"
        );
    }

    #[test]
    fn mermaid_fence_escapes_html_chars() {
        let (html, _) = render("```mermaid\ngraph TD;\nA<>B;\n```\n");
        assert!(
            html.contains("A&lt;&gt;B;"),
            "diagram source must be escaped, got: {html}"
        );
    }

    #[test]
    fn mermaid_fence_detection_is_case_insensitive() {
        let (html, _) = render("```MERMAID\ngraph TD;\nA-->B;\n```\n");
        assert!(
            html.contains("<pre class=\"mermaid\">"),
            "uppercase MERMAID should be detected, got: {html}"
        );
    }

    #[test]
    fn autolink_renders() {
        let (html, _) = render("https://example.com\n");
        assert!(
            html.contains("<a ") && html.contains("https://example.com"),
            "expected autolinked anchor"
        );
    }

    #[test]
    fn blockquote_renders() {
        let (html, _) = render("> quoted text\n");
        assert!(html.contains("<blockquote>"), "expected <blockquote>");
    }

    #[test]
    fn ordered_list_renders() {
        let (html, _) = render("1. Item\n");
        assert!(html.contains("<ol>"), "expected <ol>");
        assert!(html.contains("<li>"), "expected <li>");
    }

    #[test]
    fn unordered_list_renders() {
        let (html, _) = render("- Item\n");
        assert!(html.contains("<ul>"), "expected <ul>");
        assert!(html.contains("<li>"), "expected <li>");
    }

    // --- R3: raw HTML / XSS mitigation ---

    #[test]
    fn script_tag_stripped_from_output() {
        let (html, _) = render("<script>alert(1)</script>\n");
        assert!(
            !html.contains("<script>"),
            "script tag must not appear in rendered output (R3)"
        );
    }

    // --- R4: anchor ID deduplication ---

    #[test]
    fn duplicate_headings_get_sequential_anchors() {
        // ## Foo, ## Foo, ## Foo → foo, foo-1, foo-2
        let input = "## Foo\n\n## Foo\n\n## Foo\n";
        let (_, headings) = render(input);
        assert_eq!(headings.len(), 3);
        assert_eq!(headings[0].anchor_id, "foo");
        assert_eq!(headings[1].anchor_id, "foo-1");
        assert_eq!(headings[2].anchor_id, "foo-2");
    }

    #[test]
    fn headings_at_different_levels_share_slug_counter() {
        // ## Foo then ### Foo → foo, foo-1 (no collision isolation between levels)
        let input = "## Foo\n\n### Foo\n";
        let (_, headings) = render(input);
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].anchor_id, "foo");
        assert_eq!(headings[1].anchor_id, "foo-1");
    }

    #[test]
    fn anchor_ids_are_stable_across_renders() {
        let input = "# Title\n\n## Section\n\n## Section\n";
        let (_, h1) = render(input);
        let (_, h2) = render(input);
        assert_eq!(h1, h2, "heading entries must be identical across renders");
    }

    // --- inject_heading_ids ---

    #[test]
    fn inject_ids_adds_id_attribute_to_headings() {
        let html = "<h1>Title</h1>\n<h2>Section</h2>\n";
        let headings = vec![
            HeadingEntry {
                level: 1,
                text: "Title".into(),
                anchor_id: "title".into(),
            },
            HeadingEntry {
                level: 2,
                text: "Section".into(),
                anchor_id: "section".into(),
            },
        ];
        let result = inject_heading_ids(html, &headings);
        assert!(result.contains("<h1 id=\"title\">"), "h1 id injected");
        assert!(result.contains("<h2 id=\"section\">"), "h2 id injected");
    }

    #[test]
    fn inject_ids_processes_in_document_order() {
        // Two h2 at different slugs — first match is replaced, second on next pass.
        let html = "<h2>Alpha</h2>\n<h2>Beta</h2>\n";
        let headings = vec![
            HeadingEntry {
                level: 2,
                text: "Alpha".into(),
                anchor_id: "alpha".into(),
            },
            HeadingEntry {
                level: 2,
                text: "Beta".into(),
                anchor_id: "beta".into(),
            },
        ];
        let result = inject_heading_ids(html, &headings);
        assert!(
            result.contains("<h2 id=\"alpha\">Alpha</h2>"),
            "first h2 id=alpha"
        );
        assert!(
            result.contains("<h2 id=\"beta\">Beta</h2>"),
            "second h2 id=beta"
        );
    }

    // --- build_page_shell ---

    #[test]
    fn page_shell_contains_nav_with_toc() {
        let input = "# Title\n\n## Section\n";
        let (html_body, headings) = render(input);
        let page = build_page_shell(
            &html_body,
            &headings,
            Path::new("/root/doc.md"),
            Path::new("/root"),
            &PageShellContext { backlinks: &[], file_mtime_secs: None, page_url_path: None },
        );
        assert!(
            page.contains("<nav class=\"toc-sidebar\">"),
            "nav element present"
        );
        assert!(page.contains("href=\"#title\""), "toc link to h1");
        assert!(page.contains("href=\"#section\""), "toc link to h2");
    }

    #[test]
    fn page_shell_contains_script_tag() {
        let (html_body, headings) = render("# Hi\n");
        let page = build_page_shell(&html_body, &headings, Path::new("/r/f.md"), Path::new("/r"), &PageShellContext { backlinks: &[], file_mtime_secs: None, page_url_path: None });
        assert!(
            page.contains("<script src=\"/assets/mdmd.js\">"),
            "script tag present"
        );
    }

    #[test]
    fn page_shell_contains_pinned_mermaid_cdn_script() {
        let (html_body, headings) = render("# Hi\n");
        let page = build_page_shell(&html_body, &headings, Path::new("/r/f.md"), Path::new("/r"), &PageShellContext { backlinks: &[], file_mtime_secs: None, page_url_path: None });
        assert!(
            page.contains(
                "<script src=\"https://cdn.jsdelivr.net/npm/mermaid@10.9.3/dist/mermaid.min.js\">"
            ),
            "mermaid CDN script must be present with pinned semver"
        );
    }

    #[test]
    fn page_shell_contains_css_link() {
        let (html_body, headings) = render("# Hi\n");
        let page = build_page_shell(&html_body, &headings, Path::new("/r/f.md"), Path::new("/r"), &PageShellContext { backlinks: &[], file_mtime_secs: None, page_url_path: None });
        assert!(
            page.contains("href=\"/assets/mdmd.css\""),
            "css link present"
        );
    }

    #[test]
    fn page_shell_heading_ids_injected() {
        let input = "# Title\n\n## Sub\n";
        let (html_body, headings) = render(input);
        let page = build_page_shell(&html_body, &headings, Path::new("/r/f.md"), Path::new("/r"), &PageShellContext { backlinks: &[], file_mtime_secs: None, page_url_path: None });
        assert!(
            page.contains("<h1 id=\"title\">"),
            "h1 id injected in content"
        );
        assert!(
            page.contains("<h2 id=\"sub\">"),
            "h2 id injected in content"
        );
    }

    // --- html_escape ---

    #[test]
    fn html_escape_handles_special_chars() {
        assert_eq!(html_escape("<>&\"'"), "&lt;&gt;&amp;&quot;&#39;");
    }

    // --- Heading extraction ---

    #[test]
    fn headings_extracted_in_order() {
        let input = "# H1\n\n## H2\n\n### H3\n";
        let (_, headings) = render(input);
        assert_eq!(headings.len(), 3);
        assert_eq!(headings[0].level, 1);
        assert_eq!(headings[0].text, "H1");
        assert_eq!(headings[1].level, 2);
        assert_eq!(headings[1].text, "H2");
        assert_eq!(headings[2].level, 3);
        assert_eq!(headings[2].text, "H3");
    }

    // --- bd-1p6: local link rewriting ---

    /// Convenience wrapper: render with absolute paths.
    ///
    /// `serve_root` is the absolute serve root; `file_rel` is the relative path
    /// of the file within that root (e.g. `"index.md"` or `"docs/subdir/page.md"`).
    fn render_abs(input: &str, serve_root: &str, file_rel: &str) -> String {
        let root = Path::new(serve_root);
        let file = root.join(file_rel);
        let (html, _) = render_markdown(input, &file, root);
        html
    }

    #[test]
    fn rewrite_md_link_from_root_file() {
        // [t](other.md) from root-level file → /other.md
        let html = render_abs("[t](other.md)\n", "/root", "index.md");
        assert!(
            html.contains("href=\"/other.md\""),
            "expected /other.md, got: {html}"
        );
    }

    #[test]
    fn rewrite_subdir_link_from_root_file() {
        // [t](subdir/page.md) from root-level file → /subdir/page.md
        let html = render_abs("[t](subdir/page.md)\n", "/root", "index.md");
        assert!(
            html.contains("href=\"/subdir/page.md\""),
            "expected /subdir/page.md, got: {html}"
        );
    }

    #[test]
    fn rewrite_dotdot_link_from_nested_file() {
        // [t](../parent.md) from docs/subdir/page.md → /docs/parent.md
        let html = render_abs("[t](../parent.md)\n", "/root", "docs/subdir/page.md");
        assert!(
            html.contains("href=\"/docs/parent.md\""),
            "expected /docs/parent.md, got: {html}"
        );
    }

    #[test]
    fn rewrite_extensionless_link_from_root_file() {
        // [t](subdir/doc) extensionless → /subdir/doc (resolver will add .md)
        let html = render_abs("[t](subdir/doc)\n", "/root", "index.md");
        assert!(
            html.contains("href=\"/subdir/doc\""),
            "expected /subdir/doc, got: {html}"
        );
    }

    #[test]
    fn external_link_unchanged() {
        // [t](https://example.com/page.md) → href unchanged
        let html = render_abs("[t](https://example.com/page.md)\n", "/root", "index.md");
        assert!(
            html.contains("href=\"https://example.com/page.md\""),
            "external href must be unchanged, got: {html}"
        );
    }

    #[test]
    fn fragment_only_link_unchanged() {
        // [t](#section) → href unchanged
        let html = render_abs("[t](#section)\n", "/root", "index.md");
        assert!(
            html.contains("href=\"#section\""),
            "fragment-only href must be unchanged, got: {html}"
        );
    }

    #[test]
    fn rewrite_preserves_fragment_and_query() {
        // [t](doc.md#section?query=1) → /doc.md#section?query=1
        let html = render_abs("[t](doc.md#section?query=1)\n", "/root", "index.md");
        assert!(
            html.contains("href=\"/doc.md#section?query=1\""),
            "fragment+query must be preserved, got: {html}"
        );
    }

    #[test]
    fn rewrite_traversal_escaping_serve_root_left_as_is() {
        // [t](../../outside.md) from docs/subdir/page.md
        // Resolved path escapes /root → leave URL as-is (server will 404).
        let html = render_abs("[t](../../outside.md)\n", "/root", "docs/subdir/page.md");
        // The URL must not contain an href pointing above the root.
        // It is left unchanged as "../../outside.md" or rewritten to something safe.
        // Either way, it must NOT produce an absolute path outside /root.
        assert!(
            !html.contains("href=\"/../../outside.md\"")
                && !html.contains("href=\"/../outside.md\""),
            "rewritten href must not escape serve_root, got: {html}"
        );
    }

    #[test]
    fn link_in_fenced_code_block_not_rewritten() {
        // Links inside fenced code blocks are plain text, not AST Link nodes.
        let input = "```\n[t](other.md)\n```\n";
        let html = render_abs(input, "/root", "index.md");
        // The URL should appear as literal text in a <code> block, not as an href.
        assert!(
            !html.contains("href=\"/other.md\""),
            "link in code block must NOT be rewritten as an href, got: {html}"
        );
        assert!(
            html.contains("other.md"),
            "link text should still appear in code block, got: {html}"
        );
    }

    // --- rewrite_url unit tests ---

    #[test]
    fn rewrite_url_skips_https() {
        assert!(rewrite_url("https://example.com", Path::new("/r"), Path::new("/r")).is_none());
    }

    #[test]
    fn rewrite_url_skips_http() {
        assert!(rewrite_url("http://example.com", Path::new("/r"), Path::new("/r")).is_none());
    }

    #[test]
    fn rewrite_url_skips_protocol_relative() {
        assert!(rewrite_url("//example.com/path", Path::new("/r"), Path::new("/r")).is_none());
    }

    #[test]
    fn rewrite_url_skips_mailto() {
        assert!(rewrite_url("mailto:user@example.com", Path::new("/r"), Path::new("/r")).is_none());
    }

    #[test]
    fn rewrite_url_skips_fragment() {
        assert!(rewrite_url("#anchor", Path::new("/r"), Path::new("/r")).is_none());
    }

    #[test]
    fn rewrite_url_skips_absolute_path() {
        assert!(rewrite_url("/already/absolute", Path::new("/r"), Path::new("/r")).is_none());
    }

    #[test]
    fn rewrite_url_local_md_link() {
        let result = rewrite_url("page.md", Path::new("/root"), Path::new("/root"));
        assert_eq!(result, Some("/page.md".to_owned()));
    }

    #[test]
    fn rewrite_url_preserves_fragment() {
        let result = rewrite_url("page.md#section", Path::new("/root"), Path::new("/root"));
        assert_eq!(result, Some("/page.md#section".to_owned()));
    }

    #[test]
    fn rewrite_url_preserves_query() {
        let result = rewrite_url("page.md?q=1", Path::new("/root"), Path::new("/root"));
        assert_eq!(result, Some("/page.md?q=1".to_owned()));
    }

    #[test]
    fn rewrite_url_dotdot_within_root() {
        // ../parent.md from /root/subdir → resolves to /root/parent.md → /parent.md
        let result = rewrite_url(
            "../parent.md",
            Path::new("/root/subdir"),
            Path::new("/root"),
        );
        assert_eq!(result, Some("/parent.md".to_owned()));
    }

    #[test]
    fn rewrite_url_dotdot_escaping_root_returns_none() {
        // ../../outside.md from /root/sub → resolves above /root → None
        let result = rewrite_url(
            "../../outside.md",
            Path::new("/root/sub"),
            Path::new("/root"),
        );
        assert!(result.is_none(), "path escaping root must return None");
    }

    // --- bd-2ag: cross-directory link resolution with broad and narrow serve_root ---

    #[test]
    fn rewrite_url_cross_dir_broad_root_allows() {
        // serve_root = /tmp (broad), file_dir = /tmp/docs
        // link: ../other/b.md → resolves to /tmp/other/b.md
        // strip_prefix(/tmp) = other/b.md → "/other/b.md" (inside broad root → ALLOWED)
        let result = rewrite_url(
            "../other/b.md",
            Path::new("/tmp/docs"),
            Path::new("/tmp"),
        );
        assert_eq!(
            result,
            Some("/other/b.md".to_owned()),
            "cross-dir link inside broad root must rewrite to root-relative href"
        );
    }

    #[test]
    fn rewrite_url_cross_dir_narrow_root_blocks() {
        // serve_root = /tmp/docs (narrow), file_dir = /tmp/docs
        // link: ../other/b.md → resolves to /tmp/other/b.md
        // strip_prefix(/tmp/docs) fails (target escapes narrow root) → None (BLOCKED)
        let result = rewrite_url(
            "../other/b.md",
            Path::new("/tmp/docs"),
            Path::new("/tmp/docs"),
        );
        assert!(
            result.is_none(),
            "cross-dir link escaping narrow serve_root must return None"
        );
    }

    #[test]
    fn rewrite_url_sibling_dir_cwd_root_allows() {
        // serve_root = /workspace (CWD), file_dir = /workspace/docs
        // link: ../sibling/page.md → resolves to /workspace/sibling/page.md
        // strip_prefix(/workspace) = sibling/page.md → "/sibling/page.md" (ALLOWED)
        let result = rewrite_url(
            "../sibling/page.md",
            Path::new("/workspace/docs"),
            Path::new("/workspace"),
        );
        assert_eq!(
            result,
            Some("/sibling/page.md".to_owned()),
            "sibling-dir link inside CWD root must rewrite to root-relative href"
        );
    }

    // --- bd-t6w: CWD-root nested entry regression guards ---
    //
    // When serve_root = CWD (e.g. /workspace) and the entry file lives in a
    // subdirectory (e.g. /workspace/playground/README.md), relative links must
    // rewrite to hrefs that include the subdirectory prefix.  These tests will
    // fail if serve_root is inadvertently set to entry_file.parent() instead of
    // CWD.

    #[test]
    fn rewrite_nested_entry_sibling_link_cwd_root() {
        // serve_root = /workspace  (CWD)
        // file_path  = /workspace/playground/README.md
        // link: [t](subdir/nested.md) → /playground/subdir/nested.md
        let html = render_abs("[t](subdir/nested.md)\n", "/workspace", "playground/README.md");
        assert!(
            html.contains("href=\"/playground/subdir/nested.md\""),
            "expected /playground/subdir/nested.md, got: {html}"
        );
    }

    #[test]
    fn rewrite_nested_entry_dotdot_to_cwd_root() {
        // serve_root = /workspace
        // file_path  = /workspace/playground/README.md
        // link: [t](../code.md) resolves to /workspace/code.md → /code.md
        let html = render_abs("[t](../code.md)\n", "/workspace", "playground/README.md");
        assert!(
            html.contains("href=\"/code.md\""),
            "expected /code.md (dotdot from nested entry), got: {html}"
        );
    }

    #[test]
    fn rewrite_nested_entry_extensionless_link_cwd_root() {
        // serve_root = /workspace
        // file_path  = /workspace/playground/links.md
        // link: [t](subdir/doc) (extensionless) → /playground/subdir/doc
        let html = render_abs("[t](subdir/doc)\n", "/workspace", "playground/links.md");
        assert!(
            html.contains("href=\"/playground/subdir/doc\""),
            "expected /playground/subdir/doc (extensionless), got: {html}"
        );
    }

    #[test]
    fn rewrite_nested_entry_image_cwd_root() {
        // serve_root = /workspace
        // file_path  = /workspace/playground/README.md
        // image: ![img](img/logo.png) → /playground/img/logo.png
        let html = render_abs("![img](img/logo.png)\n", "/workspace", "playground/README.md");
        assert!(
            html.contains("src=\"/playground/img/logo.png\""),
            "expected /playground/img/logo.png (image rewrite), got: {html}"
        );
    }

    #[test]
    fn rewrite_root_level_entry_unchanged_by_cwd_root() {
        // When the entry IS at the root level the behavior must be identical
        // regardless of whether serve_root is the entry's parent or CWD.
        // serve_root = /workspace
        // file_path  = /workspace/README.md (root-level entry)
        // link: [t](docs/page.md) → /docs/page.md
        let html = render_abs("[t](docs/page.md)\n", "/workspace", "README.md");
        assert!(
            html.contains("href=\"/docs/page.md\""),
            "expected /docs/page.md for root-level entry, got: {html}"
        );
    }

    // --- bd-1fc: backlinks panel, change-notice, and CSS audit ---
    //
    // 'Back to' audit: grep of serve.rs and html.rs found no 'back to' / 'back-to'
    // UI strings or HTML patterns.  All occurrences are code comments that say
    // "fall back to …" which are not UI artifacts.  No removal required.

    #[test]
    fn backlinks_panel_populated() {
        let bls = vec![
            BacklinkRef {
                source_url_path: "/docs/a.md".to_owned(),
                source_display: "Doc A".to_owned(),
                snippet: "see <also> here".to_owned(),
                target_fragment: None,
            },
            BacklinkRef {
                source_url_path: "/docs/b.md".to_owned(),
                source_display: "Doc B".to_owned(),
                snippet: "another ref".to_owned(),
                target_fragment: Some("section-1".to_owned()),
            },
        ];
        let (html_body, headings) = render("# Hi\n");
        let page = build_page_shell(
            &html_body,
            &headings,
            Path::new("/r/f.md"),
            Path::new("/r"),
            &PageShellContext { backlinks: &bls, file_mtime_secs: None, page_url_path: None },
        );
        // Header label with count (2 backlink refs supplied)
        assert!(
            page.contains(">Backlinks (2)<"),
            "populated panel must show header with count, got: {page}"
        );
        // Source link for item without fragment
        assert!(
            page.contains("href=\"/docs/a.md\""),
            "first backlink href, got: {page}"
        );
        // Source link for item with fragment
        assert!(
            page.contains("href=\"/docs/b.md#section-1\""),
            "second backlink href with fragment, got: {page}"
        );
        // Fragment hint span
        assert!(
            page.contains("backlinks-fragment"),
            "fragment span class, got: {page}"
        );
        // HTML-escaped snippet
        assert!(
            page.contains("see &lt;also&gt; here"),
            "snippet must be html-escaped, got: {page}"
        );
        // Section element
        assert!(
            page.contains("<section class=\"backlinks-panel\""),
            "section element with correct class, got: {page}"
        );
    }

    #[test]
    fn backlinks_panel_empty() {
        let (html_body, headings) = render("# Hi\n");
        let page = build_page_shell(
            &html_body,
            &headings,
            Path::new("/r/f.md"),
            Path::new("/r"),
            &PageShellContext { backlinks: &[], file_mtime_secs: None, page_url_path: None },
        );
        assert!(
            !page.contains("backlinks-panel"),
            "empty state must render no backlinks section, got: {page}"
        );
        assert!(
            !page.contains("No backlinks yet."),
            "empty state must not show 'No backlinks yet.' text, got: {page}"
        );
        assert!(
            !page.contains("backlinks-empty"),
            "empty state must not render aside, got: {page}"
        );
    }

    #[test]
    fn change_notice_present_and_hidden() {
        let (html_body, headings) = render("# Hi\n");
        let page = build_page_shell(
            &html_body,
            &headings,
            Path::new("/r/f.md"),
            Path::new("/r"),
            &PageShellContext { backlinks: &[], file_mtime_secs: None, page_url_path: None },
        );
        assert!(
            page.contains("id=\"mdmd-change-notice\""),
            "change notice id, got: {page}"
        );
        assert!(
            page.contains("hidden"),
            "change notice hidden attribute, got: {page}"
        );
    }

    // -----------------------------------------------------------------------
    // bd-3oh.2: PageShellContext / meta tag / backlinks HTML contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn page_shell_mtime_meta_tag_emitted() {
        // Test 8: file_mtime_secs = Some(12345), page_url_path = Some("docs/test.md")
        // → HTML contains content="12345" and content="docs/test.md".
        let (html_body, headings) = render("# Test\n");
        let ctx = PageShellContext {
            backlinks: &[],
            file_mtime_secs: Some(12345),
            page_url_path: Some("docs/test.md"),
        };
        let page = build_page_shell(
            &html_body,
            &headings,
            Path::new("/r/docs/test.md"),
            Path::new("/r"),
            &ctx,
        );
        assert!(
            page.contains("name=\"mdmd-mtime\""),
            "mdmd-mtime meta name must be present, got: {page}"
        );
        assert!(
            page.contains("content=\"12345\""),
            "mtime meta content must equal 12345, got: {page}"
        );
        assert!(
            page.contains("content=\"docs/test.md\""),
            "path meta content must equal docs/test.md, got: {page}"
        );
    }

    #[test]
    fn page_shell_no_mtime_meta_tag_when_none() {
        // Test 9: file_mtime_secs = None → HTML must NOT contain mdmd-mtime meta tag.
        let (html_body, headings) = render("# Test\n");
        let ctx = PageShellContext {
            backlinks: &[],
            file_mtime_secs: None,
            page_url_path: None,
        };
        let page = build_page_shell(
            &html_body,
            &headings,
            Path::new("/r/f.md"),
            Path::new("/r"),
            &ctx,
        );
        assert!(
            !page.contains("mdmd-mtime"),
            "mdmd-mtime meta tag must be absent when file_mtime_secs is None, got: {page}"
        );
    }

    #[test]
    fn backlinks_source_display_as_link_text() {
        // Test 11: source_display = "My Title" → HTML contains ">My Title</a>".
        let bls = vec![BacklinkRef {
            source_url_path: "/a.md".to_owned(),
            source_display: "My Title".to_owned(),
            snippet: "some context".to_owned(),
            target_fragment: None,
        }];
        let (html_body, headings) = render("# Hi\n");
        let page = build_page_shell(
            &html_body,
            &headings,
            Path::new("/r/f.md"),
            Path::new("/r"),
            &PageShellContext {
                backlinks: &bls,
                file_mtime_secs: None,
                page_url_path: None,
            },
        );
        assert!(
            page.contains(">My Title</a>"),
            "source_display must be rendered as exact link text, got: {page}"
        );
    }

    #[test]
    fn backlinks_source_display_path_fallback() {
        // Test 12: source_display = "docs/a.md" (path fallback) → HTML contains ">docs/a.md</a>".
        let bls = vec![BacklinkRef {
            source_url_path: "/docs/a.md".to_owned(),
            source_display: "docs/a.md".to_owned(),
            snippet: "context".to_owned(),
            target_fragment: None,
        }];
        let (html_body, headings) = render("# Hi\n");
        let page = build_page_shell(
            &html_body,
            &headings,
            Path::new("/r/f.md"),
            Path::new("/r"),
            &PageShellContext {
                backlinks: &bls,
                file_mtime_secs: None,
                page_url_path: None,
            },
        );
        assert!(
            page.contains(">docs/a.md</a>"),
            "path-fallback source_display must be rendered as link text, got: {page}"
        );
    }

    #[test]
    fn backlinks_xss_escaping() {
        // Test 13: source_display with XSS payload → escaped;
        // snippet with pre-existing & entity → double-escaped (&amp;amp;).
        let bls = vec![BacklinkRef {
            source_url_path: "/a.md".to_owned(),
            source_display: "<script>xss</script>".to_owned(),
            snippet: "&amp;".to_owned(), // & → &amp;amp; after html_escape
            target_fragment: None,
        }];
        let (html_body, headings) = render("# Hi\n");
        let page = build_page_shell(
            &html_body,
            &headings,
            Path::new("/r/f.md"),
            Path::new("/r"),
            &PageShellContext {
                backlinks: &bls,
                file_mtime_secs: None,
                page_url_path: None,
            },
        );
        // source_display: <script>xss</script> → &lt;script&gt;xss&lt;/script&gt;
        assert!(
            page.contains("&lt;script&gt;"),
            "< in source_display must be html-escaped, got: {page}"
        );
        // The page shell contains trusted <script> tags (FOUC prevention, Mermaid CDN,
        // mdmd.js), but user-supplied data must never produce a raw <script>xss</script>.
        assert!(
            !page.contains("<script>xss</script>"),
            "raw user-supplied <script> payload must be escaped, got: {page}"
        );
        // snippet: &amp; → html_escape converts & → &amp;, producing &amp;amp;
        assert!(
            page.contains("&amp;amp;"),
            "& in snippet must produce &amp;amp; (double-escaped), got: {page}"
        );
    }
}
