#!/usr/bin/env bash
# E2E navigation test matrix for mdmd serve (bd-3h2)
#
# Required sections:
#   root-index-flow       — GET / returns index; entry link navigates to rendered HTML
#   directory-index-policy — dotfiles excluded, dirs-first sort, breadcrumbs present
#   link-rewrite-flow     — rewrite_local_links() produces correct root-relative hrefs
#   resolve-fallback-order — resolve_candidate() branches (exact/extensionless/readme/index)
#   rich-404-recovery     — rich HTML 404 page with requested path and recovery links
#   logging-signatures    — resolver branches and rewrite logs appear in stderr
#
# Per-request assertions report: url, expected, observed, description.
#
# Usage:
#   # From the project root (binary already built):
#   BINARY=./target/debug/mdmd bash tests/e2e_serve_navigation.sh
#
#   # Auto-build then run:
#   bash tests/e2e_serve_navigation.sh
#
# Exit code: 0 = all checks passed, non-zero = at least one check failed.

set -euo pipefail

PASS=0
FAIL=0

# ---------------------------------------------------------------------------
# Assertion helpers — each reports url, expected, observed, and description
# ---------------------------------------------------------------------------

# assert_status URL EXPECTED_STATUS OBSERVED_STATUS DESCRIPTION
assert_status() {
    local url="$1" expected="$2" observed="$3" description="$4"
    if [[ "$observed" == "$expected" ]]; then
        echo "PASS  url=$url  status expected=$expected observed=$observed  ($description)"
        PASS=$((PASS + 1))
    else
        echo "FAIL  url=$url  status expected=$expected observed=$observed  ($description)" >&2
        FAIL=$((FAIL + 1))
    fi
}

# assert_body_contains URL MARKER BODY DESCRIPTION
assert_body_contains() {
    local url="$1" marker="$2" body="$3" description="$4"
    if echo "$body" | grep -qF "$marker"; then
        echo "PASS  url=$url  marker present: '$marker'  ($description)"
        PASS=$((PASS + 1))
    else
        echo "FAIL  url=$url  marker NOT found: '$marker'  ($description)" >&2
        FAIL=$((FAIL + 1))
    fi
}

# assert_body_not_contains URL MARKER BODY DESCRIPTION
assert_body_not_contains() {
    local url="$1" marker="$2" body="$3" description="$4"
    if echo "$body" | grep -qF "$marker"; then
        echo "FAIL  url=$url  marker unexpectedly found: '$marker'  ($description)" >&2
        FAIL=$((FAIL + 1))
    else
        echo "PASS  url=$url  marker absent: '$marker'  ($description)"
        PASS=$((PASS + 1))
    fi
}

# assert_header_contains URL HEADER MARKER HEADERS_OUTPUT DESCRIPTION
assert_header_contains() {
    local url="$1" header="$2" marker="$3" headers="$4" description="$5"
    local value
    value=$(echo "$headers" | grep -i "^${header}:" | head -1 | cut -d' ' -f2-)
    if echo "$value" | grep -qiF "$marker"; then
        echo "PASS  url=$url  header $header contains '$marker'  ($description)"
        PASS=$((PASS + 1))
    else
        echo "FAIL  url=$url  header $header='$value' does not contain '$marker'  ($description)" >&2
        FAIL=$((FAIL + 1))
    fi
}

# ---------------------------------------------------------------------------
# Binary resolution
# ---------------------------------------------------------------------------
if [[ -z "${BINARY:-}" ]]; then
    echo "[e2e] BINARY not set — building mdmd..."
    cargo build --bin mdmd --quiet
    BINARY=./target/debug/mdmd
fi

if [[ ! -x "$BINARY" ]]; then
    echo "error: binary not executable: $BINARY" >&2
    exit 1
fi

echo "[e2e] binary=$BINARY"

# ---------------------------------------------------------------------------
# Temporary fixture setup
# ---------------------------------------------------------------------------
TMP=$(mktemp -d)
SERVER_PID=""

cleanup() {
    if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -rf "$TMP"
}
trap cleanup EXIT

# --- Entry file: nested under subdir/ with relative links ---
mkdir -p "$TMP/subdir"
cat > "$TMP/subdir/README.md" << 'MDEOF'
# Nested Entry

[sibling](sibling.md)

[parent file](../other.md)

[extensionless](sibling)
MDEOF

echo "# Sibling"  > "$TMP/subdir/sibling.md"
echo "# Other"    > "$TMP/other.md"

# --- Files for resolve-fallback-order section ---
echo "# ExactPage"  > "$TMP/page.md"
echo "# NoExtPage"  > "$TMP/noext.md"
mkdir -p "$TMP/has-readme"
echo "# DirReadme"  > "$TMP/has-readme/README.md"
mkdir -p "$TMP/has-index"
echo "# DirIndex"   > "$TMP/has-index/index.md"

# --- Files for directory-index-policy section ---
# bare-dir has mixed content for policy validation: subdir before files, dotfiles excluded.
mkdir -p "$TMP/bare-dir/bbb-subdir"
echo "# aaa" > "$TMP/bare-dir/aaa-file.md"
echo "zzz"   > "$TMP/bare-dir/zzz-file.txt"
echo "hidden" > "$TMP/bare-dir/.hidden-file"

# --- Files for rich-404-recovery section ---
mkdir -p "$TMP/docs"
echo "# Intro" > "$TMP/docs/intro.md"
echo "# Guide" > "$TMP/docs/guide.md"

# ---------------------------------------------------------------------------
# Start server — CWD must be TMP so serve_root = TMP
# ---------------------------------------------------------------------------
PORT=47391   # deterministic port for reproducible log artifacts

# The server derives serve_root from CWD; use exec inside the subshell so that
# $! captures the mdmd PID and RUST_LOG is exported into the process environment.
(cd "$TMP" && RUST_LOG=debug exec "$BINARY" serve \
    --bind 127.0.0.1 \
    --port "$PORT" \
    "$TMP/subdir/README.md") \
    > "$TMP/stdout.log" 2> "$TMP/stderr.log" &
SERVER_PID=$!

echo "[e2e] server pid=$SERVER_PID port=$PORT"

# Wait for server readiness (poll GET /).
READY=0
for i in $(seq 1 40); do
    if curl -sf --max-time 1 "http://127.0.0.1:$PORT/" > /dev/null 2>&1; then
        READY=1
        break
    fi
    sleep 0.15
done

if [[ $READY -eq 0 ]]; then
    echo "error: server did not become ready within timeout" >&2
    echo "--- stdout ---" >&2
    cat "$TMP/stdout.log" >&2 || true
    echo "--- stderr ---" >&2
    cat "$TMP/stderr.log" >&2 || true
    exit 1
fi

echo "[e2e] server ready"

# ---------------------------------------------------------------------------
# HTTP helpers
# ---------------------------------------------------------------------------
BASE="http://127.0.0.1:$PORT"

http_status()  { curl -s -o /dev/null -w "%{http_code}" --max-time 5 "$1"; }
http_body()    { curl -s --max-time 5 "$1"; }
http_headers() { curl -sI --max-time 5 "$1"; }

# ===========================================================================
# Section: root-index-flow
# Validates: GET / returns directory index, entry link is present and reachable.
# ===========================================================================
echo ""
echo "=== root-index-flow ==="

ROOT_STATUS=$(http_status "$BASE/")
ROOT_BODY=$(http_body "$BASE/")
ROOT_HEADERS=$(http_headers "$BASE/")

assert_status "$BASE/" "200" "$ROOT_STATUS" "root index returns 200"
assert_body_contains "$BASE/" "Index of /" "$ROOT_BODY" "root index contains listing header"
assert_body_not_contains "$BASE/" "# Nested Entry" "$ROOT_BODY" "root index must not serve raw markdown source"
assert_body_contains "$BASE/" "subdir" "$ROOT_BODY" "root index lists subdir/ entry"

# Entry document link: since entry is subdir/README.md, listing shows subdir/ dir.
# Navigate into subdir/ and verify the entry document is listed there.
SUBDIR_STATUS=$(http_status "$BASE/subdir/")
SUBDIR_BODY=$(http_body "$BASE/subdir/")
assert_status "$BASE/subdir/" "200" "$SUBDIR_STATUS" "subdir/ index returns 200"
assert_body_contains "$BASE/subdir/" "README.md" "$SUBDIR_BODY" "subdir/ listing contains README.md"

# Follow the entry document link and verify rendered HTML.
ENTRY_STATUS=$(http_status "$BASE/subdir/README.md")
ENTRY_BODY=$(http_body "$BASE/subdir/README.md")
ENTRY_HEADERS=$(http_headers "$BASE/subdir/README.md")
assert_status "$BASE/subdir/README.md" "200" "$ENTRY_STATUS" "entry document returns 200"
assert_header_contains "$BASE/subdir/README.md" "content-type" "text/html" "$ENTRY_HEADERS" "entry document has text/html content-type"
assert_body_contains "$BASE/subdir/README.md" "<h1" "$ENTRY_BODY" "entry document renders as HTML with heading"
assert_body_not_contains "$BASE/subdir/README.md" "# Nested Entry" "$ENTRY_BODY" "entry response must not contain raw markdown source"

# ===========================================================================
# Section: directory-index-policy
# Validates: dirs-first ordering, dotfiles excluded, breadcrumbs present.
# ===========================================================================
echo ""
echo "=== directory-index-policy ==="

DIR_STATUS=$(http_status "$BASE/bare-dir/")
DIR_BODY=$(http_body "$BASE/bare-dir/")
assert_status "$BASE/bare-dir/" "200" "$DIR_STATUS" "bare-dir/ index returns 200"
assert_body_contains "$BASE/bare-dir/" "Index of /bare-dir" "$DIR_BODY" "directory index header present"
assert_body_contains "$BASE/bare-dir/" "bbb-subdir" "$DIR_BODY" "bbb-subdir/ visible in listing"
assert_body_contains "$BASE/bare-dir/" "aaa-file.md" "$DIR_BODY" "aaa-file.md visible in listing"
assert_body_contains "$BASE/bare-dir/" "zzz-file.txt" "$DIR_BODY" "zzz-file.txt visible in listing"
assert_body_not_contains "$BASE/bare-dir/" ".hidden-file" "$DIR_BODY" "dotfiles excluded from listing"

# Dirs-first: bbb-subdir must appear before aaa-file.md.
POS_SUBDIR=$(echo "$DIR_BODY" | grep -bo "bbb-subdir" | head -1 | cut -d: -f1 || echo "99999")
POS_FILE=$(echo "$DIR_BODY" | grep -bo "aaa-file.md" | head -1 | cut -d: -f1 || echo "0")
if [[ -n "$POS_SUBDIR" && -n "$POS_FILE" && "$POS_SUBDIR" -lt "$POS_FILE" ]]; then
    echo "PASS  url=$BASE/bare-dir/  dirs-first: bbb-subdir (pos=$POS_SUBDIR) before aaa-file.md (pos=$POS_FILE)  (dirs before files)"
    PASS=$((PASS + 1))
else
    echo "FAIL  url=$BASE/bare-dir/  dirs-first: bbb-subdir (pos=$POS_SUBDIR) should precede aaa-file.md (pos=$POS_FILE)  (dirs before files)" >&2
    FAIL=$((FAIL + 1))
fi

# Alphabetical order within files: aaa-file.md before zzz-file.txt.
POS_AAA=$(echo "$DIR_BODY" | grep -bo "aaa-file.md" | head -1 | cut -d: -f1 || echo "99999")
POS_ZZZ=$(echo "$DIR_BODY" | grep -bo "zzz-file.txt" | head -1 | cut -d: -f1 || echo "0")
if [[ -n "$POS_AAA" && -n "$POS_ZZZ" && "$POS_AAA" -lt "$POS_ZZZ" ]]; then
    echo "PASS  url=$BASE/bare-dir/  alpha order: aaa-file.md (pos=$POS_AAA) before zzz-file.txt (pos=$POS_ZZZ)  (files alphabetical)"
    PASS=$((PASS + 1))
else
    echo "FAIL  url=$BASE/bare-dir/  alpha order: aaa-file.md (pos=$POS_AAA) should precede zzz-file.txt (pos=$POS_ZZZ)  (files alphabetical)" >&2
    FAIL=$((FAIL + 1))
fi

# Breadcrumb root link must be present.
assert_body_contains "$BASE/bare-dir/" 'href="/"' "$DIR_BODY" "breadcrumb root link present"

# ===========================================================================
# Section: link-rewrite-flow
# Validates: rewrite_local_links() produces root-relative hrefs for nested entry.
# ===========================================================================
echo ""
echo "=== link-rewrite-flow ==="

LR_BODY=$(http_body "$BASE/subdir/README.md")

assert_body_contains "$BASE/subdir/README.md" 'href="/subdir/sibling.md"' "$LR_BODY" "sibling link rewrites to /subdir/sibling.md"
assert_body_contains "$BASE/subdir/README.md" 'href="/other.md"' "$LR_BODY" "parent link rewrites to /other.md"
assert_body_contains "$BASE/subdir/README.md" 'href="/subdir/sibling"' "$LR_BODY" "extensionless link rewrites to /subdir/sibling"

# Rewritten link targets must be reachable (HTTP 200).
for path in "/subdir/sibling.md" "/other.md" "/subdir/sibling"; do
    S=$(http_status "$BASE$path")
    assert_status "$BASE$path" "200" "$S" "rewritten target $path reachable"
done

# ===========================================================================
# Section: resolve-fallback-order
# Validates: exact / extensionless / readme / index branches each return 200.
# ===========================================================================
echo ""
echo "=== resolve-fallback-order ==="

# Branch 1 — exact path.
S=$(http_status "$BASE/page.md")
B=$(http_body "$BASE/page.md")
assert_status "$BASE/page.md" "200" "$S" "exact branch: /page.md"
assert_body_contains "$BASE/page.md" "ExactPage" "$B" "exact branch: content check"

# Branch 2 — extensionless → .md appended.
S=$(http_status "$BASE/noext")
B=$(http_body "$BASE/noext")
assert_status "$BASE/noext" "200" "$S" "extensionless branch: /noext"
assert_body_contains "$BASE/noext" "NoExtPage" "$B" "extensionless branch: content check"

# Branch 3 — directory → README.md.
S=$(http_status "$BASE/has-readme/")
B=$(http_body "$BASE/has-readme/")
assert_status "$BASE/has-readme/" "200" "$S" "readme branch: /has-readme/"
assert_body_contains "$BASE/has-readme/" "DirReadme" "$B" "readme branch: content check"

# Branch 4 — directory → index.md (README.md absent).
S=$(http_status "$BASE/has-index/")
B=$(http_body "$BASE/has-index/")
assert_status "$BASE/has-index/" "200" "$S" "index branch: /has-index/"
assert_body_contains "$BASE/has-index/" "DirIndex" "$B" "index branch: content check"

# ===========================================================================
# Section: rich-404-recovery
# Validates: 404 HTML page contains path, recovery links, nearest-parent listing.
# ===========================================================================
echo ""
echo "=== rich-404-recovery ==="

# Missing leaf: /docs/nonexistent.md → nearest parent is /docs/.
NF_STATUS=$(http_status "$BASE/docs/nonexistent.md")
NF_BODY=$(http_body "$BASE/docs/nonexistent.md")
NF_HEADERS=$(http_headers "$BASE/docs/nonexistent.md")
assert_status "$BASE/docs/nonexistent.md" "404" "$NF_STATUS" "missing file returns 404"
assert_header_contains "$BASE/docs/nonexistent.md" "content-type" "text/html" "$NF_HEADERS" "404 response is text/html"
assert_body_contains "$BASE/docs/nonexistent.md" "docs/nonexistent.md" "$NF_BODY" "requested path shown in 404 body"
assert_body_contains "$BASE/docs/nonexistent.md" 'href="/"' "$NF_BODY" "root index recovery link present"
assert_body_contains "$BASE/docs/nonexistent.md" 'href="/subdir/README.md"' "$NF_BODY" "entry document recovery link present"
assert_body_contains "$BASE/docs/nonexistent.md" 'href="/docs/"' "$NF_BODY" "nearest-parent /docs/ recovery link present"
assert_body_contains "$BASE/docs/nonexistent.md" "intro.md" "$NF_BODY" "nearest-parent listing contains intro.md"
assert_body_contains "$BASE/docs/nonexistent.md" "guide.md" "$NF_BODY" "nearest-parent listing contains guide.md"

# Multi-level miss: /docs/a/b/missing.md → nearest reachable parent is /docs/.
ML_STATUS=$(http_status "$BASE/docs/a/b/missing.md")
ML_BODY=$(http_body "$BASE/docs/a/b/missing.md")
assert_status "$BASE/docs/a/b/missing.md" "404" "$ML_STATUS" "multi-level miss returns 404"
if echo "$ML_BODY" | grep -qF 'href="/docs/"' || echo "$ML_BODY" | grep -qF 'href="/"'; then
    echo "PASS  url=$BASE/docs/a/b/missing.md  multi-level miss recovery link present  (recovery link)"
    PASS=$((PASS + 1))
else
    echo "FAIL  url=$BASE/docs/a/b/missing.md  multi-level miss recovery link missing  (recovery link)" >&2
    FAIL=$((FAIL + 1))
fi

# Entirely missing tree: /gone/missing.md → nearest parent is root /.
GM_STATUS=$(http_status "$BASE/gone/missing.md")
GM_BODY=$(http_body "$BASE/gone/missing.md")
assert_status "$BASE/gone/missing.md" "404" "$GM_STATUS" "fully missing path returns 404"
assert_body_contains "$BASE/gone/missing.md" 'href="/"' "$GM_BODY" "root fallback recovery link present"

# Expected 404 scenarios must include structured recovery context in logs.
# (Logged as [request] path=... mode=rich_404 in serve.rs)
if grep -q '\[request\].*mode=rich_404' "$TMP/stderr.log"; then
    echo "PASS  rich-404 structured log context present  (logging)"
    PASS=$((PASS + 1))
else
    echo "FAIL  rich-404 structured log context missing from server stderr  (logging)" >&2
    FAIL=$((FAIL + 1))
fi

# ===========================================================================
# Section: logging-signatures
# Validates: resolver branch logs, rewrite logs, no unexpected WARN/ERROR.
# ===========================================================================
echo ""
echo "=== logging-signatures ==="

# Resolver branch decisions must appear in stderr.
if grep -q '\[resolve\].*branch=' "$TMP/stderr.log"; then
    echo "PASS  resolver branch decisions logged  ([resolve] branch= present)"
    PASS=$((PASS + 1))
else
    echo "FAIL  no [resolve] branch= lines found in server stderr" >&2
    FAIL=$((FAIL + 1))
fi

# rewrite_local_links log must appear for the nested entry render.
if grep -q '\[rewrite\].*rewritten=' "$TMP/stderr.log"; then
    echo "PASS  [rewrite] rewritten= log present in stderr"
    PASS=$((PASS + 1))
else
    echo "FAIL  no [rewrite] rewritten= lines found in server stderr" >&2
    FAIL=$((FAIL + 1))
fi

# No unexpected WARN or ERROR lines on valid navigation paths.
# (Expected 404s are happy-path for this script since we tested them above.)
# Filter to exclude known-safe patterns before asserting.
BAD_LINES=$(grep -E '\bWARN\b|\bERROR\b' "$TMP/stderr.log" || true)
if [[ -z "$BAD_LINES" ]]; then
    echo "PASS  no unexpected WARN/ERROR on valid navigation"
    PASS=$((PASS + 1))
else
    echo "FAIL  unexpected WARN/ERROR lines in server stderr:" >&2
    echo "$BAD_LINES" >&2
    FAIL=$((FAIL + 1))
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "=== summary ==="
echo "passed=$PASS failed=$FAIL"

if [[ $FAIL -gt 0 ]]; then
    echo ""
    echo "--- server stdout ---"
    cat "$TMP/stdout.log" || true
    echo "--- server stderr (last 60 lines) ---"
    tail -60 "$TMP/stderr.log" || true
    exit 1
fi

exit 0
