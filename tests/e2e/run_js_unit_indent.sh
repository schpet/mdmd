#!/usr/bin/env bash
# run_js_unit_indent.sh — Run the JS unit test suite for the indentation
# hierarchy toggle and write timestamped logs.
#
# Coverage:  tests/js/indent_transform.test.mjs
# Log dir:   target/e2e-logs/indent-hierarchy/unit/<timestamp>/
#
# Usage (from the project root):
#   bash tests/e2e/run_js_unit_indent.sh
#
# Environment overrides:
#   NODE    — path to the node binary (default: node)
#
# Exit code: 0 if all tests pass, non-zero otherwise.

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

NODE="${NODE:-node}"
TEST_FILE="${PROJECT_ROOT}/tests/js/indent_transform.test.mjs"

TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
LOG_DIR="${PROJECT_ROOT}/target/e2e-logs/indent-hierarchy/unit/${TIMESTAMP}"
STDOUT_LOG="${LOG_DIR}/stdout.log"
STDERR_LOG="${LOG_DIR}/stderr.log"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
log()  { echo "[run_js_unit_indent] $*"; }
fail() { echo "[run_js_unit_indent] FAIL: $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Pre-flight checks
# ---------------------------------------------------------------------------
command -v "${NODE}" >/dev/null 2>&1 \
    || fail "node not found (NODE=${NODE}).  Install Node.js 18+ to run JS unit tests."

[[ -f "${TEST_FILE}" ]] \
    || fail "Test file not found: ${TEST_FILE}"

# ---------------------------------------------------------------------------
# Create log directory
# ---------------------------------------------------------------------------
mkdir -p "${LOG_DIR}"
log "Log directory: ${LOG_DIR}"

# ---------------------------------------------------------------------------
# Run tests
# ---------------------------------------------------------------------------
log "Running: ${NODE} --test ${TEST_FILE}"
log "Stdout log: ${STDOUT_LOG}"
log "Stderr log: ${STDERR_LOG}"

EXIT_CODE=0
"${NODE}" --test "${TEST_FILE}" \
    > >(tee "${STDOUT_LOG}")     \
    2> >(tee "${STDERR_LOG}" >&2) \
    || EXIT_CODE=$?

# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------
if [[ ${EXIT_CODE} -eq 0 ]]; then
    log "All tests PASSED.  Logs written to ${LOG_DIR}"
else
    log "One or more tests FAILED (exit ${EXIT_CODE}).  See ${LOG_DIR}"
fi

exit ${EXIT_CODE}
