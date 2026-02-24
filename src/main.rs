mod html;
mod parse;
mod render;
mod serve;
mod web_assets;

use std::{
    fs, io,
    path::{Path, PathBuf},
    process,
};

use clap::{Parser, Subcommand};
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

/// A single search match position in the rendered output.
struct SearchMatch {
    /// 0-based line index in the rendered output.
    rendered_line: usize,
    /// 0-based column where the match starts (byte offset in line text).
    column_start: usize,
    /// 0-based column where the match ends (exclusive, byte offset).
    column_end: usize,
}

/// Explicit subcommands.
#[derive(Subcommand)]
enum Commands {
    /// View a markdown file in TUI mode (equivalent to legacy positional form)
    View {
        /// Path to the markdown file
        file: String,
    },
    /// Serve a markdown file over HTTP
    Serve {
        /// Path to the markdown file
        file: String,
        /// Interface address to bind to
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// Starting port number for the HTTP server
        #[arg(long, default_value = "3333")]
        port: u16,
    },
}

/// Full CLI with explicit subcommands.
#[derive(Parser)]
#[command(
    name = "mdmd",
    version,
    about = "A TUI markdown viewer and navigator",
    after_help = "INVOCATION FORMS:\n  mdmd <file>                      View file in TUI mode (legacy)\n  mdmd view <file>                 View file in TUI mode\n  mdmd serve [OPTIONS] <file>      Serve file over HTTP"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Legacy positional form: mdmd <file>
#[derive(Parser)]
#[command(name = "mdmd", version, about = "A TUI markdown viewer and navigator")]
struct LegacyCli {
    /// Path to a markdown file to view
    file: String,
}

/// Resolved dispatch mode after CLI argument parsing.
enum DispatchMode {
    Legacy {
        file: String,
    },
    View {
        file: String,
    },
    Serve {
        file: String,
        bind: String,
        port: u16,
    },
}

/// State for vim-like `/` search.
struct SearchState {
    /// The current search query.
    query: String,
    /// Whether the user is still typing (true) or has confirmed with Enter (false).
    typing: bool,
    /// All match positions in the document.
    matches: Vec<SearchMatch>,
    /// Index into `matches` of the current/focused match.
    current_match: Option<usize>,
    /// Scroll offset saved when search was initiated (for Esc restore).
    saved_scroll: usize,
}

/// State for the help/shortcuts modal overlay.
struct HelpState {
    /// Current filter string for narrowing displayed shortcuts.
    filter: String,
    /// Scroll offset within the help modal content.
    scroll_offset: usize,
    /// Scroll offset saved when the help modal was opened (for restore on close).
    saved_scroll: usize,
}

/// A single keyboard shortcut entry.
struct ShortcutEntry {
    key: &'static str,
    description: &'static str,
}

/// A group of related shortcuts.
struct ShortcutCategory {
    name: &'static str,
    entries: Vec<ShortcutEntry>,
}

/// Build the complete list of shortcut categories.
fn shortcut_categories() -> Vec<ShortcutCategory> {
    vec![
        ShortcutCategory {
            name: "Navigation",
            entries: vec![
                ShortcutEntry {
                    key: "j / \u{2193}",
                    description: "Scroll down one line",
                },
                ShortcutEntry {
                    key: "k / \u{2191}",
                    description: "Scroll up one line",
                },
                ShortcutEntry {
                    key: "Ctrl-d / PgDn",
                    description: "Scroll down half page",
                },
                ShortcutEntry {
                    key: "Ctrl-u / PgUp",
                    description: "Scroll up half page",
                },
                ShortcutEntry {
                    key: "g / Home",
                    description: "Jump to top",
                },
                ShortcutEntry {
                    key: "G / End",
                    description: "Jump to bottom",
                },
            ],
        },
        ShortcutCategory {
            name: "Headings",
            entries: vec![
                ShortcutEntry {
                    key: "n",
                    description: "Next heading",
                },
                ShortcutEntry {
                    key: "p",
                    description: "Previous heading",
                },
                ShortcutEntry {
                    key: "o",
                    description: "Open outline",
                },
            ],
        },
        ShortcutCategory {
            name: "Search",
            entries: vec![
                ShortcutEntry {
                    key: "/",
                    description: "Start search",
                },
                ShortcutEntry {
                    key: "Ctrl-n",
                    description: "Next search match",
                },
                ShortcutEntry {
                    key: "Ctrl-p",
                    description: "Previous search match",
                },
                ShortcutEntry {
                    key: "Enter",
                    description: "Confirm search",
                },
                ShortcutEntry {
                    key: "Esc",
                    description: "Cancel search",
                },
            ],
        },
        ShortcutCategory {
            name: "Links",
            entries: vec![
                ShortcutEntry {
                    key: "Tab",
                    description: "Next link",
                },
                ShortcutEntry {
                    key: "Shift-Tab",
                    description: "Previous link",
                },
                ShortcutEntry {
                    key: "Enter",
                    description: "Follow focused link",
                },
                ShortcutEntry {
                    key: "Backspace",
                    description: "Navigate back",
                },
            ],
        },
        ShortcutCategory {
            name: "General",
            entries: vec![
                ShortcutEntry {
                    key: "?",
                    description: "Toggle this help",
                },
                ShortcutEntry {
                    key: "q",
                    description: "Quit",
                },
                ShortcutEntry {
                    key: "Esc",
                    description: "Clear search or link focus",
                },
            ],
        },
    ]
}

/// Saved navigation state for back-navigation when following links.
struct NavigationEntry {
    file_path: PathBuf,
    scroll_offset: usize,
    focused_link: Option<usize>,
}

fn resolve_dispatch_mode() -> DispatchMode {
    match Cli::try_parse() {
        Ok(cli) => match cli.command {
            Commands::View { file } => DispatchMode::View { file },
            Commands::Serve { file, bind, port } => DispatchMode::Serve { file, bind, port },
        },
        Err(clap_err) => {
            // Pass --help, --version, and subcommand-level help through to the full Cli handler.
            use clap::error::ErrorKind;
            if matches!(
                clap_err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                clap_err.exit();
            }
            // Fall back to legacy positional parse: mdmd <file>
            match LegacyCli::try_parse() {
                Ok(legacy) => DispatchMode::Legacy { file: legacy.file },
                Err(legacy_err) => legacy_err.exit(),
            }
        }
    }
}

fn main() -> io::Result<()> {
    match resolve_dispatch_mode() {
        DispatchMode::Legacy { file } => {
            eprintln!("[legacy] TUI viewer dispatched for: {file}");
            run_tui_file(&file)
        }
        DispatchMode::View { file } => {
            eprintln!("[view] TUI viewer dispatched for: {file}");
            run_tui_file(&file)
        }
        DispatchMode::Serve { file, bind, port } => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            rt.block_on(serve::run_serve(file, bind, port))
        }
    }
}

fn run_tui_file(file_arg: &str) -> io::Result<()> {
    let path = Path::new(file_arg);

    // Check the file extension before attempting to read.
    match path.extension().and_then(|e| e.to_str()) {
        Some("md" | "markdown" | "mdx" | "mdown" | "mkd" | "mkdn") => {}
        Some(ext) => {
            eprintln!("Error: '{ext}' is not a recognized markdown extension.");
            eprintln!("Expected a markdown file (.md, .markdown, .mdx, .mdown, .mkd, .mkdn).");
            process::exit(1);
        }
        None => {
            eprintln!("Error: '{file_arg}' has no file extension.");
            eprintln!("Expected a markdown file (.md, .markdown, .mdx, .mdown, .mkd, .mkdn).");
            process::exit(1);
        }
    }

    let source = fs::read_to_string(path).unwrap_or_else(|e| {
        match e.kind() {
            io::ErrorKind::NotFound => {
                eprintln!("Error: file not found: {file_arg}");
            }
            io::ErrorKind::PermissionDenied => {
                eprintln!("Error: permission denied: {file_arg}");
            }
            _ => {
                eprintln!("Error reading '{file_arg}': {e}");
            }
        }
        process::exit(1);
    });
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    ratatui::run(|terminal| run(terminal, &canonical, source))
}

fn run(
    terminal: &mut DefaultTerminal,
    initial_path: &Path,
    initial_source: String,
) -> io::Result<()> {
    let mut current_path = initial_path.to_path_buf();
    let doc = parse::parse(&initial_source);
    let mut rendered = render::render_document(&doc);
    let mut total_lines = rendered.text.lines.len();
    let mut scroll_offset: usize = 0;
    let mut focused_link: Option<usize> = None;
    let mut outline: Option<OutlineState> = None;
    let mut search: Option<SearchState> = None;
    let mut help: Option<HelpState> = None;
    let mut nav_stack: Vec<NavigationEntry> = Vec::new();

    loop {
        terminal.draw(|frame| {
            ui(
                frame,
                &rendered,
                scroll_offset,
                total_lines,
                focused_link,
                outline.as_ref().map(|o| o.selected),
                search.as_ref(),
                help.as_ref(),
                &current_path,
                !nav_stack.is_empty(),
            );
        })?;

        let event = event::read()?;

        // Recalculate bounds and clamp scroll offset on every event,
        // including Event::Resize, so the view stays valid after terminal resize.
        let viewport_height = terminal.size()?.height.saturating_sub(1) as usize;
        let max_scroll = total_lines.saturating_sub(viewport_height);
        scroll_offset = scroll_offset.min(max_scroll);

        if let Event::Key(key) = event {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            if let Some(ref mut hl) = help {
                // Help modal is open — handle help-specific keys
                match key.code {
                    KeyCode::Esc | KeyCode::Char('?') => {
                        scroll_offset = hl.saved_scroll;
                        help = None;
                    }
                    KeyCode::Backspace => {
                        hl.filter.pop();
                        hl.scroll_offset = 0;
                    }
                    KeyCode::Down => {
                        hl.scroll_offset = hl.scroll_offset.saturating_add(1);
                    }
                    KeyCode::Up => {
                        hl.scroll_offset = hl.scroll_offset.saturating_sub(1);
                    }
                    KeyCode::Char(c) => {
                        hl.filter.push(c);
                        hl.scroll_offset = 0;
                    }
                    _ => {}
                }
            } else if let Some(ref mut ol) = outline {
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
            } else if search.as_ref().map_or(false, |s| s.typing) {
                // Search typing mode — handle search input
                let mut cancel = false;
                match key.code {
                    KeyCode::Enter => {
                        let empty = search.as_ref().map_or(true, |s| s.matches.is_empty());
                        if empty {
                            cancel = true;
                        } else if let Some(ref mut s) = search {
                            s.typing = false;
                        }
                    }
                    KeyCode::Esc => {
                        // Restore scroll position from before search started
                        if let Some(ref s) = search {
                            scroll_offset = s.saved_scroll;
                        }
                        cancel = true;
                    }
                    KeyCode::Backspace => {
                        if let Some(ref mut s) = search {
                            s.query.pop();
                            s.matches = find_matches(&rendered, &s.query);
                            s.current_match = nearest_match_from(&s.matches, s.saved_scroll);
                        }
                    }
                    KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        advance_search_match(&mut search, true);
                    }
                    KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        advance_search_match(&mut search, false);
                    }
                    KeyCode::Char(c) => {
                        if let Some(ref mut s) = search {
                            s.query.push(c);
                            s.matches = find_matches(&rendered, &s.query);
                            s.current_match = nearest_match_from(&s.matches, s.saved_scroll);
                        }
                    }
                    _ => {}
                }
                if cancel {
                    search = None;
                }
                // Auto-scroll to current match
                if let Some(ref s) = search {
                    if let Some(idx) = s.current_match {
                        let line = s.matches[idx].rendered_line;
                        if line < scroll_offset || line >= scroll_offset + viewport_height {
                            scroll_offset =
                                line.saturating_sub(viewport_height / 3).min(max_scroll);
                        }
                    }
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

                    // Next search match (Ctrl-n)
                    KeyCode::Char('n')
                        if key.modifiers.contains(KeyModifiers::CONTROL) && search.is_some() =>
                    {
                        advance_search_match(&mut search, true);
                        if let Some(ref s) = search {
                            if let Some(idx) = s.current_match {
                                let line = s.matches[idx].rendered_line;
                                if line < scroll_offset || line >= scroll_offset + viewport_height {
                                    scroll_offset =
                                        line.saturating_sub(viewport_height / 3).min(max_scroll);
                                }
                            }
                        }
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

                    // Previous search match (Ctrl-p)
                    KeyCode::Char('p')
                        if key.modifiers.contains(KeyModifiers::CONTROL) && search.is_some() =>
                    {
                        advance_search_match(&mut search, false);
                        if let Some(ref s) = search {
                            if let Some(idx) = s.current_match {
                                let line = s.matches[idx].rendered_line;
                                if line < scroll_offset || line >= scroll_offset + viewport_height {
                                    scroll_offset =
                                        line.saturating_sub(viewport_height / 3).min(max_scroll);
                                }
                            }
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
                            if let Some(link) =
                                focused_link.and_then(|idx| rendered.link_positions.get(idx))
                            {
                                let line = link.rendered_line;
                                if line < scroll_offset || line >= scroll_offset + viewport_height {
                                    scroll_offset =
                                        line.saturating_sub(viewport_height / 3).min(max_scroll);
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
                            if let Some(link) =
                                focused_link.and_then(|idx| rendered.link_positions.get(idx))
                            {
                                let line = link.rendered_line;
                                if line < scroll_offset || line >= scroll_offset + viewport_height {
                                    scroll_offset =
                                        line.saturating_sub(viewport_height / 3).min(max_scroll);
                                }
                            }
                        }
                    }

                    // Follow focused link (Enter)
                    KeyCode::Enter => {
                        if let Some(link_idx) = focused_link {
                            if let Some(link) = rendered.link_positions.get(link_idx) {
                                let url = link.url.clone();
                                if is_external_url(&url) {
                                    open_url_in_browser(&url);
                                } else if let Some(target) =
                                    resolve_markdown_link(&current_path, &url)
                                {
                                    if let Ok(new_source) = fs::read_to_string(&target) {
                                        nav_stack.push(NavigationEntry {
                                            file_path: current_path.clone(),
                                            scroll_offset,
                                            focused_link,
                                        });
                                        current_path = target;
                                        let new_doc = parse::parse(&new_source);
                                        rendered = render::render_document(&new_doc);
                                        total_lines = rendered.text.lines.len();
                                        scroll_offset = 0;
                                        focused_link = None;
                                        outline = None;
                                        search = None;
                                    }
                                }
                            }
                        }
                    }

                    // Navigate back (Backspace)
                    KeyCode::Backspace => {
                        if let Some(entry) = nav_stack.pop() {
                            if let Ok(new_source) = fs::read_to_string(&entry.file_path) {
                                current_path = entry.file_path;
                                let new_doc = parse::parse(&new_source);
                                rendered = render::render_document(&new_doc);
                                total_lines = rendered.text.lines.len();
                                scroll_offset = entry.scroll_offset;
                                focused_link = entry.focused_link;
                                outline = None;
                                search = None;
                            }
                        }
                    }

                    // Open help modal
                    KeyCode::Char('?') => {
                        help = Some(HelpState {
                            filter: String::new(),
                            scroll_offset: 0,
                            saved_scroll: scroll_offset,
                        });
                        focused_link = None;
                    }

                    // Enter search mode
                    KeyCode::Char('/') => {
                        search = Some(SearchState {
                            query: String::new(),
                            typing: true,
                            matches: Vec::new(),
                            current_match: None,
                            saved_scroll: scroll_offset,
                        });
                        focused_link = None;
                    }

                    // Escape clears search (if active) or link focus
                    KeyCode::Esc => {
                        if search.is_some() {
                            search = None;
                        } else {
                            focused_link = None;
                        }
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

/// Find all case-insensitive occurrences of `query` in the rendered text.
fn find_matches(rendered: &RenderedDocument, query: &str) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let query_lower = query.to_lowercase();
    let mut matches = Vec::new();
    for (line_idx, line) in rendered.text.lines.iter().enumerate() {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let text_lower = text.to_lowercase();
        let mut pos = 0;
        while pos < text_lower.len() {
            match text_lower[pos..].find(&query_lower) {
                Some(rel) => {
                    let start = pos + rel;
                    matches.push(SearchMatch {
                        rendered_line: line_idx,
                        column_start: start,
                        column_end: start + query_lower.len(),
                    });
                    pos = start + 1;
                }
                None => break,
            }
        }
    }
    matches
}

/// Find the nearest match at or after `scroll_offset`, wrapping to the first match if needed.
fn nearest_match_from(matches: &[SearchMatch], scroll_offset: usize) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    matches
        .iter()
        .position(|m| m.rendered_line >= scroll_offset)
        .or(Some(0))
}

/// Advance the current search match forward or backward.
fn advance_search_match(search: &mut Option<SearchState>, forward: bool) {
    if let Some(ref mut s) = search {
        if s.matches.is_empty() {
            return;
        }
        s.current_match = Some(if forward {
            match s.current_match {
                Some(idx) => (idx + 1) % s.matches.len(),
                None => 0,
            }
        } else {
            match s.current_match {
                Some(0) => s.matches.len() - 1,
                Some(idx) => idx - 1,
                None => s.matches.len() - 1,
            }
        });
    }
}

/// Check if a URL is an external URL (http/https/mailto).
fn is_external_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:")
}

/// Resolve a link URL to a local markdown file path.
/// Returns None if the link is not a resolvable local markdown file.
fn resolve_markdown_link(current_file: &Path, url: &str) -> Option<PathBuf> {
    // Skip fragment-only links
    if url.starts_with('#') {
        return None;
    }

    // Strip fragment if present
    let path_part = url.split('#').next()?;
    if path_part.is_empty() {
        return None;
    }

    // Resolve relative to the directory containing the current file
    let base_dir = current_file.parent()?;
    let target = base_dir.join(path_part);

    // Check if it's a markdown file
    let ext = target.extension()?.to_str()?;
    if !matches!(ext, "md" | "markdown" | "mdx" | "mdown" | "mkd" | "mkdn") {
        return None;
    }

    // Check if file exists
    if target.is_file() {
        Some(fs::canonicalize(&target).unwrap_or(target))
    } else {
        None
    }
}

/// Open an external URL in the system browser.
fn open_url_in_browser(url: &str) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn ui(
    frame: &mut Frame,
    rendered: &RenderedDocument,
    scroll_offset: usize,
    total_lines: usize,
    focused_link: Option<usize>,
    outline_selected: Option<usize>,
    search: Option<&SearchState>,
    help: Option<&HelpState>,
    current_file: &Path,
    can_go_back: bool,
) {
    let area = frame.area();

    // Minimum usable terminal size: need width for content and height for viewport + status bar
    const MIN_WIDTH: u16 = 20;
    const MIN_HEIGHT: u16 = 5;
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let msg = "Terminal too small";
        let msg_len = msg.len() as u16;
        let x = area.x + area.width.saturating_sub(msg_len) / 2;
        let y = area.y + area.height / 2;
        let w = msg_len.min(area.width);
        if w > 0 && area.height > 0 {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    msg,
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )),
                Rect::new(x, y, w, 1),
            );
        }
        return;
    }

    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);

    let viewport_height = chunks[0].height as usize;

    // Render scrolled content
    let widget = Paragraph::new(rendered.text.clone()).scroll((scroll_offset as u16, 0));
    frame.render_widget(widget, chunks[0]);

    // Apply search match highlights
    if let Some(s) = search {
        if !s.query.is_empty() {
            let match_style = Style::default().bg(Color::Yellow).fg(Color::Black);
            let current_style = Style::default()
                .bg(Color::LightGreen)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD);

            for (idx, m) in s.matches.iter().enumerate() {
                let rel_line = m.rendered_line as isize - scroll_offset as isize;
                if rel_line >= 0 && (rel_line as usize) < viewport_height {
                    let row = chunks[0].y + rel_line as u16;
                    let style = if s.current_match == Some(idx) {
                        current_style
                    } else {
                        match_style
                    };
                    for col in m.column_start..m.column_end {
                        let pos = Position::new(chunks[0].x + col as u16, row);
                        if let Some(cell) = frame.buffer_mut().cell_mut(pos) {
                            cell.set_style(style);
                        }
                    }
                }
            }
        }
    }

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

    // Render help modal overlay
    if let Some(hl) = help {
        render_help(frame, hl, chunks[0]);
    }

    // Render status bar or search input bar
    if let Some(s) = search {
        if s.typing {
            // Search input bar
            let match_info = if s.matches.is_empty() && !s.query.is_empty() {
                " [No matches]".to_owned()
            } else if !s.matches.is_empty() {
                let current = s.current_match.map(|i| i + 1).unwrap_or(0);
                format!(" [{}/{}]", current, s.matches.len())
            } else {
                String::new()
            };
            let bar_text = format!("/{}|{}", s.query, match_info);
            let bar = Paragraph::new(Span::styled(
                bar_text,
                Style::default().fg(Color::White).bg(Color::DarkGray),
            ))
            .style(Style::default().bg(Color::DarkGray));
            frame.render_widget(bar, chunks[1]);
            return;
        }
    }

    // Render normal status bar with scroll position indicator
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
        .map(|h| format!(" {} {}", "\u{00A7}", h.text))
        .unwrap_or_default();

    let link_info = focused_link
        .and_then(|idx| rendered.link_positions.get(idx))
        .map(|l| format!(" -> {}", l.url))
        .unwrap_or_default();

    let search_info = search
        .map(|s| {
            if s.matches.is_empty() {
                format!("  /{} [No matches]", s.query)
            } else {
                let current = s.current_match.map(|i| i + 1).unwrap_or(0);
                format!("  /{} [{}/{}]", s.query, current, s.matches.len())
            }
        })
        .unwrap_or_default();

    let nav_info = if can_go_back {
        let name = current_file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");
        format!("  \u{2190} {}", name)
    } else {
        String::new()
    };

    let status = format!(
        " Line {}/{} \u{2014} {}{}{}{}{}",
        scroll_offset + 1,
        total_lines,
        position,
        nav_info,
        heading_ctx,
        link_info,
        search_info,
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
            Line::from(Span::styled(format!("{indent}{prefix} {}", h.text), style))
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

/// Render the help/shortcuts modal overlay with filterable shortcut list.
fn render_help(frame: &mut Frame, help: &HelpState, viewport_area: Rect) {
    let popup = centered_rect(60, 70, viewport_area);

    // Clear the popup area
    frame.render_widget(Clear, popup);

    let categories = shortcut_categories();
    let filter_lower = help.filter.to_lowercase();

    // Build styled lines: filter input, then grouped shortcuts
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Filter input line
    let filter_display = if help.filter.is_empty() {
        " Type to filter...".to_owned()
    } else {
        format!(" {}\u{2502}", help.filter) // │ as cursor
    };
    lines.push(Line::from(Span::styled(
        filter_display,
        Style::default().fg(Color::Yellow),
    )));
    lines.push(Line::from("")); // blank separator

    let mut any_match = false;
    for cat in &categories {
        // Filter entries in this category
        let filtered: Vec<&ShortcutEntry> = cat
            .entries
            .iter()
            .filter(|e| {
                if filter_lower.is_empty() {
                    return true;
                }
                e.key.to_lowercase().contains(&filter_lower)
                    || e.description.to_lowercase().contains(&filter_lower)
                    || cat.name.to_lowercase().contains(&filter_lower)
            })
            .collect();

        if filtered.is_empty() {
            continue;
        }
        any_match = true;

        // Category header
        lines.push(Line::from(Span::styled(
            format!(" {}", cat.name),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));

        // Shortcut entries
        for entry in &filtered {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("   {:16}", entry.key),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    entry.description.to_owned(),
                    Style::default().fg(Color::White),
                ),
            ]));
        }

        // Blank line after each category
        lines.push(Line::from(""));
    }

    if !any_match && !filter_lower.is_empty() {
        lines.push(Line::from(Span::styled(
            " No matching shortcuts",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let title = if help.filter.is_empty() {
        " Help \u{2014} ? to close "
    } else {
        " Help \u{2014} Esc to close "
    };

    let block = Block::bordered()
        .title(title)
        .style(Style::default().fg(Color::White));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((help.scroll_offset as u16, 0));

    frame.render_widget(paragraph, popup);
}
