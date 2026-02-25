## Overview

Add an **Indentation Hierarchy** toggle to the serve-mode viewer (not an editor), matching dark-mode behavior (client-side button + localStorage + immediate DOM update).

Chosen approach: **DOM-based hierarchy rendering in `main.content`**.
- The browser only has rendered HTML, not raw markdown, so no markdown-string transform pipeline is used.
- Each heading starts a section per document-outline semantics:
  - Content belongs to a heading until the next heading of the same or higher level.
  - Lower-level headings create nested sections.
- Indentation is visual (CSS padding/margin on wrappers), not text-prefix whitespace.

Primary requirements:
- Preserve heading semantics and anchor IDs.
- Keep toggle idempotent and reversible without reload.
- Include smooth CSS transitions when indentation hierarchy is toggled.
- Keep changes minimal in existing assets (`src/assets/mdmd.js`, `src/assets/mdmd.css`, `src/html.rs` only if button markup is added there).

## Workflow Diagram

```text
User clicks "Indentation Hierarchy" button
  |
  +--> Read state (DOM + localStorage)
  |
  +--> If turning ON:
  |      1) Walk direct children of <main class="content">
  |      2) Build section wrappers from h1..h6 outline levels
  |      3) Move heading + following nodes into wrapper(s)
  |      4) Apply depth class/attribute for CSS indentation
  |
  +--> If turning OFF:
  |      1) Restore original DOM structure from saved snapshot
  |         OR unwrap generated section containers
  |
  +--> Persist state in localStorage
  |
  +--> Refresh TOC observer bindings if headings were rewrapped
```

## Implementation Notes

1. Toggle wiring and persistence:
- Add second fixed button beside theme toggle (same size/style family, offset left so controls do not overlap).
- Suggested ids/classes:
  - Button id: `indent-toggle`
  - Root state class: `indent-hierarchy-on` on `<html>` or `<body>`
- Suggested storage key: `mdmd-indent-hierarchy` with values `"on"` / `"off"`.
- Apply saved state on load similarly to theme toggle initialization.

2. DOM section-building algorithm (outline semantics):
- Select headings under `main.content`: `h1..h6`.
- If no headings: no-op toggle (button still updates state).
- Create generated wrappers (for example, `<section class="indent-section" data-depth="N">`).
- Maintain heading-level stack while iterating document-order headings.
- For each heading, include that heading and all following siblings up to (but not including) the next heading with level `<= current`.
- Content before the first heading remains depth 0 (no added indent).

3. TOC + observer compatibility:
- Current `mdmd.js` returns early when no headings; refactor so this does not prevent toggle setup.
- After DOM restructuring, rebuild heading NodeList and rebind `IntersectionObserver`.
- Keep heading elements and ids intact so `.toc-sidebar a[href="#id"]` links remain valid.

4. CSS indentation behavior:
- Indent by wrapper depth using CSS (for example, `padding-inline-start` with depth multiplier).
- Add smooth transitions for indentation-related properties on generated wrappers (for example, `padding-inline-start` and/or `margin-inline-start`) so ON/OFF feels gradual instead of abrupt.
- Keep animations lightweight (short duration/ease) and avoid layout-jank-heavy properties.
- Respect reduced-motion preferences (disable or minimize transitions under `@media (prefers-reduced-motion: reduce)`).
- Do not add indentation inside code text nodes; only wrapper/container layout changes.
- Ensure nested lists, blockquotes, tables, and `<pre><code>` remain readable (no double-indenting code lines).

5. Idempotency and reversal:
- ON when already ON: no additional wrapping.
- OFF restores baseline structure cleanly.
- Repeated ON/OFF cycles must not accumulate wrappers/classes.

## Risks and Mitigations

1. **Snapshot vs. unwrap reversal**: Storing a deep-clone snapshot of `main.content` for reversal is memory-intensive on large documents and does not preserve event listeners attached by other scripts. Mitigation: do not use `cloneNode(true)` for reversal. Instead, mark generated wrappers with a data attribute (e.g., `data-indent-section`) and unwrap them in-place on OFF. Original heading elements must remain the same DOM nodes (not clones) so IntersectionObserver bindings and existing event listeners are preserved.

2. **IntersectionObserver validity after wrapping**: Moving heading elements into new `<section>` wrappers does not invalidate existing `IntersectionObserver` bindings—the observer holds references to the element objects, which are unchanged. However, if any code queries `main.content h1..h6` after restructuring and gets stale references, active-highlight may break. Mitigation: after ON, re-query headings from `main.content` and reset `headingEls` and `visibleIds` so the observer reflects the current DOM state.

3. **Serve-mode live reload**: If the serve watcher replaces `main.content` innerHTML on file change, section wrappers are cleared but the `html`/`body` class still shows `indent-hierarchy-on`. Mitigation: on `DOMContentLoaded` (and after any live-reload content swap), re-apply indent state from `localStorage` before observer setup.

4. **Markdown renderer pre-existing `<section>` elements**: If the markdown pipeline or Rust html-builder emits `<section>` tags, the outline algorithm must not double-nest them. Mitigation: the DOM-walk should only wrap raw block siblings of `main.content` (i.e., direct children that are NOT themselves generated indent sections).

5. **Narrow viewport button overlap**: Adding a second fixed button to the right of `.theme-toggle` (which is positioned at `top: 0.75rem; right: 0.75rem`) may collide with the theme toggle on narrow viewports. Mitigation: position indent toggle at `right: 3.25rem` (or similar calculated offset) so both buttons sit side-by-side with a small gap. Verify on a 320px viewport width.

6. **Idempotency guard in JS**: Multiple rapid clicks (or calling the toggle function twice before a layout tick) must not double-wrap. Mitigation: guard ON path with `document.querySelector('[data-indent-section]')` check; if wrappers already exist, skip restructure and only update class/localStorage.

7. **Code block and table overflow after indentation**: Adding `padding-inline-start` to section wrappers may push wide content (tables, long code lines) past the viewport, causing horizontal scroll. Mitigation: apply `overflow-x: auto` to generated `[data-indent-section]` wrappers, and verify `pre` / `table` inside indented sections still scroll independently.

## Implementation Constraints

- Wrap all new JS in a separate IIFE with `'use strict';`, following the exact pattern used by the dark-mode and TOC IIFEs in `mdmd.js`.
- Use try-catch around `localStorage.setItem` / `localStorage.getItem` calls (same as theme toggle).
- Do not clone heading elements; move them as-is to preserve `id` attributes and any existing event listeners.
- The `headingEls` array and `visibleIds` set used by the IntersectionObserver IIFE must be refreshed after restructuring. Since the two IIFEs are separate scopes, expose a module-level refresh function or re-initialize by disconnecting and re-observing current heading nodes.
- CSS transitions for `padding-inline-start` / `margin-inline-start` on `[data-indent-section]` must be gated with `@media (prefers-reduced-motion: no-preference)` so they only fire when the user has not requested reduced motion.

## Verification

1. Logic checks (unit-level, manual trace before browser test):
- Flat document (all h2s, no h1): each h2 and its content forms a depth-1 section; no h1 section wrapper exists.
- Mixed heading levels (h1 → h2 → h3 → h2): produces correct nesting; second h2 closes the h3 section before opening a sibling.
- Pre-heading content (paragraphs before first heading): remains at depth 0, not wrapped.
- No-headings page: toggle is a no-op for DOM; button still updates class and localStorage without throwing.
- Repeated ON / OFF cycles (≥ 5 rounds): no extra wrappers accumulate; DOM returns to original state on each OFF.
- ON after reload with `localStorage` value `"on"`: indent is applied before first paint (initialization path).

2. Chrome MCP verification (required, performed in this order):

**Setup**
- Run `mdmd serve` pointing at a markdown directory that includes at least one file with mixed h1/h2/h3 headings and one file with no headings.
- Open the multi-heading document in Chrome via the served URL.

**Baseline**
- Confirm dark-mode toggle still works (click → class changes, click again → reverts).
- Confirm TOC active-heading highlight updates when scrolling.

**Indentation ON**
- Click indent toggle button; confirm `html` (or `body`) gains `indent-hierarchy-on` class.
- Inspect DOM under `main.content`: confirm `[data-indent-section]` wrappers exist; h2-level sections nest inside h1 sections.
- Confirm heading `id` attributes are unchanged on the original heading elements.
- Confirm visual indentation increases with nesting level (screenshot or ruler check).
- Confirm the indent animation is smooth (not abrupt jump) — watch transition on `padding-inline-start`.
- Scroll the page; confirm TOC active-heading highlight still follows scroll position.
- Click a TOC link; confirm it still jumps to the correct heading anchor.
- Click an in-content `<a href="#anchor">` link (if present); confirm it navigates correctly.
- Confirm code blocks, tables, and lists are readable and scroll horizontally if needed (no overflow clip).

**Indentation OFF**
- Click toggle; confirm class is removed.
- Inspect DOM: confirm no `[data-indent-section]` wrappers remain; `main.content` matches pre-toggle structure.
- Confirm spacing returns to baseline (no residual padding/margin).
- Confirm OFF transition animates (or is instant under reduced-motion).

**Persistence**
- With indent ON, reload the page; confirm indent is re-applied from localStorage before visible flash.
- Toggle OFF, reload; confirm indent is not applied.

**No-headings page**
- Navigate to a page with no headings.
- Click toggle; confirm no error in console and button state still updates.

**Narrow viewport**
- Resize to 375px wide; confirm indent toggle button and theme toggle button do not overlap.

**Reduced-motion (emulate via DevTools)**
- Enable `prefers-reduced-motion: reduce` in DevTools rendering panel.
- Toggle indent ON and OFF; confirm transitions are suppressed or minimal.
