mod parse;
mod render;

use std::{env, fs, io, process};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
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
    loop {
        terminal.draw(|frame| render(frame, doc))?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                return Ok(());
            }
        }
    }
}

fn render(frame: &mut Frame, doc: &ParsedDocument) {
    let text = render::render_document(doc);
    let widget = Paragraph::new(text);
    frame.render_widget(widget, frame.area());
}
