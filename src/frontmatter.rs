#![allow(dead_code)]

use std::borrow::Cow;

use serde_yml::Value;

const MAX_DEPTH: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractResult<'a> {
    pub body: &'a str,
    pub render_body: Cow<'a, str>,
    pub meta: Option<FrontmatterMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontmatterMeta {
    pub fields: Vec<FrontmatterField>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontmatterField {
    pub key: String,
    pub value: MetaValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaValue {
    Scalar(String),
    Null,
    Sequence(Vec<MetaValue>),
    Mapping(Vec<FrontmatterField>),
}

pub fn extract(source: &str) -> ExtractResult<'_> {
    let Some((first_line, after_open)) = logical_line_at(source, 0) else {
        return unchanged(source);
    };
    if first_line != "---" {
        return unchanged(source);
    }

    let mut cursor = after_open;
    while let Some((line, next_cursor)) = logical_line_at(source, cursor) {
        if line == "---" || line == "..." {
            if cursor == after_open {
                return ExtractResult {
                    body: &source[next_cursor..],
                    render_body: Cow::Borrowed(&source[next_cursor..]),
                    meta: None,
                };
            }

            let frontmatter_slice = &source[after_open..cursor];
            let parsed = match serde_yml::from_str::<Value>(frontmatter_slice) {
                Ok(value) => value,
                Err(_) => {
                    return invalid_frontmatter(source, after_open, Some((cursor, next_cursor)))
                }
            };

            let Value::Mapping(mapping) = parsed else {
                return invalid_frontmatter(source, after_open, Some((cursor, next_cursor)));
            };

            let Some(meta) = normalize_root_mapping(mapping) else {
                return invalid_frontmatter(source, after_open, Some((cursor, next_cursor)));
            };

            return ExtractResult {
                body: &source[next_cursor..],
                render_body: Cow::Borrowed(&source[next_cursor..]),
                meta: Some(meta),
            };
        }
        cursor = next_cursor;
    }

    invalid_frontmatter(source, after_open, None)
}

fn unchanged(source: &str) -> ExtractResult<'_> {
    ExtractResult {
        body: source,
        render_body: Cow::Borrowed(source),
        meta: None,
    }
}

fn invalid_frontmatter<'a>(
    source: &'a str,
    after_open: usize,
    closing_line: Option<(usize, usize)>,
) -> ExtractResult<'a> {
    ExtractResult {
        body: source,
        render_body: Cow::Owned(escape_delimiter_lines(source, after_open, closing_line)),
        meta: None,
    }
}

fn escape_delimiter_lines(
    source: &str,
    after_open: usize,
    closing_line: Option<(usize, usize)>,
) -> String {
    let mut escaped = String::with_capacity(source.len() + 2);
    escaped.push('\\');
    escaped.push_str(&source[..after_open]);

    match closing_line {
        Some((close_start, close_end)) => {
            escaped.push_str(&source[after_open..close_start]);
            escaped.push('\\');
            escaped.push_str(&source[close_start..close_end]);
            escaped.push_str(&source[close_end..]);
        }
        None => escaped.push_str(&source[after_open..]),
    }

    escaped
}

fn logical_line_at(source: &str, start: usize) -> Option<(&str, usize)> {
    if start > source.len() {
        return None;
    }
    if start == source.len() {
        return None;
    }

    let remainder = &source[start..];
    match remainder.find('\n') {
        Some(rel_end) => {
            let raw_line = &remainder[..rel_end];
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            Some((line, start + rel_end + 1))
        }
        None => {
            let raw_line = remainder;
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            Some((line, source.len()))
        }
    }
}

fn normalize_root_mapping(mapping: serde_yml::Mapping) -> Option<FrontmatterMeta> {
    let mut fields = Vec::with_capacity(mapping.len());
    let mut title = None;

    for (key, value) in mapping {
        let Value::String(key) = key else {
            return None;
        };

        if key == "title" {
            if let Value::String(value) = &value {
                title = Some(value.clone());
            }
        }

        let value = normalize_value(value, 0)?;
        fields.push(FrontmatterField { key, value });
    }

    Some(FrontmatterMeta { fields, title })
}

fn normalize_value(value: Value, depth: usize) -> Option<MetaValue> {
    match value {
        Value::Null => Some(MetaValue::Null),
        Value::Bool(boolean) => Some(MetaValue::Scalar(boolean.to_string())),
        Value::Number(number) => Some(MetaValue::Scalar(number.to_string())),
        Value::String(string) => Some(MetaValue::Scalar(string)),
        Value::Sequence(sequence) => {
            if depth >= MAX_DEPTH {
                return Some(MetaValue::Scalar(yaml_text(&Value::Sequence(sequence))));
            }

            let mut values = Vec::with_capacity(sequence.len());
            for item in sequence {
                values.push(normalize_value(item, depth + 1)?);
            }
            Some(MetaValue::Sequence(values))
        }
        Value::Mapping(mapping) => {
            if depth >= MAX_DEPTH {
                return Some(MetaValue::Scalar(yaml_text(&Value::Mapping(mapping))));
            }

            let mut fields = Vec::with_capacity(mapping.len());
            for (key, value) in mapping {
                let Value::String(key) = key else {
                    return None;
                };
                let value = normalize_value(value, depth + 1)?;
                fields.push(FrontmatterField { key, value });
            }
            Some(MetaValue::Mapping(fields))
        }
        other => Some(MetaValue::Scalar(yaml_text(&other))),
    }
}

fn yaml_text(value: &Value) -> String {
    let serialized = serde_yml::to_string(value).unwrap_or_default();
    let without_marker = serialized
        .strip_prefix("---\n")
        .or_else(|| serialized.strip_prefix("---\r\n"))
        .unwrap_or(&serialized);

    without_marker.trim_end_matches(['\r', '\n']).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_frontmatter_returns_original_source() {
        eprintln!("scenario: no frontmatter");
        let source = "# Heading\nbody\n";

        let result = extract(source);

        assert_eq!(
            result,
            ExtractResult {
                body: source,
                render_body: Cow::Borrowed(source),
                meta: None,
            }
        );
    }

    #[test]
    fn valid_mapping_extracts_metadata_and_body() {
        eprintln!("scenario: valid mapping with nested structures");
        let source = concat!(
            "---\n",
            "title: Doc title\n",
            "summary: short\n",
            "published: true\n",
            "count: 42\n",
            "details:\n",
            "  owner: team\n",
            "  nested:\n",
            "    child: value\n",
            "tags:\n",
            "  - alpha\n",
            "  - 2\n",
            "empty: null\n",
            "---\n",
            "\n",
            "# Body\n",
        );

        let result = extract(source);

        assert_eq!(result.body, "\n# Body\n");
        assert_eq!(result.render_body, "\n# Body\n");
        assert_eq!(
            result.meta,
            Some(FrontmatterMeta {
                title: Some("Doc title".to_string()),
                fields: vec![
                    FrontmatterField {
                        key: "title".to_string(),
                        value: MetaValue::Scalar("Doc title".to_string()),
                    },
                    FrontmatterField {
                        key: "summary".to_string(),
                        value: MetaValue::Scalar("short".to_string()),
                    },
                    FrontmatterField {
                        key: "published".to_string(),
                        value: MetaValue::Scalar("true".to_string()),
                    },
                    FrontmatterField {
                        key: "count".to_string(),
                        value: MetaValue::Scalar("42".to_string()),
                    },
                    FrontmatterField {
                        key: "details".to_string(),
                        value: MetaValue::Mapping(vec![
                            FrontmatterField {
                                key: "owner".to_string(),
                                value: MetaValue::Scalar("team".to_string()),
                            },
                            FrontmatterField {
                                key: "nested".to_string(),
                                value: MetaValue::Mapping(vec![FrontmatterField {
                                    key: "child".to_string(),
                                    value: MetaValue::Scalar("value".to_string()),
                                }]),
                            },
                        ]),
                    },
                    FrontmatterField {
                        key: "tags".to_string(),
                        value: MetaValue::Sequence(vec![
                            MetaValue::Scalar("alpha".to_string()),
                            MetaValue::Scalar("2".to_string()),
                        ]),
                    },
                    FrontmatterField {
                        key: "empty".to_string(),
                        value: MetaValue::Null,
                    },
                ],
            })
        );
    }

    #[test]
    fn empty_block_is_stripped() {
        eprintln!("scenario: empty block");
        let source = "---\n---\nbody\n";

        let result = extract(source);

        assert_eq!(result.body, "body\n");
        assert_eq!(result.render_body, "body\n");
        assert_eq!(result.meta, None);
    }

    #[test]
    fn malformed_yaml_falls_back_to_original_source() {
        eprintln!("scenario: malformed yaml");
        let source = "---\ntitle: [unterminated\n---\nbody\n";

        let result = extract(source);

        assert_eq!(result.body, source);
        assert_eq!(
            result.render_body,
            "\\---\ntitle: [unterminated\n\\---\nbody\n"
        );
        assert_eq!(result.meta, None);
    }

    #[test]
    fn unterminated_block_falls_back_to_original_source() {
        eprintln!("scenario: unterminated block");
        let source = "---\ntitle: nope\nbody\n";

        let result = extract(source);

        assert_eq!(result.body, source);
        assert_eq!(result.render_body, "\\---\ntitle: nope\nbody\n");
        assert_eq!(result.meta, None);
    }

    #[test]
    fn non_mapping_root_falls_back_to_original_source() {
        eprintln!("scenario: non-mapping root");
        let source = "---\n- one\n- two\n---\nbody\n";

        let result = extract(source);

        assert_eq!(result.body, source);
        assert_eq!(result.render_body, "\\---\n- one\n- two\n\\---\nbody\n");
        assert_eq!(result.meta, None);
    }

    #[test]
    fn non_string_root_key_falls_back_to_original_source() {
        eprintln!("scenario: non-string root key");
        let source = "---\n? [a, b]\n: nope\n---\nbody\n";

        let result = extract(source);

        assert_eq!(result.body, source);
        assert_eq!(result.render_body, "\\---\n? [a, b]\n: nope\n\\---\nbody\n");
        assert_eq!(result.meta, None);
    }

    #[test]
    fn non_string_nested_key_falls_back_to_original_source() {
        eprintln!("scenario: non-string nested key");
        let source = "---\nouter:\n  ? [a, b]\n  : nope\n---\nbody\n";

        let result = extract(source);

        assert_eq!(result.body, source);
        assert_eq!(
            result.render_body,
            "\\---\nouter:\n  ? [a, b]\n  : nope\n\\---\nbody\n"
        );
        assert_eq!(result.meta, None);
    }

    #[test]
    fn crlf_delimiters_and_body_are_preserved() {
        eprintln!("scenario: crlf preservation");
        let source = "---\r\ntitle: Doc\r\n---\r\n\r\nBody\r\n";

        let result = extract(source);

        assert_eq!(result.body, "\r\nBody\r\n");
        assert_eq!(
            result.meta.as_ref().and_then(|meta| meta.title.as_deref()),
            Some("Doc")
        );
    }

    #[test]
    fn dotdotdot_closing_delimiter_is_accepted() {
        eprintln!("scenario: dot closing delimiter");
        let source = "---\ntitle: Doc\n...\nbody\n";

        let result = extract(source);

        assert_eq!(result.body, "body\n");
        assert_eq!(
            result.meta.as_ref().and_then(|meta| meta.title.as_deref()),
            Some("Doc")
        );
    }

    #[test]
    fn field_order_is_preserved() {
        eprintln!("scenario: field order preservation");
        let source = "---\nfirst: 1\nsecond: 2\nthird: 3\n---\n";

        let result = extract(source);
        let keys: Vec<_> = result
            .meta
            .unwrap()
            .fields
            .into_iter()
            .map(|field| field.key)
            .collect();

        assert_eq!(keys, vec!["first", "second", "third"]);
    }

    #[test]
    fn plain_string_title_is_extracted() {
        eprintln!("scenario: plain string title");
        let source = "---\ntitle: Hello\n---\n";

        let result = extract(source);

        assert_eq!(
            result.meta.as_ref().and_then(|meta| meta.title.as_deref()),
            Some("Hello")
        );
    }

    #[test]
    fn non_string_title_is_ignored() {
        eprintln!("scenario: non-string title");
        let source = "---\ntitle:\n  nested: true\n---\n";

        let result = extract(source);

        assert_eq!(
            result.meta.as_ref().and_then(|meta| meta.title.as_deref()),
            None
        );
        assert!(matches!(
            result.meta.unwrap().fields[0].value,
            MetaValue::Mapping(_)
        ));
    }

    #[test]
    fn depth_cap_serializes_deep_subtree_into_scalar() {
        eprintln!("scenario: depth cap fallback");
        let source = concat!(
            "---\n",
            "outer:\n",
            "  middle:\n",
            "    inner:\n",
            "      leaf: value\n",
            "---\n",
        );

        let result = extract(source);
        let mut meta = result.meta.unwrap();
        let value = meta.fields.remove(0).value;

        assert_eq!(
            value,
            MetaValue::Mapping(vec![FrontmatterField {
                key: "middle".to_string(),
                value: MetaValue::Mapping(vec![FrontmatterField {
                    key: "inner".to_string(),
                    value: MetaValue::Mapping(vec![FrontmatterField {
                        key: "leaf".to_string(),
                        value: MetaValue::Scalar("value".to_string()),
                    }]),
                }]),
            }])
        );
    }

    #[test]
    fn opening_delimiter_not_on_first_logical_line_is_ignored() {
        eprintln!("scenario: opening delimiter not first line");
        let source = "\n---\ntitle: Doc\n---\nbody\n";

        let result = extract(source);

        assert_eq!(result.body, source);
        assert_eq!(result.meta, None);
    }

    #[test]
    fn body_bytes_after_closing_delimiter_are_preserved_exactly() {
        eprintln!("scenario: exact body bytes preserved");
        let source = "---\ntitle: Doc\n---\n\nBody\r\nTrailing";

        let result = extract(source);

        assert_eq!(result.body.as_bytes(), b"\nBody\r\nTrailing");
        assert_eq!(result.render_body.as_bytes(), b"\nBody\r\nTrailing");
    }
}
