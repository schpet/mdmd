# mdmd

here in the year 2026, markdown is the dominant pl. and here i am shedding a tear for every slop h4 i pipe into less. this fixes that.

---

`mdmd` is built for one job: making markdown easier to look at.

Markdown is great for writing, but rough for reading once files get long, linked, and deeply nested. `mdmd` gives you two fast ways to inspect docs:

- `serve`: open markdown in a browser, with link-aware navigation
- `view`: use a keyboard-driven TUI pager in the terminal

## The Problem It Solves

If your docs live in markdown, you often bounce between raw text, ad-hoc scripts, and browser previews.

`mdmd` is a focused "look at markdown" tool:

- Browse a markdown tree like a site
- Open pages quickly from a stable local URL
- Keep a fast terminal-native pager workflow when you do not want a browser

## Install

```bash
cargo install --path .
```

Or run directly during development:

```bash
cargo run -- <command>
```

## Serve Markdown in the Browser (Primary Workflow)

Use `serve` when you want markdown available in a browser:

```bash
mdmd serve docs/README.md
```

On startup, `mdmd` prints URLs like:

- `url:` direct URL to the entry markdown page
- `index:` browsable root index (`/`)

Useful flags:

```bash
mdmd serve --bind 127.0.0.1 --port 3333 docs/
```

- `--bind`: interface to bind (default `0.0.0.0`)
- `--port`: starting port (default `3333`, auto-increments if busy)

Behavior highlights:

- `GET /` always shows a directory index
- Directory paths resolve `README.md`, then `index.md`
- Extensionless paths fall back to `.md` (for example `/guide` -> `/guide.md`)
- `?raw=1` serves raw markdown as plain text

See `docs/serve-semantics.md` for the full contract.

## TUI Pager

Use `view` (or legacy `mdmd <file>`) for an in-terminal pager:

```bash
mdmd view docs/README.md
# or
mdmd docs/README.md
```

Key capabilities:

- Vim-like scrolling (`j`, `k`, `g`, `G`, `Ctrl-d`, `Ctrl-u`)
- Heading jumps (`n`, `p`) and outline modal (`o`)
- Incremental search (`/`, `Ctrl-n`, `Ctrl-p`)
- Link focus/follow and back navigation (`Tab`, `Shift-Tab`, `Enter`, `Backspace`)
- In-app shortcut help (`?`)

## CLI Summary

```bash
mdmd <file>                # legacy TUI form
mdmd view <file>           # explicit TUI mode
mdmd serve [options] <file-or-dir>
```

## License

MIT. See `LICENSE`.
