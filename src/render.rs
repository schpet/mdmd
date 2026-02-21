//! Markdown rendering module.
//!
//! Converts a [`ParsedDocument`] into styled ratatui [`Text`] for display
//! in the terminal viewport.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};

use crate::parse::{BlockKind, ContentBlock, ParsedDocument};

/// Convert a parsed markdown document into styled [`Text`] ready for rendering.
///
/// The returned `Text` contains all blocks rendered with appropriate styling.
/// The caller is responsible for clipping to the viewport height.
pub fn render_document(doc: &ParsedDocument) -> Text<'static> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    for (i, block) in doc.blocks.iter().enumerate() {
        if i > 0 {
            // Blank line between blocks
            lines.push(Line::default());
        }
        render_block(block, &mut lines);
    }

    Text::from(lines)
}

fn render_block(block: &ContentBlock, lines: &mut Vec<Line<'static>>) {
    match &block.kind {
        BlockKind::Heading(level) => render_heading(*level, &block.content, lines),
        BlockKind::Paragraph => render_paragraph(&block.content, lines),
        BlockKind::CodeBlock => render_code_block(&block.content, lines),
        BlockKind::List => render_list(&block.content, lines),
        BlockKind::BlockQuote => render_block_quote(&block.content, lines),
        BlockKind::ThematicBreak => render_thematic_break(lines),
        BlockKind::HtmlBlock => render_paragraph(&block.content, lines),
        BlockKind::Table => render_table(&block.content, lines),
    }
}

fn heading_style(level: u8) -> Style {
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

fn render_heading(level: u8, content: &str, lines: &mut Vec<Line<'static>>) {
    let style = heading_style(level);
    let prefix = heading_prefix(level);
    for text_line in content.lines() {
        lines.push(Line::from(Span::styled(
            format!("{prefix}{text_line}"),
            style,
        )));
    }
}

fn render_paragraph(content: &str, lines: &mut Vec<Line<'static>>) {
    for text_line in content.lines() {
        lines.push(Line::from(Span::raw(text_line.to_owned())));
    }
}

fn render_code_block(content: &str, lines: &mut Vec<Line<'static>>) {
    let border_style = Style::default().fg(Color::DarkGray);
    let code_style = Style::default().fg(Color::Green).bg(Color::Black);

    lines.push(Line::from(Span::styled("┌───", border_style)));
    for text_line in content.lines() {
        lines.push(Line::from(vec![
            Span::styled("│ ", border_style),
            Span::styled(text_line.to_owned(), code_style),
        ]));
    }
    lines.push(Line::from(Span::styled("└───", border_style)));
}

fn render_list(content: &str, lines: &mut Vec<Line<'static>>) {
    let bullet_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    for text_line in content.lines() {
        let trimmed = text_line.trim();
        if !trimmed.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("  • ", bullet_style),
                Span::raw(trimmed.to_owned()),
            ]));
        }
    }
}

fn render_block_quote(content: &str, lines: &mut Vec<Line<'static>>) {
    let bar_style = Style::default().fg(Color::DarkGray);
    let text_style = Style::default().add_modifier(Modifier::ITALIC).fg(Color::Gray);
    for text_line in content.lines() {
        lines.push(Line::from(vec![
            Span::styled("  ▌ ", bar_style),
            Span::styled(text_line.to_owned(), text_style),
        ]));
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
            lines.push(Line::from(Span::styled(
                format!("  {trimmed}"),
                style,
            )));
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
        let text = render_document(&doc);
        // Should produce lines for each heading plus blank separators
        assert!(!text.lines.is_empty());
        // First line should be the H1
        let first = &text.lines[0];
        assert!(first.to_string().contains("# H1"));
    }

    #[test]
    fn code_block_has_borders() {
        let doc = parse::parse("```\nhello\n```\n");
        let text = render_document(&doc);
        let joined: String = text.lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("┌"));
        assert!(joined.contains("hello"));
        assert!(joined.contains("└"));
    }

    #[test]
    fn list_has_bullets() {
        let doc = parse::parse("- alpha\n- beta\n");
        let text = render_document(&doc);
        let joined: String = text.lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("•"));
        assert!(joined.contains("alpha"));
        assert!(joined.contains("beta"));
    }

    #[test]
    fn block_quote_has_bar() {
        let doc = parse::parse("> quoted\n");
        let text = render_document(&doc);
        let joined: String = text.lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("▌"));
        assert!(joined.contains("quoted"));
    }

    #[test]
    fn thematic_break_renders() {
        let doc = parse::parse("above\n\n---\n\nbelow\n");
        let text = render_document(&doc);
        let joined: String = text.lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("────"));
    }

    #[test]
    fn empty_document_renders() {
        let doc = parse::parse("");
        let text = render_document(&doc);
        assert!(text.lines.is_empty());
    }
}
