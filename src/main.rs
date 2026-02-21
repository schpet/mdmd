mod parse;
mod render;

use std::{env, fs, io, process};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Position},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::Paragraph,
    DefaultTerminal, Frame,
};

use render::{HeadingPosition, RenderedDocument};

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

    loop {
        terminal.draw(|frame| {
            ui(frame, &rendered, scroll_offset, total_lines, focused_link);
        })?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            let viewport_height = terminal.size()?.height.saturating_sub(1) as usize;
            let max_scroll = total_lines.saturating_sub(viewport_height);

            match key.code {
                KeyCode::Char('q') => return Ok(()),

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
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
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
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
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
