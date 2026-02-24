//! Markdown rendering module.
//!
//! Converts a [`ParsedDocument`] into styled ratatui [`Text`] for display
//! in the terminal viewport.

use std::sync::OnceLock;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};
use syntect::{
    highlighting::{Theme, ThemeSet},
    parsing::SyntaxSet,
};

use crate::parse::{BlockKind, ContentBlock, InlineLink, ParsedDocument};

fn syntax_set() -> &'static SyntaxSet {
    static SS: OnceLock<SyntaxSet> = OnceLock::new();
    SS.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme() -> &'static Theme {
    static TH: OnceLock<Theme> = OnceLock::new();
    TH.get_or_init(|| {
        let ts = ThemeSet::load_defaults();
        ts.themes["base16-eighties.dark"].clone()
    })
}

fn syntect_to_ratatui_color(c: syntect::highlighting::Color) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

/// A heading's position in the rendered output.
#[derive(Debug, Clone)]
pub struct HeadingPosition {
    /// 0-based line index in the rendered output.
    pub rendered_line: usize,
    /// Heading level (1–6).
    pub level: u8,
    /// Text content of the heading.
    pub text: String,
}

/// A link's position in the rendered output, for Tab navigation and focus highlighting.
#[derive(Debug, Clone)]
pub struct LinkPosition {
    /// 0-based line index in the rendered output.
    pub rendered_line: usize,
    /// 0-based column where the link text starts.
    pub column_start: usize,
    /// 0-based column where the link text ends (exclusive).
    pub column_end: usize,
    /// Destination URL.
    pub url: String,
    /// Display text of the link.
    pub text: String,
}

/// The result of rendering a parsed document.
pub struct RenderedDocument {
    /// Styled text ready for display.
    pub text: Text<'static>,
    /// Positions of all headings in the rendered output.
    pub heading_lines: Vec<HeadingPosition>,
    /// Positions of all links in the rendered output.
    pub link_positions: Vec<LinkPosition>,
}

/// Convert a parsed markdown document into styled [`Text`] ready for rendering,
/// along with heading positions in the rendered output.
///
/// The caller is responsible for clipping to the viewport height.
pub fn render_document(doc: &ParsedDocument) -> RenderedDocument {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut heading_lines: Vec<HeadingPosition> = Vec::new();
    let mut link_positions: Vec<LinkPosition> = Vec::new();

    for (i, block) in doc.blocks.iter().enumerate() {
        if i > 0 {
            // Blank line between blocks
            lines.push(Line::default());
        }
        if let BlockKind::Heading(level) = &block.kind {
            heading_lines.push(HeadingPosition {
                rendered_line: lines.len(),
                level: *level,
                text: block.content.clone(),
            });
        }
        render_block(block, &mut lines, &mut link_positions);
    }

    RenderedDocument {
        text: Text::from(lines),
        heading_lines,
        link_positions,
    }
}

fn render_block(
    block: &ContentBlock,
    lines: &mut Vec<Line<'static>>,
    link_positions: &mut Vec<LinkPosition>,
) {
    match &block.kind {
        BlockKind::Heading(level) => render_heading(
            *level,
            &block.content,
            &block.inline_links,
            lines,
            link_positions,
        ),
        BlockKind::Paragraph => {
            render_paragraph(&block.content, &block.inline_links, lines, link_positions)
        }
        BlockKind::CodeBlock(ref lang) => render_code_block(&block.content, lang.as_deref(), lines),
        BlockKind::List => render_list(&block.content, &block.inline_links, lines, link_positions),
        BlockKind::BlockQuote => {
            render_block_quote(&block.content, &block.inline_links, lines, link_positions)
        }
        BlockKind::ThematicBreak => render_thematic_break(lines),
        BlockKind::HtmlBlock => {
            render_paragraph(&block.content, &block.inline_links, lines, link_positions)
        }
        BlockKind::Table => render_table(&block.content, lines),
    }
}

pub fn heading_style(level: u8) -> Style {
    let base = Style::default().add_modifier(Modifier::BOLD);
    match level {
        1 => base.fg(Color::Magenta),
        2 => base.fg(Color::Cyan),
        3 => base.fg(Color::Green),
        4 => base.fg(Color::Yellow),
        _ => base.fg(Color::White),
    }
}

fn heading_prefix(level: u8) -> &'static str {
    match level {
        1 => "# ",
        2 => "## ",
        3 => "### ",
        4 => "#### ",
        5 => "##### ",
        6 => "###### ",
        _ => "# ",
    }
}

/// Style for link text (non-focused).
fn link_style() -> Style {
    Style::default()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED)
}

/// Split a single line of text at link boundaries, producing styled spans.
///
/// `line_text` is the text to render for this line.
/// `line_content_offset` is the byte offset of `line_text` within the block's content.
/// `column_offset` is the display column where content starts (after any prefix spans).
fn split_line_at_links(
    line_text: &str,
    line_content_offset: usize,
    inline_links: &[InlineLink],
    base_style: Style,
    column_offset: usize,
    rendered_line_idx: usize,
    link_positions: &mut Vec<LinkPosition>,
) -> Vec<Span<'static>> {
    let line_start = line_content_offset;
    let line_end = line_content_offset + line_text.len();

    // Collect links that overlap with this line
    let overlapping: Vec<&InlineLink> = inline_links
        .iter()
        .filter(|l| l.start < line_end && l.end > line_start)
        .collect();

    if overlapping.is_empty() {
        return vec![Span::styled(line_text.to_owned(), base_style)];
    }

    let ls = link_style();
    let mut spans = Vec::new();
    let mut pos = line_start;

    for link in &overlapping {
        let vis_start = link.start.max(line_start);
        let vis_end = link.end.min(line_end);

        // Text before this link
        if vis_start > pos {
            let before = &line_text[pos - line_start..vis_start - line_start];
            spans.push(Span::styled(before.to_owned(), base_style));
        }

        // Link text
        let link_slice_start = vis_start - line_start;
        let link_slice_end = vis_end - line_start;
        let link_text = &line_text[link_slice_start..link_slice_end];
        spans.push(Span::styled(link_text.to_owned(), ls));

        // Record position
        let col_start = column_offset + link_slice_start;
        link_positions.push(LinkPosition {
            rendered_line: rendered_line_idx,
            column_start: col_start,
            column_end: col_start + link_text.len(),
            url: link.url.clone(),
            text: link_text.to_owned(),
        });

        pos = vis_end;
    }

    // Text after the last link
    if pos < line_end {
        let after = &line_text[pos - line_start..];
        spans.push(Span::styled(after.to_owned(), base_style));
    }

    spans
}

fn render_heading(
    level: u8,
    content: &str,
    inline_links: &[InlineLink],
    lines: &mut Vec<Line<'static>>,
    link_positions: &mut Vec<LinkPosition>,
) {
    let style = heading_style(level);
    let prefix = heading_prefix(level);
    let prefix_width = prefix.len();

    let mut content_offset = 0;
    for text_line in content.lines() {
        let mut spans = vec![Span::styled(prefix.to_owned(), style)];
        let link_spans = split_line_at_links(
            text_line,
            content_offset,
            inline_links,
            style,
            prefix_width,
            lines.len(),
            link_positions,
        );
        spans.extend(link_spans);
        lines.push(Line::from(spans));
        content_offset += text_line.len() + 1;
    }
}

fn render_paragraph(
    content: &str,
    inline_links: &[InlineLink],
    lines: &mut Vec<Line<'static>>,
    link_positions: &mut Vec<LinkPosition>,
) {
    let base_style = Style::default();
    let mut content_offset = 0;
    for text_line in content.lines() {
        let spans = split_line_at_links(
            text_line,
            content_offset,
            inline_links,
            base_style,
            0,
            lines.len(),
            link_positions,
        );
        lines.push(Line::from(spans));
        content_offset += text_line.len() + 1;
    }
}

fn render_code_block(content: &str, lang: Option<&str>, lines: &mut Vec<Line<'static>>) {
    let border_style = Style::default().fg(Color::DarkGray);
    let fallback_style = Style::default().fg(Color::Green).bg(Color::Black);

    let ss = syntax_set();
    let syntax = lang
        .and_then(|l| ss.find_syntax_by_token(l))
        .or_else(|| lang.and_then(|l| ss.find_syntax_by_extension(l)));

    lines.push(Line::from(Span::styled("┌───", border_style)));

    if let Some(syn) = syntax {
        let th = theme();
        let mut highlighter = syntect::easy::HighlightLines::new(syn, th);

        for text_line in content.lines() {
            let mut spans = vec![Span::styled("│ ", border_style)];

            match highlighter.highlight_line(text_line, ss) {
                Ok(regions) => {
                    for (style, text) in regions {
                        let fg = syntect_to_ratatui_color(style.foreground);
                        let ratatui_style = Style::default().fg(fg).bg(Color::Black);
                        spans.push(Span::styled(text.to_owned(), ratatui_style));
                    }
                }
                Err(_) => {
                    spans.push(Span::styled(text_line.to_owned(), fallback_style));
                }
            }

            lines.push(Line::from(spans));
        }
    } else {
        // No recognized syntax — plain monospace fallback
        for text_line in content.lines() {
            lines.push(Line::from(vec![
                Span::styled("│ ", border_style),
                Span::styled(text_line.to_owned(), fallback_style),
            ]));
        }
    }

    lines.push(Line::from(Span::styled("└───", border_style)));
}

fn render_list(
    content: &str,
    inline_links: &[InlineLink],
    lines: &mut Vec<Line<'static>>,
    link_positions: &mut Vec<LinkPosition>,
) {
    let bullet_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let base_style = Style::default();
    let prefix_width = 4; // "  • " is 4 display columns

    let mut content_offset = 0;
    for text_line in content.lines() {
        let trimmed = text_line.trim();
        if !trimmed.is_empty() {
            let leading_ws = text_line.len() - text_line.trim_start().len();
            let trimmed_offset = content_offset + leading_ws;

            let mut spans = vec![Span::styled("  • ", bullet_style)];
            let link_spans = split_line_at_links(
                trimmed,
                trimmed_offset,
                inline_links,
                base_style,
                prefix_width,
                lines.len(),
                link_positions,
            );
            spans.extend(link_spans);
            lines.push(Line::from(spans));
        }
        content_offset += text_line.len() + 1;
    }
}

fn render_block_quote(
    content: &str,
    inline_links: &[InlineLink],
    lines: &mut Vec<Line<'static>>,
    link_positions: &mut Vec<LinkPosition>,
) {
    let bar_style = Style::default().fg(Color::DarkGray);
    let text_style = Style::default()
        .add_modifier(Modifier::ITALIC)
        .fg(Color::Gray);
    let prefix_width = 4; // "  ▌ " is 4 display columns

    let mut content_offset = 0;
    for text_line in content.lines() {
        let mut spans = vec![Span::styled("  ▌ ", bar_style)];
        let link_spans = split_line_at_links(
            text_line,
            content_offset,
            inline_links,
            text_style,
            prefix_width,
            lines.len(),
            link_positions,
        );
        spans.extend(link_spans);
        lines.push(Line::from(spans));
        content_offset += text_line.len() + 1;
    }
}

fn render_thematic_break(lines: &mut Vec<Line<'static>>) {
    let style = Style::default().fg(Color::DarkGray);
    lines.push(Line::from(Span::styled(
        "────────────────────────────────────────",
        style,
    )));
}

fn render_table(content: &str, lines: &mut Vec<Line<'static>>) {
    let style = Style::default().fg(Color::White);
    for text_line in content.lines() {
        let trimmed = text_line.trim();
        if !trimmed.is_empty() {
            lines.push(Line::from(Span::styled(format!("  {trimmed}"), style)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn heading_levels_styled() {
        let doc = parse::parse("# H1\n\n## H2\n\n### H3\n");
        let rendered = render_document(&doc);
        // Should produce lines for each heading plus blank separators
        assert!(!rendered.text.lines.is_empty());
        // First line should be the H1
        let first = &rendered.text.lines[0];
        assert!(first.to_string().contains("# H1"));
    }

    #[test]
    fn code_block_has_borders() {
        let doc = parse::parse("```\nhello\n```\n");
        let rendered = render_document(&doc);
        let joined: String = rendered
            .text
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("┌"));
        assert!(joined.contains("hello"));
        assert!(joined.contains("└"));
    }

    #[test]
    fn list_has_bullets() {
        let doc = parse::parse("- alpha\n- beta\n");
        let rendered = render_document(&doc);
        let joined: String = rendered
            .text
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("•"));
        assert!(joined.contains("alpha"));
        assert!(joined.contains("beta"));
    }

    #[test]
    fn block_quote_has_bar() {
        let doc = parse::parse("> quoted\n");
        let rendered = render_document(&doc);
        let joined: String = rendered
            .text
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("▌"));
        assert!(joined.contains("quoted"));
    }

    #[test]
    fn thematic_break_renders() {
        let doc = parse::parse("above\n\n---\n\nbelow\n");
        let rendered = render_document(&doc);
        let joined: String = rendered
            .text
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("────"));
    }

    #[test]
    fn empty_document_renders() {
        let doc = parse::parse("");
        let rendered = render_document(&doc);
        assert!(rendered.text.lines.is_empty());
        assert!(rendered.heading_lines.is_empty());
    }

    #[test]
    fn heading_positions_tracked() {
        let doc = parse::parse("# Title\n\nBody\n\n## Section\n");
        let rendered = render_document(&doc);

        assert_eq!(rendered.heading_lines.len(), 2);

        // First heading at rendered line 0
        assert_eq!(rendered.heading_lines[0].rendered_line, 0);
        assert_eq!(rendered.heading_lines[0].level, 1);
        assert_eq!(rendered.heading_lines[0].text, "Title");

        // Second heading after: "# Title", blank, "Body", blank => line 4
        assert_eq!(rendered.heading_lines[1].rendered_line, 4);
        assert_eq!(rendered.heading_lines[1].level, 2);
        assert_eq!(rendered.heading_lines[1].text, "Section");
    }
}
