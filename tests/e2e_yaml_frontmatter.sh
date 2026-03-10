#!/usr/bin/env bash

set -euo pipefail

PASS=0
FAIL=0
KEEP_TEMP=0
SERVER_PID=""
TMP_DIR=""

log_pass() {
    echo "[yaml-frontmatter-e2e] PASS: $*"
    PASS=$((PASS + 1))
}

log_fail() {
    echo "[yaml-frontmatter-e2e] FAIL: $*" >&2
    FAIL=$((FAIL + 1))
    KEEP_TEMP=1
}

cleanup() {
    if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi

    if [[ -z "$TMP_DIR" ]]; then
        return
    fi

    if [[ $KEEP_TEMP -eq 0 ]]; then
        rm -rf "$TMP_DIR"
    else
        echo "[yaml-frontmatter-e2e] temp log directory: $TMP_DIR" >&2
    fi
}

trap cleanup EXIT

assert_file_contains() {
    local label="$1" needle="$2" file="$3"
    if grep -qiF "$needle" "$file"; then
        log_pass "$label contains '$needle'"
    else
        log_fail "$label missing '$needle' (file=$file)"
    fi
}

assert_file_not_contains() {
    local label="$1" needle="$2" file="$3"
    if grep -qiF "$needle" "$file"; then
        log_fail "$label unexpectedly contained '$needle' (file=$file)"
    else
        log_pass "$label omits '$needle'"
    fi
}

assert_status() {
    local label="$1" expected="$2" observed="$3"
    if [[ "$observed" == "$expected" ]]; then
        log_pass "$label returned HTTP $observed"
    else
        log_fail "$label returned HTTP $observed (expected $expected)"
    fi
}

assert_order() {
    local label="$1" first="$2" second="$3" file="$4"
    if python3 - "$file" "$first" "$second" <<'PY'
import sys
path, first, second = sys.argv[1:]
text = open(path, "r", encoding="utf-8").read()
sys.exit(0 if text.find(first) != -1 and text.find(second) != -1 and text.find(first) < text.find(second) else 1)
PY
    then
        log_pass "$label order verified"
    else
        log_fail "$label order check failed (file=$file)"
    fi
}

fetch() {
    local name="$1" path="$2"
    local body_file="$TMP_DIR/${name}.body"
    local header_file="$TMP_DIR/${name}.headers"
    local status
    status=$(curl -sS --max-time 5 -D "$header_file" -o "$body_file" -w "%{http_code}" "${BASE_URL}${path}") || status="curl-failed"
    echo "$status"
}

if [[ -z "${BINARY:-}" ]]; then
    cargo build --bin mdmd --quiet
    BINARY=./target/debug/mdmd
fi

if [[ ! -x "$BINARY" ]]; then
    echo "error: binary not executable: $BINARY" >&2
    exit 1
fi

BINARY=$(realpath "$BINARY")

TMP_DIR=$(mktemp -d)
README_PATH="$TMP_DIR/README.md"

cat > "$README_PATH" <<'EOF_MD'
---
title: Browser Title
summary: summary text
tags:
  - alpha
  - beta
empty: null
details:
  owner: team docs
  links:
    - href: /docs/ref
    - fallback
"<script>alert(1)</script>": "\"quoted\" & <tag>"
---
# Article Heading

Body paragraph.
EOF_MD

cat > "$TMP_DIR/referrer.md" <<'EOF_MD'
# Referrer

[Article](README.md)
EOF_MD

cat > "$TMP_DIR/empty.md" <<'EOF_MD'
---
---
# Empty Heading

Body after empty frontmatter.
EOF_MD

cat > "$TMP_DIR/malformed.md" <<'EOF_MD'
---
title: [oops
---
# Malformed Heading

Malformed body.
EOF_MD

cat > "$TMP_DIR/unterminated.md" <<'EOF_MD'
---
title: Broken
owner: team docs
# Unterminated Heading

Unterminated body.
EOF_MD

cat > "$TMP_DIR/non-mapping.md" <<'EOF_MD'
---
[alpha, beta]
---
# Non Mapping Heading
EOF_MD

cat > "$TMP_DIR/plain.md" <<'EOF_MD'
# Plain Heading

Plain body.
EOF_MD

PORT=$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)

(cd "$TMP_DIR" && exec "$BINARY" serve --bind 127.0.0.1 --port "$PORT" "$README_PATH") >"$TMP_DIR/stdout.log" 2>"$TMP_DIR/stderr.log" &
SERVER_PID=$!

BASE_URL=""
for _ in $(seq 1 60); do
    if [[ -s "$TMP_DIR/stdout.log" ]]; then
        url_line=$(grep -E '^http://' "$TMP_DIR/stdout.log" | head -n 1 || true)
        if [[ -n "$url_line" ]]; then
            BASE_URL="${url_line%/}"
            if curl -sf --max-time 1 "${BASE_URL}/" >/dev/null 2>&1; then
                break
            fi
        fi
    fi
    sleep 0.1
done

if [[ -z "$BASE_URL" ]]; then
    echo "[yaml-frontmatter-e2e] FAIL: server did not emit a startup URL" >&2
    echo "--- stdout ---" >&2
    cat "$TMP_DIR/stdout.log" >&2 || true
    echo "--- stderr ---" >&2
    cat "$TMP_DIR/stderr.log" >&2 || true
    KEEP_TEMP=1
    exit 1
fi

valid_status=$(fetch valid "/README.md")
assert_status "valid mapping page" "200" "$valid_status"
assert_file_contains "valid mapping page" "<details class=\"frontmatter-panel\" open aria-label=\"Document metadata\">" "$TMP_DIR/valid.body"
assert_file_contains "valid mapping page" "<title>Browser Title · mdmd serve</title>" "$TMP_DIR/valid.body"
assert_file_contains "valid mapping page" "<h1 id=\"article-heading\">Article Heading</h1>" "$TMP_DIR/valid.body"
assert_file_contains "valid mapping page" "<span class=\"meta-tag\">alpha</span>" "$TMP_DIR/valid.body"
assert_file_contains "valid mapping page" "<span class=\"val-null\">null</span>" "$TMP_DIR/valid.body"
assert_file_contains "valid mapping page" "&lt;script&gt;alert(1)&lt;/script&gt;" "$TMP_DIR/valid.body"
assert_file_not_contains "valid mapping page" "title: Browser Title" "$TMP_DIR/valid.body"
assert_order "frontmatter before article heading" "<details class=\"frontmatter-panel\"" "<h1 id=\"article-heading\">" "$TMP_DIR/valid.body"
assert_order "backlinks after article body" "<h1 id=\"article-heading\">" "<section class=\"backlinks-panel\"" "$TMP_DIR/valid.body"

empty_status=$(fetch empty "/empty.md")
assert_status "empty frontmatter page" "200" "$empty_status"
assert_file_not_contains "empty frontmatter page" "frontmatter-panel" "$TMP_DIR/empty.body"
assert_file_not_contains "empty frontmatter page" "---" "$TMP_DIR/empty.body"
assert_file_contains "empty frontmatter page" "<h1 id=\"empty-heading\">Empty Heading</h1>" "$TMP_DIR/empty.body"

malformed_status=$(fetch malformed "/malformed.md")
assert_status "malformed frontmatter page" "200" "$malformed_status"
assert_file_not_contains "malformed frontmatter page" "frontmatter-panel" "$TMP_DIR/malformed.body"
assert_file_contains "malformed frontmatter page" "---" "$TMP_DIR/malformed.body"
assert_file_contains "malformed frontmatter page" "title: [oops" "$TMP_DIR/malformed.body"
assert_file_contains "malformed frontmatter page" "# Malformed Heading" "$TMP_DIR/malformed.body"

unterminated_status=$(fetch unterminated "/unterminated.md")
assert_status "unterminated frontmatter page" "200" "$unterminated_status"
assert_file_not_contains "unterminated frontmatter page" "frontmatter-panel" "$TMP_DIR/unterminated.body"
assert_file_contains "unterminated frontmatter page" "---" "$TMP_DIR/unterminated.body"
assert_file_contains "unterminated frontmatter page" "title: Broken" "$TMP_DIR/unterminated.body"
assert_file_contains "unterminated frontmatter page" "# Unterminated Heading" "$TMP_DIR/unterminated.body"

non_mapping_status=$(fetch non_mapping "/non-mapping.md")
assert_status "non-mapping frontmatter page" "200" "$non_mapping_status"
assert_file_not_contains "non-mapping frontmatter page" "frontmatter-panel" "$TMP_DIR/non_mapping.body"
assert_file_contains "non-mapping frontmatter page" "---" "$TMP_DIR/non_mapping.body"
assert_file_contains "non-mapping frontmatter page" "[alpha, beta]" "$TMP_DIR/non_mapping.body"

plain_status=$(fetch plain "/plain.md")
assert_status "plain markdown page" "200" "$plain_status"
assert_file_not_contains "plain markdown page" "frontmatter-panel" "$TMP_DIR/plain.body"
assert_file_contains "plain markdown page" "<h1 id=\"plain-heading\">Plain Heading</h1>" "$TMP_DIR/plain.body"

raw_status=$(fetch raw "/README.md?raw=1")
assert_status "raw mode page" "200" "$raw_status"
assert_file_contains "raw mode headers" "content-type: text/plain" "$TMP_DIR/raw.headers"
if cmp -s "$README_PATH" "$TMP_DIR/raw.body"; then
    log_pass "raw mode preserved original file contents exactly"
else
    log_fail "raw mode body differed from source fixture"
fi
assert_file_not_contains "raw mode page" "<!DOCTYPE html>" "$TMP_DIR/raw.body"

css_status=$(fetch css "/assets/mdmd.css")
assert_status "stylesheet" "200" "$css_status"
assert_file_contains "stylesheet" ".frontmatter-panel {" "$TMP_DIR/css.body"
assert_file_contains "stylesheet" ".val-null {" "$TMP_DIR/css.body"
assert_file_contains "stylesheet" ".meta-tag {" "$TMP_DIR/css.body"
assert_file_contains "stylesheet" "@media (max-width: 768px)" "$TMP_DIR/css.body"

if [[ $FAIL -ne 0 ]]; then
    echo "[yaml-frontmatter-e2e] FAIL: $FAIL checks failed; see $TMP_DIR" >&2
    exit 1
fi

echo "[yaml-frontmatter-e2e] PASS: all $PASS checks passed"
