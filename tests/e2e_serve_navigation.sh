#!/usr/bin/env bash
# E2E navigation test: link-rewrite-flow + logging-signatures
#
# Validates that:
#   1. rewrite_local_links() produces correct root-relative hrefs when the
#      entry file is nested under a subdirectory (CWD-root scenario, bd-t6w).
#   2. resolve_candidate() fallback branches (exact, extensionless, readme,
#      index) each produce HTTP 200 in a real server.
#   3. Resolver branch decisions appear in server stderr logs.
#   4. No unexpected WARN/ERROR lines appear during valid navigation.
#
# Usage:
#   # From the project root (cargo must have already built the binary):
#   BINARY=./target/debug/mdmd bash tests/e2e_serve_navigation.sh
#
#   # Or let the script build first:
#   bash tests/e2e_serve_navigation.sh
#
# Exit code: 0 = all checks passed, non-zero = at least one check failed.

set -euo pipefail

PASS=0
FAIL=0

pass() { echo "PASS: $*"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $*" >&2; FAIL=$((FAIL + 1)); }

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

mkdir -p "$TMP/subdir"

# Entry file: nested under subdir/
# Contains three kinds of relative links to exercise rewrite_local_links().
cat > "$TMP/subdir/README.md" << 'MDEOF'
# Nested Entry

[sibling](sibling.md)

[parent file](../other.md)

[extensionless](sibling)
MDEOF

echo "# Sibling"  > "$TMP/subdir/sibling.md"
echo "# Other"    > "$TMP/other.md"

# Additional files for resolve_candidate fallback-order checks.
echo "# ExactPage"  > "$TMP/page.md"
echo "# NoExtPage"  > "$TMP/noext.md"
mkdir -p "$TMP/has-readme"
echo "# DirReadme"  > "$TMP/has-readme/README.md"
mkdir -p "$TMP/has-index"
echo "# DirIndex"   > "$TMP/has-index/index.md"

# ---------------------------------------------------------------------------
# Start server  (CWD = TMP so serve_root = TMP)
# ---------------------------------------------------------------------------
PORT=47391   # pick a hopefully-free port

RUST_LOG=debug "$BINARY" serve \
    --bind 127.0.0.1 \
    --port "$PORT" \
    "$TMP/subdir/README.md" \
    > "$TMP/stdout.log" 2> "$TMP/stderr.log" &
SERVER_PID=$!

echo "[e2e] server pid=$SERVER_PID port=$PORT"

# Wait for the server to become ready (poll /).
READY=0
for i in $(seq 1 40); do
    if curl -sf --max-time 1 "http://127.0.0.1:$PORT/" > /dev/null 2>&1; then
        READY=1
        break
    fi
    sleep 0.15
done

if [[ $READY -eq 0 ]]; then
    echo "error: server did not become ready" >&2
    echo "--- stdout ---"
    cat "$TMP/stdout.log" || true
    echo "--- stderr ---"
    cat "$TMP/stderr.log" || true
    exit 1
fi

echo "[e2e] server ready"

# ---------------------------------------------------------------------------
# Helper: HTTP status + body via curl
# ---------------------------------------------------------------------------
http_status() { curl -s -o /dev/null -w "%{http_code}" --max-time 5 "$1"; }
http_body()   { curl -s --max-time 5 "$1"; }

BASE="http://127.0.0.1:$PORT"

# ===========================================================================
# Section 1: link-rewrite-flow
# ===========================================================================
echo ""
echo "=== link-rewrite-flow ==="

BODY=$(http_body "$BASE/subdir/README.md")

# Sibling link must be /subdir/sibling.md (root-relative, with prefix).
if echo "$BODY" | grep -q 'href="/subdir/sibling.md"'; then
    pass "sibling link rewrites to /subdir/sibling.md"
else
    fail "sibling link not root-relative: expected href=\"/subdir/sibling.md\" in rendered HTML"
fi

# Parent-level link (../other.md) must resolve to /other.md.
if echo "$BODY" | grep -q 'href="/other.md"'; then
    pass "parent link rewrites to /other.md"
else
    fail "parent link not correct: expected href=\"/other.md\" in rendered HTML"
fi

# Extensionless link must include the subdir prefix.
if echo "$BODY" | grep -q 'href="/subdir/sibling"'; then
    pass "extensionless link rewrites to /subdir/sibling"
else
    fail "extensionless link not correct: expected href=\"/subdir/sibling\" in rendered HTML"
fi

# Rewritten targets must be reachable (HTTP 200).
for path in "/subdir/sibling.md" "/other.md" "/subdir/sibling"; do
    STATUS=$(http_status "$BASE$path")
    if [[ "$STATUS" == "200" ]]; then
        pass "rewritten target $path → 200"
    else
        fail "rewritten target $path → $STATUS (expected 200)"
    fi
done

# ===========================================================================
# Section 2: resolve_candidate fallback order via HTTP
# ===========================================================================
echo ""
echo "=== resolve fallback order ==="

# Branch 1 – exact path.
STATUS=$(http_status "$BASE/page.md")
BODY=$(http_body "$BASE/page.md")
if [[ "$STATUS" == "200" ]] && echo "$BODY" | grep -q "ExactPage"; then
    pass "exact branch: /page.md → 200 with expected content"
else
    fail "exact branch: /page.md → $STATUS, content check failed"
fi

# Branch 2 – extensionless → .md appended.
STATUS=$(http_status "$BASE/noext")
BODY=$(http_body "$BASE/noext")
if [[ "$STATUS" == "200" ]] && echo "$BODY" | grep -q "NoExtPage"; then
    pass "extensionless branch: /noext → 200 with expected content"
else
    fail "extensionless branch: /noext → $STATUS, content check failed"
fi

# Branch 3 – directory → README.md.
STATUS=$(http_status "$BASE/has-readme/")
BODY=$(http_body "$BASE/has-readme/")
if [[ "$STATUS" == "200" ]] && echo "$BODY" | grep -q "DirReadme"; then
    pass "readme branch: /has-readme/ → 200 with expected content"
else
    fail "readme branch: /has-readme/ → $STATUS, content check failed"
fi

# Branch 4 – directory → index.md (README.md absent).
STATUS=$(http_status "$BASE/has-index/")
BODY=$(http_body "$BASE/has-index/")
if [[ "$STATUS" == "200" ]] && echo "$BODY" | grep -q "DirIndex"; then
    pass "index branch: /has-index/ → 200 with expected content"
else
    fail "index branch: /has-index/ → $STATUS, content check failed"
fi

# ===========================================================================
# Section 3: logging-signatures
# ===========================================================================
echo ""
echo "=== logging-signatures ==="

# Resolver branch decisions must appear in stderr.
if grep -q '\[resolve\].*branch=' "$TMP/stderr.log"; then
    pass "resolver branch decisions logged in stderr"
else
    fail "no [resolve] branch= lines found in server stderr"
fi

# rewrite_local_links log must appear for the nested entry render.
if grep -q '\[rewrite\].*rewritten=' "$TMP/stderr.log"; then
    pass "[rewrite] rewritten= log present in stderr"
else
    fail "no [rewrite] rewritten= lines found in server stderr"
fi

# No unexpected WARN or ERROR on valid navigation.
# Filter out lines that are known-safe patterns before checking.
BAD_LINES=$(grep -E '\bWARN\b|\bERROR\b' "$TMP/stderr.log" || true)
if [[ -z "$BAD_LINES" ]]; then
    pass "no unexpected WARN/ERROR on valid navigation"
else
    fail "unexpected WARN/ERROR lines in server stderr:\n$BAD_LINES"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "=== summary ==="
echo "passed=$PASS failed=$FAIL"

if [[ $FAIL -gt 0 ]]; then
    echo ""
    echo "--- server stderr (last 40 lines) ---"
    tail -40 "$TMP/stderr.log" || true
    exit 1
fi

exit 0
