mod parse;
mod render;

use std::{env, fs, io, process};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::{Span, Text},
    widgets::Paragraph,
    DefaultTerminal, Frame,
};

use parse::ParsedDocument;

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

fn run(terminal: &mut DefaultTerminal, doc: &ParsedDocument) -> io::Result<()> {
    let text = render::render_document(doc);
    let total_lines = text.lines.len();
    let mut scroll_offset: usize = 0;

    loop {
        terminal.draw(|frame| render(frame, &text, scroll_offset, total_lines))?;

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
                }

                // Single line up
                KeyCode::Char('k') | KeyCode::Up => {
                    scroll_offset = scroll_offset.saturating_sub(1);
                }

                // Half page down
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let half = viewport_height / 2;
                    scroll_offset = (scroll_offset + half).min(max_scroll);
                }
                KeyCode::PageDown => {
                    let half = viewport_height / 2;
                    scroll_offset = (scroll_offset + half).min(max_scroll);
                }

                // Half page up
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let half = viewport_height / 2;
                    scroll_offset = scroll_offset.saturating_sub(half);
                }
                KeyCode::PageUp => {
                    let half = viewport_height / 2;
                    scroll_offset = scroll_offset.saturating_sub(half);
                }

                // Jump to top
                KeyCode::Char('g') | KeyCode::Home => {
                    scroll_offset = 0;
                }

                // Jump to bottom
                KeyCode::Char('G') | KeyCode::End => {
                    scroll_offset = max_scroll;
                }

                _ => {}
            }
        }
    }
}

fn render(frame: &mut Frame, text: &Text, scroll_offset: usize, total_lines: usize) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let viewport_height = chunks[0].height as usize;

    // Render scrolled content
    let widget = Paragraph::new(text.clone()).scroll((scroll_offset as u16, 0));
    frame.render_widget(widget, chunks[0]);

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

    let status = format!(" Line {}/{} — {}", scroll_offset + 1, total_lines, position);
    let status_bar = Paragraph::new(Span::styled(
        status,
        Style::default().fg(Color::Black).bg(Color::White),
    ))
    .style(Style::default().bg(Color::White));
    frame.render_widget(status_bar, chunks[1]);
}
