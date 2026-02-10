# mdmd - Markdown Document Viewer

## Executive Summary

mdmd is a terminal-based markdown pager with accessibility-first design, built on the frankentui TUI framework. It provides vim-inspired navigation, delta-style heading traversal, a Zed-like outline modal, and link-following capabilities.

---

## 1. Architecture Overview

### 1.1 Core Design Principles

1. **Alt-Screen Mode**: Full-screen TUI experience (like less/vim), preserving scrollback on exit
2. **Elm Architecture**: frankentui's `Model::update()` + `Model::view()` pattern
3. **Accessibility First**: Screen reader hints, keyboard-only navigation, high contrast
4. **Streaming Support**: Handle large markdown files without loading entirely into memory
5. **Modular Components**: Separate concerns for parsing, rendering, navigation, and modals

### 1.2 High-Level Component Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                           mdmd Application                          │
├─────────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌────────────┐ │
│  │   Parser    │  │   Viewer    │  │   Outline   │  │  Searcher  │ │
│  │  (pulldown  │  │  (rendered  │  │   Modal     │  │  (regex/   │ │
│  │   -cmark)   │  │   buffer)   │  │             │  │   fuzzy)   │ │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  └─────┬──────┘ │
│         │                │                │                │        │
│         ▼                ▼                ▼                ▼        │
│  ┌──────────────────────────────────────────────────────────────┐  │
│  │                    DocumentState                             │  │
│  │  - parsed headings & positions                               │  │
│  │  - links & anchors                                          │  │
│  │  - current scroll position                                   │  │
│  │  - current heading index                                     │  │
│  └──────────────────────────────────────────────────────────────┘  │
│                                │                                    │
│                                ▼                                    │
│  ┌──────────────────────────────────────────────────────────────┐  │
│  │                    ftui Runtime (App)                        │  │
│  │  - Event loop, key dispatch                                  │  │
│  │  - Modal stack management                                    │  │
│  │  - Screen mode (AltScreen)                                   │  │
│  └──────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
```

### 1.3 Screen Layout (Main View)

```
┌──────────────────────────────────────────────────────────────────┐
│ mdmd: README.md                     [Line 42/256] # Installation │  ← Status bar
├──────────────────────────────────────────────────────────────────┤
│                                                                  │
│ # Installation                                                   │
│                                                                  │
│ First, clone the repository:                                     │
│                                                                  │
│ ```bash                                                          │
│ git clone https://github.com/example/mdmd                        │
│ ```                                                              │
│                                                                  │  ← Main viewport
│ ## Dependencies                                                  │
│                                                                  │
│ Make sure you have Rust 1.70+ installed.                         │
│                                                                  │
│ ## Building                                                      │
│                                                                  │
│ ```bash                                                          │
│ cargo build --release                                            │
│ ```                                                              │
│                                                                  │
├──────────────────────────────────────────────────────────────────┤
│ n:next  p:prev  o:outline  /:search  ?:help  q:quit             │  ← Help bar
└──────────────────────────────────────────────────────────────────┘
```

---

## 2. Data Structures

### 2.1 Core Document Model

```rust
/// A parsed markdown heading with position information
#[derive(Debug, Clone)]
pub struct Heading {
    /// Heading level (1-6)
    pub level: u8,
    /// The heading text content
    pub text: String,
    /// Line number in the source document (0-indexed)
    pub source_line: usize,
    /// Line number in the rendered output (0-indexed)
    pub rendered_line: usize,
    /// Unique anchor ID for linking (e.g., "installation")
    pub anchor: String,
}

/// A parsed markdown link
#[derive(Debug, Clone)]
pub struct Link {
    /// Display text
    pub text: String,
    /// Link destination
    pub href: String,
    /// Line number in rendered output
    pub rendered_line: usize,
    /// Column start position
    pub col_start: usize,
    /// Column end position
    pub col_end: usize,
    /// Whether this is an internal anchor link
    pub is_anchor: bool,
    /// Whether this links to another markdown file
    pub is_markdown_file: bool,
}

/// Complete document representation
pub struct Document {
    /// Original markdown source
    pub source: String,
    /// File path (for relative link resolution)
    pub path: PathBuf,
    /// Rendered text lines (post-markdown processing)
    pub rendered_lines: Vec<Line>,
    /// All headings in document order
    pub headings: Vec<Heading>,
    /// All links in document order
    pub links: Vec<Link>,
    /// Mapping from rendered line -> source line
    pub line_map: Vec<usize>,
}

/// Navigation history for back/forward
pub struct NavigationHistory {
    /// Stack of visited files + positions
    history: Vec<HistoryEntry>,
    /// Current position in history
    current_idx: usize,
}

pub struct HistoryEntry {
    pub path: PathBuf,
    pub line: usize,
    pub heading_idx: Option<usize>,
}
```

### 2.2 Application State

```rust
/// Main application state
pub struct MdmdState {
    /// Current document
    pub document: Document,

    /// Current scroll offset (top visible line)
    pub scroll_offset: usize,

    /// Index of current heading (for n/p navigation)
    pub current_heading_idx: Option<usize>,

    /// Index of currently focused link (for link navigation)
    pub current_link_idx: Option<usize>,

    /// Whether in link navigation mode
    pub link_mode: bool,

    /// Navigation history
    pub history: NavigationHistory,

    /// Active modal state
    pub modal: ModalState,

    /// Search state
    pub search: SearchState,

    /// Terminal dimensions
    pub viewport_height: u16,
    pub viewport_width: u16,
}

pub enum ModalState {
    None,
    Outline(OutlineModalState),
    Help(HelpModalState),
    Search(SearchModalState),
}

pub struct OutlineModalState {
    /// List state for selection
    pub list_state: ListState,
    /// Filter text for outline search
    pub filter: String,
    /// Filtered heading indices
    pub filtered_indices: Vec<usize>,
}

pub struct SearchModalState {
    /// Current search query
    pub query: String,
    /// Search direction (forward/backward)
    pub direction: SearchDirection,
    /// Current match index
    pub current_match: Option<usize>,
    /// All match positions (line, col_start, col_end)
    pub matches: Vec<(usize, usize, usize)>,
    /// Search mode (regex, fuzzy, literal)
    pub mode: SearchMode,
}

pub enum SearchDirection {
    Forward,
    Backward,
}

pub enum SearchMode {
    Literal,
    Regex,
    Fuzzy,
}
```

---

## 3. Module Structure

```
src/
├── main.rs              # Entry point, CLI arg parsing
├── app.rs               # MdmdApp implementing Model trait
├── document/
│   ├── mod.rs           # Document struct and loading
│   ├── parser.rs        # Markdown parsing (uses ftui-extras/markdown)
│   ├── heading.rs       # Heading extraction and anchor generation
│   └── link.rs          # Link extraction and classification
├── view/
│   ├── mod.rs           # Main viewport rendering
│   ├── status_bar.rs    # Top status bar
│   ├── help_bar.rs      # Bottom help bar
│   └── scrollbar.rs     # Optional scrollbar widget
├── modal/
│   ├── mod.rs           # Modal management
│   ├── outline.rs       # Outline/TOC modal (cmd-shift-o style)
│   ├── help.rs          # Keyboard shortcuts modal
│   └── search.rs        # Search input modal
├── navigation/
│   ├── mod.rs           # Navigation logic
│   ├── heading.rs       # Heading traversal (n/p)
│   ├── link.rs          # Link following
│   └── history.rs       # Back/forward navigation
├── search/
│   ├── mod.rs           # Search engine
│   ├── regex.rs         # Regex search
│   └── highlight.rs     # Match highlighting
├── keybindings.rs       # Key mapping and dispatch
└── config.rs            # Configuration (theme, keybindings)
```

---

## 4. Feature Implementation Details

### 4.1 Phase 1: Core Viewer (MVP)

**Goal**: Display a markdown file with basic scrolling

#### 4.1.1 Markdown Parsing & Rendering

- Use `ftui-extras::markdown::MarkdownRenderer` for rendering
- Parse headings during initial load to build heading index
- Store rendered `Text` (Vec<Line>) for viewport rendering

```rust
impl Document {
    pub fn load(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path)?;
        let renderer = MarkdownRenderer::new(MarkdownTheme::default());
        let rendered = renderer.render(&source);

        // Extract headings from source using pulldown-cmark
        let headings = extract_headings(&source, &rendered);
        let links = extract_links(&source, &rendered);

        Ok(Document {
            source,
            path: path.to_owned(),
            rendered_lines: rendered.lines().to_vec(),
            headings,
            links,
            line_map: compute_line_map(&source, &rendered),
        })
    }
}
```

#### 4.1.2 Basic Scrolling

- `j`/`Down` - scroll down one line
- `k`/`Up` - scroll up one line
- `Ctrl+d` - scroll down half page
- `Ctrl+u` - scroll up half page
- `Ctrl+f`/`PageDown` - scroll down full page
- `Ctrl+b`/`PageUp` - scroll up full page
- `g`/`Home` - go to top
- `G`/`End` - go to bottom
- Mouse wheel scrolling (via ftui mouse events)

#### 4.1.3 Status Bar

- Left: `mdmd: <filename>`
- Center: Current heading (if any)
- Right: `[Line X/Y]` or percentage

#### 4.1.4 Help Bar

- Short keybinding hints at bottom
- `n:next  p:prev  o:outline  /:search  ?:help  q:quit`

#### Testing Checklist (Phase 1)
- [ ] Load and display a simple markdown file
- [ ] All scroll operations work correctly
- [ ] Status bar updates on scroll
- [ ] `q` exits cleanly
- [ ] Large files (1000+ lines) scroll smoothly
- [ ] Unicode content displays correctly

---

### 4.2 Phase 2: Heading Navigation (Delta-style)

**Goal**: Navigate between headings with `n` and `p`

#### 4.2.1 Heading Index

During parsing, build a sorted list of headings with their rendered line positions.

```rust
fn extract_headings(source: &str, rendered: &Text) -> Vec<Heading> {
    use pulldown_cmark::{Parser, Event, Tag, HeadingLevel};

    let parser = Parser::new_ext(source, Options::all());
    let mut headings = Vec::new();
    let mut current_heading: Option<(HeadingLevel, String)> = None;
    let mut source_line = 0;

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current_heading = Some((level, String::new()));
            }
            Event::Text(text) if current_heading.is_some() => {
                if let Some((_, ref mut content)) = current_heading {
                    content.push_str(&text);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, text)) = current_heading.take() {
                    let anchor = slugify(&text);
                    let rendered_line = find_heading_line(rendered, &text);
                    headings.push(Heading {
                        level: heading_level_to_u8(level),
                        text,
                        source_line,
                        rendered_line,
                        anchor,
                    });
                }
            }
            // Track source lines via SoftBreak/HardBreak events
            _ => {}
        }
    }
    headings
}
```

#### 4.2.2 Navigation Logic

```rust
impl MdmdState {
    /// Move to next heading, returning true if moved
    pub fn next_heading(&mut self) -> bool {
        let current_line = self.scroll_offset;

        // Find first heading after current scroll position
        for (idx, heading) in self.document.headings.iter().enumerate() {
            if heading.rendered_line > current_line {
                self.current_heading_idx = Some(idx);
                self.scroll_to_line(heading.rendered_line);
                return true;
            }
        }
        false
    }

    /// Move to previous heading
    pub fn prev_heading(&mut self) -> bool {
        let current_line = self.scroll_offset;

        // Find last heading before current scroll position
        for (idx, heading) in self.document.headings.iter().enumerate().rev() {
            if heading.rendered_line < current_line {
                self.current_heading_idx = Some(idx);
                self.scroll_to_line(heading.rendered_line);
                return true;
            }
        }
        false
    }
}
```

#### Testing Checklist (Phase 2)
- [ ] `n` moves to next heading
- [ ] `p` moves to previous heading
- [ ] Navigation wraps correctly at document boundaries
- [ ] Status bar shows current heading
- [ ] Current heading is highlighted in viewport

---

### 4.3 Phase 3: Outline Modal (Zed-style)

**Goal**: `o` opens outline modal, filter and select headings

#### 4.3.1 Modal Design

```
┌────────────────────────────────────────────────────────────────┐
│ Outline                                                    [X] │
├────────────────────────────────────────────────────────────────┤
│ > _                                                            │  ← Filter input
├────────────────────────────────────────────────────────────────┤
│ ▶ # Installation                                               │  ← Selected
│   ## Dependencies                                              │
│   ## Building                                                  │
│     ### Debug Build                                            │
│     ### Release Build                                          │
│   # Usage                                                      │
│   ## Basic Commands                                            │
│   ## Advanced Features                                         │
│   # Configuration                                              │
│   # Contributing                                               │
│   # License                                                    │
├────────────────────────────────────────────────────────────────┤
│ ↑/↓: Navigate  Enter: Go to  Esc: Close                       │
└────────────────────────────────────────────────────────────────┘
```

#### 4.3.2 Key Behaviors

1. **On open**: Pre-select the heading containing the current cursor position
2. **As you navigate**: Background viewport scrolls to show selected heading
3. **Filter as you type**: Fuzzy match on heading text
4. **Hierarchy display**: Indent based on heading level, show `#` symbols
5. **Enter**: Jump to heading and close modal
6. **Escape**: Close modal, return to original position

#### 4.3.3 Implementation

```rust
pub struct OutlineModal {
    filter_input: TextInput,
    list_state: ListState,
    original_scroll: usize,  // Restore on Esc
}

impl OutlineModal {
    pub fn new(state: &MdmdState) -> Self {
        let mut list_state = ListState::default();

        // Find current heading to pre-select
        let current_idx = state.current_heading_idx
            .unwrap_or_else(|| find_heading_for_line(&state.document.headings, state.scroll_offset));

        list_state.select(Some(current_idx));

        Self {
            filter_input: TextInput::new(),
            list_state,
            original_scroll: state.scroll_offset,
        }
    }

    pub fn filtered_headings(&self, doc: &Document) -> Vec<(usize, &Heading)> {
        if self.filter_input.value().is_empty() {
            doc.headings.iter().enumerate().collect()
        } else {
            let query = self.filter_input.value().to_lowercase();
            doc.headings.iter()
                .enumerate()
                .filter(|(_, h)| h.text.to_lowercase().contains(&query))
                .collect()
        }
    }

    pub fn render_heading(heading: &Heading) -> String {
        let indent = "  ".repeat((heading.level - 1) as usize);
        let hashes = "#".repeat(heading.level as usize);
        format!("{}{} {}", indent, hashes, heading.text)
    }
}
```

#### Testing Checklist (Phase 3)
- [ ] `o` opens outline modal
- [ ] Current heading is pre-selected
- [ ] Up/Down navigates the list
- [ ] Background scrolls as selection changes
- [ ] Filter narrows down headings
- [ ] Enter jumps to heading and closes modal
- [ ] Escape returns to original position
- [ ] Hierarchy is visually clear

---

### 4.4 Phase 4: Search (Vim-style)

**Goal**: `/` for forward search, `?` for backward search, `n`/`N` for next/prev match

#### 4.4.1 Search Modal

```
┌────────────────────────────────────────────────────────────────┐
│ /pattern_                                            [3 of 12] │
└────────────────────────────────────────────────────────────────┘
```

- Minimal modal at bottom of screen (like vim's command line)
- Live highlighting as you type
- Show match count

#### 4.4.2 Search Behavior

- `/` - Forward search from current position
- `?` - Backward search from current position
- `Enter` - Confirm search, close input, stay on first match
- `Escape` - Cancel search, return to original position
- `n` - Next match (in search direction)
- `N` - Previous match (opposite direction)

#### 4.4.3 Implementation

```rust
pub struct SearchEngine {
    compiled_regex: Option<Regex>,
    matches: Vec<SearchMatch>,
    current_idx: usize,
}

pub struct SearchMatch {
    pub line: usize,
    pub col_start: usize,
    pub col_end: usize,
}

impl SearchEngine {
    pub fn search(&mut self, doc: &Document, query: &str, is_regex: bool) -> Result<()> {
        self.matches.clear();

        let pattern = if is_regex {
            Regex::new(query)?
        } else {
            Regex::new(&regex::escape(query))?
        };

        self.compiled_regex = Some(pattern.clone());

        for (line_idx, line) in doc.rendered_lines.iter().enumerate() {
            let text = line_to_string(line);
            for mat in pattern.find_iter(&text) {
                self.matches.push(SearchMatch {
                    line: line_idx,
                    col_start: mat.start(),
                    col_end: mat.end(),
                });
            }
        }

        Ok(())
    }

    pub fn next_match(&mut self, from_line: usize, direction: SearchDirection) -> Option<&SearchMatch> {
        // Find next match in direction from current line
        // ...
    }
}
```

#### 4.4.4 Match Highlighting

- Highlight all matches in viewport with distinct style
- Current match has different (brighter) highlight
- Matches in status area: "[3 of 12]"

#### Testing Checklist (Phase 4)
- [ ] `/` opens search input at bottom
- [ ] Live highlighting as you type
- [ ] Enter confirms and jumps to first match
- [ ] Escape cancels and returns to original position
- [ ] `n` goes to next match
- [ ] `N` goes to previous match
- [ ] Match count shown correctly
- [ ] Wraps around document
- [ ] Case-insensitive by default (or configurable)

---

### 4.5 Phase 5: Link Navigation & Following

**Goal**: Navigate between links, follow to other documents

#### 4.5.1 Link Mode

- `Tab` - Enter link mode (or toggle)
- `l` - Next link
- `h` - Previous link
- `Enter` - Follow link
- `Escape` - Exit link mode

#### 4.5.2 Link Types

1. **Internal anchors**: `#installation` → scroll to heading
2. **Relative markdown files**: `./docs/guide.md` → load new document
3. **Absolute markdown files**: `/path/to/file.md` → load new document
4. **External URLs**: `https://...` → open in browser (via `open` command)

#### 4.5.3 Link Highlighting

- In link mode, current link is highlighted/underlined
- Links are visually distinct (color, underline) in normal mode

#### 4.5.4 Navigation History

```rust
impl NavigationHistory {
    pub fn push(&mut self, path: PathBuf, line: usize) {
        // Truncate forward history if we're not at the end
        self.history.truncate(self.current_idx + 1);
        self.history.push(HistoryEntry { path, line, heading_idx: None });
        self.current_idx = self.history.len() - 1;
    }

    pub fn back(&mut self) -> Option<&HistoryEntry> {
        if self.current_idx > 0 {
            self.current_idx -= 1;
            Some(&self.history[self.current_idx])
        } else {
            None
        }
    }

    pub fn forward(&mut self) -> Option<&HistoryEntry> {
        if self.current_idx < self.history.len() - 1 {
            self.current_idx += 1;
            Some(&self.history[self.current_idx])
        } else {
            None
        }
    }
}
```

#### 4.5.5 Keybindings

- `Backspace` or `Ctrl+o` - Go back in history
- `Ctrl+i` - Go forward in history

#### Testing Checklist (Phase 5)
- [ ] Tab enters link mode
- [ ] Links are highlighted
- [ ] l/h navigate between links
- [ ] Enter on anchor scrolls to heading
- [ ] Enter on relative .md file loads it
- [ ] Back navigation works
- [ ] Forward navigation works
- [ ] External links open in browser

---

### 4.6 Phase 6: Help Modal

**Goal**: `?` shows filterable shortcuts dialog

#### 4.6.1 Help Modal Design

```
┌────────────────────────────────────────────────────────────────┐
│ Keyboard Shortcuts                                         [X] │
├────────────────────────────────────────────────────────────────┤
│ > _                                                            │
├────────────────────────────────────────────────────────────────┤
│                                                                │
│ NAVIGATION                                                     │
│   j, ↓          Scroll down one line                          │
│   k, ↑          Scroll up one line                            │
│   Ctrl+d        Scroll down half page                         │
│   Ctrl+u        Scroll up half page                           │
│   g, Home       Go to top                                     │
│   G, End        Go to bottom                                  │
│                                                                │
│ HEADINGS                                                       │
│   n             Next heading                                  │
│   p             Previous heading                              │
│   o             Open outline                                  │
│                                                                │
│ SEARCH                                                         │
│   /             Search forward                                │
│   ?             This help (or search backward if in normal)   │
│   n             Next match                                    │
│   N             Previous match                                │
│                                                                │
│ LINKS                                                          │
│   Tab           Enter link mode                               │
│   Enter         Follow link                                   │
│   Backspace     Go back                                       │
│                                                                │
├────────────────────────────────────────────────────────────────┤
│ ↑/↓: Scroll  /: Filter  Esc: Close                            │
└────────────────────────────────────────────────────────────────┘
```

#### 4.6.2 Implementation

Use ftui-widgets `Help` widget with categories:
- Navigation
- Headings
- Search
- Links
- General

Make it filterable via the filter input.

#### Testing Checklist (Phase 6)
- [ ] `?` opens help modal
- [ ] All shortcuts are listed
- [ ] Categories are clear
- [ ] Filter works
- [ ] Escape closes modal

---

## 5. Keybinding Summary

| Key | Mode | Action |
|-----|------|--------|
| `q` | Normal | Quit |
| `j` / `↓` | Normal | Scroll down line |
| `k` / `↑` | Normal | Scroll up line |
| `Ctrl+d` | Normal | Scroll down half page |
| `Ctrl+u` | Normal | Scroll up half page |
| `Ctrl+f` / `PageDown` | Normal | Scroll down page |
| `Ctrl+b` / `PageUp` | Normal | Scroll up page |
| `g` / `Home` | Normal | Go to top |
| `G` / `End` | Normal | Go to bottom |
| `n` | Normal | Next heading |
| `p` | Normal | Previous heading |
| `o` | Normal | Open outline modal |
| `/` | Normal | Search forward |
| `?` | Normal | Open help modal |
| `n` | After search | Next match |
| `N` | After search | Previous match |
| `Tab` | Normal | Enter link mode |
| `Enter` | Link mode | Follow link |
| `Escape` | Link mode | Exit link mode |
| `l` | Link mode | Next link |
| `h` | Link mode | Previous link |
| `Backspace` / `Ctrl+o` | Normal | Go back in history |
| `Ctrl+i` | Normal | Go forward in history |
| `Escape` | Any modal | Close modal |
| `Enter` | Outline modal | Go to heading |
| `↑` / `↓` | Outline modal | Navigate list |

---

## 6. Testing Strategy

### 6.1 Unit Tests

```rust
#[cfg(test)]
mod tests {
    // Document parsing
    #[test]
    fn test_extract_headings_simple() { ... }

    #[test]
    fn test_extract_headings_nested() { ... }

    #[test]
    fn test_extract_links() { ... }

    #[test]
    fn test_anchor_generation() { ... }

    // Navigation
    #[test]
    fn test_next_heading() { ... }

    #[test]
    fn test_prev_heading_at_start() { ... }

    // Search
    #[test]
    fn test_search_literal() { ... }

    #[test]
    fn test_search_regex() { ... }

    #[test]
    fn test_search_next_wraps() { ... }
}
```

### 6.2 Integration Tests

Use `ftui-harness` snapshot testing:

```rust
#[test]
fn snapshot_main_view() {
    let doc = Document::load(Path::new("fixtures/test.md")).unwrap();
    let state = MdmdState::new(doc);
    // Render and compare to golden snapshot
}

#[test]
fn snapshot_outline_modal() {
    // Test outline modal rendering
}

#[test]
fn snapshot_search_active() {
    // Test search highlighting
}
```

### 6.3 Manual QA Protocol

For each feature:
1. Build: `cargo build`
2. Run: `cargo run -- fixtures/test.md`
3. Test the specific feature
4. Document what was verified
5. Run `cargo fmt && cargo test`
6. Commit with jj

### 6.4 Test Fixtures

Create `fixtures/` directory with:
- `simple.md` - Basic headings and paragraphs
- `nested.md` - Deeply nested headings
- `links.md` - Various link types
- `large.md` - 1000+ lines for performance testing
- `unicode.md` - Unicode content, emoji, CJK
- `code.md` - Code blocks with syntax highlighting

---

## 7. Performance Considerations

### 7.1 Large File Handling

- **Lazy rendering**: Only render visible viewport + buffer
- **Heading index**: Built once on load, O(1) lookup
- **Search**: Use rope-like structure for large files? (or just Vec<String>)
- **Memory budget**: ~100KB per 1000 lines is acceptable

### 7.2 Rendering Budget

- Target: 16ms frame time (60fps)
- Minimize allocations in render loop
- Cache rendered lines, invalidate on resize

### 7.3 Startup Time

- Parse markdown in background if file is large
- Show loading indicator for files > 100KB

---

## 8. Error Handling

### 8.1 File Errors

```rust
pub enum LoadError {
    NotFound(PathBuf),
    PermissionDenied(PathBuf),
    NotMarkdown(PathBuf),
    IoError(std::io::Error),
}
```

- Show error in UI, don't crash
- For link following, show error message if target not found

### 8.2 Invalid Markdown

- Gracefully handle malformed markdown
- pulldown-cmark is very tolerant
- Display raw text if parsing fails completely

### 8.3 Terminal Issues

- Handle resize events
- Handle Ctrl+C gracefully (via ftui's keybinding policy)
- Clean terminal state on exit (alt-screen restore)

---

## 9. Configuration (Future)

### 9.1 Config File (~/.config/mdmd/config.toml)

```toml
[theme]
heading_style = "bold yellow"
link_style = "underline cyan"
search_highlight = "reverse"

[keybindings]
next_heading = "n"
prev_heading = "p"
# ... customizable

[behavior]
wrap_lines = true
show_line_numbers = false
```

### 9.2 Command Line Args

```
mdmd [OPTIONS] <FILE>

OPTIONS:
    -l, --line <LINE>       Start at line number
    -h, --heading <TEXT>    Jump to heading matching TEXT
    -p, --pattern <REGEX>   Search for pattern
    --no-color              Disable colors
    --help                  Show help
    --version               Show version
```

---

## 10. Dependencies

### 10.1 Required

```toml
[dependencies]
ftui = { path = "../frankentui/crates/ftui" }
ftui-extras = { path = "../frankentui/crates/ftui-extras", features = ["markdown"] }
pulldown-cmark = "0.9"
regex = "1"
clap = { version = "4", features = ["derive"] }
```

### 10.2 Development

```toml
[dev-dependencies]
insta = "1"  # Snapshot testing
```

---

## 11. Implementation Order (Phases → Tasks)

### Phase 1: Foundation (2-3 sessions)
1. Project setup (Cargo.toml, directory structure)
2. CLI argument parsing with clap
3. Basic document loading and markdown parsing
4. Simple viewport rendering (no scrolling)
5. Basic scrolling (j/k, Ctrl+d/u)
6. Status bar implementation
7. Help bar implementation
8. Quit functionality (q)

### Phase 2: Heading Navigation (1-2 sessions)
1. Heading extraction during parsing
2. Current heading detection
3. Next/previous heading navigation (n/p)
4. Status bar current heading display
5. Heading highlighting in viewport

### Phase 3: Outline Modal (2-3 sessions)
1. Modal framework integration (from ftui-widgets/modal)
2. Outline modal layout and rendering
3. Heading list with hierarchy display
4. Pre-selection of current heading
5. Navigation with background scroll
6. Filter input
7. Fuzzy filtering
8. Enter to jump, Escape to cancel

### Phase 4: Search (2 sessions)
1. Search input modal at bottom
2. Regex search engine
3. Live match highlighting
4. Match count display
5. n/N for next/previous match
6. Wrap-around behavior

### Phase 5: Link Navigation (2 sessions)
1. Link extraction during parsing
2. Link highlighting in viewport
3. Link mode toggle (Tab)
4. Link navigation (l/h)
5. Internal anchor following
6. Relative markdown file following
7. Navigation history (back/forward)
8. External URL opening

### Phase 6: Help Modal (1 session)
1. Help modal layout
2. Keybinding registry
3. Category grouping
4. Filter functionality

### Phase 7: Polish (1-2 sessions)
1. Edge case handling
2. Performance optimization
3. Error message improvements
4. Final testing

---

## 12. Success Criteria

### Minimum Viable Product (Phases 1-2)
- [x] Open and display any markdown file
- [x] Scroll smoothly with vim-style keys
- [x] Navigate between headings with n/p
- [x] Clean exit with q
- [x] Works in standard terminals (kitty, iTerm2, Terminal.app)

### Full Feature Set (All Phases)
- [x] All MVP features
- [x] Outline modal with filter
- [x] Vim-style search with highlighting
- [x] Link following between documents
- [x] Help modal with all shortcuts
- [x] Navigation history

### Quality Gates
- All `cargo test` tests pass
- `cargo clippy` has no warnings
- `cargo fmt` produces no changes
- Manual QA checklist complete for each feature
- Works on Linux and macOS

---

## 13. Open Questions / Decisions Needed

1. **Search behavior**: Should search be case-insensitive by default? (Recommend: yes)
2. **Heading scope for n/p**: Navigate all headings or just H1/H2? (Recommend: all)
3. **Link mode vs. inline**: Should links be navigable without explicit mode? (Recommend: explicit mode for clarity)
4. **External link handling**: Open in browser or just display URL? (Recommend: open in browser)
5. **Mouse support**: How extensive? (Recommend: scroll only in Phase 1, clicks in future)
6. **Theme customization**: In scope for v1? (Recommend: defer to future)

---

## 14. References

- Delta pager: https://github.com/dandavison/delta
- Zed's outline: Cmd+Shift+O behavior
- Vim's search: / ? n N behavior
- frankentui docs: /home/exedev/repos/frankentui/docs/
- frankentui command palette spec: A reference for modal behavior
- pulldown-cmark: https://docs.rs/pulldown-cmark
