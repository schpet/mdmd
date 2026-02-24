//! Embedded static web assets for the mdmd serve mode.
//!
//! Both files are compiled into the binary via `include_str!` so the binary
//! is fully self-contained; no external asset files need to be distributed.

/// Stylesheet for the serve-mode HTML viewer.
///
/// Loaded from `src/assets/mdmd.css` at compile time.
pub const CSS: &str = include_str!("assets/mdmd.css");

/// JavaScript for the serve-mode HTML viewer.
///
/// Handles TOC active-heading highlighting via `IntersectionObserver` and
/// contains the Mermaid initialisation stub.
/// Loaded from `src/assets/mdmd.js` at compile time.
pub const JS: &str = include_str!("assets/mdmd.js");
