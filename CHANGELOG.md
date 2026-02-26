# Changelog

## [Unreleased]

## [0.2.0] - 2026-02-26

### Fixed

- backlinks panel no longer captured inside indent wrappers when indent mode is on
- indent-on animation now transitions smoothly: sections are built before the class is added so the browser has a zero-padding baseline to animate from

### Changed

- indentation hierarchy now indents only a heading's content, not the heading itself — heading sits at the parent section's content edge (analogous to a function signature vs its body)

### Added

- playground deep-headings.md document for stress-testing indentation hierarchy across all six heading levels, skipped levels, and alternating depths

## [0.1.0] - 2026-02-25

[Unreleased]: https://github.com/schpet/mdmd/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/schpet/mdmd/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/schpet/mdmd/releases/tag/v0.1.0
