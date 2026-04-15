# Changelog

## [Unreleased]

## [0.6.0] - 2026-04-15

### Added

- `mdmd html` subcommand exports a markdown file as a self-contained HTML page with inlined CSS/JS

## [0.5.0] - 2026-03-20

### Added

- headings subcommand lists all headings in a markdown file with index numbers and indentation
- select subcommand prints a section of raw markdown by heading name or index

## [0.4.0] - 2026-03-19

### Added

- full-width toggle button in serve mode lets you remove the 860px content cap

## [0.3.1] - 2026-03-18

### Fixed

- release workflow now regenerates Cargo.lock to keep version in sync

### Changed

- startup URL output now shows only the tailscale IP address instead of both MagicDNS hostname and IP

## [0.3.0] - 2026-03-10

### Fixed

- frontmatter detail/summary panels now collapsed by default
- pre-existing test assertions for frontmatter null class and panel element type

### Added

- YAML frontmatter rendering with collapsible detail panels

## [0.2.5] - 2026-02-27

### Fixed

- integration tests no longer fail on machines with tailscale

### Changed

- serve layout is now left-aligned with TOC sidebar on a distinct background separated by a border
- file change notice is now fixed-positioned in the bottom-right corner

## [0.2.4] - 2026-02-26

### Changed

- serve now shows tailscale IP URL alongside MagicDNS hostname URL when tailscale is available

## [0.2.3] - 2026-02-26

### Fixed

- serve now prints both tailscale and localhost URLs at startup

## [0.2.2] - 2026-02-26

### Fixed

- serve no longer prints backlinks index stats unless --verbose is set

### Changed

- serve now shows tailscale URL instead of localhost when tailscale is available

## [0.2.1] - 2026-02-26

### Changed

- [backlinks], [mermaid], [rewrite], and [render] log lines are now silent by default and only emitted when --verbose is passed

## [0.2.0] - 2026-02-26

### Fixed

- backlinks panel no longer captured inside indent wrappers when indent mode is on
- indent-on animation now transitions smoothly: sections are built before the class is added so the browser has a zero-padding baseline to animate from

### Changed

- indentation hierarchy now indents only a heading's content, not the heading itself — heading sits at the parent section's content edge (analogous to a function signature vs its body)

### Added

- playground deep-headings.md document for stress-testing indentation hierarchy across all six heading levels, skipped levels, and alternating depths

## [0.1.0] - 2026-02-25

[Unreleased]: https://github.com/schpet/mdmd/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/schpet/mdmd/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/schpet/mdmd/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/schpet/mdmd/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/schpet/mdmd/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/schpet/mdmd/compare/v0.2.5...v0.3.0
[0.2.5]: https://github.com/schpet/mdmd/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/schpet/mdmd/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/schpet/mdmd/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/schpet/mdmd/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/schpet/mdmd/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/schpet/mdmd/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/schpet/mdmd/releases/tag/v0.1.0
