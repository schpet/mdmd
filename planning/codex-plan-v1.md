# mdmd v1 Plan

## 0) Planning Contract

This plan follows a "design first" workflow.

- Target split: ~85% planning, ~15% implementation.
- Every milestone is independently runnable and testable.
- Every milestone has explicit automated tests and manual QA checks.
- No feature is considered complete without `cargo fmt`, `cargo test`, and QA notes.

## 1) Product Definition

### 1.1 Vision

`mdmd` is an accessibility-first markdown pager in the terminal, built with Rust + frankentui.

Primary outcome:
- Read markdown quickly.
- Move between headings quickly (`n` / `p`).
- See and jump via an outline modal (Zed Cmd+Shift+O style in terminal form).
- Use slash search (`/`) for fast find.
- Follow links across markdown files.
- Discover all shortcuts through a filterable `?` help dialog.

### 1.2 User Goals

- "I can open a markdown file and navigate structure, not just raw lines."
- "I can jump to sections with consistent keyboard shortcuts."
- "I can preview and select headings from a structured outline."
- "I can find text quickly with slash-search semantics."
- "I can follow links to connected docs and come back."
- "I can always discover keybindings from inside the app."

### 1.3 Non-Goals (v1)

- Full WYSIWYG markdown layout parity with GUI editors.
- Editing/writing markdown (viewer/pager only).
- Network fetching (`http(s)` link crawling) inside the app.
- Multi-pane document comparison.
- Plugin system.

## 2) Architecture Decisions and Trade-offs

### 2.1 Runtime Model

Use frankentui Elm-style loop:
- `Event -> Model::update -> Model::view -> diff/present`

Decision:
- Keep app state in one top-level model with strict sub-state modules.

Trade-off:
- Pro: predictable event handling, high testability.
- Con: state struct grows if boundaries are not enforced.
- Mitigation: isolate feature state into dedicated modules and action reducers.

### 2.2 Screen Mode

Default to `AltScreen`.

Decision rationale:
- Pager behavior matches user expectations from `less`-style tools.
- Full-screen layout enables stable outline/help overlays.
- Existing shell output is restored on exit.

Trade-off:
- Inline mode preserves scrollback while running.
- v1 keeps alt-screen default and defers inline mode to a later milestone.

### 2.3 Rendering Approach

Use source-faithful markdown display (styled markdown source lines), not full semantic reflow rendering in v1.

Decision rationale:
- Heading and link navigation depend on exact source line mapping.
- Source-faithful rendering keeps offsets stable for outline/search/link focus.
- Lower complexity and lower bug risk for v1.

Trade-off:
- Less "pretty" than fully rendered markdown.
- Much more robust for precise structural navigation.

### 2.4 Parsing Strategy

Use `pulldown-cmark` with offset tracking to build a structural index.

Decision rationale:
- Correctly ignores pseudo-headings inside code blocks.
- Captures heading hierarchy + links + source offsets.
- Enables robust section and anchor mapping.

### 2.5 State and Feature Modularity

Use single binary crate for v1 with strict module boundaries.

Decision rationale:
- Fastest path to deliver.
- Lower workspace overhead at early stage.

Trade-off:
- Harder future reuse if core logic and UI are tightly coupled.
- Mitigation: keep document/index/navigation/search logic pure and UI-agnostic.

### 2.6 Search Concurrency

Perform short-file search synchronously; switch to background task for large files.

Decision rationale:
- Keeps simple path simple.
- Avoids UI freeze on large docs.

Rule of thumb:
- If line count > threshold (for example 15k), compute matches via `Cmd::task`.

## 3) Proposed Project Structure

```text
mdmd/
  Cargo.toml
  src/
    main.rs
    app.rs
    cli.rs
    error.rs
    constants.rs
    model/
      mod.rs
      app_state.rs
      modes.rs
      document_state.rs
      viewport_state.rs
      outline_state.rs
      search_state.rs
      link_state.rs
      help_state.rs
      status_state.rs
    domain/
      mod.rs
      document.rs
      heading.rs
      link.rs
      anchor.rs
      pathing.rs
      shortcuts.rs
    parser/
      mod.rs
      markdown_index.rs
      slugify.rs
      line_map.rs
    nav/
      mod.rs
      heading_nav.rs
      link_nav.rs
      history_nav.rs
      section_nav.rs
    input/
      mod.rs
      action.rs
      keymap.rs
      dispatcher.rs
    ui/
      mod.rs
      layout.rs
      theme.rs
      draw_context.rs
      widgets/
        document_view.rs
        status_bar.rs
        outline_modal.rs
        search_prompt.rs
        help_modal.rs
        link_badge.rs
    infra/
      mod.rs
      fs.rs
      opener.rs
      clock.rs
  tests/
    parser_headings.rs
    parser_links.rs
    nav_headings.rs
    nav_outline.rs
    search_behavior.rs
    link_following.rs
    help_filtering.rs
    app_event_flow.rs
  fixtures/
    docs/
      simple.md
      nested_headings.md
      links_internal.md
      links_external.md
      duplicate_anchors.md
      unicode.md
      large.md
```

## 4) Core Data Structures and State Management

### 4.1 Document Model

```rust
struct Document {
    path: PathBuf,
    source: Arc<str>,
    lines: Vec<LineRecord>,
    headings: Vec<Heading>,
    links: Vec<LinkRef>,
    sections: Vec<Section>,
    anchors: AnchorIndex,
}
```

`LineRecord`:
- `line_number: usize`
- `start_byte: usize`
- `end_byte: usize`
- `text: Arc<str>`
- `flags: LineFlags` (heading/link/code-fence/etc)

`Heading`:
- `id: HeadingId`
- `level: u8`
- `title: String`
- `line_number: usize`
- `start_byte: usize`
- `end_byte: usize`
- `slug: String`
- `parent: Option<HeadingId>`

`Section`:
- `heading_id: HeadingId`
- `start_line: usize`
- `end_line_exclusive: usize`

`LinkRef`:
- `id: LinkId`
- `label: String`
- `destination_raw: String`
- `resolved: ResolvedLink`
- `line_number: usize`
- `start_col: usize`
- `end_col: usize`

`ResolvedLink`:
- `SameDocAnchor { slug }`
- `LocalMarkdown { path, anchor: Option<String> }`
- `ExternalUrl { url }`
- `Unsupported { reason }`

### 4.2 App State

```rust
enum AppMode {
    Normal,
    Outline,
    SearchInput,
    Help,
}

struct AppState {
    mode: AppMode,
    doc_stack: Vec<DocumentSession>,
    viewport: ViewportState,
    outline: OutlineState,
    search: SearchState,
    links: LinkState,
    help: HelpState,
    status: StatusState,
    pending: PendingOps,
}
```

`DocumentSession`:
- current `Document`
- previous cursor/scroll position for return navigation
- base directory for relative link resolution

`ViewportState`:
- `top_line`
- `cursor_line`
- `height`
- `width`
- `preferred_col`

`OutlineState`:
- `open: bool`
- `query: String`
- `filtered_heading_ids: Vec<HeadingId>`
- `selected_idx`
- `origin_cursor_line` (for cancel restore)
- `preview_cursor_line`

`SearchState`:
- `query: String`
- `active: bool`
- `matches: Vec<SearchMatch>`
- `current_match_idx`
- `case_sensitive: bool`
- `in_progress: bool`

`LinkState`:
- `focused_link: Option<LinkId>`
- `link_focus_order: Vec<LinkId>`
- `last_follow_error: Option<String>`

`HelpState`:
- `open: bool`
- `filter: String`
- `filtered_shortcuts: Vec<ShortcutId>`
- `selected_idx`

### 4.3 State Invariants

- `cursor_line` is always within document bounds.
- `top_line <= cursor_line` unless document is empty.
- Outline selected heading always exists in `filtered_heading_ids`.
- Search match indices always point to valid lines.
- Link focus never points to missing link ID.
- On modal close, mode returns to `Normal` deterministically.

## 5) UI Component Hierarchy and Event Handling

### 5.1 View Tree

```text
Root
  DocumentViewport
  StatusBar
  OverlayLayer (conditional)
    OutlineModal
    SearchPrompt
    HelpModal
```

### 5.2 Event Pipeline

```text
ftui Event
  -> input::dispatcher (mode-aware)
  -> Action enum
  -> reducer(s) mutate state
  -> optional Cmd (quit, async task, etc.)
  -> view re-render
```

### 5.3 Action Model

Representative actions:
- `Quit`
- `ScrollUp`, `ScrollDown`, `PageUp`, `PageDown`, `GoTop`, `GoBottom`
- `NextHeading`, `PrevHeading`
- `OpenOutline`, `CloseOutline`, `OutlineFilterInput`, `OutlineMoveUp`, `OutlineMoveDown`, `OutlineConfirm`
- `OpenSearch`, `SearchInputChar`, `SearchBackspace`, `SearchSubmit`, `SearchCancel`, `SearchNext`, `SearchPrev`
- `FocusNextLink`, `FocusPrevLink`, `FollowFocusedLink`, `NavigateBack`
- `OpenHelp`, `HelpFilterInput`, `HelpMoveUp`, `HelpMoveDown`, `HelpClose`
- `Resize`

### 5.4 Keymap (v1)

Normal mode:
- `q`: quit
- `j` / `Down`: scroll down
- `k` / `Up`: scroll up
- `PageDown` / `Space`: page down
- `PageUp`: page up
- `g`: top
- `G`: bottom
- `n`: next heading
- `p`: previous heading
- `o` (terminal equivalent for Cmd+Shift+O intent): outline modal
- `/`: search prompt
- `Tab`: next link
- `Shift+Tab`: previous link
- `Enter`: follow focused link
- `b`: back to previous document in stack
- `?`: shortcuts dialog

Outline mode:
- `j`/`k` or arrows: move selection and live-preview target heading
- text input: filter headings
- `Enter`: confirm jump
- `Esc`: cancel and restore origin position

Search mode:
- text input + `Backspace`
- `Enter`: run search and jump to first match
- `Ctrl+n`: next search match
- `Ctrl+p`: previous search match
- `Esc`: close search prompt

Help mode:
- text input filter
- arrows `j/k`: move through filtered shortcuts
- `Esc` or `?`: close

## 6) Feature Implementation Approach

### 6.1 Feature A: Open and Render Markdown

Behavior:
- Load file from CLI path.
- Parse structure and index headings/links.
- Render styled source lines in viewport.
- Display status: file name, line/total, mode, hints.

Edge handling:
- Missing file, unreadable file, invalid UTF-8.
- Empty file.
- Huge file (graceful startup with progress status if needed).

Tests:
- Unit: file load success/failure mapping to error types.
- Integration: opening fixture docs in CLI and model initialization.
- Manual: open sample doc and verify basic scrolling + status updates.

### 6.2 Feature B: Heading Navigation (`n` / `p`)

Behavior:
- `n`: jump cursor/viewport to next heading after current cursor line.
- `p`: jump to previous heading before current cursor line.
- If no target heading, keep position and show non-intrusive status.

Algorithm:
- Maintain heading line numbers sorted ascending.
- Use binary search for O(log n) next/previous lookup.
- Jump sets cursor to heading line and recenters viewport.

Edge handling:
- No headings.
- Cursor before first heading / after last heading.
- Consecutive headings with no body text.

Tests:
- Unit: boundary and binary-search correctness.
- Integration: sequence of `n`/`p` in nested heading fixtures.
- Manual: verify jump accuracy and status messages.

### 6.3 Feature C: Outline Modal with Live Preview

Behavior:
- Open modal from normal mode.
- List headings with indentation and `#` prefixes.
- Pre-select heading containing current cursor.
- On move up/down in modal, background doc live-jumps to preview heading.
- `Enter` confirms; `Esc` cancels and restores origin line.

Data flow:
- Build `OutlineEntry` list from heading index.
- Keep `origin_cursor_line` when modal opens.
- On selection change, update `preview_cursor_line` and viewport.

Edge handling:
- No headings: modal shows "No headings".
- Filter reduces list to zero results.
- Very deep heading levels (clamp indentation visual width).

Tests:
- Unit: current-heading preselection and filter results.
- Integration: preview vs confirm vs cancel restore semantics.
- Snapshot: modal layout at small and large terminal sizes.
- Manual: verify real-time background jumping while browsing list.

### 6.4 Feature D: Slash Search

Behavior:
- `/` opens search input prompt.
- Typing updates query state.
- `Enter` computes matches and jumps to first.
- `Ctrl+n` / `Ctrl+p` navigate next/previous match.
- Current match highlighted in document.

Search semantics:
- Default literal substring, case-insensitive by default (configurable).
- Match list stores line + column ranges.
- Reuse previous query when reopening `/`.

Performance:
- Small docs: synchronous search.
- Large docs: background task with interim "searching..." status.

Edge handling:
- Empty query.
- No matches.
- Unicode search boundaries.

Tests:
- Unit: matching behavior and case rules.
- Unit: wrap-around next/previous match navigation.
- Integration: search prompt lifecycle and highlight state.
- Manual: verify behavior on large file and no-match case.

### 6.5 Feature E: Link Following

Behavior:
- `Tab` / `Shift+Tab` cycle visible links.
- Focused link is high-contrast highlighted.
- `Enter` follows focused link.
- Local markdown links open target doc and push current doc to stack.
- Anchor-only links jump within current doc.
- `b` returns to previous doc/position.

Resolution rules:
- Resolve relative paths from current document directory.
- Support `file.md#anchor` and `#anchor`.
- Use slug map for anchor -> heading line resolution.
- External links shown as unsupported in v1 (status message) or optional shell open behind feature flag.

Edge handling:
- Broken links.
- Missing anchor in existing file.
- Cyclic docs (A links B links A) with stack navigation.

Tests:
- Unit: path resolution and anchor slug lookup.
- Integration: multi-file fixture traversal and back stack restoration.
- Manual: broken links, duplicate anchors, anchor-only links.

### 6.6 Feature F: Filterable Shortcuts Dialog (`?`)

Behavior:
- `?` opens help overlay containing all shortcuts.
- Search/filter input narrows list by key or description.
- Clear labels show key, scope, and behavior.

Accessibility:
- Always keyboard navigable.
- Active row clearly highlighted with non-color cue.
- Works in narrow terminal widths with truncation rules.

Tests:
- Unit: shortcut filter matching.
- Snapshot: help dialog rendering across widths.
- Manual: confirm discoverability and correctness against actual keymap.

### 6.7 Feature G: Accessibility Baseline

Requirements:
- Every interactive element has a focus state.
- Focus is not communicated by color alone.
- Status messages are text-explicit (for terminal/screen reader workflows).
- All features usable without mouse.
- Reduced-motion default for modal transitions.

Tests:
- Unit/integration: focus movement and mode transitions.
- Snapshot: high-contrast theme readability.
- Manual: keyboard-only end-to-end scenarios.

## 7) Robustness and Error Handling

### 7.1 Error Taxonomy

`AppError` categories:
- `FileReadError`
- `ParseError`
- `PathResolutionError`
- `LinkFollowError`
- `AnchorNotFound`
- `SearchTaskError`
- `TerminalError`

### 7.2 Error Policy

- Fatal startup errors: return clear stderr + exit code 1.
- Runtime recoverable errors: keep app alive, show status message, log details.
- Never panic for malformed markdown or malformed links.
- Use saturating and bounds-checked math for viewport indices.

### 7.3 Defensive Guards

- Clamp all list/modal selections.
- Validate line/column ranges before render.
- Validate doc stack transitions before pop/push.
- Timebox long operations and surface progress state.

## 8) Performance Plan

### 8.1 Target Budgets (v1)

- Open and index 1 MB markdown file: under 150 ms on typical dev hardware.
- `n`/`p` heading jump: under 5 ms.
- Outline open on 5k headings: under 30 ms.
- Search start on 50k lines: responsive UI (async path).
- Steady-state render path: one diffed frame per input event with no full redraw artifacts.

### 8.2 Optimization Techniques

- Parse once per document open.
- Store heading lines in sorted vector for binary search.
- Cache wrapped/processed lines keyed by terminal width.
- Compute visible link focus order from viewport slice, not full doc each frame.
- Avoid allocations in hot path by reusing buffers.

### 8.3 Scaling and Degradation

- For very large docs, enable incremental/background search.
- If parser metadata fails partially, fallback to plain line viewer with reduced navigation features and explicit warning.

## 9) Testing Strategy

### 9.1 Automated Tests

Test layers:
- Unit tests for parser/index/navigation/search/path resolution.
- Integration tests for event-action-state flows.
- Snapshot tests for modal and status rendering.
- Property tests for index/selection invariants (optional but recommended).

Minimum CI gates per merged milestone:
- `cargo fmt --check`
- `cargo test`

Recommended additional gate:
- `cargo clippy --all-targets -- -D warnings`

### 9.2 Manual QA Matrix

Run per milestone using fixture docs:
- `fixtures/docs/simple.md`
- `fixtures/docs/nested_headings.md`
- `fixtures/docs/links_internal.md`
- `fixtures/docs/duplicate_anchors.md`
- `fixtures/docs/unicode.md`
- `fixtures/docs/large.md`

Manual checks:
- Open app and verify layout integrity at 80x24 and wide terminal.
- Heading jumps (`n`/`p`) produce expected section transitions.
- Outline preselect matches current section.
- Outline live preview updates background immediately.
- Search prompt opens and returns expected matches.
- Link focus/follow/back cycle works without mode confusion.
- `?` help filter returns correct shortcuts.
- All flows work keyboard-only.

### 9.3 Test-As-You-Build Rule

For each feature increment:
1. Implement smallest useful slice.
2. Run app manually.
3. Execute focused tests.
4. Execute full `cargo test` + `cargo fmt`.
5. Record what was verified before moving on.

## 10) Milestones and Execution Phases

### Phase 0: Bootstrap + Core Skeleton

Deliverables:
- Cargo binary scaffold.
- Minimal `AppState`, event loop, basic viewport draw.
- CLI arg parsing for input file path.

Acceptance criteria:
- App opens a file and displays raw lines.
- Scroll up/down works.

Testing:
- Unit: CLI/file loading errors.
- Manual: launch + scroll + quit.
- Gate: fmt + tests pass.

### Phase 1: Markdown Structural Index

Deliverables:
- Heading/link parser + index.
- Section range builder.
- Anchor slug map.

Acceptance criteria:
- Headings and links extracted correctly from fixtures.

Testing:
- Unit-heavy parser tests.
- Integration with fixture set.
- Gate: fmt + tests pass.

### Phase 2: Heading Navigation (`n` / `p`)

Deliverables:
- Binary-search heading navigation.
- Status messages for boundary/no-heading conditions.

Acceptance criteria:
- `n` and `p` deterministic across nested heading docs.

Testing:
- Unit + integration navigation tests.
- Manual jump validation.
- Gate: fmt + tests pass.

### Phase 3: Outline Modal + Live Preview

Deliverables:
- Outline overlay with indentation + `#` hierarchy marker.
- Preselect current heading.
- Preview on selection move.
- Confirm/cancel semantics.

Acceptance criteria:
- Modal behavior matches spec, including cancel restore.

Testing:
- Integration flow tests.
- Snapshot tests for modal layout.
- Manual live-preview QA.
- Gate: fmt + tests pass.

### Phase 4: Slash Search

Deliverables:
- Search prompt and match highlighting.
- Next/prev match navigation.
- Async path for large files.

Acceptance criteria:
- Search remains responsive and correct on large fixtures.

Testing:
- Unit search engine tests.
- Integration event flow tests.
- Manual no-match/large-doc checks.
- Gate: fmt + tests pass.

### Phase 5: Link Focus + Follow + Back Stack

Deliverables:
- Tab-based link focus.
- Link following for local markdown and anchors.
- Document stack and return navigation.

Acceptance criteria:
- Internal docs and anchors are traversable without losing position history.

Testing:
- Unit path/anchor resolution tests.
- Integration multi-doc flow tests.
- Manual broken-link and back-stack tests.
- Gate: fmt + tests pass.

### Phase 6: Shortcuts Dialog + Accessibility Pass

Deliverables:
- `?` modal with filterable shortcut list.
- High-contrast, non-color-only focus indicators.
- Final keymap consistency audit.

Acceptance criteria:
- New user can discover and use key features from in-app help only.

Testing:
- Unit filter tests.
- Snapshot layout tests.
- Manual keyboard-only accessibility checklist.
- Gate: fmt + tests pass.

### Phase 7: Hardening + Release Prep

Deliverables:
- Error-path polish.
- Perf profiling and regression checks.
- Documentation (`README`, keybindings, troubleshooting).

Acceptance criteria:
- Stable behavior across fixtures and terminal sizes.
- Known edge cases covered by tests.

Testing:
- Full automated suite.
- Full manual QA matrix.
- Gate: fmt + tests pass.

## 11) Risk Register

- Keybinding collisions (`n` heading vs Vim search conventions).
- Anchor slug compatibility with common markdown ecosystems.
- Large-file search blocking UI if async threshold is mis-tuned.
- Unicode width/render edge cases in line/column highlight math.
- Relative link resolution inconsistencies across symlinks and `..` paths.

Mitigations:
- Central keymap with tests.
- Deterministic slugifier + duplicate handling tests.
- Async search fallback and explicit status indicator.
- Unicode fixtures and width-safe cursor calculations.
- Canonical path normalization with explicit error states.

## 12) Open Questions for Review Cycle 1

- Should external `http(s)` links be opened via OS command in v1 or deferred?
- Should search default to case-insensitive or preserve Vim case behavior (`smartcase`) in v1?
- Should inline screen mode be supported immediately as a CLI flag?
- Should we support regex search in v1 or defer to v2?
- Should heading navigation wrap around at document ends or stop with status message?

## 13) Definition of Done (v1)

`mdmd` v1 is done when:
- All core features in this plan are implemented.
- All milestone acceptance criteria are met.
- Automated tests cover parser/navigation/search/link/help behavior.
- Manual QA matrix passes on representative docs and terminal sizes.
- `cargo fmt --check` and `cargo test` pass cleanly.
- User can run app, discover shortcuts in `?`, and complete the core navigation workflows keyboard-only.

## 14) Beads-Ready Task Buckets (for next step)

Epics for conversion:
- E1: App scaffold + CLI + core model loop.
- E2: Markdown indexing (headings, sections, links, anchors).
- E3: Heading navigation + status messaging.
- E4: Outline modal + live preview + filter.
- E5: Slash search + async large-doc handling.
- E6: Link focus/follow/back + path/anchor resolution.
- E7: Help modal + keymap docs + accessibility pass.
- E8: Test harness + fixtures + CI gates + release docs.

Dependency spine:
- E1 -> E2 -> E3 -> E4
- E2 -> E5
- E2 -> E6
- E1 -> E7
- E3 + E4 + E5 + E6 + E7 -> E8

This dependency graph keeps high-risk parsing/navigation primitives early and pushes polish/testing hardening to the final integration milestone.
