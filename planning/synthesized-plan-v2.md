# mdmd v2 Synthesized Plan

## 0) Planning Contract

This plan is design-first and milestone-driven.

- Every phase must be independently runnable.
- Every phase includes explicit automated tests and manual QA checks.
- No phase is complete without `cargo fmt --check`, `cargo test`, and brief QA notes.
- No phase is complete without benchmark delta notes for touched hot paths.
- Default development mode is alt-screen pager behavior with predictable keyboard-only flows.
- Every async workflow must support cancellation and stale-result rejection.

## 1) Product Definition

### 1.1 Vision

`mdmd` is an accessibility-first terminal markdown pager built with Rust + frankentui. It should feel fast for structured reading, not just line-by-line scrolling.

Primary outcomes:
- Open markdown files quickly.
- Jump across headings quickly (`n` / `p`).
- Use an outline modal to scan structure and jump (`o`).
- Search text quickly (`/`) and repeat matches.
- Follow links across markdown files and anchors.
- Discover shortcuts in-app via a filterable help dialog (`?`).
- Recover context quickly across files and sessions.
- Stay safe by default when handling external links.

### 1.2 User Goals

- "I can read markdown as a structured document in the terminal."
- "I can move by section, not only by raw lines."
- "I can open outline/search/help without leaving home-row workflows."
- "I can traverse linked docs and return to where I came from."
- "I can complete all workflows without a mouse."
- "I can reopen docs and resume where I left off."
- "I can trust link behavior to be explicit and non-surprising."

### 1.3 Non-Goals (v2)

- Markdown editing.
- Full WYSIWYG semantic layout parity with GUI editors.
- Network crawling of links inside the app.
- Multi-pane comparison.
- Plugin architecture.
- Cloud sync, user accounts, or telemetry backends.

## 2) Architecture Decisions and Trade-offs

### 2.1 Runtime Model

Use frankentui Elm loop with a thin application layer:

`Event -> dispatcher -> Intent -> reducer/update -> Cmd(effect) -> Msg(result) -> reducer -> view`

Decision:
- Single top-level app model with explicit sub-state modules.
- Strict layering:
  - `domain`: pure document/index/navigation logic
  - `application`: reducers + command scheduling + cancellation
  - `ui`: rendering + input mapping only

Trade-off:
- Pro: deterministic behavior and testability.
- Con: state can bloat.
- Mitigation: strict module boundaries and action ownership.
- Con: command plumbing overhead.
- Mitigation: typed `RequestId`/`Generation` tokens and shared helper abstractions for async effects.

### 2.2 Screen Mode

Decision:
- Default to `AltScreen` for pager UX and stable overlays.
- Provide `--inline` mode in v2 for users who prefer preserving terminal scrollback.

Trade-off:
- Inline mode preserves running scrollback.
- Alt-screen remains default due to better modal and resize behavior.

### 2.3 Rendering Strategy

Decision:
- Use source-faithful rendering as canonical data, with a cached display projection.

Why:
- Keeps heading/link/search offsets stable.
- Reduces mismatch between navigation index and rendered surface.
- Enables line-wrap/layout caching without losing source mapping.

Trade-off:
- Less visually rich than fully reflowed markdown.
- Additional cache invalidation complexity for resize/theme changes.

Implementation note:
- Source mapping remains canonical for navigation/search/link focus.
- Display projection cache key: `(doc_id, viewport_width, theme_id, wrap_mode)`.

### 2.4 Parsing and Indexing

Decision:
- Parse with `pulldown-cmark` and build a structural index (headings, links, anchors, sections, line offsets).
- Use two passes:
  1. token/event collection with source spans
  2. semantic index construction + duplicate-anchor normalization

Why:
- Correct heading extraction (including code-fence safety).
- Robust anchor and section navigation.
- Makes duplicate-anchor behavior explicit and testable.

### 2.5 Search Execution Strategy

Decision:
- Use one search pipeline with dual execution strategies:
  - inline execution for tiny docs (`<= 2_500` lines)
  - background worker for larger docs
- Always assign `search_generation`; stale completions are discarded.

Initial threshold guideline:
- `> 2_500` lines or `> 256 KiB` triggers background search path.
- Thresholds are tuneable via config for benchmarking.

### 2.6 External Link Policy

Decision:
- v2 supports local markdown and same-doc anchors fully.
- External `http(s)` links are controlled by explicit policy:
1. `deny` (default): show status message only.
2. `confirm`: require one-shot user confirmation.
3. `open`: open directly through platform opener.

Security constraints:
- Never invoke a shell for URL opening.
- Sanitize and validate scheme before open.
- Log policy decisions in debug diagnostics.

### 2.7 Configuration and Overrides

Decision:
- Provide a typed config model with clear precedence:
  1. CLI flags
  2. env vars
  3. config file (`$XDG_CONFIG_HOME/mdmd/config.toml`)
  4. defaults

Scope:
- keymap overrides
- theme choice
- search defaults (case mode, regex enablement)
- external link policy
- inline/alt-screen mode

### 2.8 Observability and Diagnostics

Decision:
- Add optional structured debug log and event trace replay for non-interactive debugging.

Why:
- Terminal UI bugs are difficult to reproduce from screenshots alone.
- Event replay makes regressions deterministic.

## 3) Proposed Project Structure

```text
mdmd/
  Cargo.toml
  src/
    main.rs
    app.rs
    cli.rs
    config.rs
    constants.rs
    command.rs
    diagnostics.rs
    error.rs
    model/
      mod.rs
      app_state.rs
      modes.rs
      mode_stack.rs
      document_state.rs
      viewport_state.rs
      outline_state.rs
      search_state.rs
      link_state.rs
      help_state.rs
      bookmark_state.rs
      session_state.rs
      status_state.rs
    domain/
      mod.rs
      document.rs
      document_id.rs
      heading.rs
      link.rs
      anchor.rs
      section.rs
      bookmark.rs
      shortcuts.rs
    parser/
      mod.rs
      markdown_index.rs
      slugify.rs
      line_map.rs
      duplicate_anchor.rs
    render/
      mod.rs
      display_projection.rs
      wrap_cache.rs
    nav/
      mod.rs
      heading_nav.rs
      section_nav.rs
      link_nav.rs
      history_nav.rs
      bookmark_nav.rs
    input/
      mod.rs
      action.rs
      intent.rs
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
        help_bar.rs
        outline_modal.rs
        search_prompt.rs
        bookmark_palette.rs
        help_modal.rs
        link_focus.rs
    infra/
      mod.rs
      fs.rs
      opener.rs
      config_loader.rs
      clock.rs
      worker.rs
    bench/
      mod.rs
      open_index_bench.rs
      search_bench.rs
  tests/
    parser_headings.rs
    parser_links.rs
    parser_sections.rs
    parser_duplicate_anchors.rs
    nav_headings.rs
    nav_outline.rs
    nav_bookmarks.rs
    search_behavior.rs
    search_cancellation.rs
    link_following.rs
    link_policy.rs
    help_filtering.rs
    config_precedence.rs
    session_restore.rs
    app_event_flow.rs
    event_replay.rs
  fixtures/
    docs/
      simple.md
      nested_headings.md
      links_internal.md
      links_external.md
      duplicate_anchors.md
      unicode.md
      code_fences.md
      huge_wrapped_lines.md
      large.md
      massive.md
```

## 4) Core Data Model and State

### 4.1 Domain Model

```rust
struct Document {
    id: DocumentId,
    path: Option<PathBuf>,
    source_kind: SourceKind,
    source: Arc<str>,
    checksum: u64,
    lines: Arc<[LineRecord]>,
    display_cache: DisplayCache,
    headings: Arc<[Heading]>,
    links: Arc<[LinkRef]>,
    sections: Arc<[Section]>,
    anchors: AnchorIndex,
}

enum SourceKind {
    File { canonical_path: PathBuf },
    Stdin,
    Fixture,
}

struct Heading {
    id: HeadingId,
    level: u8,
    title: String,
    line_number: usize,
    start_byte: usize,
    end_byte: usize,
    slug: String,
    parent: Option<HeadingId>,
}

struct LinkRef {
    id: LinkId,
    label: String,
    destination_raw: String,
    resolved: ResolvedLink,
    line_number: usize,
    start_col: usize,
    end_col: usize,
}

enum ResolvedLink {
    SameDocAnchor { slug: String },
    LocalMarkdown { path: PathBuf, anchor: Option<String> },
    ExternalUrl { url: String },
    Unsupported { reason: String },
}

struct DisplayLine {
    source_line: usize,
    wrap_segment: u16,
    plain: Arc<str>,
    spans: Arc<[StyledSpan]>,
}

struct SearchJob {
    generation: u64,
    query: String,
    case_mode: CaseMode,
    use_regex: bool,
}
```

### 4.2 App State

```rust
enum AppMode {
    Normal,
    Outline,
    SearchInput,
    Help,
    BookmarkPalette,
    ConfirmExternalOpen,
}

struct AppState {
    mode: AppMode,
    mode_stack: Vec<AppMode>,
    doc_stack: Vec<DocumentSession>,
    viewport: ViewportState,
    outline: OutlineState,
    search: SearchState,
    links: LinkState,
    help: HelpState,
    bookmarks: BookmarkState,
    session: SessionState,
    config: ConfigState,
    diagnostics: DiagnosticsState,
    status: StatusState,
    pending: PendingOps,
}
```

Key sub-state notes:
- `DocumentSession` stores current `Document` plus return cursor/scroll context.
- `OutlineState` stores open/query/filtered ids/selected index/origin position/preview position.
- `SearchState` stores query, match list, current match, case mode, `generation`, and cancellation handles.
- `LinkState` stores focused link id, focus order in viewport, and follow errors.
- `HelpState` stores filter text and filtered shortcut rows.
- `BookmarkState` stores named marks (single-char key -> location) and recent jumps.
- `SessionState` stores recent files and last known cursor/viewport for resumable opens.
- `PendingOps` stores in-flight command metadata keyed by request id.

### 4.3 State Invariants

- Cursor and top-line always remain in bounds.
- Outline selection always references filtered entries.
- Search match indices always reference valid line/column ranges.
- Link focus id must exist in current link focus order.
- Modal close always returns to deterministic prior mode from `mode_stack`.
- Navigation stack operations are bounds-checked and non-panicking.
- Stale async results (`generation` mismatch) are ignored and never mutate visible state.
- Only one active search task per document generation.
- Bookmark targets always reference existing document/session ids.

## 5) UI Hierarchy, Event Pipeline, and Keymap

### 5.1 Main Layout

Main screen:
- Top status bar: file name, current heading, line position, mode/status, transient severity.
- Document viewport.
- Bottom help bar: concise shortcuts (`n`, `p`, `o`, `/`, `m`, `?`, `q`).

Overlay layer (conditional):
- Outline modal.
- Search prompt (command-line style, bottom overlay).
- Help modal.
- Bookmark palette.
- External-link confirmation dialog (if policy is `confirm`).

### 5.2 Event Pipeline

```text
ftui Event
  -> input::dispatcher (mode-aware)
  -> Intent enum
  -> reducer mutates state + emits Cmd
  -> worker executes Cmd (optionally async)
  -> Msg::Result(request_id, payload)
  -> reducer validates generation/request_id
  -> view re-render
```

### 5.3 Intent Groups

Representative intents:
- Global: `Quit`, `Resize`
- Viewport: `ScrollUp`, `ScrollDown`, `PageUp`, `PageDown`, `GoTop`, `GoBottom`
- Heading nav: `NextHeading`, `PrevHeading`
- Outline: `OpenOutline`, `OutlineFilterInput`, `OutlineMoveUp`, `OutlineMoveDown`, `OutlineConfirm`, `OutlineCancel`
- Search: `OpenSearch`, `SearchInputChar`, `SearchBackspace`, `SearchSubmit`, `SearchCancel`, `SearchNext`, `SearchPrev`, `ToggleRegex`, `ToggleCaseMode`
- Links: `FocusNextLink`, `FocusPrevLink`, `FollowFocusedLink`, `NavigateBack`, `NavigateForward`, `ClearLinkFocus`, `ConfirmExternalOpen`, `CancelExternalOpen`
- Bookmarks: `SetBookmark`, `OpenBookmarkPalette`, `JumpBookmark`
- Document/session: `ReloadDocument`, `OpenRecent`, `ResumeLastPosition`
- Help: `OpenHelp`, `HelpFilterInput`, `HelpMoveUp`, `HelpMoveDown`, `HelpClose`

### 5.4 Keymap (v2)

Normal mode:
- `q`: quit
- `j` / `Down`: scroll down
- `k` / `Up`: scroll up
- `PageDown` / `Space` / `Ctrl+f`: page down
- `PageUp` / `Ctrl+b`: page up
- `Ctrl+d`: half-page down
- `Ctrl+u`: half-page up
- `g`: top
- `G`: bottom
- `n`: next heading
- `p`: previous heading
- `o`: open outline
- `/`: open search prompt
- `m`: set bookmark at cursor
- `'`: open bookmark palette
- `R`: reload current document
- `Tab`: focus next visible link
- `Shift+Tab`: focus previous visible link
- `Enter`: follow focused link (if focused)
- `Esc`: clear link focus
- `b` or `Ctrl+o`: navigate back
- `Ctrl+i`: navigate forward
- `?`: open help

Outline mode:
- text input: filter headings
- `j`/`k` or arrows: move selection with live preview
- `Enter`: confirm jump
- `Esc`: cancel and restore origin

Search prompt:
- text input + `Backspace`
- `Enter`: run search and jump to first match
- `Esc`: cancel prompt
- `Alt+r`: toggle regex mode
- `Alt+c`: toggle case mode
- result navigation after search: `Ctrl+n` next, `Ctrl+p` previous

Help mode:
- text input filter
- `j`/`k` or arrows: move selected row
- `Esc` or `?`: close

Bookmark palette:
- `j`/`k` or arrows: move entries
- text input: filter by label/path
- `Enter`: jump to bookmark
- `Esc`: close

## 6) Feature Implementation Plan

### 6.1 Feature A: Open and Render Markdown

Behavior:
- Load file from CLI argument or stdin.
- Build structural index and display projection cache.
- Show status with file name/source, current heading, and position.
- Restore last cursor/viewport if session data exists and is valid.

Edge handling:
- Missing/unreadable file, invalid UTF-8, empty docs, huge docs, stdin without TTY.
- Source reload (`R`) handles file changed/deleted races.

Acceptance checks:
- Can open all fixture docs and scroll reliably in alt-screen and inline modes.
- Status updates correctly with movement and source type.

Tests:
- Unit: file load and error mapping.
- Integration: initialization from fixture docs.
- Integration: session resume behavior.
- Manual: open, scroll, resize, reload, quit.

### 6.2 Feature B: Heading Navigation (`n` / `p`)

Behavior:
- `n` jumps to first heading after cursor.
- `p` jumps to last heading before cursor.
- No target -> keep position and show unobtrusive status.

Algorithm:
- Keep heading line numbers sorted.
- Use binary search for O(log n) next/prev lookup.
- Recenter viewport around destination line.

Edge handling:
- No headings.
- Boundaries (before first/after last).
- Consecutive headings.

Tests:
- Unit: boundary + binary-search correctness.
- Integration: nested headings navigation sequence.
- Manual: verify line-accurate jumps.
- Property: heading nav never escapes bounds for arbitrary heading distributions.

### 6.3 Feature C: Outline Modal with Live Preview

Behavior:
- `o` opens outline overlay.
- Pre-select heading that contains current cursor line.
- Moving selection previews heading in background viewport.
- `Enter` confirms jump; `Esc` cancels and restores origin.

Display:
- Hierarchy shown with indentation and heading markers.
- Filterable list via text query.
- Sticky query state while modal remains open.

Edge handling:
- No headings.
- Filter-to-zero results.
- Deep heading nesting (indent clamping).
- Terminal resize while modal open.

Tests:
- Unit: preselection and filtering.
- Integration: preview/confirm/cancel semantics.
- Snapshot: modal layout at narrow/wide terminals.
- Snapshot: resize during preview.

### 6.4 Feature D: Slash Search

Behavior:
- `/` opens search prompt.
- Typing can trigger debounced preview search.
- `Enter` computes/commits matches and jumps to first.
- Current and non-current matches are highlighted distinctly.
- `Ctrl+n`/`Ctrl+p` navigate matches.
- Reuse previous query on reopen.
- `Alt+r` toggles regex mode; `Alt+c` toggles case mode.

Search semantics:
- Default: literal substring, case-insensitive.
- Optional: smartcase and regex (configurable; off by default).

Performance:
- Unified search engine with inline fast path and worker path.
- Every run increments `search_generation`; stale results are dropped.
- Long-running searches expose "searching..." status with cancellation.

Edge handling:
- Empty query.
- No matches.
- Unicode match boundaries.
- Invalid regex (recover with status message, no crash).

Tests:
- Unit: matching rules + wrap navigation.
- Integration: prompt lifecycle and highlight state.
- Manual: large-doc responsiveness and no-match behavior.
- Integration: cancellation/stale-result rejection.

### 6.5 Feature E: Link Focus, Follow, and History

Behavior:
- `Tab` / `Shift+Tab` cycles visible links.
- Focused link gets high-contrast, non-color-only indicator.
- `Enter` follows focused link.
- `#anchor`: jump in current doc.
- `file.md#anchor`: load target file and anchor.
- `b`/`Ctrl+o`: back to previous doc + position.
- `Ctrl+i`: forward when available.
- External URL handling follows policy (`deny`/`confirm`/`open`).

Resolution rules:
- Resolve relative paths from current doc directory.
- Canonicalize paths where safe.
- Map anchor slugs deterministically.
- Reject unsupported/unsafe schemes explicitly.

Edge handling:
- Broken link.
- Missing anchor.
- Cyclic document references.
- Link target deleted between parse and follow.

Tests:
- Unit: path resolution + slug lookup.
- Integration: multi-doc navigation stack restoration.
- Manual: broken links and repeated traversals.
- Integration: external-link policy transitions.

### 6.6 Feature F: Filterable Help Dialog (`?`)

Behavior:
- `?` opens shortcuts dialog.
- Filter by key or description.
- Present key, context/mode, and action.
- Include effective key overrides from user config.

Accessibility:
- Keyboard-only operation.
- Strong focus indicator not relying on color alone.
- Works at narrow widths with truncation rules.

Tests:
- Unit: filter matching.
- Snapshot: narrow and wide layouts.
- Manual: verify all documented shortcuts are functional.
- Integration: help rows update after config override reload.

### 6.7 Feature G: Bookmarks and Session Resume

Behavior:
- `m` stores current location as a bookmark.
- `'` opens bookmark palette and jumps to selected bookmark.
- Session file stores recent docs and last-known positions.
- On startup/open, session restore is offered automatically (configurable).

Edge handling:
- Bookmark collisions (overwrite confirmation).
- Stale bookmark targets after file changes.
- Corrupted session file (recover with defaults).

Tests:
- Unit: bookmark insert/update/delete.
- Integration: session restore across restarts.
- Manual: jump correctness after heavy navigation.

### 6.8 Feature H: Accessibility and Interaction Baseline

Requirements:
- All interactions available without mouse.
- Focus state always visible and explicit.
- Status messages are text-clear and non-ambiguous.
- Modal transitions avoid motion-heavy effects.
- Critical states are not color-only (symbols/text accompany color).

Verification:
- Keyboard-only end-to-end scenarios.
- High-contrast readability checks.
- 80x24, 100x30, and wide-terminal interaction passes.

## 7) Error Handling and Robustness

### 7.1 Error Taxonomy

`AppError` categories:
- `FileReadError`
- `ConfigError`
- `ParseError`
- `PathResolutionError`
- `LinkFollowError`
- `AnchorNotFound`
- `SearchTaskError`
- `WorkerCancelled`
- `SessionStoreError`
- `TerminalError`

### 7.2 Error Policy

- Startup-fatal errors: clear stderr + non-zero exit.
- Runtime recoverable errors: keep app alive, show status message.
- Never panic on malformed markdown or malformed links.
- Use bounds-checked, saturating index math in viewport/navigation.
- Attach severity (`info`/`warn`/`error`) to status messages.
- Store last recoverable error in diagnostics panel/log for debugging.

### 7.3 Defensive Guards

- Clamp modal/list indices.
- Validate all render ranges.
- Guard history stack push/pop transitions.
- Timebox/async long operations and report progress.
- Reject stale async completions via request id + generation checks.
- Fail closed on external link policy ambiguity.

## 8) Performance Plan

### 8.1 v2 Budgets

- Open + index 1 MB markdown file: p50 < 120 ms, p95 < 180 ms.
- Open + index 10 MB markdown file: p95 < 1.2 s.
- Heading jump: p95 < 5 ms.
- Outline open with ~5k headings: p95 < 30 ms.
- Search first result on 10 MB fixture: p95 < 400 ms with responsive UI.
- Steady-state memory for 10 MB file: < 180 MB RSS target.

### 8.2 Optimization Techniques

- Parse once per open.
- Use sorted heading vectors + binary search.
- Cache width-dependent display projection artifacts.
- Restrict link focus calculation to viewport slice.
- Reuse buffers in hot paths.
- Debounce search input and cancel superseded worker tasks.
- Use incremental repaint regions when only status/overlay changes.

### 8.3 Graceful Degradation

- If full metadata parse degrades, fallback to plain line viewer with explicit reduced-feature status.
- Preserve scrolling and basic file visibility even when advanced navigation metadata is partial.
- If search worker is unavailable, fallback to inline search and report reduced mode.

## 9) Testing Strategy

### 9.1 Automated Tests

Test layers:
- Unit: parser/index/nav/search/pathing.
- Integration: event -> intent -> state/cmd/message flows.
- Snapshot: overlays, status/help bars, link focus, search highlight.
- Property tests: navigation and index invariants.
- Fuzz tests: markdown parsing + link/path resolution.

Minimum CI gates per phase:
- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`

Recommended gates:
- `cargo test --release -- --ignored` (for large fixtures/perf-sensitive tests)
- benchmark smoke run with threshold assertions
- Linux and macOS CI matrix for key-event parity

### 9.2 Manual QA Matrix

Run per phase against:
- `fixtures/docs/simple.md`
- `fixtures/docs/nested_headings.md`
- `fixtures/docs/links_internal.md`
- `fixtures/docs/duplicate_anchors.md`
- `fixtures/docs/unicode.md`
- `fixtures/docs/code_fences.md`
- `fixtures/docs/huge_wrapped_lines.md`
- `fixtures/docs/large.md`
- `fixtures/docs/massive.md`

Manual checks:
- 80x24, 100x30, and wide terminals.
- Alt-screen and inline mode parity.
- Heading navigation determinism.
- Outline preselect + preview + cancel restore.
- Search lifecycle and match movement.
- Link follow/back/forward behavior.
- External-link policy (`deny`/`confirm`/`open`) behavior.
- Help discoverability and filter correctness.
- Bookmark create/jump/overwrite flows.
- Session restore correctness.
- Keyboard-only completion of core workflows.

### 9.3 Test-As-You-Build Rule

For each increment:
1. Implement smallest complete slice.
2. Run app manually for that slice.
3. Run focused tests.
4. Run full `cargo test` and `cargo fmt --check`.
5. Capture short QA notes.
6. Capture perf notes if a hot path changed.

## 10) Milestones and Execution Phases

### Phase 0: Bootstrap + Command Foundation

Deliverables:
- Binary scaffold, CLI parser, config loader, base app loop, command/request-id foundation.

Acceptance:
- Open file and basic scroll works; config precedence is test-covered.

### Phase 1: Markdown Structural Index

Deliverables:
- Heading/link/section/anchor index from parser with duplicate-anchor normalization.

Acceptance:
- Fixture extraction correctness.

### Phase 2: Rendering + Viewport + Heading Navigation

Deliverables:
- Display projection cache, viewport scrolling, `n`/`p` navigation with boundary statuses.

Acceptance:
- Deterministic behavior across nested fixtures and resize events.

### Phase 3: Outline Modal + Live Preview

Deliverables:
- Filterable outline modal with preview and cancel-restore semantics.

Acceptance:
- Matches expected modal interaction behavior.

### Phase 4: Slash Search

Deliverables:
- Prompt, highlighting, match traversal, cancellation-safe worker path.

Acceptance:
- Correct + responsive search on small and large fixtures.

### Phase 5: Link Focus + Follow + History

Deliverables:
- Link focus cycling, follow local links/anchors, back/forward stack, external-link policy enforcement.

Acceptance:
- Multi-doc traversal without losing context.

### Phase 6: Help + Bookmarks + Session Resume

Deliverables:
- Filterable shortcuts modal, bookmark palette, session persistence.

Acceptance:
- New user can discover and perform all key workflows from in-app help only; sessions restore reliably.

### Phase 7: Accessibility + Hardening

Deliverables:
- Accessibility hardening, error polish, perf tuning, docs, and final QA.

Acceptance:
- Stable behavior across fixtures and terminal sizes.

### Phase 8: Release Prep

Deliverables:
- Final benchmarks, changelog, packaged release artifacts, rollback notes.

Acceptance:
- Release checklist complete and all quality/perf gates pass.

## 11) Risk Register

- Anchor slug compatibility and duplicate slug handling.
- Large-doc search causing interaction stalls if cancellation/debounce tuning is poor.
- Unicode width and highlight column math inconsistencies.
- Path normalization edge cases (`..`, symlinks, mixed separators).
- Async result races causing stale UI state writes.
- Cross-platform key event differences (`Ctrl+i`/`Tab`, modifier encoding).
- Session persistence corruption or incompatible schema drift.

Mitigations:
- Deterministic slugifier + duplicate tests.
- Async search with generation-based stale result rejection + visible progress state.
- Unicode fixtures + width-aware calculations.
- Canonicalization and explicit error messages for unresolved paths.
- Dedicated key handling tests per mode and platform CI matrix.
- Versioned session schema with migration and fallback-to-default on parse failure.

## 12) Open Questions (Review Cycle 2)

Defaults now proposed:
- External URL handling default is `deny`.
- Search defaults are literal + case-insensitive with optional regex/smartcase toggles.
- Heading navigation stops at bounds with explicit status (no wrap by default).
- Inline mode is supported via `--inline` and config.

Remaining open:
- Should session restore be automatic or prompt-first by default?
- Should regex mode stay opt-in forever or be enabled in advanced profile preset?

## 13) Definition of Done (v2)

`mdmd` v2 is complete when:
- Core features in this plan are implemented.
- All phase acceptance criteria pass.
- Automated tests cover parser/navigation/search/link/help/bookmark/session flows.
- Manual QA matrix passes on representative fixtures and terminal sizes.
- `cargo fmt --check`, `cargo test`, and `cargo clippy --all-targets -- -D warnings` pass cleanly.
- Benchmarks meet v2 performance budgets (or have accepted documented exceptions).
- A keyboard-only user can discover shortcuts in `?` and complete core workflows.
- External link behavior is policy-controlled and safe by default.

## 14) Beads-Ready Task Buckets

Epics:
- E1: App scaffold + CLI + config precedence + command foundation.
- E2: Markdown indexing (headings, sections, links, anchors, duplicate handling).
- E3: Rendering cache + viewport + heading navigation.
- E4: Outline modal + filtering + live preview.
- E5: Slash search + cancellation-safe async worker path.
- E6: Link focus/follow/back-forward + policy-driven external opening.
- E7: Help modal + keymap overrides + docs sync.
- E8: Bookmarks + session persistence.
- E9: Accessibility pass + robustness hardening.
- E10: Test harness + fixtures + CI gates + perf + release prep.

Dependency spine:
- `E1 -> E2 -> E3 -> E4`
- `E3 -> E5`
- `E2 + E3 -> E6`
- `E1 + E3 -> E7`
- `E3 -> E8`
- `E4 + E5 + E6 + E7 + E8 -> E9`
- `E5 + E6 + E8 + E9 -> E10`
