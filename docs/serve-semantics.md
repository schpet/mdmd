# mdmd serve — Behavior Contract

This document is the canonical reference for `mdmd serve` semantics.  It
describes what the server does, not how it is implemented.  Source of truth:
`src/serve.rs`.

---

## 1. Serve Root

The **serve root** is the current working directory (CWD) at the moment
`mdmd serve` is launched.

```
cd /home/user/my-docs
mdmd serve README.md        # serve root = /home/user/my-docs
```

The entry file must lie inside the serve root.  If the resolved entry path is
outside the serve root, `mdmd serve` prints an error and exits with a non-zero
status.  Run `mdmd` from your document root directory.

---

## 2. Entry File Resolution

The `<file>` argument may be a file path or a directory path.

- **File path**: used directly (must exist and be readable).
- **Directory path**: the server looks for `README.md` first, then `index.md`,
  inside that directory.  If neither exists, `mdmd serve` exits with an error.

The resolved entry file path is canonicalized (symlinks resolved) before use.

---

## 3. Startup Banner (stdout)

On successful startup, the following lines are printed to **stdout** in order.
This output is stable and intended for automated parsing by scripts and
integration tests.

```
mdmd serve
root:  /absolute/path/to/serve_root
entry: /absolute/path/to/entry_file.md
url:   http://127.0.0.1:<port>/<url-path-to-entry>
index: http://127.0.0.1:<port>/
```

When Tailscale is detected, two additional lines are appended:

```
url:   http://<tailscale-hostname>:<port>/<url-path-to-entry>
index: http://<tailscale-hostname>:<port>/
```

- `url:` is the direct link to the entry document (with its URL path).
- `index:` is always the root directory index (`/`).
- The key names (`root:`, `entry:`, `url:`, `index:`) are stable; do not
  parse positional fields.

Port selection: `--port` (default 3333) is tried first.  On `EADDRINUSE` the
port is incremented by one and retried up to 100 times.

---

## 4. Request Routing Pipeline

All requests go through the following pipeline in order.  Each step either
produces a final response or falls through to the next step.

### Step 0 — Embedded assets (early exit)

`GET /assets/mdmd.css` and `GET /assets/mdmd.js` are served from bytes
embedded in the binary.  No filesystem access occurs.  Both assets support
`ETag` / `If-None-Match` and `Last-Modified` / `If-Modified-Since` caching.

### Step 1 — Percent-decode

The raw request path is percent-decoded (RFC 3986 §2.1).  Malformed encoding
(truncated `%XX` sequences, non-hex digits) and null bytes in the decoded path
produce a terse **404 Not Found** response.  No path information is disclosed.

### Step 2 — Normalize

`.` and `..` path components are resolved by iteration.  A `..` that would
escape the serve root (stack underflow) produces a terse **404 Not Found**
with no path information disclosed.

### Step 3 — Root index (early exit for `GET /`)

`GET /` **always** renders a browsable directory listing of the serve root.
This behavior is unconditional: even if `README.md` exists at the root, `GET /`
shows the directory index, not the README.

### Steps 4–7 — Non-root path resolution

For all other paths:

| Priority | Condition | Action |
|----------|-----------|--------|
| 1 | Exact file exists | Serve that file |
| 2 | No file extension and `<path>.md` exists | Serve `<path>.md` (extensionless fallback) |
| 3 | Path is a directory and `<dir>/README.md` exists | Serve `README.md` |
| 4 | Path is a directory and `<dir>/index.md` exists | Serve `index.md` |
| 5 | Path is a directory with no markdown index | Render directory listing (200) |
| 6 | Path does not exist | Render rich 404 page |

After resolution, the resolved path is canonicalized and verified to lie
inside the serve root (R1 containment check, symlink-safe).  Paths that
escape the serve root via symlinks produce a terse 404.

Files larger than **16 MiB** are rejected with **413 Content Too Large**.

---

## 5. Serving Markdown Files

`.md` files are rendered to HTML using the `mdmd` stylesheet and TOC sidebar.

Append `?raw=1` to any `.md` URL to receive the raw markdown source as
`text/plain; charset=utf-8`.

All 200 responses include `ETag`, `Last-Modified`, and
`X-Content-Type-Options: nosniff` headers.  Conditional requests
(`If-None-Match`, `If-Modified-Since`) are evaluated and return **304 Not
Modified** with no body when the resource has not changed.

---

## 6. Directory Index Policy

When a directory listing is rendered (either for `GET /` or for a directory
path with no markdown index), the following rules apply:

| Rule | Detail |
|------|--------|
| Dotfiles excluded | Any entry whose name begins with `.` is silently omitted |
| Out-of-root symlinks excluded | Symlinks whose canonicalized target lies outside the serve root are silently omitted and logged as `[dir-index] omit out-of-root symlink` |
| Sort order | Directories first (case-insensitive alphabetical), then files (case-insensitive alphabetical) |
| Breadcrumbs | A breadcrumb navigation bar is rendered above the listing |
| Content-Type | `text/html; charset=utf-8` |

Directory listings are not cached with a file-system mtime (the HTML is
generated on each request), but an `ETag` based on the HTML content is
included.

---

## 7. Rich 404 Page

When a non-root path is not found (after all resolution fallbacks fail), a
rich **404** HTML page is returned.  It includes:

- The requested path.
- A link to the **root index** (`/`).
- A link to the **entry document**.
- A link to the **nearest existing ancestor directory** within the serve root.
- A directory listing of that nearest ancestor.

Security-denial branches (path traversal, symlink escape, encoding violations)
use a terse `text/plain` 404 to avoid disclosing internal path information.

---

## 8. Log Keys (stderr)

All diagnostic output goes to **stderr**.  Each line starts with a bracketed
key for reliable grepping.

| Key | Meaning |
|-----|---------|
| `[serve] listening on <addr>:<port>` | Server bound and accepting connections |
| `[serve] serve_root=<path> entry_url_path=<url>` | Debug startup info |
| `[bind] trying port=<N>` | Port bind attempt |
| `[bind] success port=<N>` | Port bind succeeded |
| `[bind] EADDRINUSE, trying <N>` | Port in use, retrying next port |
| `[resolve] path=<url> branch=<name>` | Resolution outcome (`exact`, `extensionless`, `readme`, `index`, `dir-index`, `not-found`, `denied`) |
| `[dir-index] path=<url> entries=<N>` | Directory listing rendered |
| `[dir-index] omit out-of-root symlink name=<n> dir=<d>` | Symlink excluded from listing |
| `[request] path=<url> mode=<mode>` | Request dispatch outcome (`asset`, `raw`, `rendered`, `static_asset`, `directory_index`, `rich_404`) |
| `[cache] path=<url> etag=<tag> status=<200\|304>` | Cache validation result |
| `[rewrite] file=<path> rewritten=<N> skipped=<M>` | Link rewriting stats |
| `[404] path=<url> nearest_parent=<path>` | Rich 404 fired |
| `[tailscale] skipped reason=<reason>` | Tailscale detection failed |
| `[compression] encoding=<enc>` | Negotiated compression encoding (`br`, `gzip`, or `none`) |
| `[shutdown] complete` | SIGINT received, clean exit |

---

## 9. Security Properties

- **R1 — Containment**: All resolved paths are canonicalized and verified to
  start with `canonical_root` before any file content is read.  Symlinks that
  escape the serve root are rejected with a terse 404.
- **R5 — Size guard**: Files larger than 16 MiB are rejected with 413.
- **Null-byte rejection**: Any decoded path containing `\0` is rejected.
- **Path traversal rejection**: `..` components that would escape the root
  produce a terse 404.
- `X-Content-Type-Options: nosniff` is set on all responses.
- Security-denial branches never echo path information in the response body.

---

## 10. Static Asset Serving

Non-`.md` files that pass all checks are served as static assets with a
`Content-Type` derived from their file extension.  Unknown extensions receive
`application/octet-stream`.

Supported extensions: `.css`, `.js`, `.png`, `.jpg`/`.jpeg`, `.svg`, `.gif`,
`.ico`, `.woff2`, `.pdf`.

---

## 11. Options Reference

| Flag | Default | Description |
|------|---------|-------------|
| `--bind <addr>` | `0.0.0.0` | Interface address to bind |
| `--port <N>` | `3333` | Starting port (auto-increments on EADDRINUSE) |

Compression (gzip / brotli) is negotiated automatically via the client's
`Accept-Encoding` header.  No flag is needed.
