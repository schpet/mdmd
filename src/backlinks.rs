use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A reference to this document from another document (a "backlink").
#[derive(Debug, Clone)]
pub struct BacklinkRef {
    /// Root-relative URL path to the source document, with leading slash.
    /// Example: `/docs/a.md`
    pub source_url_path: String,
    /// Display name: first H1 title if available, else rel path without leading slash.
    pub source_display: String,
    /// Short context snippet around the link (~80 chars before/after, whitespace-collapsed,
    /// max 200 chars).
    pub snippet: String,
    /// Optional fragment (without `#`) from the original link, for display and href construction.
    pub target_fragment: Option<String>,
}

/// An outbound link discovered in a source document during index build.
#[derive(Debug, Clone)]
pub(crate) struct OutboundRef {
    /// Root-relative URL path of the target document, with leading slash.
    /// Example: `/docs/b.md`
    pub target_url_path: String,
    /// Optional fragment from the original link (without `#`).
    pub target_fragment: Option<String>,
    /// Context snippet around the link text (~80 chars before/after,
    /// whitespace-collapsed, max 200 chars).
    pub snippet: String,
}

/// Result of extracting outbound links and metadata from a single document.
#[derive(Debug, Default)]
pub(crate) struct DocExtractResult {
    /// First H1 heading text found in the document, if any.
    pub title: Option<String>,
    /// Outbound local markdown links discovered in the document.
    pub outbound_refs: Vec<OutboundRef>,
}

/// Convert a root-relative path string (no leading slash) to a canonical URL key.
///
/// This function is used at both index-build time and request-lookup time to
/// guarantee key format parity and prevent index/lookup drift.
///
/// # Examples
///
/// ```
/// use mdmd::backlinks::url_key_from_rel_path;
/// assert_eq!(url_key_from_rel_path("docs/readme.md"), "/docs/readme.md");
/// assert_eq!(url_key_from_rel_path("readme.md"), "/readme.md");
/// assert_eq!(url_key_from_rel_path(""), "/");
/// ```
pub fn url_key_from_rel_path(rel: &str) -> String {
    format!("/{rel}")
}

/// In-memory backlinks index type.
///
/// Keys are root-relative URL paths with leading slash (e.g. `/docs/readme.md`).
/// Values are all [`BacklinkRef`]s from other documents that link to that target.
pub type BacklinksIndex = HashMap<String, Vec<BacklinkRef>>;

/// Build the in-memory backlinks index by traversing `serve_root` and
/// extracting outbound links from all markdown files.
///
/// # Traversal rules
///
/// - Recursively visits all directories under `serve_root`.
/// - Skips directories named `.git`, `node_modules`, and `.jj`.
/// - Processes only files with `.md` or `.markdown` extensions.
/// - On read error, emits one `eprintln!` line and continues to the next file.
///
/// # Index construction
///
/// For each outbound link found in a source file a [`BacklinkRef`] is inserted
/// into the index under the target's URL key.  Self-links (source URL ==
/// target URL) are silently filtered out.
///
/// # Output
///
/// After the full traversal emits:
/// - `eprintln!("[backlinks] indexed files={} edges={}", …)` to stderr
/// - `println!("backlinks: startup-indexed; restart server after file edits to pick up changes")` to stdout
pub fn build_backlinks_index(serve_root: &Path) -> BacklinksIndex {
    use std::collections::VecDeque;
    use std::fs;

    let mut index: BacklinksIndex = HashMap::new();
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    queue.push_back(serve_root.to_path_buf());

    let mut file_count: usize = 0;
    let mut edge_count: usize = 0;

    while let Some(dir) = queue.pop_front() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                eprintln!(
                    "[backlinks] skipping path='{}' reason='read-error: {}'",
                    dir.display(),
                    e
                );
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();

            if path.is_dir() {
                // Skip well-known VCS and dependency directories.
                let dir_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                if matches!(dir_name, ".git" | "node_modules" | ".jj") {
                    continue;
                }
                queue.push_back(path);
                continue;
            }

            // Only process .md and .markdown files.
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if !matches!(ext, "md" | "markdown") {
                continue;
            }

            // Read the file contents; skip on error.
            let src = match fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "[backlinks] skipping path='{}' reason='read-error: {}'",
                        path.display(),
                        e
                    );
                    continue;
                }
            };

            file_count += 1;

            // Extract outbound links and title.
            let extracted = extract_outbound_links(&src, &path, serve_root);

            // Compute the source URL key.
            let source_rel = path
                .strip_prefix(serve_root)
                .ok()
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let source_url_path = url_key_from_rel_path(&source_rel);

            // Display name: H1 title when present, else rel path without leading slash.
            let source_display = extracted
                .title
                .clone()
                .unwrap_or_else(|| source_rel.clone());

            // Invert edges into the index, filtering self-links.
            for outbound in &extracted.outbound_refs {
                if outbound.target_url_path == source_url_path {
                    continue; // self-link – skip
                }
                edge_count += 1;
                index
                    .entry(outbound.target_url_path.clone())
                    .or_default()
                    .push(BacklinkRef {
                        source_url_path: source_url_path.clone(),
                        source_display: source_display.clone(),
                        snippet: outbound.snippet.clone(),
                        target_fragment: outbound.target_fragment.clone(),
                    });
            }
        }
    }

    eprintln!(
        "[backlinks] indexed files={} edges={}",
        file_count, edge_count
    );
    println!("backlinks: startup-indexed; restart server after file edits to pick up changes");

    index
}

/// Normalize an absolute file-system path by resolving `.` and `..` components
/// using a stack-based approach.
///
/// Returns `None` if a `..` would pop above the filesystem root (path traversal
/// beyond `/`).  On non-Unix platforms the leading separator is preserved by
/// starting with an empty first segment representing the root.
fn normalize_abs_path(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    let mut has_root = false;
    for comp in path.components() {
        match comp {
            Component::RootDir => {
                has_root = true;
            }
            Component::Normal(name) => parts.push(name.to_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    // Would go above root – treat as path-traversal, reject.
                    return None;
                }
            }
            Component::Prefix(_) => {
                // Windows drive prefix; preserve as-is.
                has_root = true;
            }
        }
    }
    let mut result = PathBuf::new();
    if has_root {
        result.push("/");
    }
    for part in parts {
        result.push(part);
    }
    Some(result)
}

/// Extract outbound local links and the first H1 title from a markdown source.
///
/// # Arguments
///
/// - `src` – valid UTF-8 markdown source text.
/// - `source_path` – absolute path to the file `src` was read from (used to
///   resolve relative link targets).
/// - `serve_root` – absolute path to the serve root; links that resolve to
///   targets outside this directory are silently dropped.
///
/// # Returns
///
/// A [`DocExtractResult`] containing:
/// - `title`: the plain-text of the first H1 heading, or `None`.
/// - `outbound_refs`: all local links whose resolved targets lie inside
///   `serve_root`, keyed by root-relative URL path.
///
/// Self-links (source URL == target URL) are included here and must be filtered
/// out by the caller during index inversion (see bd-1hd).
pub(crate) fn extract_outbound_links(
    src: &str,
    source_path: &Path,
    serve_root: &Path,
) -> DocExtractResult {
    use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

    let source_parent = source_path.parent().unwrap_or(source_path);
    let src_len = src.len();

    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let parser = Parser::new_ext(src, options).into_offset_iter();

    let mut result = DocExtractResult::default();
    let mut in_h1 = false;
    let mut h1_done = false;
    let mut title_buf = String::new();

    // Pending state for the link currently being processed.
    let mut link_byte_start: Option<usize> = None;
    let mut link_dest: Option<String> = None;

    for (event, range) in parser {
        match event {
            // --- H1 title extraction ---
            Event::Start(Tag::Heading {
                level: HeadingLevel::H1,
                ..
            }) if !h1_done => {
                in_h1 = true;
                title_buf.clear();
            }
            Event::Text(ref text) if in_h1 => {
                title_buf.push_str(text);
            }
            Event::End(TagEnd::Heading(HeadingLevel::H1)) if in_h1 => {
                result.title = Some(title_buf.trim().to_owned());
                in_h1 = false;
                h1_done = true;
            }

            // --- Link extraction ---
            Event::Start(Tag::Link { ref dest_url, .. }) => {
                link_byte_start = Some(range.start);
                link_dest = Some(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                let ls = match link_byte_start.take() {
                    Some(s) => s,
                    None => continue,
                };
                let dest = match link_dest.take() {
                    Some(d) => d,
                    None => continue,
                };
                let le = range.end;

                // Filter out external schemes and bare-fragment links.
                let low = dest.to_lowercase();
                if low.starts_with("http:")
                    || low.starts_with("https:")
                    || low.starts_with("mailto:")
                    || low.starts_with("ftp:")
                    || dest.starts_with('#')
                {
                    continue;
                }

                // Split on the first `#` to separate path and fragment.
                let (path_part, fragment) = match dest.split_once('#') {
                    Some((p, f)) => (p, if f.is_empty() { None } else { Some(f.to_owned()) }),
                    None => (dest.as_str(), None),
                };

                // Fragment-only links (path_part is empty after split) are skipped.
                if path_part.is_empty() {
                    continue;
                }

                // Resolve the path component to an absolute file-system path.
                let raw = if path_part.starts_with('/') {
                    serve_root.join(path_part.trim_start_matches('/'))
                } else {
                    source_parent.join(path_part)
                };

                // Normalize `.` and `..` using a stack-based clean.
                let resolved = match normalize_abs_path(&raw) {
                    Some(p) => p,
                    None => continue, // path-traversal above root – silently drop
                };

                // Outside-root drop: silently discard targets that are not
                // under serve_root (strip_prefix returns Err in that case).
                let rel = match resolved.strip_prefix(serve_root) {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                // Compute the canonical URL key for this target.
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                let target_url_path = url_key_from_rel_path(&rel_str);

                // Build the context snippet: ~80 bytes before/after the link,
                // whitespace-collapsed, capped at 200 chars.
                // Adjust to char boundaries so we never slice mid-multibyte-char.
                let mut snippet_start = ls.saturating_sub(80);
                while snippet_start > 0 && !src.is_char_boundary(snippet_start) {
                    snippet_start -= 1;
                }
                let mut snippet_end = le.saturating_add(80).min(src_len);
                while snippet_end < src_len && !src.is_char_boundary(snippet_end) {
                    snippet_end += 1;
                }
                let raw_snippet = &src[snippet_start..snippet_end];
                let collapsed = raw_snippet
                    .split_ascii_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                let snippet = if collapsed.len() > 200 {
                    collapsed[..200].to_owned()
                } else {
                    collapsed
                };

                result.outbound_refs.push(OutboundRef {
                    target_url_path,
                    target_fragment: fragment,
                    snippet,
                });
            }

            _ => {}
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_key_adds_leading_slash() {
        assert_eq!(url_key_from_rel_path("docs/readme.md"), "/docs/readme.md");
        assert_eq!(url_key_from_rel_path("readme.md"), "/readme.md");
        assert_eq!(url_key_from_rel_path(""), "/");
    }

    #[test]
    fn url_key_nested_path() {
        assert_eq!(url_key_from_rel_path("a/b/c.md"), "/a/b/c.md");
    }

    #[test]
    fn backlink_ref_fields_accessible() {
        let r = BacklinkRef {
            source_url_path: "/a.md".to_string(),
            source_display: "A Doc".to_string(),
            snippet: "some context".to_string(),
            target_fragment: Some("section".to_string()),
        };
        assert_eq!(r.source_url_path, "/a.md");
        assert_eq!(r.source_display, "A Doc");
        assert_eq!(r.snippet, "some context");
        assert_eq!(r.target_fragment.as_deref(), Some("section"));
    }

    #[test]
    fn backlink_ref_no_fragment() {
        let r = BacklinkRef {
            source_url_path: "/b.md".to_string(),
            source_display: "b.md".to_string(),
            snippet: "".to_string(),
            target_fragment: None,
        };
        assert!(r.target_fragment.is_none());
    }

    #[test]
    fn backlinks_index_type_works() {
        let mut idx: BacklinksIndex = HashMap::new();
        idx.insert(
            "/target.md".to_string(),
            vec![BacklinkRef {
                source_url_path: "/source.md".to_string(),
                source_display: "Source".to_string(),
                snippet: "see [target](target.md)".to_string(),
                target_fragment: None,
            }],
        );
        assert_eq!(idx["/target.md"].len(), 1);
        assert_eq!(idx["/target.md"][0].source_url_path, "/source.md");
    }

    // -----------------------------------------------------------------------
    // build_backlinks_index unit tests
    // -----------------------------------------------------------------------

    use tempfile::TempDir;

    /// Create a file at `root/rel_path` with `contents`, creating parent dirs
    /// as needed.  Returns the absolute `PathBuf` of the created file.
    fn write_fixture(root: &TempDir, rel_path: &str, contents: &str) -> std::path::PathBuf {
        let full = root.path().join(rel_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, contents).unwrap();
        full
    }

    #[test]
    fn build_index_basic_inversion() {
        // a.md → b.md; b.md should have one backlink from a.md.
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "a.md", "# A Doc\n\nSee [B](b.md).\n");
        write_fixture(&tmp, "b.md", "# B Doc\n\nNo outbound links.\n");

        let idx = build_backlinks_index(tmp.path());

        let refs = idx.get("/b.md").expect("b.md should have a backlink");
        assert_eq!(refs.len(), 1, "b.md should have exactly one backlink");
        let r = &refs[0];
        assert_eq!(r.source_url_path, "/a.md");
        assert_eq!(r.source_display, "A Doc", "source_display should be H1 title");
        assert!(!r.snippet.is_empty(), "snippet should not be empty");
    }

    #[test]
    fn build_index_no_entry_for_a_when_only_outbound() {
        // a.md links to b.md; a.md itself should have no backlinks.
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "a.md", "See [B](b.md).\n");
        write_fixture(&tmp, "b.md", "# B\n");

        let idx = build_backlinks_index(tmp.path());

        assert!(
            !idx.contains_key("/a.md"),
            "a.md has no inbound links so it must not appear as a key"
        );
    }

    #[test]
    fn build_index_self_links_excluded() {
        // a.md links to itself; self-link must not appear in the index.
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "a.md", "# Self\n\nLink to [self](a.md).\n");

        let idx = build_backlinks_index(tmp.path());

        assert!(
            !idx.contains_key("/a.md"),
            "self-link must not produce a backlink entry"
        );
    }

    #[test]
    fn build_index_source_display_fallback_to_path() {
        // a.md has no H1 title; source_display should fall back to rel path.
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "a.md", "No heading here.\n\nSee [B](b.md).\n");
        write_fixture(&tmp, "b.md", "# B\n");

        let idx = build_backlinks_index(tmp.path());

        let refs = idx.get("/b.md").expect("b.md must have a backlink");
        assert_eq!(refs[0].source_display, "a.md", "should fall back to rel path");
    }

    #[test]
    fn build_index_git_dir_excluded() {
        // .git/some.md must not be indexed.
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "real.md", "# Real\n");
        write_fixture(&tmp, ".git/secret.md", "# Git internals\n\nSee [real](../real.md).\n");

        let idx = build_backlinks_index(tmp.path());

        // real.md must not receive a backlink from .git/secret.md
        assert!(
            !idx.contains_key("/real.md"),
            ".git directory must be skipped; real.md must not have backlinks"
        );
    }

    #[test]
    fn build_index_node_modules_excluded() {
        // node_modules/dep.md must not be indexed.
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "main.md", "# Main\n");
        write_fixture(
            &tmp,
            "node_modules/dep.md",
            "# Dep\n\nSee [main](../main.md).\n",
        );

        let idx = build_backlinks_index(tmp.path());

        assert!(
            !idx.contains_key("/main.md"),
            "node_modules directory must be skipped"
        );
    }

    #[test]
    fn build_index_jj_dir_excluded() {
        // .jj/internal.md must not be indexed.
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "doc.md", "# Doc\n");
        write_fixture(&tmp, ".jj/internal.md", "# JJ\n\nSee [doc](../doc.md).\n");

        let idx = build_backlinks_index(tmp.path());

        assert!(
            !idx.contains_key("/doc.md"),
            ".jj directory must be skipped"
        );
    }

    #[test]
    fn build_index_non_markdown_files_skipped() {
        // Only .md and .markdown files should be processed.
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "target.md", "# Target\n");
        write_fixture(&tmp, "source.txt", "See [target](target.md).\n");
        write_fixture(&tmp, "source.html", "<a href=\"target.md\">target</a>\n");

        let idx = build_backlinks_index(tmp.path());

        // target.md has no .md/.markdown sources linking to it → no entry
        assert!(
            !idx.contains_key("/target.md"),
            "non-markdown files must not contribute backlinks"
        );
    }

    #[test]
    fn build_index_dot_markdown_extension() {
        // .markdown extension should be processed.
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "source.markdown", "See [target](target.md).\n");
        write_fixture(&tmp, "target.md", "# Target\n");

        let idx = build_backlinks_index(tmp.path());

        assert!(
            idx.contains_key("/target.md"),
            ".markdown extension files must be indexed"
        );
        assert_eq!(idx["/target.md"][0].source_url_path, "/source.markdown");
    }

    #[test]
    fn build_index_subdirectory_links() {
        // docs/a.md → docs/b.md: verify key paths include the subdirectory.
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "docs/a.md", "# A\n\nSee [B](b.md).\n");
        write_fixture(&tmp, "docs/b.md", "# B\n");

        let idx = build_backlinks_index(tmp.path());

        let refs = idx
            .get("/docs/b.md")
            .expect("docs/b.md must have a backlink");
        assert_eq!(refs[0].source_url_path, "/docs/a.md");
    }

    #[test]
    fn build_index_multiple_sources_to_same_target() {
        // Both a.md and b.md link to target.md; target should have two backlinks.
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "a.md", "# A\n\nSee [T](target.md).\n");
        write_fixture(&tmp, "b.md", "# B\n\nAlso [T](target.md).\n");
        write_fixture(&tmp, "target.md", "# Target\n");

        let idx = build_backlinks_index(tmp.path());

        let refs = idx
            .get("/target.md")
            .expect("target.md must have backlinks");
        assert_eq!(refs.len(), 2, "target.md must have exactly two backlinks");
        let mut sources: Vec<&str> = refs.iter().map(|r| r.source_url_path.as_str()).collect();
        sources.sort_unstable();
        assert_eq!(sources, ["/a.md", "/b.md"]);
    }
}
