# mdmd v1 Synthesized Plan

## 0) Planning Contract

This plan is design-first and milestone-driven.

- Every phase must be independently runnable.
- Every phase includes explicit automated tests and manual QA checks.
- No phase is complete without `cargo fmt --check`, `cargo test`, and brief QA notes.
- Default development mode is alt-screen pager behavior with predictable keyboard-only flows.

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

### 1.2 User Goals

- "I can read markdown as a structured document in the terminal."
- "I can move by section, not only by raw lines."
- "I can open outline/search/help without leaving home-row workflows."
- "I can traverse linked docs and return to where I came from."
- "I can complete all workflows without a mouse."

### 1.3 Non-Goals (v1)

- Markdown editing.
- Full WYSIWYG semantic layout parity with GUI editors.
- Network crawling of links inside the app.
- Multi-pane comparison.
- Plugin architecture.

## 2) Architecture Decisions and Trade-offs

### 2.1 Runtime Model

Use frankentui Elm loop:

`Event -> mode-aware dispatcher -> Action -> reducer/update -> optional Cmd -> view`

Decision:
- Single top-level app model with explicit sub-state modules.

Trade-off:
- Pro: deterministic behavior and testability.
- Con: state can bloat.
- Mitigation: strict module boundaries and action ownership.

### 2.2 Screen Mode

Decision:
- Default to `AltScreen` for pager UX and stable overlays.

Trade-off:
- Inline mode preserves running scrollback.
- v1 defers inline mode to a later optional CLI flag.

### 2.3 Rendering Strategy

Decision:
- Use source-faithful, styled line rendering as the canonical model in v1.

Why:
- Keeps heading/link/search offsets stable.
- Reduces mismatch between navigation index and rendered surface.
- Lowers complexity/risk for v1.

Trade-off:
- Less visually rich than fully reflowed markdown.

Implementation note:
- It is fine to use ftui markdown styling primitives, but source-line mapping remains canonical for navigation/search/link focus.

### 2.4 Parsing and Indexing

Decision:
- Parse with `pulldown-cmark` and build a structural index (headings, links, anchors, sections, line offsets).

Why:
- Correct heading extraction (including code-fence safety).
- Robust anchor and section navigation.

### 2.5 Search Execution Strategy

Decision:
- Synchronous search for small docs.
- Background task search for large docs (threshold-based, tuneable).

Initial threshold guideline:
- `> 15_000` lines or equivalent byte threshold triggers async search path.

### 2.6 External Link Policy

Decision:
- v1 supports local markdown and same-doc anchors fully.
- External `http(s)` links are either:
1. Status-only unsupported (default-safe), or
2. Optional opener behind explicit config/flag.

## 3) Proposed Project Structure

```text
mdmd/
  Cargo.toml
  src/
    main.rs
    app.rs
    cli.rs
    constants.rs
    error.rs
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
      section.rs
      shortcuts.rs
    parser/
      mod.rs
      markdown_index.rs
      slugify.rs
      line_map.rs
    nav/
      mod.rs
      heading_nav.rs
      section_nav.rs
      link_nav.rs
      history_nav.rs
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
        help_bar.rs
        outline_modal.rs
        search_prompt.rs
        help_modal.rs
        link_focus.rs
    infra/
      mod.rs
      fs.rs
      opener.rs
      clock.rs
  tests/
    parser_headings.rs
    parser_links.rs
    parser_sections.rs
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
      code_fences.md
      large.md
```

## 4) Core Data Model and State

### 4.1 Domain Model

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
```

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

Key sub-state notes:
- `DocumentSession` stores current `Document` plus return cursor/scroll context.
- `OutlineState` stores open/query/filtered ids/selected index/origin position/preview position.
- `SearchState` stores query, match list, current match, case mode, and `in_progress` for async path.
- `LinkState` stores focused link id, focus order in viewport, and follow errors.
- `HelpState` stores filter text and filtered shortcut rows.

### 4.3 State Invariants

- Cursor and top-line always remain in bounds.
- Outline selection always references filtered entries.
- Search match indices always reference valid line/column ranges.
- Link focus id must exist in current link focus order.
- Modal close always returns to deterministic prior mode.
- Navigation stack operations are bounds-checked and non-panicking.

## 5) UI Hierarchy, Event Pipeline, and Keymap

### 5.1 Main Layout

Main screen:
- Top status bar: file name, current heading, line position, mode/status.
- Document viewport.
- Bottom help bar: concise shortcuts (`n`, `p`, `o`, `/`, `?`, `q`).

Overlay layer (conditional):
- Outline modal.
- Search prompt (command-line style, bottom overlay).
- Help modal.

### 5.2 Event Pipeline

```text
ftui Event
  -> input::dispatcher (mode-aware)
  -> Action enum
  -> reducer(s) mutate state
  -> optional Cmd::task / Cmd::quit
  -> view re-render
```

### 5.3 Action Groups

Representative actions:
- Global: `Quit`, `Resize`
- Viewport: `ScrollUp`, `ScrollDown`, `PageUp`, `PageDown`, `GoTop`, `GoBottom`
- Heading nav: `NextHeading`, `PrevHeading`
- Outline: `OpenOutline`, `OutlineFilterInput`, `OutlineMoveUp`, `OutlineMoveDown`, `OutlineConfirm`, `OutlineCancel`
- Search: `OpenSearch`, `SearchInputChar`, `SearchBackspace`, `SearchSubmit`, `SearchCancel`, `SearchNext`, `SearchPrev`
- Links: `FocusNextLink`, `FocusPrevLink`, `FollowFocusedLink`, `NavigateBack`, `NavigateForward`, `ClearLinkFocus`
- Help: `OpenHelp`, `HelpFilterInput`, `HelpMoveUp`, `HelpMoveDown`, `HelpClose`

### 5.4 Keymap (v1)

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
- result navigation after search: `Ctrl+n` next, `Ctrl+p` previous

Help mode:
- text input filter
- `j`/`k` or arrows: move selected row
- `Esc` or `?`: close

## 6) Feature Implementation Plan

### 6.1 Feature A: Open and Render Markdown

Behavior:
- Load file from CLI argument.
- Build structural index and render viewport lines.
- Show status with file name and position.

Edge handling:
- Missing/unreadable file, invalid UTF-8, empty docs, huge docs.

Acceptance checks:
- Can open fixture docs and scroll reliably.
- Status updates correctly with movement.

Tests:
- Unit: file load and error mapping.
- Integration: initialization from fixture docs.
- Manual: open, scroll, resize, quit.

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

### 6.3 Feature C: Outline Modal with Live Preview

Behavior:
- `o` opens outline overlay.
- Pre-select heading that contains current cursor line.
- Moving selection previews heading in background viewport.
- `Enter` confirms jump; `Esc` cancels and restores origin.

Display:
- Hierarchy shown with indentation and heading markers.
- Filterable list via text query.

Edge handling:
- No headings.
- Filter-to-zero results.
- Deep heading nesting (indent clamping).

Tests:
- Unit: preselection and filtering.
- Integration: preview/confirm/cancel semantics.
- Snapshot: modal layout at narrow/wide terminals.

### 6.4 Feature D: Slash Search

Behavior:
- `/` opens search prompt.
- `Enter` computes matches and jumps to first.
- Current and non-current matches are highlighted distinctly.
- `Ctrl+n`/`Ctrl+p` navigate matches.
- Reuse previous query on reopen.

Search semantics:
- Default: literal substring, case-insensitive by default.
- Optional follow-up: smartcase or regex mode behind setting.

Performance:
- Small docs sync path.
- Large docs async path with "searching..." status.

Edge handling:
- Empty query.
- No matches.
- Unicode match boundaries.

Tests:
- Unit: matching rules + wrap navigation.
- Integration: prompt lifecycle and highlight state.
- Manual: large-doc responsiveness and no-match behavior.

### 6.5 Feature E: Link Focus, Follow, and History

Behavior:
- `Tab` / `Shift+Tab` cycles visible links.
- Focused link gets high-contrast, non-color-only indicator.
- `Enter` follows focused link.
- `#anchor`: jump in current doc.
- `file.md#anchor`: load target file and anchor.
- `b`/`Ctrl+o`: back to previous doc + position.
- `Ctrl+i`: forward when available.

Resolution rules:
- Resolve relative paths from current doc directory.
- Canonicalize paths where safe.
- Map anchor slugs deterministically.

Edge handling:
- Broken link.
- Missing anchor.
- Cyclic document references.

Tests:
- Unit: path resolution + slug lookup.
- Integration: multi-doc navigation stack restoration.
- Manual: broken links and repeated traversals.

### 6.6 Feature F: Filterable Help Dialog (`?`)

Behavior:
- `?` opens shortcuts dialog.
- Filter by key or description.
- Present key, context/mode, and action.

Accessibility:
- Keyboard-only operation.
- Strong focus indicator not relying on color alone.
- Works at narrow widths with truncation rules.

Tests:
- Unit: filter matching.
- Snapshot: narrow and wide layouts.
- Manual: verify all documented shortcuts are functional.

### 6.7 Feature G: Accessibility Baseline

Requirements:
- All interactions available without mouse.
- Focus state always visible and explicit.
- Status messages are text-clear and non-ambiguous.
- Modal transitions avoid motion-heavy effects.

Verification:
- Keyboard-only end-to-end scenarios.
- High-contrast readability checks.

## 7) Error Handling and Robustness

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

- Startup-fatal errors: clear stderr + non-zero exit.
- Runtime recoverable errors: keep app alive, show status message.
- Never panic on malformed markdown or malformed links.
- Use bounds-checked, saturating index math in viewport/navigation.

### 7.3 Defensive Guards

- Clamp modal/list indices.
- Validate all render ranges.
- Guard history stack push/pop transitions.
- Timebox/async long operations and report progress.

## 8) Performance Plan

### 8.1 v1 Budgets

- Open + index ~1 MB markdown file: target under 150 ms on typical dev hardware.
- Heading jump: target under 5 ms.
- Outline open with ~5k headings: target under 30 ms.
- Large-doc search: UI remains responsive via async path.

### 8.2 Optimization Techniques

- Parse once per open.
- Use sorted heading vectors + binary search.
- Cache width-dependent rendering artifacts.
- Restrict link focus calculation to viewport slice.
- Reuse buffers in hot paths.

### 8.3 Graceful Degradation

- If full metadata parse degrades, fallback to plain line viewer with explicit reduced-feature status.
- Preserve scrolling and basic file visibility even when advanced navigation metadata is partial.

## 9) Testing Strategy

### 9.1 Automated Tests

Test layers:
- Unit: parser/index/nav/search/pathing.
- Integration: event -> action -> state flows.
- Snapshot: overlays, status/help bars, link focus, search highlight.

Minimum CI gates per phase:
- `cargo fmt --check`
- `cargo test`

Recommended gate:
- `cargo clippy --all-targets -- -D warnings`

### 9.2 Manual QA Matrix

Run per phase against:
- `fixtures/docs/simple.md`
- `fixtures/docs/nested_headings.md`
- `fixtures/docs/links_internal.md`
- `fixtures/docs/duplicate_anchors.md`
- `fixtures/docs/unicode.md`
- `fixtures/docs/code_fences.md`
- `fixtures/docs/large.md`

Manual checks:
- 80x24 and wide terminals.
- Heading navigation determinism.
- Outline preselect + preview + cancel restore.
- Search lifecycle and match movement.
- Link follow/back/forward behavior.
- Help discoverability and filter correctness.
- Keyboard-only completion of core workflows.

### 9.3 Test-As-You-Build Rule

For each increment:
1. Implement smallest complete slice.
2. Run app manually for that slice.
3. Run focused tests.
4. Run full `cargo test` and `cargo fmt --check`.
5. Capture short QA notes.

## 10) Milestones and Execution Phases

### Phase 0: Bootstrap + Core Skeleton

Deliverables:
- Binary scaffold, CLI file arg, base app loop, minimal viewport.

Acceptance:
- Open file and basic scroll works.

### Phase 1: Markdown Structural Index

Deliverables:
- Heading/link/section/anchor index from parser.

Acceptance:
- Fixture extraction correctness.

### Phase 2: Heading Navigation

Deliverables:
- `n`/`p` navigation with boundary statuses.

Acceptance:
- Deterministic behavior across nested fixtures.

### Phase 3: Outline Modal + Live Preview

Deliverables:
- Filterable outline modal with preview and cancel-restore semantics.

Acceptance:
- Matches expected modal interaction behavior.

### Phase 4: Slash Search

Deliverables:
- Prompt, highlighting, match traversal, async large-file path.

Acceptance:
- Correct + responsive search on small and large fixtures.

### Phase 5: Link Focus + Follow + History

Deliverables:
- Link focus cycling, follow local links/anchors, back/forward stack.

Acceptance:
- Multi-doc traversal without losing context.

### Phase 6: Help Dialog + Accessibility Pass

Deliverables:
- Filterable shortcuts modal and focus/accessibility hardening.

Acceptance:
- New user can discover and perform all key workflows from in-app help only.

### Phase 7: Hardening + Release Prep

Deliverables:
- Error polish, perf tuning, docs, and final QA.

Acceptance:
- Stable behavior across fixtures and terminal sizes.

## 11) Risk Register

- Keybinding ambiguity between heading nav and search-repeat commands.
- Anchor slug compatibility and duplicate slug handling.
- Large-doc search causing interaction stalls if threshold tuning is poor.
- Unicode width and highlight column math inconsistencies.
- Path normalization edge cases (`..`, symlinks, mixed separators).

Mitigations:
- Centralized keymap tests and explicit mode behavior docs.
- Deterministic slugifier + duplicate tests.
- Async search fallback + visible progress state.
- Unicode fixtures + width-aware calculations.
- Canonicalization and explicit error messages for unresolved paths.

## 12) Open Questions (Review Cycle 1)

- Default external URL handling: status-only or opener enabled by default?
- Search mode defaults: literal-only v1 or optional regex/smartcase in v1?
- Heading navigation at ends: stop with status or optional wrap mode?
- Should inline (non-alt-screen) mode be added as an experimental flag in v1?

## 13) Definition of Done (v1)

`mdmd` v1 is complete when:
- Core features in this plan are implemented.
- All phase acceptance criteria pass.
- Automated tests cover parser/navigation/search/link/help flows.
- Manual QA matrix passes on representative fixtures and terminal sizes.
- `cargo fmt --check` and `cargo test` pass cleanly.
- A keyboard-only user can discover shortcuts in `?` and complete core workflows.

## 14) Beads-Ready Task Buckets

Epics:
- E1: App scaffold + CLI + core event loop.
- E2: Markdown indexing (headings, sections, links, anchors).
- E3: Heading navigation + status messaging.
- E4: Outline modal + filtering + live preview.
- E5: Slash search + large-doc async path.
- E6: Link focus/follow/back-forward + path/anchor resolution.
- E7: Help modal + accessibility pass + keymap docs.
- E8: Test harness + fixtures + CI gates + release hardening.

Dependency spine:
- `E1 -> E2 -> E3 -> E4`
- `E2 -> E5`
- `E2 -> E6`
- `E1 -> E7`
- `E3 + E4 + E5 + E6 + E7 -> E8`
