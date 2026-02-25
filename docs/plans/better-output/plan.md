# Better `mdmd serve` Startup Output

## Overview

`mdmd serve` should default to a minimal startup UX that only prints the rendered entry URL(s), while still allowing full diagnostics via `--verbose`.

Target default stdout output:

- `http://127.0.0.1:3333/README.md`
- `http://<tailscale-hostname>:3333/README.md` (only when Tailscale endpoint detection succeeds)

Scope and intent:

- Default mode prints URLs only to stdout (no extra startup lines).
- Existing ad-hoc diagnostics (`eprintln!`) are suppressed unless verbose mode is enabled.
- Browser auto-open is enabled by default only in headed environments, with explicit opt-out.
- Works on macOS and Linux.

## CLI and API Changes

- Add flags to `serve` command parsing in `main.rs`:
  - `--no-open`: never auto-open browser.
  - `-v, --verbose`: enable diagnostic output.
- Update serve entrypoint function signature to carry these controls:
  - `run_serve(file, bind_addr, start_port, no_open, verbose)`.
- Update the `DispatchMode::Serve` match arm in `main.rs` to pass the two new args.
- Preserve existing defaults except output/auto-open behavior.

Precedence rules:

- `--no-open` always wins, even in a headed environment.
- Verbose flag controls diagnostic logging only; URL lines remain printed in both modes.

## Output Contract

Default mode (`mdmd serve`):

- Print local entry URL to stdout.
- Print tailscale entry URL to stdout only when available.
- Do not print additional startup labels (`mdmd serve`, `root:`, `entry:`, `url:`, `index:`) in default mode.
- Do not print index/root URL line in the new default design.
- Do not print the backlinks startup-indexed reminder line in default mode.

Verbose mode (`mdmd serve --verbose`):

- Keep current diagnostic behavior by allowing existing `eprintln!` paths.
- Continue printing URL lines to stdout.

Safety-critical stderr (always on, regardless of verbose flag):

- Out-of-cwd network-exposure warning (lines ~1344–1347 of `serve.rs`) must remain always-on.
- Fatal error messages (canonicalize failure, invalid entry path, bind failure) must remain always-on.
- The interactive out-of-cwd confirmation prompt must remain always-on.

## Logging Implementation Strategy

Current risk: diagnostics are spread across many `eprintln!` call sites in `serve.rs` and helper paths.

Minimal implementation approach:

- Add `verbose: bool` to `AppState` so all request-handler closures can read it via `Arc<AppState>`.
- Thread `verbose: bool` through every function that currently calls `eprintln!` for diagnostics:
  - `tailscale_dns_name(verbose: bool)` — has two eprintln! call sites.
  - `bind_with_retry(bind_addr, start_port, verbose: bool)` — has three eprintln! call sites; it is `pub fn` used by integration tests so signature change must be reflected in test call sites.
  - All request handlers via `AppState.verbose`.
- Ensure `tailscale_dns_name()` skip/miss diagnostics are also gated (common non-tailscale case must be silent by default).
- Do not introduce a new logging framework in this change; keep existing `eprintln!` style behind gating.
- Safety-critical stderr lines (warnings, fatal errors, interactive prompts) are explicitly excluded from the verbose gate and must not be touched.

## Browser Auto-Open Behavior

Open attempt conditions:

- `!no_open`
- headed environment detection passes
- local URL is known

Headed detection requirements:

- macOS: require a non-SSH interactive session (suppress when `SSH_CONNECTION` or `SSH_TTY` is present).
- Linux: require desktop/session indicators (`DISPLAY` or `WAYLAND_DISPLAY`), and suppress in obvious CI/headless cases.
- Detection is best-effort heuristic; false negatives (suppressing open when a display is available) are acceptable.

Open commands:

- macOS: `open <url>`
- Linux: `xdg-open <url>` (may not be installed on all distros; treat `Err` from spawn as a silent no-op in default mode, or a logged notice in verbose mode)

Execution requirements:

- Spawn browser opener as fire-and-forget (non-blocking) using `.spawn()`, not `.output()`.
- Failure to open must not fail serving; only report failure in verbose mode.

## Tests and Validation

Update integration tests to match new contract:

- Update `ServerHandle::new` in `tests/serve_integration.rs` to pass `--no-open` in all spawned command invocations. This prevents spurious browser opens when developers run the test suite locally on a headed machine.
- Rewrite `test_serve_startup_stdout_format` completely:
  - Remove all assertions for old banner/label lines (`"mdmd serve"`, `root:`, `entry:`, `index:`, `backlinks:`).
  - Assert stdout contains exactly one line matching `http://127.0.0.1:<port>/<path>`.
  - Assert no additional stdout lines except the optional tailscale URL.
- Add/adjust coverage for:
  - `--no-open` flag accepted without error (smoke test).
  - `--verbose` flag accepted without error; stderr non-empty after a request in verbose mode.
  - Default mode keeps stderr quiet in normal non-error startup (assert stderr is empty or contains no `[`-prefixed diagnostic lines).
- Add unit tests in `serve.rs` or a test module:
  - `is_headed_environment()` (pure env-var logic; test with mocked env vars using `std::env::set_var` in isolated tests).
  - `tailscale_dns_name(false)` and `tailscale_dns_name(true)` do not panic when `tailscale` binary is absent.

Validation checklist:

- `mdmd serve` outputs only URL lines to stdout.
- Default port in examples/tests is `3333`.
- Non-tailscale environment does not emit tailscale skip logs unless verbose.
- Browser launch does not block server startup.
- `bind_with_retry` signature change compiles cleanly; no test call sites broken.
- `AppState` with `verbose` field compiles; all `AppState` construction sites updated.
- Safety-critical stderr lines (out-of-cwd warning, fatal errors) still appear in default mode.

## Acceptance Criteria

- Default startup output is exactly one or two entry URLs (`127.0.0.1` always; tailscale only when detected), on stdout.
- No extra startup log output appears without `--verbose`.
- `--verbose` re-enables existing diagnostics.
- `--no-open` prevents browser launch regardless of environment.
- macOS and Linux auto-open works in headed local sessions and is suppressed in SSH/headless contexts.
- Integration tests are updated to enforce the new stdout contract.
- All existing integration tests continue to pass (no regressions).
