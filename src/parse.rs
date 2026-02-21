//! Markdown parsing module.
//!
//! Parses markdown text into a structured representation containing:
//! - A flat list of content blocks with their line ranges
//! - A heading list with level, text, and line position
//! - A collection of all links with text, URL, and position

use pulldown_cmark::{Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The kind of a top-level content block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    CodeBlock,
    List,
    BlockQuote,
    ThematicBreak,
    HtmlBlock,
    Table,
}

/// A link whose text appears inline within a [`ContentBlock`]'s content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineLink {
    /// Byte offset of the link text start within `ContentBlock::content`.
    pub start: usize,
    /// Byte offset of the link text end (exclusive) within `ContentBlock::content`.
    pub end: usize,
    /// Destination URL.
    pub url: String,
}

/// A top-level content block in the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentBlock {
    pub kind: BlockKind,
    /// 1-based starting line number.
    pub line_start: usize,
    /// 1-based ending line number (inclusive).
    pub line_end: usize,
    /// Flattened text content of the block.
    pub content: String,
    /// Links whose text appears within `content`, with byte offsets.
    pub inline_links: Vec<InlineLink>,
}

/// A heading extracted from the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// Heading level (1–6).
    pub level: u8,
    /// Flattened text content of the heading.
    pub text: String,
    /// 1-based line number where the heading appears.
    pub line: usize,
}

/// The kind of a collected link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    Inline,
    Reference,
    Autolink,
    Email,
    Collapsed,
    Shortcut,
    Image,
}

/// A link (or image) extracted from the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Visible text of the link.
    pub text: String,
    /// Destination URL.
    pub url: String,
    /// 1-based line number where the link appears.
    pub line: usize,
    pub kind: LinkKind,
}

/// The fully parsed representation of a markdown document.
#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub blocks: Vec<ContentBlock>,
    pub headings: Vec<Heading>,
    pub links: Vec<Link>,
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Maps byte offsets into a source string to 1-based line numbers.
struct LineIndex {
    /// Byte offsets of each `\n` character in the source.
    newline_offsets: Vec<usize>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let newline_offsets = source
            .bytes()
            .enumerate()
            .filter_map(|(i, b)| if b == b'\n' { Some(i) } else { None })
            .collect();
        Self { newline_offsets }
    }

    /// Convert a byte offset to a 1-based line number.
    fn line_at(&self, offset: usize) -> usize {
        match self.newline_offsets.binary_search(&offset) {
            Ok(idx) | Err(idx) => idx + 1,
        }
    }
}

fn heading_level_to_u8(level: &HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn link_type_to_kind(lt: &LinkType, is_image: bool) -> LinkKind {
    if is_image {
        return LinkKind::Image;
    }
    match lt {
        LinkType::Inline => LinkKind::Inline,
        LinkType::Reference | LinkType::ReferenceUnknown => LinkKind::Reference,
        LinkType::Autolink => LinkKind::Autolink,
        LinkType::Email => LinkKind::Email,
        LinkType::Collapsed | LinkType::CollapsedUnknown => LinkKind::Collapsed,
        LinkType::Shortcut | LinkType::ShortcutUnknown => LinkKind::Shortcut,
    }
}

/// Returns `true` for block-level tags (as opposed to inline spans).
fn is_block_level(tag: &Tag) -> bool {
    !matches!(
        tag,
        Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }
    )
}

fn is_block_level_end(tag: &TagEnd) -> bool {
    !matches!(
        tag,
        TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image
    )
}

/// Map a *top-level* block tag to its [`BlockKind`].
///
/// Returns `None` for block tags that only appear nested (e.g. `Item`,
/// `TableRow`) and for types we intentionally skip (e.g. metadata blocks).
fn tag_to_block_kind(tag: &Tag) -> Option<BlockKind> {
    match tag {
        Tag::Paragraph => Some(BlockKind::Paragraph),
        Tag::Heading { level, .. } => Some(BlockKind::Heading(heading_level_to_u8(level))),
        Tag::CodeBlock(_) => Some(BlockKind::CodeBlock),
        Tag::BlockQuote(..) => Some(BlockKind::BlockQuote),
        Tag::List(_) => Some(BlockKind::List),
        Tag::Table(_) => Some(BlockKind::Table),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a markdown source string into a [`ParsedDocument`].
pub fn parse(source: &str) -> ParsedDocument {
    let line_index = LineIndex::new(source);

    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(source, options);

    let mut blocks: Vec<ContentBlock> = Vec::new();
    let mut headings: Vec<Heading> = Vec::new();
    let mut links: Vec<Link> = Vec::new();

    // Block tracking
    let mut block_depth: usize = 0;
    let mut current_block: Option<(BlockKind, usize)> = None; // (kind, start_offset)
    let mut text_buf = String::new();

    // Heading tracking
    let mut in_heading: Option<u8> = None;
    let mut heading_line: usize = 0;
    let mut heading_text_buf = String::new();

    // Link tracking
    let mut in_link: Option<(String, LinkKind)> = None; // (url, kind)
    let mut link_line: usize = 0;
    let mut link_text_buf = String::new();

    // Inline link tracking (byte offsets within current block's text_buf)
    let mut link_content_start: usize = 0;
    let mut block_inline_links: Vec<InlineLink> = Vec::new();

    for (event, range) in parser.into_offset_iter() {
        match &event {
            Event::Start(tag) => {
                if is_block_level(tag) {
                    if block_depth == 0 {
                        if let Some(kind) = tag_to_block_kind(tag) {
                            current_block = Some((kind, range.start));
                            text_buf.clear();
                        }
                    }
                    // Insert newlines between list items / table rows for
                    // cleaner flattened content.
                    if block_depth >= 1 {
                        if matches!(tag, Tag::Item | Tag::TableRow) {
                            if !text_buf.is_empty() && !text_buf.ends_with('\n') {
                                text_buf.push('\n');
                            }
                        }
                    }
                    block_depth += 1;
                }

                // Heading tracking
                if let Tag::Heading { level, .. } = tag {
                    in_heading = Some(heading_level_to_u8(level));
                    heading_line = line_index.line_at(range.start);
                    heading_text_buf.clear();
                }

                // Link / image tracking
                match tag {
                    Tag::Link {
                        link_type,
                        dest_url,
                        ..
                    } => {
                        in_link =
                            Some((dest_url.to_string(), link_type_to_kind(link_type, false)));
                        link_line = line_index.line_at(range.start);
                        link_text_buf.clear();
                        link_content_start = text_buf.len();
                    }
                    Tag::Image {
                        link_type,
                        dest_url,
                        ..
                    } => {
                        in_link =
                            Some((dest_url.to_string(), link_type_to_kind(link_type, true)));
                        link_line = line_index.line_at(range.start);
                        link_text_buf.clear();
                        link_content_start = text_buf.len();
                    }
                    _ => {}
                }
            }

            Event::End(tag_end) => {
                if is_block_level_end(tag_end) {
                    block_depth = block_depth.saturating_sub(1);
                    if block_depth == 0 {
                        if let Some((kind, start_offset)) = current_block.take() {
                            let start_line = line_index.line_at(start_offset);
                            let end_line = line_index
                                .line_at(range.end.saturating_sub(1).max(start_offset));
                            blocks.push(ContentBlock {
                                kind,
                                line_start: start_line,
                                line_end: end_line,
                                content: text_buf.clone(),
                                inline_links: std::mem::take(&mut block_inline_links),
                            });
                        }
                        text_buf.clear();
                    }
                }

                // Finalize heading
                if let TagEnd::Heading(_) = tag_end {
                    if let Some(level) = in_heading.take() {
                        headings.push(Heading {
                            level,
                            text: heading_text_buf.clone(),
                            line: heading_line,
                        });
                        heading_text_buf.clear();
                    }
                }

                // Finalize link / image
                if matches!(tag_end, TagEnd::Link | TagEnd::Image) {
                    if let Some((url, kind)) = in_link.take() {
                        // Record inline link for block-level rendering
                        if block_depth > 0 {
                            block_inline_links.push(InlineLink {
                                start: link_content_start,
                                end: text_buf.len(),
                                url: url.clone(),
                            });
                        }
                        links.push(Link {
                            text: link_text_buf.clone(),
                            url,
                            line: link_line,
                            kind,
                        });
                    }
                    link_text_buf.clear();
                }
            }

            Event::Text(text) => {
                text_buf.push_str(text);
                if in_heading.is_some() {
                    heading_text_buf.push_str(text);
                }
                if in_link.is_some() {
                    link_text_buf.push_str(text);
                }
            }

            Event::Code(code) => {
                text_buf.push_str(code);
                if in_heading.is_some() {
                    heading_text_buf.push_str(code);
                }
                if in_link.is_some() {
                    link_text_buf.push_str(code);
                }
            }

            Event::SoftBreak | Event::HardBreak => {
                text_buf.push('\n');
                if in_heading.is_some() {
                    heading_text_buf.push('\n');
                }
                if in_link.is_some() {
                    link_text_buf.push('\n');
                }
            }

            Event::Html(html) => {
                if block_depth == 0 {
                    blocks.push(ContentBlock {
                        kind: BlockKind::HtmlBlock,
                        line_start: line_index.line_at(range.start),
                        line_end: line_index
                            .line_at(range.end.saturating_sub(1).max(range.start)),
                        content: html.to_string(),
                        inline_links: Vec::new(),
                    });
                } else {
                    text_buf.push_str(html);
                }
            }

            Event::InlineHtml(html) => {
                text_buf.push_str(html);
            }

            Event::Rule => {
                let line = line_index.line_at(range.start);
                blocks.push(ContentBlock {
                    kind: BlockKind::ThematicBreak,
                    line_start: line,
                    line_end: line,
                    content: String::new(),
                    inline_links: Vec::new(),
                });
            }

            _ => {}
        }
    }

    ParsedDocument {
        blocks,
        headings,
        links,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_document() {
        let doc = parse("");
        assert!(doc.blocks.is_empty());
        assert!(doc.headings.is_empty());
        assert!(doc.links.is_empty());
    }

    #[test]
    fn single_paragraph() {
        let doc = parse("Hello world.\n");
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(doc.blocks[0].content, "Hello world.");
        assert_eq!(doc.blocks[0].line_start, 1);
    }

    #[test]
    fn headings_extracted() {
        let src = "# Title\n\nBody\n\n## Section\n\nMore\n\n### Sub\n";
        let doc = parse(src);

        assert_eq!(doc.headings.len(), 3);

        assert_eq!(doc.headings[0].level, 1);
        assert_eq!(doc.headings[0].text, "Title");
        assert_eq!(doc.headings[0].line, 1);

        assert_eq!(doc.headings[1].level, 2);
        assert_eq!(doc.headings[1].text, "Section");
        assert_eq!(doc.headings[1].line, 5);

        assert_eq!(doc.headings[2].level, 3);
        assert_eq!(doc.headings[2].text, "Sub");
        assert_eq!(doc.headings[2].line, 9);
    }

    #[test]
    fn headings_appear_as_blocks() {
        let doc = parse("# Heading\n\nParagraph\n");
        let kinds: Vec<&BlockKind> = doc.blocks.iter().map(|b| &b.kind).collect();
        assert!(kinds.contains(&&BlockKind::Heading(1)));
        assert!(kinds.contains(&&BlockKind::Paragraph));
    }

    #[test]
    fn inline_links_collected() {
        let src = "See [example](https://example.com) and [other](https://other.com).\n";
        let doc = parse(src);

        assert_eq!(doc.links.len(), 2);

        assert_eq!(doc.links[0].text, "example");
        assert_eq!(doc.links[0].url, "https://example.com");
        assert_eq!(doc.links[0].kind, LinkKind::Inline);
        assert_eq!(doc.links[0].line, 1);

        assert_eq!(doc.links[1].text, "other");
        assert_eq!(doc.links[1].url, "https://other.com");
    }

    #[test]
    fn link_inside_heading() {
        let src = "# [Title](https://example.com)\n";
        let doc = parse(src);

        assert_eq!(doc.headings.len(), 1);
        assert_eq!(doc.headings[0].text, "Title");

        assert_eq!(doc.links.len(), 1);
        assert_eq!(doc.links[0].text, "Title");
        assert_eq!(doc.links[0].url, "https://example.com");
    }

    #[test]
    fn code_block_content() {
        let src = "```\nhello world\n```\n";
        let doc = parse(src);

        let code: Vec<&ContentBlock> = doc
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::CodeBlock)
            .collect();
        assert_eq!(code.len(), 1);
        assert_eq!(code[0].content, "hello world\n");
    }

    #[test]
    fn fenced_code_with_language() {
        let src = "```rust\nfn main() {}\n```\n";
        let doc = parse(src);

        let code: Vec<&ContentBlock> = doc
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::CodeBlock)
            .collect();
        assert_eq!(code.len(), 1);
        assert_eq!(code[0].content, "fn main() {}\n");
    }

    #[test]
    fn unordered_list() {
        let src = "- alpha\n- beta\n- gamma\n";
        let doc = parse(src);

        let lists: Vec<&ContentBlock> = doc
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::List)
            .collect();
        assert_eq!(lists.len(), 1);
        assert!(lists[0].content.contains("alpha"));
        assert!(lists[0].content.contains("gamma"));
    }

    #[test]
    fn block_quote() {
        let src = "> quoted text\n";
        let doc = parse(src);

        let bqs: Vec<&ContentBlock> = doc
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::BlockQuote)
            .collect();
        assert_eq!(bqs.len(), 1);
        assert!(bqs[0].content.contains("quoted text"));
    }

    #[test]
    fn thematic_break() {
        let src = "above\n\n---\n\nbelow\n";
        let doc = parse(src);

        let breaks: Vec<&ContentBlock> = doc
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::ThematicBreak)
            .collect();
        assert_eq!(breaks.len(), 1);
    }

    #[test]
    fn table_block() {
        let src = "| A | B |\n|---|---|\n| 1 | 2 |\n";
        let doc = parse(src);

        let tables: Vec<&ContentBlock> = doc
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::Table)
            .collect();
        assert_eq!(tables.len(), 1);
        assert!(tables[0].content.contains("A"));
        assert!(tables[0].content.contains("2"));
    }

    #[test]
    fn image_collected_as_link() {
        let src = "![alt text](image.png)\n";
        let doc = parse(src);

        assert_eq!(doc.links.len(), 1);
        assert_eq!(doc.links[0].text, "alt text");
        assert_eq!(doc.links[0].url, "image.png");
        assert_eq!(doc.links[0].kind, LinkKind::Image);
    }

    #[test]
    fn mixed_document() {
        let src = "\
# Introduction

Welcome to **mdmd**.

## Features

- Fast [rendering](https://example.com)
- Keyboard navigation

```bash
mdmd README.md
```

---

> Note: still in development.
";
        let doc = parse(src);

        // Headings
        assert_eq!(doc.headings.len(), 2);
        assert_eq!(doc.headings[0].text, "Introduction");
        assert_eq!(doc.headings[1].text, "Features");

        // Links
        assert_eq!(doc.links.len(), 1);
        assert_eq!(doc.links[0].text, "rendering");

        // Block variety
        let kinds: Vec<&BlockKind> = doc.blocks.iter().map(|b| &b.kind).collect();
        assert!(kinds.contains(&&BlockKind::Heading(1)));
        assert!(kinds.contains(&&BlockKind::Heading(2)));
        assert!(kinds.contains(&&BlockKind::Paragraph));
        assert!(kinds.contains(&&BlockKind::List));
        assert!(kinds.contains(&&BlockKind::CodeBlock));
        assert!(kinds.contains(&&BlockKind::ThematicBreak));
        assert!(kinds.contains(&&BlockKind::BlockQuote));
    }

    #[test]
    fn line_ranges_increase() {
        let src = "# A\n\nPara 1\n\n## B\n\nPara 2\n";
        let doc = parse(src);

        for window in doc.blocks.windows(2) {
            assert!(
                window[0].line_start <= window[1].line_start,
                "blocks should appear in source order"
            );
        }
    }

    #[test]
    fn multiline_paragraph() {
        let src = "Line one\nline two\nline three\n";
        let doc = parse(src);

        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].kind, BlockKind::Paragraph);
        // pulldown-cmark emits SoftBreak between lines
        assert!(doc.blocks[0].content.contains("Line one"));
        assert!(doc.blocks[0].content.contains("line three"));
        assert_eq!(doc.blocks[0].line_start, 1);
    }
}
