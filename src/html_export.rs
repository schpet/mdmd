//! `mdmd html` subcommand: export a markdown file as a self-contained HTML page.

use std::path::{Path, PathBuf};
use std::{fs, io, process};

use crate::frontmatter;
use crate::html::{self, PageShellContext, RenderTarget};

/// Run the `html` subcommand: read a markdown file and write a standalone HTML page.
///
/// # Parameters
/// - `file`: path to the source markdown file.
/// - `output`: optional explicit output path; defaults to `<stem>.html` next to the input.
/// - `full_width`: whether to render in full-width mode (default `true`).
pub fn run_html(file: &str, output: Option<&str>, full_width: bool) -> io::Result<()> {
    let input_path = Path::new(file);

    // Validate extension (same rules as other file-based commands).
    match input_path.extension().and_then(|e| e.to_str()) {
        Some("md" | "markdown" | "mdx" | "mdown" | "mkd" | "mkdn") => {}
        Some(ext) => {
            eprintln!("Error: '{ext}' is not a recognized markdown extension.");
            eprintln!(
                "Expected a markdown file (.md, .markdown, .mdx, .mdown, .mkd, .mkdn)."
            );
            process::exit(1);
        }
        None => {
            eprintln!("Error: '{file}' has no file extension.");
            eprintln!(
                "Expected a markdown file (.md, .markdown, .mdx, .mdown, .mkd, .mkdn)."
            );
            process::exit(1);
        }
    }

    let source = fs::read_to_string(input_path).unwrap_or_else(|e| {
        match e.kind() {
            io::ErrorKind::NotFound => eprintln!("Error: file not found: {file}"),
            io::ErrorKind::PermissionDenied => eprintln!("Error: permission denied: {file}"),
            _ => eprintln!("Error reading '{file}': {e}"),
        }
        process::exit(1);
    });

    let canonical = fs::canonicalize(input_path).unwrap_or_else(|_| input_path.to_path_buf());
    let parent = canonical.parent().unwrap_or(Path::new("."));

    // Extract frontmatter.
    let extracted = frontmatter::extract(&source);

    // Render markdown with Html target (preserves authored relative links).
    let (html_body, headings) = html::render_markdown(
        extracted.render_body.as_ref(),
        &canonical,
        parent, // serve_root is unused for Html target but required by the signature
        RenderTarget::Html,
        false,
    );

    // Build page shell with no backlinks, no mtime, no url path.
    let ctx = PageShellContext {
        frontmatter: extracted.meta.as_ref(),
        backlinks: &[],
        file_mtime_secs: None,
        page_url_path: None,
        full_width,
    };
    let page = html::build_page_shell(
        &html_body,
        &headings,
        &canonical,
        parent,
        &ctx,
        RenderTarget::Html,
    );

    // Determine output path.
    let output_path: PathBuf = match output {
        Some(p) => PathBuf::from(p),
        None => input_path.with_extension("html"),
    };

    // Write the file.
    fs::write(&output_path, page)?;

    // Print the written path to stdout.
    println!("{}", output_path.display());

    Ok(())
}
