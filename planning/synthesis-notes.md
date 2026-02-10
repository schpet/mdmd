# Synthesis Notes for `synthesized-plan-v1.md`

## 1) How the two plans were evaluated

I compared both plans across the same criteria:
- Product clarity (vision, goals, non-goals)
- Architecture quality (decisions, trade-offs, invariants)
- Interaction design quality (key flows, modal behavior, keybindings)
- Implementation realism (phase order, dependency spine)
- Quality discipline (tests, QA, performance, error policy, risks)

## 2) What Claude did better

Strengths pulled from `planning/claude-plan-v1.md`:
- Strong interaction detail and concrete UX artifacts.
  - The ASCII layouts for main screen and modals make behaviors tangible.
  - Per-feature behavior checklists are very actionable for implementation.
- Rich feature walkthrough style.
  - Outline/search/link/help sections are concrete and easy to execute.
- Practical keybinding ergonomics.
  - Includes vim-like paging/movement aliases and history shortcuts.
- Good implementation momentum.
  - Session-oriented phase breakdown with practical subtask sequencing.

Why these were kept:
- They reduce ambiguity in UI behavior.
- They make QA easier because expected behavior is explicitly visible.
- They are useful for aligning implementation and manual testing quickly.

## 3) What Codex did better

Strengths pulled from `planning/codex-plan-v1.md`:
- Better planning discipline.
  - Clear planning contract and completion gates per milestone.
- Stronger architecture reasoning.
  - Explicit decisions + trade-offs + mitigations.
- Better state design rigor.
  - App state decomposition and invariant list are clearer and safer.
- Better systems-level quality coverage.
  - Risk register, error taxonomy/policy, performance budgets, DoD.
- Better dependency-aware milestone spine.
  - Execution order reflects primitive dependencies and reduces rework.

Why these were kept:
- They reduce technical risk and regressions.
- They make testing, debugging, and code review substantially easier.
- They improve predictability of delivery.

## 4) Key conflicts and how they were resolved

### 4.1 Search repeat keys vs heading navigation keys

Conflict:
- Claude leans toward vim-style `n/N` for search repeat.
- Both plans also require `n/p` for heading navigation.

Resolution in hybrid:
- Keep `n/p` for heading navigation as core.
- Use `Ctrl+n` / `Ctrl+p` for search repeat to avoid ambiguity.

Reason:
- Prevents overloaded behavior in normal mode.
- Keeps heading traversal simple and deterministic.

### 4.2 Rendering model

Conflict:
- Claude implies stronger rendered markdown surface usage.
- Codex explicitly prefers source-faithful rendering for stable offsets.

Resolution in hybrid:
- Source-faithful line model is canonical in v1.
- Styling can still leverage markdown theming primitives.

Reason:
- Preserves exact line/offset mapping for heading/search/link navigation.

### 4.3 Link interaction model

Conflict:
- Claude includes explicit link mode (`Tab`, then `l/h`, `Esc`).
- Codex favors Tab/Shift+Tab focus cycling integrated into normal flow.

Resolution in hybrid:
- Keep Tab/Shift+Tab focus cycling with explicit visual focus.
- `Esc` clears focus; `Enter` follows.
- Keep back/forward shortcuts (`b`/`Ctrl+o`, `Ctrl+i`).

Reason:
- Lower mode complexity with strong discoverability.
- Retains efficient keyboard traversal.

### 4.4 External URL handling

Conflict:
- Claude recommends opening browser in v1.
- Codex treats it as optional/deferred safety choice.

Resolution in hybrid:
- Full support for local markdown + anchors in v1.
- External URL opening is optional behind explicit setting/flag.

Reason:
- Avoids platform/security surprises while preserving extensibility.

## 5) Material intentionally not carried forward (and why)

From Claude plan:
- Overlap and ambiguity around `?` as both help and backward search was not retained.
  - Kept `?` strictly for help to preserve in-app discoverability consistency.
- Some speculative items (for example broad streaming claims without concrete mechanisms) were narrowed into threshold-based async/search and explicit fallback behavior.

From Codex plan:
- Some terse UI descriptions were expanded using Claude-style concrete behavior/checklists.
- Kept rigor, but made user-facing interactions more explicit and testable.

## 6) Net effect of the synthesis

The final hybrid plan is intentionally:
- As rigorous as Codex on architecture, risk, and quality gates.
- As concrete as Claude on interaction behavior and implementation execution.
- More conflict-free on key semantics and mode behavior.
- Better suited for direct conversion into beads epics/tasks and incremental implementation.
