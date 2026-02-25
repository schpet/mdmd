use std::collections::HashMap;
use std::path::PathBuf;

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
    /// Absolute path to the target file.
    pub target_path: PathBuf,
    /// Optional fragment from the original link (without `#`).
    pub fragment: Option<String>,
    /// Context snippet around the link text.
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
}
