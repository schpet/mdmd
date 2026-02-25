#!/usr/bin/env bash
# E2E startup matrix for mdmd serve (bd-3bl / bd-3bl.1)
#
# Exercises four startup scenarios and validates the output/logging contract:
#
#   default    — default startup, URL-only stdout, quiet stderr
#   verbose    — --verbose startup, URL on stdout plus diagnostics on stderr
#   no-open    — --no-open with headed-like env, no [browser] diagnostic
#   headless   — CI=1 / no DISPLAY / no WAYLAND_DISPLAY, no [browser] diagnostic
#
# Per-scenario logs are written to:
#   target/e2e-logs/serve-startup/<timestamp>/<scenario>_{stdout,stderr}.log
#
# Usage:
#   # From the project root (binary already built):
#   BINARY=./target/debug/mdmd bash tests/e2e/serve_startup_matrix.sh
#
#   # Auto-build then run:
#   bash tests/e2e/serve_startup_matrix.sh
#
# Environment overrides:
#   BINARY          — path to the mdmd binary (default: auto-build debug binary)
#   MDMD_OPEN_CMD   — injected into each server child to stub the browser opener
#                     (default: __mdmd_no_such_open_cmd__, a deterministic failing stub)
#
# Exit code: 0 = all assertions passed, non-zero = at least one assertion failed.

set -euo pipefail

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
PASS=0
FAIL=0
CURRENT_SCENARIO=""

scenario_header() {
    local name="$1"
    CURRENT_SCENARIO="$name"
    echo ""
    echo "============================================================"
    echo "  SCENARIO: $name"
    echo "============================================================"
}

log_pass() {
    echo "PASS  [$CURRENT_SCENARIO]  $*"
    PASS=$((PASS + 1))
}

log_fail() {
    echo "FAIL  [$CURRENT_SCENARIO]  $*" >&2
    FAIL=$((FAIL + 1))
}

assert_stdout_contains_url() {
    local stdout="$1" port="$2"
    local url="http://127.0.0.1:${port}/"
    if echo "$stdout" | grep -qF "$url"; then
        log_pass "stdout contains bare local URL $url"
    else
        log_fail "stdout must contain bare local URL $url | stdout=$stdout"
    fi
}

assert_stdout_url_only() {
    local stdout="$1"
    local -a forbidden=("[serve]" "[bind]" "[browser]" "[tailscale]")
    for prefix in "${forbidden[@]}"; do
        if echo "$stdout" | grep -qF "$prefix"; then
            log_fail "stdout must not contain diagnostic prefix '$prefix' | stdout=$stdout"
        else
            log_pass "stdout does not contain forbidden prefix '$prefix'"
        fi
    done
}

assert_stderr_quiet() {
    local stderr="$1"
    local -a categories=("[serve]" "[bind]" "[tailscale]" "[browser]")
    for cat in "${categories[@]}"; do
        if echo "$stderr" | grep -qF "$cat"; then
            log_fail "default mode: stderr must not contain $cat | stderr=$stderr"
        else
            log_pass "default mode: stderr does not contain $cat (quiet)"
        fi
    done
}

assert_stderr_has() {
    local stderr="$1" marker="$2"
    if echo "$stderr" | grep -qF "$marker"; then
        log_pass "stderr contains '$marker'"
    else
        log_fail "stderr must contain '$marker' | stderr=$stderr"
    fi
}

assert_stderr_absent() {
    local stderr="$1" marker="$2"
    if echo "$stderr" | grep -qF "$marker"; then
        log_fail "stderr must NOT contain '$marker' | stderr=$stderr"
    else
        log_pass "stderr does not contain '$marker'"
    fi
}

# ---------------------------------------------------------------------------
# Binary resolution
# ---------------------------------------------------------------------------
if [[ -z "${BINARY:-}" ]]; then
    echo "[matrix] BINARY not set — building mdmd..."
    cargo build --bin mdmd --quiet
    BINARY=./target/debug/mdmd
fi

if [[ ! -x "$BINARY" ]]; then
    echo "error: binary not executable: $BINARY" >&2
    exit 1
fi

echo "[matrix] binary=$BINARY"

# Deterministic browser-open stub: any open attempt will fail with a predictable
# error, causing the server to log [browser] open failed: (if verbose).
STUB_OPEN_CMD="${MDMD_OPEN_CMD:-__mdmd_no_such_open_cmd__}"

# ---------------------------------------------------------------------------
# Per-run log directory
# ---------------------------------------------------------------------------
TIMESTAMP=$(date +%Y%m%dT%H%M%S)
LOG_DIR="target/e2e-logs/serve-startup/${TIMESTAMP}"
mkdir -p "$LOG_DIR"
echo "[matrix] logs directory: $LOG_DIR"

# ---------------------------------------------------------------------------
# Temporary fixture
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

mkdir -p "$TMP"
echo "# Home" > "$TMP/README.md"
echo "# Guide" > "$TMP/guide.md"
ENTRY="$TMP/README.md"

# ---------------------------------------------------------------------------
# Helper: allocate a free TCP port
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
# Helper: start server, wait for readiness, capture output on shutdown
#
#   run_scenario <scenario_name> <port> <extra_args_csv> <env_overrides_csv> <env_remove_csv>
#
# env_overrides_csv: comma-separated KEY=VALUE pairs (no spaces around =)
# env_remove_csv:    comma-separated variable names to unset
#
# Writes <scenario>_stdout.log and <scenario>_stderr.log under LOG_DIR.
# Sets globals: SCENARIO_STDOUT, SCENARIO_STDERR
# ---------------------------------------------------------------------------
SCENARIO_STDOUT=""
SCENARIO_STDERR=""

run_scenario() {
    local name="$1"
    local port="$2"
    local extra_args_csv="$3"      # e.g. "--verbose,--no-open"
    local env_overrides_csv="$4"   # e.g. "CI=1,DISPLAY=:99"
    local env_remove_csv="$5"      # e.g. "CI,GITHUB_ACTIONS"

    local stdout_log="$LOG_DIR/${name}_stdout.log"
    local stderr_log="$LOG_DIR/${name}_stderr.log"

    # Build env prefix for display.
    local env_display=""
    [[ -n "$env_overrides_csv" ]] && env_display="env_overrides=$env_overrides_csv "
    [[ -n "$env_remove_csv"    ]] && env_display="${env_display}env_remove=$env_remove_csv "

    local args_display=""
    [[ -n "$extra_args_csv" ]] && args_display="args=$extra_args_csv "

    echo ""
    echo "--- scenario=$name port=$port ${args_display}${env_display}---"
    echo "    cmd: $BINARY serve --bind 127.0.0.1 --port $port $extra_args_csv $ENTRY"

    # Build the env array for env(1).
    local -a env_cmd=("env")
    # Inject stub opener.
    env_cmd+=("MDMD_OPEN_CMD=$STUB_OPEN_CMD")
    # Remove requested vars (env -u requires GNU env, use unset workaround).
    # We'll use a subshell with exports instead.

    # Build the command in a subshell string for eval.
    # Split CSV args into array.
    local -a extra_arr=()
    if [[ -n "$extra_args_csv" ]]; then
        IFS=',' read -ra extra_arr <<< "$extra_args_csv"
    fi

    # Apply env overrides and removals via subshell.
    (
        # Unset removed vars.
        if [[ -n "$env_remove_csv" ]]; then
            IFS=',' read -ra remove_arr <<< "$env_remove_csv"
            for v in "${remove_arr[@]}"; do
                unset "$v" 2>/dev/null || true
            done
        fi

        # Set override vars.
        if [[ -n "$env_overrides_csv" ]]; then
            IFS=',' read -ra override_arr <<< "$env_overrides_csv"
            for kv in "${override_arr[@]}"; do
                kv_key="${kv%%=*}"
                kv_val="${kv#*=}"
                export "$kv_key"="$kv_val"
            done
        fi

        # Always inject stub opener.
        export MDMD_OPEN_CMD="$STUB_OPEN_CMD"

        exec "$BINARY" serve \
            --bind 127.0.0.1 \
            --port "$port" \
            "${extra_arr[@]}" \
            "$ENTRY"
    ) > "$stdout_log" 2> "$stderr_log" &
    SERVER_PID=$!

    echo "    pid=$SERVER_PID"

    # Wait for readiness.
    local ready=0
    for _i in $(seq 1 60); do
        if curl -sf --max-time 1 "http://127.0.0.1:${port}/" > /dev/null 2>&1; then
            ready=1
            break
        fi
        sleep 0.15
    done

    if [[ $ready -eq 0 ]]; then
        echo "error: scenario=$name server did not become ready" >&2
        echo "--- stdout ---" >&2
        cat "$stdout_log" >&2 || true
        echo "--- stderr ---" >&2
        cat "$stderr_log" >&2 || true
        # Kill and reset so cleanup doesn't double-kill.
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
        SERVER_PID=""
        log_fail "server failed to start"
        return
    fi

    # Make one request to ensure the server processed at least one event.
    curl -sf --max-time 5 "http://127.0.0.1:${port}/" > /dev/null 2>&1 || true

    # Shut down gracefully with SIGINT.
    kill -INT "$SERVER_PID" 2>/dev/null || true
    local waited=0
    while kill -0 "$SERVER_PID" 2>/dev/null; do
        sleep 0.1
        waited=$((waited + 1))
        if [[ $waited -gt 50 ]]; then
            kill -KILL "$SERVER_PID" 2>/dev/null || true
            break
        fi
    done
    wait "$SERVER_PID" 2>/dev/null || true
    SERVER_PID=""

    SCENARIO_STDOUT=$(cat "$stdout_log")
    SCENARIO_STDERR=$(cat "$stderr_log")

    echo "    stdout lines=$(echo "$SCENARIO_STDOUT" | wc -l | tr -d ' ')"
    echo "    stderr lines=$(echo "$SCENARIO_STDERR" | wc -l | tr -d ' ')"
    echo "    logs: stdout=$stdout_log stderr=$stderr_log"
}

# ===========================================================================
# Scenario 1: default startup
# URL-only stdout, quiet stderr.
# ===========================================================================
scenario_header "default"
PORT=$(free_port)
run_scenario "default" "$PORT" "" "" "CI,GITHUB_ACTIONS,SSH_CONNECTION,SSH_TTY"

assert_stdout_contains_url "$SCENARIO_STDOUT" "$PORT"
assert_stdout_url_only "$SCENARIO_STDOUT"
assert_stderr_quiet "$SCENARIO_STDERR"

# ===========================================================================
# Scenario 2: verbose startup
# URL retained on stdout, startup diagnostics appear on stderr.
# ===========================================================================
scenario_header "verbose"
PORT=$(free_port)
run_scenario "verbose" "$PORT" "--verbose" "" "CI,GITHUB_ACTIONS,SSH_CONNECTION,SSH_TTY"

assert_stdout_contains_url "$SCENARIO_STDOUT" "$PORT"
assert_stdout_url_only "$SCENARIO_STDOUT"
assert_stderr_has "$SCENARIO_STDERR" "[serve]"
assert_stderr_has "$SCENARIO_STDERR" "[bind]"

# Tailscale: either a [tailscale] diagnostic on stderr (absent) or a second URL line on stdout (present).
STDOUT_LINES=$(echo "$SCENARIO_STDOUT" | wc -l | tr -d ' ')
if echo "$SCENARIO_STDERR" | grep -qF "[tailscale]" || [[ "$STDOUT_LINES" -ge 2 ]]; then
    log_pass "tailscale state visible: [tailscale] on stderr or second URL on stdout"
else
    log_fail "--verbose must report tailscale state via [tailscale] on stderr or a second URL on stdout"
fi

# ===========================================================================
# Scenario 3: --no-open with headed-like environment
# No [browser] diagnostic should appear even with DISPLAY=:99.
# ===========================================================================
scenario_header "no-open"
PORT=$(free_port)
run_scenario "no-open" "$PORT" "--no-open,--verbose" "DISPLAY=:99" "CI,GITHUB_ACTIONS,SSH_CONNECTION,SSH_TTY"

assert_stdout_contains_url "$SCENARIO_STDOUT" "$PORT"
assert_stderr_absent "$SCENARIO_STDERR" "[browser]"
log_pass "--no-open prevents [browser] diagnostics even in headed-like env"

# ===========================================================================
# Scenario 4: headless CI environment (CI=1, no DISPLAY, no WAYLAND_DISPLAY)
# No [browser] diagnostic even without --no-open.
# ===========================================================================
scenario_header "headless"
PORT=$(free_port)
run_scenario "headless" "$PORT" "--verbose" "CI=1" "DISPLAY,WAYLAND_DISPLAY,GITHUB_ACTIONS,SSH_CONNECTION,SSH_TTY"

assert_stdout_contains_url "$SCENARIO_STDOUT" "$PORT"
assert_stderr_absent "$SCENARIO_STDERR" "[browser]"
log_pass "headless CI env suppresses [browser] diagnostics without --no-open"

# ===========================================================================
# Summary
# ===========================================================================
echo ""
echo "============================================================"
echo "  SUMMARY"
echo "============================================================"
echo "  passed=$PASS  failed=$FAIL"
echo "  log dir: $LOG_DIR"
echo "============================================================"

if [[ $FAIL -gt 0 ]]; then
    echo ""
    echo "One or more assertions failed. Check $LOG_DIR for per-scenario logs." >&2
    exit 1
fi

exit 0
