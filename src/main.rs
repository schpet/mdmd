mod parse;
mod render;

use std::{env, fs, io, process};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
    DefaultTerminal, Frame,
};

use render::{HeadingPosition, RenderedDocument};

/// State for the outline modal overlay.
struct OutlineState {
    /// Index into `heading_lines` of the currently selected heading.
    selected: usize,
    /// Scroll offset saved when the outline was opened (for Esc restore).
    saved_scroll: usize,
}

fn main() -> io::Result<()> {
    let path = match env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("Usage: mdmd <file.md>");
            process::exit(1);
        }
    };
    let source = fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("Error reading {path}: {e}");
        process::exit(1);
    });
    let doc = parse::parse(&source);

    ratatui::run(|terminal| run(terminal, &doc))
}

fn run(terminal: &mut DefaultTerminal, doc: &parse::ParsedDocument) -> io::Result<()> {
    let rendered = render::render_document(doc);
    let total_lines = rendered.text.lines.len();
    let mut scroll_offset: usize = 0;
    let mut focused_link: Option<usize> = None;
    let mut outline: Option<OutlineState> = None;

    loop {
        terminal.draw(|frame| {
            ui(
                frame,
                &rendered,
                scroll_offset,
                total_lines,
                focused_link,
                outline.as_ref().map(|o| o.selected),
            );
        })?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            let viewport_height = terminal.size()?.height.saturating_sub(1) as usize;
            let max_scroll = total_lines.saturating_sub(viewport_height);

            if let Some(ref mut ol) = outline {
                // Outline modal is open — handle outline-specific keys
                let num_headings = rendered.heading_lines.len();
                match key.code {
                    KeyCode::Char('j') | KeyCode::Down => {
                        if num_headings > 0 {
                            ol.selected = (ol.selected + 1).min(num_headings - 1);
                            scroll_offset = rendered.heading_lines[ol.selected]
                                .rendered_line
                                .min(max_scroll);
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        ol.selected = ol.selected.saturating_sub(1);
                        scroll_offset = rendered.heading_lines[ol.selected]
                            .rendered_line
                            .min(max_scroll);
                    }
                    KeyCode::Char('g') | KeyCode::Home => {
                        ol.selected = 0;
                        scroll_offset = rendered.heading_lines[ol.selected]
                            .rendered_line
                            .min(max_scroll);
                    }
                    KeyCode::Char('G') | KeyCode::End => {
                        if num_headings > 0 {
                            ol.selected = num_headings - 1;
                            scroll_offset = rendered.heading_lines[ol.selected]
                                .rendered_line
                                .min(max_scroll);
                        }
                    }
                    KeyCode::Enter => {
                        // Close and stay at selected heading position
                        outline = None;
                    }
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('o') => {
                        // Close and restore original position
                        scroll_offset = ol.saved_scroll;
                        outline = None;
                    }
                    _ => {}
                }
            } else {
                // Normal mode — handle regular keys
                match key.code {
                    KeyCode::Char('q') => return Ok(()),

                    // Open outline modal
                    KeyCode::Char('o') => {
                        if !rendered.heading_lines.is_empty() {
                            let current_idx = rendered
                                .heading_lines
                                .iter()
                                .rposition(|h| h.rendered_line <= scroll_offset)
                                .unwrap_or(0);
                            outline = Some(OutlineState {
                                selected: current_idx,
                                saved_scroll: scroll_offset,
                            });
                            focused_link = None;
                        }
                    }

                    // Single line down
                    KeyCode::Char('j') | KeyCode::Down => {
                        scroll_offset = (scroll_offset + 1).min(max_scroll);
                        focused_link = None;
                    }

                    // Single line up
                    KeyCode::Char('k') | KeyCode::Up => {
                        scroll_offset = scroll_offset.saturating_sub(1);
                        focused_link = None;
                    }

                    // Half page down
                    KeyCode::Char('d')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        let half = viewport_height / 2;
                        scroll_offset = (scroll_offset + half).min(max_scroll);
                        focused_link = None;
                    }
                    KeyCode::PageDown => {
                        let half = viewport_height / 2;
                        scroll_offset = (scroll_offset + half).min(max_scroll);
                        focused_link = None;
                    }

                    // Half page up
                    KeyCode::Char('u')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        let half = viewport_height / 2;
                        scroll_offset = scroll_offset.saturating_sub(half);
                        focused_link = None;
                    }
                    KeyCode::PageUp => {
                        let half = viewport_height / 2;
                        scroll_offset = scroll_offset.saturating_sub(half);
                        focused_link = None;
                    }

                    // Jump to top
                    KeyCode::Char('g') | KeyCode::Home => {
                        scroll_offset = 0;
                        focused_link = None;
                    }

                    // Jump to bottom
                    KeyCode::Char('G') | KeyCode::End => {
                        scroll_offset = max_scroll;
                        focused_link = None;
                    }

                    // Next heading
                    KeyCode::Char('n') => {
                        if let Some(pos) = rendered
                            .heading_lines
                            .iter()
                            .find(|h| h.rendered_line > scroll_offset)
                        {
                            scroll_offset = pos.rendered_line.min(max_scroll);
                        }
                        focused_link = None;
                    }

                    // Previous heading
                    KeyCode::Char('p') => {
                        if let Some(pos) = rendered
                            .heading_lines
                            .iter()
                            .rev()
                            .find(|h| h.rendered_line < scroll_offset)
                        {
                            scroll_offset = pos.rendered_line.min(max_scroll);
                        }
                        focused_link = None;
                    }

                    // Next link (Tab)
                    KeyCode::Tab => {
                        let num_links = rendered.link_positions.len();
                        if num_links > 0 {
                            focused_link = Some(match focused_link {
                                Some(idx) => (idx + 1) % num_links,
                                None => {
                                    // Find first link at or after current scroll position
                                    rendered
                                        .link_positions
                                        .iter()
                                        .position(|l| l.rendered_line >= scroll_offset)
                                        .unwrap_or(0)
                                }
                            });
                            // Auto-scroll to bring focused link into view
                            if let Some(link) = focused_link
                                .and_then(|idx| rendered.link_positions.get(idx))
                            {
                                let line = link.rendered_line;
                                if line < scroll_offset
                                    || line >= scroll_offset + viewport_height
                                {
                                    scroll_offset = line
                                        .saturating_sub(viewport_height / 3)
                                        .min(max_scroll);
                                }
                            }
                        }
                    }

                    // Previous link (Shift-Tab)
                    KeyCode::BackTab => {
                        let num_links = rendered.link_positions.len();
                        if num_links > 0 {
                            focused_link = Some(match focused_link {
                                Some(0) => num_links - 1,
                                Some(idx) => idx - 1,
                                None => {
                                    // Find last link at or before current scroll + viewport
                                    let visible_end = scroll_offset + viewport_height;
                                    rendered
                                        .link_positions
                                        .iter()
                                        .rposition(|l| l.rendered_line < visible_end)
                                        .unwrap_or(num_links - 1)
                                }
                            });
                            // Auto-scroll to bring focused link into view
                            if let Some(link) = focused_link
                                .and_then(|idx| rendered.link_positions.get(idx))
                            {
                                let line = link.rendered_line;
                                if line < scroll_offset
                                    || line >= scroll_offset + viewport_height
                                {
                                    scroll_offset = line
                                        .saturating_sub(viewport_height / 3)
                                        .min(max_scroll);
                                }
                            }
                        }
                    }

                    // Escape clears link focus
                    KeyCode::Esc => {
                        focused_link = None;
                    }

                    _ => {}
                }
            }
        }
    }
}

/// Find the heading context for the current scroll position.
///
/// Returns the most recent heading at or before `scroll_offset`.
fn current_heading_context(
    heading_lines: &[HeadingPosition],
    scroll_offset: usize,
) -> Option<&HeadingPosition> {
    heading_lines
        .iter()
        .rev()
        .find(|h| h.rendered_line <= scroll_offset)
}

fn ui(
    frame: &mut Frame,
    rendered: &RenderedDocument,
    scroll_offset: usize,
    total_lines: usize,
    focused_link: Option<usize>,
    outline_selected: Option<usize>,
) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let viewport_height = chunks[0].height as usize;

    // Render scrolled content
    let widget = Paragraph::new(rendered.text.clone()).scroll((scroll_offset as u16, 0));
    frame.render_widget(widget, chunks[0]);

    // Apply focus highlight overlay on the focused link
    if let Some(link) = focused_link.and_then(|idx| rendered.link_positions.get(idx)) {
        let rel_line = link.rendered_line as isize - scroll_offset as isize;
        if rel_line >= 0 && (rel_line as usize) < viewport_height {
            let row = chunks[0].y + rel_line as u16;
            let focused_style = Style::default()
                .fg(Color::White)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD);
            for col in link.column_start..link.column_end {
                let pos = Position::new(chunks[0].x + col as u16, row);
                if let Some(cell) = frame.buffer_mut().cell_mut(pos) {
                    cell.set_style(focused_style);
                }
            }
        }
    }

    // Render outline modal overlay
    if let Some(selected) = outline_selected {
        render_outline(frame, &rendered.heading_lines, selected, chunks[0]);
    }

    // Render status bar with scroll position indicator
    let position = if total_lines == 0 {
        "Empty".to_owned()
    } else if total_lines <= viewport_height {
        "All".to_owned()
    } else if scroll_offset == 0 {
        "Top".to_owned()
    } else if scroll_offset >= total_lines.saturating_sub(viewport_height) {
        "Bot".to_owned()
    } else {
        let pct = (scroll_offset * 100) / total_lines;
        format!("{pct}%")
    };

    let heading_ctx = current_heading_context(&rendered.heading_lines, scroll_offset)
        .map(|h| format!(" § {}", h.text))
        .unwrap_or_default();

    let link_info = focused_link
        .and_then(|idx| rendered.link_positions.get(idx))
        .map(|l| format!(" -> {}", l.url))
        .unwrap_or_default();

    let status = format!(
        " Line {}/{} — {}{}{}",
        scroll_offset + 1,
        total_lines,
        position,
        heading_ctx,
        link_info,
    );
    let status_bar = Paragraph::new(Span::styled(
        status,
        Style::default().fg(Color::Black).bg(Color::White),
    ))
    .style(Style::default().bg(Color::White));
    frame.render_widget(status_bar, chunks[1]);
}

/// Compute a centered rectangle within `area`.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let width = (area.width * percent_x / 100).max(30).min(area.width);
    let height = (area.height * percent_y / 100).max(5).min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width, height)
}

/// Render the outline modal overlay showing all headings hierarchically.
fn render_outline(
    frame: &mut Frame,
    heading_lines: &[HeadingPosition],
    selected: usize,
    viewport_area: Rect,
) {
    let popup = centered_rect(60, 70, viewport_area);

    // Clear the popup area
    frame.render_widget(Clear, popup);

    // Build styled lines for each heading
    let lines: Vec<Line<'static>> = heading_lines
        .iter()
        .map(|h| {
            let indent = "  ".repeat((h.level as usize).saturating_sub(1));
            let prefix = "#".repeat(h.level as usize);
            let style = render::heading_style(h.level);
            Line::from(Span::styled(
                format!("{indent}{prefix} {}", h.text),
                style,
            ))
        })
        .collect();

    // Calculate scroll offset to keep selected item visible (roughly centered)
    let inner_height = popup.height.saturating_sub(2) as usize;
    let scroll = if heading_lines.is_empty() || inner_height == 0 {
        0
    } else {
        let max_scroll = heading_lines.len().saturating_sub(inner_height);
        selected.saturating_sub(inner_height / 2).min(max_scroll)
    };

    let block = Block::bordered()
        .title(" Outline ")
        .style(Style::default().fg(Color::White));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((scroll as u16, 0));

    frame.render_widget(paragraph, popup);

    // Apply full-width highlight to the selected heading line
    if !heading_lines.is_empty() && inner_height > 0 {
        let rel_line = selected as isize - scroll as isize;
        if rel_line >= 0 && (rel_line as usize) < inner_height {
            let row = popup.y + 1 + rel_line as u16; // +1 for top border
            let highlight = Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD);
            for col in (popup.x + 1)..(popup.x + popup.width.saturating_sub(1)) {
                let pos = Position::new(col, row);
                if let Some(cell) = frame.buffer_mut().cell_mut(pos) {
                    cell.set_style(highlight);
                }
            }
        }
    }
}
