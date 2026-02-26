# Changelog

## Unreleased

## 0.2.1 - 2026-02-26

### Changed

- [backlinks], [mermaid], [rewrite], and [render] log lines are now silent by default and only emitted when --verbose is passed

## 0.2.0 - 2026-02-26

### Fixed

- backlinks panel no longer captured inside indent wrappers when indent mode is on
- indent-on animation now transitions smoothly: sections are built before the class is added so the browser has a zero-padding baseline to animate from

### Changed

- indentation hierarchy now indents only a heading's content, not the heading itself — heading sits at the parent section's content edge (analogous to a function signature vs its body)

### Added

- playground deep-headings.md document for stress-testing indentation hierarchy across all six heading levels, skipped levels, and alternating depths

## 0.1.0 - 2026-02-25
