//! HTML rendering module for serve mode.
//!
//! Converts markdown text to HTML using comrak with GFM extensions.
//! Heading metadata (level, text, anchor ID) is extracted for TOC construction.
//!
//! The TUI parse/render path (`parse.rs`, `render.rs`) is not touched here.

use std::collections::HashMap;
use std::path::Path;

use comrak::{
    arena_tree::NodeEdge,
    nodes::{AstNode, NodeValue},
    Arena, Options, format_html, parse_document,
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
        let anchor = heading.anchor_id.as_str();   // anchor_id is already a URL-safe slug
        let text = html_escape(&heading.text);
        html.push_str(&format!(
            "<li class=\"{class}\"><a href=\"#{anchor}\">{text}</a></li>\n",
        ));
    }
    html.push_str("</ul>\n");
    html
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render a markdown string to HTML and extract heading metadata.
///
/// # Parameters
/// - `input`: raw markdown source.
/// - `file_path`: path of the source file. Used for logging; also available to
///   the link-rewriting pass in bd-1p6 during the same traversal.
/// - `serve_root`: root directory of the serve tree. Available to bd-1p6.
///
/// # Returns
/// `(html, headings)` where `html` is the full HTML string and `headings` is
/// the ordered list of [`HeadingEntry`] values for TOC construction.
///
/// Logs `[render] path=<file> headings=<count>` at info level.
pub fn render_markdown(
    input: &str,
    file_path: &Path,
    _serve_root: &Path,
) -> (String, Vec<HeadingEntry>) {
    let arena = Arena::new();
    let options = make_options();
    let root = parse_document(&arena, input, &options);

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
    format_html(root, &options, &mut html_bytes)
        .expect("comrak HTML formatting should not fail");
    let html = String::from_utf8(html_bytes).expect("comrak output must be valid UTF-8");

    eprintln!(
        "[render] path={} headings={}",
        file_path.display(),
        entries.len()
    );

    (html, entries)
}

/// Build the full HTML page shell: `<!DOCTYPE html>` with header, sticky TOC
/// sidebar, and rendered content area.
///
/// # Parameters
/// - `body_html`: the raw HTML fragment produced by `render_markdown`.
/// - `headings`: ordered heading entries for the TOC (from `render_markdown`).
/// - `file_path`: absolute path to the source `.md` file (for display).
/// - `serve_root`: root directory of the serve tree (used to compute the
///   relative display path shown in the header).
///
/// # Returns
/// A complete `text/html` document ready to send to the browser.
pub fn build_page_shell(
    body_html: &str,
    headings: &[HeadingEntry],
    file_path: &Path,
    serve_root: &Path,
) -> String {
    // Relative path for the header, falling back to the full path.
    let display_path = file_path
        .strip_prefix(serve_root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| file_path.display().to_string());

    // Page title: first H1 text, then file stem, then a safe default.
    let title_raw = headings
        .iter()
        .find(|h| h.level == 1)
        .map(|h| h.text.as_str())
        .or_else(|| file_path.file_stem().and_then(|s| s.to_str()))
        .unwrap_or("Document");

    let title = html_escape(title_raw);
    let display_path_escaped = html_escape(&display_path);
    let content_html = inject_heading_ids(body_html, headings);
    let toc_html = build_toc_html(headings);

    format!(
        "<!DOCTYPE html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{title}</title>\n\
<link rel=\"stylesheet\" href=\"/assets/mdmd.css\">\n\
</head>\n\
<body>\n\
<header class=\"site-header\">\n\
<span class=\"brand\">mdmd serve</span>\n\
<span class=\"file-path\">{display_path_escaped}</span>\n\
</header>\n\
<div class=\"layout\">\n\
<nav class=\"toc-sidebar\">\n\
{toc_html}</nav>\n\
<main class=\"content\">\n\
{content_html}</main>\n\
</div>\n\
<script src=\"/assets/mdmd.js\"></script>\n\
</body>\n\
</html>\n"
    )
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
            HeadingEntry { level: 1, text: "Title".into(), anchor_id: "title".into() },
            HeadingEntry { level: 2, text: "Section".into(), anchor_id: "section".into() },
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
            HeadingEntry { level: 2, text: "Alpha".into(), anchor_id: "alpha".into() },
            HeadingEntry { level: 2, text: "Beta".into(), anchor_id: "beta".into() },
        ];
        let result = inject_heading_ids(html, &headings);
        assert!(result.contains("<h2 id=\"alpha\">Alpha</h2>"), "first h2 id=alpha");
        assert!(result.contains("<h2 id=\"beta\">Beta</h2>"), "second h2 id=beta");
    }

    // --- build_page_shell ---

    #[test]
    fn page_shell_contains_nav_with_toc() {
        let input = "# Title\n\n## Section\n";
        let (html_body, headings) = render(input);
        let page = build_page_shell(&html_body, &headings, Path::new("/root/doc.md"), Path::new("/root"));
        assert!(page.contains("<nav class=\"toc-sidebar\">"), "nav element present");
        assert!(page.contains("href=\"#title\""), "toc link to h1");
        assert!(page.contains("href=\"#section\""), "toc link to h2");
    }

    #[test]
    fn page_shell_contains_script_tag() {
        let (html_body, headings) = render("# Hi\n");
        let page = build_page_shell(&html_body, &headings, Path::new("/r/f.md"), Path::new("/r"));
        assert!(page.contains("<script src=\"/assets/mdmd.js\">"), "script tag present");
    }

    #[test]
    fn page_shell_contains_css_link() {
        let (html_body, headings) = render("# Hi\n");
        let page = build_page_shell(&html_body, &headings, Path::new("/r/f.md"), Path::new("/r"));
        assert!(page.contains("href=\"/assets/mdmd.css\""), "css link present");
    }

    #[test]
    fn page_shell_shows_relative_file_path() {
        let (html_body, headings) = render("# Hi\n");
        let page = build_page_shell(
            &html_body,
            &headings,
            Path::new("/docs/guide/intro.md"),
            Path::new("/docs"),
        );
        assert!(page.contains("guide/intro.md"), "relative path shown in header");
    }

    #[test]
    fn page_shell_heading_ids_injected() {
        let input = "# Title\n\n## Sub\n";
        let (html_body, headings) = render(input);
        let page = build_page_shell(&html_body, &headings, Path::new("/r/f.md"), Path::new("/r"));
        assert!(page.contains("<h1 id=\"title\">"), "h1 id injected in content");
        assert!(page.contains("<h2 id=\"sub\">"), "h2 id injected in content");
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
}
