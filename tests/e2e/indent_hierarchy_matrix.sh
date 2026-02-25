#!/usr/bin/env bash
# indent_hierarchy_matrix.sh — E2E indentation toggle matrix for mdmd serve
# (bd-1zl.8.1)
#
# Validates indentation toggle behavior in real serve-mode browser runtime with
# Playwright-driven headless Chromium and detailed per-assertion diagnostics.
#
# Scenarios:
#   baseline-load  — indent wrappers absent, indent/theme buttons present
#   toggle-on      — click ON: wrappers appear, depths set, localStorage='on'
#   toggle-off     — click OFF: wrappers removed after transition, localStorage='off'
#   no-heading     — no crash, toggle remains interactive, state persists
#   toc-scroll     — heading navigation and active class still update after mode changes
#
# Per-scenario logs written to:
#   target/e2e-logs/indent-hierarchy/<timestamp>/<scenario>_{stdout,stderr}.log
# Structured summary written to:
#   target/e2e-logs/indent-hierarchy/<timestamp>/summary.json
#
# Usage (from the project root):
#   BINARY=./target/debug/mdmd bash tests/e2e/indent_hierarchy_matrix.sh
#   bash tests/e2e/indent_hierarchy_matrix.sh
#
# Environment overrides:
#   BINARY           — path to the mdmd binary (default: auto-build debug binary)
#   NODE             — path to the node binary (default: node)
#   PLAYWRIGHT_MODS  — explicit node_modules dir containing 'playwright' package
#
# Exit code: 0 = all assertions passed, non-zero = at least one assertion failed.

set -euo pipefail

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
LOG_DIR="${PROJECT_ROOT}/target/e2e-logs/indent-hierarchy/${TIMESTAMP}"

NODE="${NODE:-node}"

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
PASS=0
FAIL=0

log()    { echo "[indent-e2e] $*"; }
log_err(){ echo "[indent-e2e] $*" >&2; }

# ---------------------------------------------------------------------------
# Binary resolution
# ---------------------------------------------------------------------------
if [[ -z "${BINARY:-}" ]]; then
    log "BINARY not set — building mdmd..."
    (cd "$PROJECT_ROOT" && cargo build --bin mdmd --quiet)
    BINARY="${PROJECT_ROOT}/target/debug/mdmd"
fi

if [[ ! -x "$BINARY" ]]; then
    log_err "error: binary not executable: $BINARY"
    exit 1
fi

log "binary=$BINARY"

# ---------------------------------------------------------------------------
# Node.js pre-flight
# ---------------------------------------------------------------------------
if ! command -v "$NODE" >/dev/null 2>&1; then
    log_err "error: node not found (NODE=$NODE). Install Node.js 18+ to run browser tests."
    exit 1
fi

NODE_VERSION=$("$NODE" --version 2>/dev/null || echo "unknown")
log "node=$NODE ($NODE_VERSION)"

# ---------------------------------------------------------------------------
# Playwright module resolution
# ---------------------------------------------------------------------------
# Priority: PLAYWRIGHT_MODS env > npx cache search > local npm install
find_playwright_mods() {
    # 1. Explicit override
    if [[ -n "${PLAYWRIGHT_MODS:-}" && -d "${PLAYWRIGHT_MODS}/playwright" ]]; then
        echo "$PLAYWRIGHT_MODS"
        return 0
    fi

    # 2. Search npx cache (~/.npm/_npx)
    local npx_cache="${HOME}/.npm/_npx"
    if [[ -d "$npx_cache" ]]; then
        local found
        found=$(find "$npx_cache" -maxdepth 4 -name "playwright" -type d 2>/dev/null \
                | grep 'node_modules/playwright$' | head -1 || true)
        if [[ -n "$found" ]]; then
            dirname "$found"
            return 0
        fi
    fi

    # 3. Install into a temp directory (slow path)
    local install_dir="$TMP/playwright_mods"
    log "Installing playwright into $install_dir ..."
    mkdir -p "$install_dir"
    npm install --prefix "$install_dir" playwright --quiet 2>/dev/null
    echo "$install_dir/node_modules"
}

# ---------------------------------------------------------------------------
# Port helper
# ---------------------------------------------------------------------------
free_port() {
    python3 -c "
import socket
s = socket.socket()
s.bind(('127.0.0.1', 0))
print(s.getsockname()[1])
s.close()
"
}

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

# Rich fixture: multiple heading levels with content under each
cat > "$TMP/rich.md" << 'MDEOF'
# Main Title

Introduction paragraph before any subheading.

## Section Alpha

Content under alpha with a [link](#section-beta).

### Subsection Alpha-One

Deep content under alpha-one.

#### Deep Level Four

Even deeper content.

## Section Beta

Content under beta. This is a long paragraph to ensure vertical scrolling works
for the intersection observer to detect heading visibility changes during tests.
Multiple sentences to create sufficient content height.

### Subsection Beta-One

More content under beta-one. Additional text to ensure scrollable content.

## Section Gamma

Final section content. This section is intentionally placed at the end to require
scroll actions before the intersection observer fires the active-class update.
MDEOF

# No-heading fixture: plain paragraphs only
cat > "$TMP/noheading.md" << 'MDEOF'
This is a page with no headings at all.

Just plain paragraphs of text that flow naturally.

More text to ensure there is enough content for the toggle to encounter.
MDEOF

# ---------------------------------------------------------------------------
# Start server on ephemeral port
# ---------------------------------------------------------------------------
PORT=$(free_port)
(
    cd "$TMP"
    RUST_LOG=info exec "$BINARY" serve \
        --bind 127.0.0.1 \
        --port "$PORT" \
        --no-open \
        "$TMP/rich.md"
) > "$TMP/server_stdout.log" 2> "$TMP/server_stderr.log" &
SERVER_PID=$!

log "server pid=$SERVER_PID port=$PORT"

# Poll for readiness
READY=0
for i in $(seq 1 50); do
    if curl -sf --max-time 1 "http://127.0.0.1:$PORT/" > /dev/null 2>&1; then
        READY=1
        break
    fi
    sleep 0.15
done

if [[ $READY -eq 0 ]]; then
    log_err "error: server did not become ready within timeout"
    log_err "--- server stdout ---"
    cat "$TMP/server_stdout.log" >&2 || true
    log_err "--- server stderr ---"
    cat "$TMP/server_stderr.log" >&2 || true
    exit 1
fi

log "server ready at http://127.0.0.1:$PORT"

# ---------------------------------------------------------------------------
# Create artifact log directory
# ---------------------------------------------------------------------------
mkdir -p "$LOG_DIR"
log "artifact dir: $LOG_DIR"

# ---------------------------------------------------------------------------
# Resolve playwright modules
# ---------------------------------------------------------------------------
PW_MODS=$(find_playwright_mods)
log "playwright modules: $PW_MODS"

# ---------------------------------------------------------------------------
# Write Playwright browser test script
# ---------------------------------------------------------------------------
BROWSER_SCRIPT="$TMP/pw_indent_matrix.mjs"

cat > "$BROWSER_SCRIPT" << 'PWEOF'
/**
 * pw_indent_matrix.mjs — Playwright browser-side assertions for indent toggle
 * (bd-1zl.8.1)
 *
 * Usage: node pw_indent_matrix.mjs <BASE_URL> <ARTIFACT_DIR> <PW_MODS>
 *
 * Scenarios:
 *   1. baseline-load  — no wrappers, buttons present, localStorage not 'on'
 *   2. toggle-on      — wrappers appear, depths set, localStorage='on', aria-pressed='true'
 *   3. toggle-off     — wrappers removed, localStorage='off', aria-pressed='false'
 *   4. no-heading     — no crash, toggle interactive, state persists
 *   5. toc-scroll     — active class updates after TOC navigation + mode changes
 */

import { createRequire } from 'module';
import { writeFileSync, mkdirSync } from 'fs';
import path from 'path';

const BASE_URL    = process.argv[2];
const ARTIFACT_DIR = process.argv[3];
const PW_MODS     = process.argv[4];

if (!BASE_URL || !ARTIFACT_DIR || !PW_MODS) {
    console.error('Usage: node pw_indent_matrix.mjs <BASE_URL> <ARTIFACT_DIR> <PW_MODS>');
    process.exit(2);
}

const require = createRequire(import.meta.url);
const { chromium } = require(path.join(PW_MODS, 'playwright'));

// ---------------------------------------------------------------------------
// Assertion state
// ---------------------------------------------------------------------------
let PASS = 0;
let FAIL = 0;
const results = [];   /* {scenario, description, check, expected, observed, ok} */

function assert(scenario, description, check, expected, observed, ok) {
    const entry = { scenario, description, check, expected: String(expected), observed: String(observed), ok };
    results.push(entry);
    if (ok) {
        console.log(`PASS  [${scenario}]  ${description}  check=${check}  expected=${expected}  observed=${observed}`);
        PASS++;
    } else {
        console.error(`FAIL  [${scenario}]  ${description}  check=${check}  expected=${expected}  observed=${observed}`);
        FAIL++;
    }
}

// ---------------------------------------------------------------------------
// Screenshot helper — captures failure evidence into artifact dir
// ---------------------------------------------------------------------------
async function screenshot(page, label) {
    try {
        const p = path.join(ARTIFACT_DIR, `${label}.png`);
        await page.screenshot({ path: p, fullPage: true });
        console.log(`[screenshot] saved ${p}`);
    } catch (_) { /* non-fatal */ }
}

// ---------------------------------------------------------------------------
// DOM snippet helper — returns outerHTML of up to 3 matching elements
// ---------------------------------------------------------------------------
async function domSnippet(page, selector) {
    try {
        return await page.evaluate((sel) => {
            const els = Array.from(document.querySelectorAll(sel)).slice(0, 3);
            return els.map(e => e.outerHTML.slice(0, 200)).join('\n') || '(none found)';
        }, selector);
    } catch (_) { return '(error)'; }
}

// ---------------------------------------------------------------------------
// Helpers: wait for wrapper count with timeout
// ---------------------------------------------------------------------------
async function waitForWrappers(page, expectedCount, timeoutMs, scenario, description) {
    const deadline = Date.now() + timeoutMs;
    let count = -1;
    while (Date.now() < deadline) {
        count = await page.evaluate(() =>
            document.querySelectorAll('[data-indent-generated="1"]').length
        );
        if (expectedCount > 0 ? count > 0 : count === 0) break;
        await page.waitForTimeout(50);
    }
    return count;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------
(async () => {
    let browser;
    try {
        browser = await chromium.launch({ headless: true });
    } catch (e) {
        console.error(`[fatal] Failed to launch chromium: ${e.message}`);
        process.exit(2);
    }

    const context = await browser.newContext();
    const page    = await context.newPage();

    /* Suppress noisy console messages from the page. */
    page.on('console', msg => {
        if (msg.type() === 'error') {
            console.error(`[page-console-error] ${msg.text()}`);
        }
    });

    try {
        // ===================================================================
        // Scenario 1: baseline-load
        // Validates: no indent wrappers on fresh page load; buttons present;
        //            localStorage not 'on'.
        // ===================================================================
        {
            const S = 'baseline-load';
            console.log('\n=== scenario: baseline-load ===');

            await page.goto(`${BASE_URL}/rich.md`, { waitUntil: 'networkidle' });

            // Clear any leftover localStorage from prior runs
            await page.evaluate(() => localStorage.clear());
            await page.reload({ waitUntil: 'networkidle' });

            /* indent-toggle button present */
            const indentBtn = await page.$('#indent-toggle');
            assert(S, 'indent-toggle button present', 'element #indent-toggle', 'found', indentBtn ? 'found' : 'null', indentBtn !== null);

            /* theme-toggle button present */
            const themeBtn = await page.$('#theme-toggle');
            assert(S, 'theme-toggle button present', 'element #theme-toggle', 'found', themeBtn ? 'found' : 'null', themeBtn !== null);

            /* No generated indent wrappers */
            const wrapperCount = await page.evaluate(() =>
                document.querySelectorAll('[data-indent-generated="1"]').length
            );
            assert(S, 'no indent wrappers on baseline load', 'querySelectorAll([data-indent-generated])', '0', String(wrapperCount), wrapperCount === 0);

            /* localStorage not 'on' */
            const stored = await page.evaluate(() => localStorage.getItem('mdmd-indent-hierarchy'));
            const storedOk = stored === null || stored === 'off';
            assert(S, 'localStorage not "on" on baseline', 'localStorage[mdmd-indent-hierarchy]', 'null or "off"', String(stored), storedOk);

            if (!storedOk || wrapperCount !== 0) {
                await screenshot(page, `${S}-failure`);
            }
        }

        // ===================================================================
        // Scenario 2: toggle-on
        // Validates: clicking toggle adds section wrappers with data-depth,
        //            sets localStorage='on', aria-pressed='true'.
        // ===================================================================
        {
            const S = 'toggle-on';
            console.log('\n=== scenario: toggle-on ===');

            /* Start from a clean baseline on the rich fixture */
            await page.evaluate(() => localStorage.clear());
            await page.reload({ waitUntil: 'networkidle' });

            await page.click('#indent-toggle');

            /* Wait up to 2 s for wrappers to appear */
            const wrapperCount = await waitForWrappers(page, 1, 2000, S, 'wrappers appear after click');
            assert(S, 'indent wrappers appear after toggle ON', 'querySelectorAll([data-indent-generated])', '>0', String(wrapperCount), wrapperCount > 0);

            /* data-depth attributes present */
            const depthCount = await page.evaluate(() =>
                document.querySelectorAll('[data-depth]').length
            );
            assert(S, 'data-depth attributes set on wrappers', 'querySelectorAll([data-depth])', '>0', String(depthCount), depthCount > 0);

            /* Root class present */
            const hasClass = await page.evaluate(() =>
                document.documentElement.classList.contains('indent-hierarchy-on')
            );
            assert(S, 'root class indent-hierarchy-on present', 'documentElement.classList', 'contains indent-hierarchy-on', String(hasClass), hasClass);

            /* localStorage = 'on' */
            const stored = await page.evaluate(() => localStorage.getItem('mdmd-indent-hierarchy'));
            assert(S, 'localStorage set to "on"', 'localStorage[mdmd-indent-hierarchy]', '"on"', String(stored), stored === 'on');

            /* aria-pressed = 'true' */
            const ariaPressedOn = await page.getAttribute('#indent-toggle', 'aria-pressed');
            assert(S, 'aria-pressed="true" after toggle ON', '#indent-toggle[aria-pressed]', '"true"', String(ariaPressedOn), ariaPressedOn === 'true');

            if (wrapperCount === 0 || stored !== 'on') {
                await screenshot(page, `${S}-failure`);
                const snippet = await domSnippet(page, 'main.content > *');
                console.error(`[evidence] main.content children:\n${snippet}`);
            }
        }

        // ===================================================================
        // Scenario 3: toggle-off
        // Validates: clicking toggle again removes wrappers after transition
        //            window (≤ 450 ms), sets localStorage='off',
        //            aria-pressed='false'.
        // ===================================================================
        {
            const S = 'toggle-off';
            console.log('\n=== scenario: toggle-off ===');

            /* State flows from scenario 2: toggle is ON; click again to turn OFF */
            await page.click('#indent-toggle');

            /* Wait for transition (mdmd.js uses 350 ms timeout; poll up to 700 ms) */
            const wrapperCount = await waitForWrappers(page, 0, 700, S, 'wrappers removed after transition');
            assert(S, 'indent wrappers removed after toggle OFF', 'querySelectorAll([data-indent-generated])', '0', String(wrapperCount), wrapperCount === 0);

            /* Root class absent */
            const hasClass = await page.evaluate(() =>
                document.documentElement.classList.contains('indent-hierarchy-on')
            );
            assert(S, 'root class indent-hierarchy-on removed', 'documentElement.classList', 'not contains indent-hierarchy-on', String(!hasClass), !hasClass);

            /* localStorage = 'off' */
            const stored = await page.evaluate(() => localStorage.getItem('mdmd-indent-hierarchy'));
            assert(S, 'localStorage set to "off"', 'localStorage[mdmd-indent-hierarchy]', '"off"', String(stored), stored === 'off');

            /* aria-pressed = 'false' */
            const ariaPressed = await page.getAttribute('#indent-toggle', 'aria-pressed');
            assert(S, 'aria-pressed="false" after toggle OFF', '#indent-toggle[aria-pressed]', '"false"', String(ariaPressed), ariaPressed === 'false');

            if (wrapperCount !== 0 || stored !== 'off') {
                await screenshot(page, `${S}-failure`);
                const snippet = await domSnippet(page, '[data-indent-generated]');
                console.error(`[evidence] remaining generated wrappers:\n${snippet}`);
            }
        }

        // ===================================================================
        // Scenario 4: no-heading
        // Validates: toggle button present on no-heading page, clicking does
        //            not throw, no wrappers generated, localStorage persists.
        // ===================================================================
        {
            const S = 'no-heading';
            console.log('\n=== scenario: no-heading ===');

            await page.goto(`${BASE_URL}/noheading.md`, { waitUntil: 'networkidle' });
            await page.evaluate(() => localStorage.clear());
            await page.reload({ waitUntil: 'networkidle' });

            /* indent-toggle button still present */
            const btn = await page.$('#indent-toggle');
            assert(S, 'indent-toggle button present on no-heading page', 'element #indent-toggle', 'found', btn ? 'found' : 'null', btn !== null);

            /* Click toggle ON — must not throw */
            let clickError = null;
            try {
                await page.click('#indent-toggle');
                await page.waitForTimeout(150);
            } catch (e) {
                clickError = e.message;
            }
            assert(S, 'toggle click ON does not throw on no-heading page', 'click #indent-toggle', 'no error', clickError || 'no error', clickError === null);

            /* No wrappers generated (no headings to wrap) */
            const wrapperCount = await page.evaluate(() =>
                document.querySelectorAll('[data-indent-generated="1"]').length
            );
            assert(S, 'no wrappers generated on no-heading page', 'querySelectorAll([data-indent-generated])', '0', String(wrapperCount), wrapperCount === 0);

            /* localStorage reflects 'on' state */
            const stored = await page.evaluate(() => localStorage.getItem('mdmd-indent-hierarchy'));
            assert(S, 'localStorage="on" persisted on no-heading page', 'localStorage[mdmd-indent-hierarchy]', '"on"', String(stored), stored === 'on');

            /* Toggle OFF and verify localStorage persists */
            await page.click('#indent-toggle');
            await page.waitForTimeout(150);
            const stored2 = await page.evaluate(() => localStorage.getItem('mdmd-indent-hierarchy'));
            assert(S, 'localStorage="off" after OFF on no-heading page', 'localStorage[mdmd-indent-hierarchy]', '"off"', String(stored2), stored2 === 'off');

            /* Button still enabled/interactive */
            const isEnabled = await page.evaluate(() => {
                const b = document.getElementById('indent-toggle');
                return b && !b.disabled;
            });
            assert(S, 'indent-toggle remains interactive after both clicks', '#indent-toggle.disabled', 'false', String(!isEnabled), isEnabled);

            if (clickError || wrapperCount !== 0) {
                await screenshot(page, `${S}-failure`);
            }
        }

        // ===================================================================
        // Scenario 5: toc-scroll
        // Validates: TOC link click scrolls to heading; active class updates
        //            after indent mode changes (rebindHeadingObserver fires).
        // ===================================================================
        {
            const S = 'toc-scroll';
            console.log('\n=== scenario: toc-scroll ===');

            /* Fresh load on rich fixture with indent OFF */
            await page.goto(`${BASE_URL}/rich.md`, { waitUntil: 'networkidle' });
            await page.evaluate(() => localStorage.clear());
            await page.reload({ waitUntil: 'networkidle' });

            /* Confirm TOC sidebar present */
            const tocPresent = await page.$('.toc-sidebar');
            assert(S, 'TOC sidebar present', 'element .toc-sidebar', 'found', tocPresent ? 'found' : 'null', tocPresent !== null);

            /* Count TOC links */
            const tocLinkCount = await page.evaluate(() =>
                document.querySelectorAll('.toc-sidebar a').length
            );
            assert(S, 'TOC sidebar has links', '.toc-sidebar a count', '>0', String(tocLinkCount), tocLinkCount > 0);

            /* Skip remaining toc-scroll assertions when no TOC links */
            if (tocLinkCount > 0) {
                /* Click the last TOC link (heading near bottom — ensures scroll occurs) */
                const lastHref = await page.evaluate(() => {
                    const links = Array.from(document.querySelectorAll('.toc-sidebar a'));
                    return links[links.length - 1].getAttribute('href');
                });
                await page.click(`.toc-sidebar a[href="${lastHref}"]`);

                /* Allow intersection observer to settle */
                await page.waitForTimeout(600);

                /* At least one TOC link must have .active after scroll */
                const activeCount = await page.evaluate(() =>
                    document.querySelectorAll('.toc-sidebar a.active').length
                );
                assert(S, 'TOC active class applied after link click (indent OFF)', '.toc-sidebar a.active count', '>0', String(activeCount), activeCount > 0);

                /* Toggle ON — rebindHeadingObserver must reconnect observer */
                await page.click('#indent-toggle');
                await waitForWrappers(page, 1, 2000, S, 'wrappers appear for toc-scroll test');

                /* Scroll back to top to clear active state, then click a TOC link */
                await page.evaluate(() => window.scrollTo(0, 0));
                await page.waitForTimeout(300);

                /* Click last TOC link again with indent mode ON */
                await page.click(`.toc-sidebar a[href="${lastHref}"]`);
                await page.waitForTimeout(600);

                const activeCountOn = await page.evaluate(() =>
                    document.querySelectorAll('.toc-sidebar a.active').length
                );
                assert(S, 'TOC active class updated after link click (indent ON)', '.toc-sidebar a.active count', '>0', String(activeCountOn), activeCountOn > 0);

                /* Toggle OFF — observer rebind must keep TOC functional */
                await page.click('#indent-toggle');
                await waitForWrappers(page, 0, 700, S, 'wrappers gone for post-OFF toc test');

                /* Scroll back to top */
                await page.evaluate(() => window.scrollTo(0, 0));
                await page.waitForTimeout(300);

                /* Click first TOC link with indent mode OFF */
                const firstHref = await page.evaluate(() => {
                    const links = Array.from(document.querySelectorAll('.toc-sidebar a'));
                    return links[0].getAttribute('href');
                });
                await page.click(`.toc-sidebar a[href="${firstHref}"]`);
                await page.waitForTimeout(600);

                /* Toggle button still interactive after full cycle */
                const isInteractive = await page.evaluate(() => {
                    const b = document.getElementById('indent-toggle');
                    return b && !b.disabled;
                });
                assert(S, 'indent-toggle interactive after full toc-scroll cycle', '#indent-toggle.disabled', 'false', String(!isInteractive), isInteractive);
            }

            if (FAIL > 0) {
                await screenshot(page, `${S}-failure`);
            }
        }

    } catch (fatalErr) {
        console.error(`[fatal] Unhandled error during browser test: ${fatalErr.message}`);
        console.error(fatalErr.stack);
        try { await screenshot(page, 'fatal-error'); } catch (_) {}
        FAIL++;
    } finally {
        await browser.close();
    }

    // -----------------------------------------------------------------------
    // Write structured summary JSON
    // -----------------------------------------------------------------------
    const summary = {
        timestamp: new Date().toISOString(),
        passed: PASS,
        failed: FAIL,
        total: PASS + FAIL,
        scenarios: results
    };
    try {
        mkdirSync(ARTIFACT_DIR, { recursive: true });
        writeFileSync(
            path.join(ARTIFACT_DIR, 'summary.json'),
            JSON.stringify(summary, null, 2) + '\n'
        );
    } catch (e) {
        console.error(`[warn] Could not write summary.json: ${e.message}`);
    }

    console.log(`\n=== browser test summary ===`);
    console.log(`passed=${PASS} failed=${FAIL} total=${PASS + FAIL}`);

    process.exit(FAIL > 0 ? 1 : 0);
})();
PWEOF

# ---------------------------------------------------------------------------
# Run Playwright browser tests
# ---------------------------------------------------------------------------
BASE_URL="http://127.0.0.1:$PORT"
BROWSER_STDOUT="${LOG_DIR}/browser_stdout.log"
BROWSER_STDERR="${LOG_DIR}/browser_stderr.log"

log "running browser tests against $BASE_URL"
log "browser stdout: $BROWSER_STDOUT"
log "browser stderr: $BROWSER_STDERR"

BROWSER_EXIT=0
NODE_PATH="$PW_MODS" "$NODE" "$BROWSER_SCRIPT" "$BASE_URL" "$LOG_DIR" "$PW_MODS" \
    > >(tee "$BROWSER_STDOUT") \
    2> >(tee "$BROWSER_STDERR" >&2) \
    || BROWSER_EXIT=$?

# ---------------------------------------------------------------------------
# Copy server logs to artifact dir
# ---------------------------------------------------------------------------
cp "$TMP/server_stdout.log" "${LOG_DIR}/server_stdout.log" 2>/dev/null || true
cp "$TMP/server_stderr.log" "${LOG_DIR}/server_stderr.log" 2>/dev/null || true

# ---------------------------------------------------------------------------
# Extract pass/fail from summary.json (if written)
# ---------------------------------------------------------------------------
SUMMARY_JSON="${LOG_DIR}/summary.json"
if [[ -f "$SUMMARY_JSON" ]]; then
    PW_PASS=$("$NODE" -e "const s=require('$SUMMARY_JSON');console.log(s.passed)" 2>/dev/null || echo "?")
    PW_FAIL=$("$NODE" -e "const s=require('$SUMMARY_JSON');console.log(s.failed)" 2>/dev/null || echo "?")
else
    PW_PASS="?"
    PW_FAIL="?"
fi

# ---------------------------------------------------------------------------
# Final summary
# ---------------------------------------------------------------------------
echo ""
echo "=== indent-hierarchy e2e summary ==="
echo "artifact dir : $LOG_DIR"
echo "browser exit : $BROWSER_EXIT"
echo "passed       : $PW_PASS"
echo "failed       : $PW_FAIL"

if [[ $BROWSER_EXIT -ne 0 ]]; then
    echo ""
    echo "--- browser stderr (last 40 lines) ---"
    tail -40 "$BROWSER_STDERR" || true
    echo ""
    echo "--- server stderr (last 20 lines) ---"
    tail -20 "${LOG_DIR}/server_stderr.log" || true
fi

exit "$BROWSER_EXIT"
