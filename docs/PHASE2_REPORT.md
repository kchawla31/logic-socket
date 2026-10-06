# Phase 2 report — scripting, collection runner, CLI

## What was built

| Area | Where | Notes |
|---|---|---|
| Script runtime | `crates/scripting` | QuickJS (rquickjs) sandbox; `prelude.js` re-implements Insomnia's `insomnia.*` object model (see PARITY.md "Phase 2 API matrix"); host functions for HTTP (`sendRequest`), templating (`replaceIn`), timers, logging, UUIDs; vendored chai, lodash, crypto-js, moment, tv4, ajv loaded lazily via `require` |
| Sandbox | same | timeout via interrupt handler + outer deadline (default 5 s, `Settings.scriptTimeoutMs`), 64 MB memory limit, no fs/process/network globals, Node modules → `NotSupported`, errors report script line/column, partial results kept |
| Pipeline | `crates/engine/src/pipeline.rs` | folder pre-scripts (outer→inner) → request pre-script → render → send → request after-script → folder after-scripts (inner→outer); env/global/cookie mutations persisted in batched transactions; tests + console stored on the response; `skipRequest`, `setNextRequest` surfaced to the runner |
| Runner | `crates/runner` | iterations, CSV/JSON data (rows cycle), delay, bail, cancel, `setNextRequest` jumps/loops (step guard), `setNextRequest(null)` stop, event stream, reporters spec/dot/json/junit |
| CLI | `lsock run collection` | inso-compatible flags (`-e -i -n -d --delay-request -b -r --env-var`), `--output`, live spec output, exit codes 0 / 1 (failures) / 2 (usage/runtime); `lsock request script` attaches scripts from files |
| Desktop | `apps/desktop` | Scripts tab on requests (pre/after) and folders, CodeMirror JS with `insomnia.` completions (Insomnia's snippet list) + example snippets; response Tests tab (pass/fail/skip, errors), Console tab (level filter, source), script-error banner, test summary pill; Runner tab (request checklist, iterations, delay, bail, CSV/JSON load or paste, live results grouped by iteration, expandable tests/console, JUnit/JSON export to Downloads, stop) |

## Verification (actual results)

- `cargo clippy --workspace --all-targets -D warnings`: clean; `cargo fmt`: applied
- `cargo test --workspace`: **88 passed, 0 failed** — scripting 21 (≥15 ported from `insomnia-scripting-environment` tests: environments, variables precedence, request/url/headers, response assertions, execution, info, console, cookies; plus vendored modules, unsupported modules, error line numbers, infinite loop, never-resolving await, memory exhaustion, sendRequest callback+promise, timers), runner 5 (3 CSV iterations, setNextRequest loop + null stop + unknown target, bail + skip, JUnit parsed by an XML parser, JSON/spec/dot), engine 14 (incl. script order, env chaining, skip, script errors, sendRequest, base-env fallback), CLI exit codes 0/1/2 end-to-end, desktop IPC 5 (incl. runner start/export)
- Frontend: `tsc` clean, vitest 5 passed; `cargo tauri build --debug --bundles app` ok
- Manual E2E: `fixtures/phase2` (MCP over raw JSON-RPC with session capture and data-driven tool calls) → 9 requests, 15/15 tests, JUnit valid; desktop UI driven headlessly (scripts editor, snippets, tests, console, runner with 3 iterations and failures) — screenshots reviewed

## Bugs found by the verification and fixed

- Large vendored libraries overflowed the native stack → dedicated script thread (D15)
- lodash init re-entered the lazy global `_` getter → infinite recursion → `require` re-entrancy guard
- ajv needed a global `console` → exposed the captured console globally
- `insomnia.environment` empty when no sub-env → base fallback (D16)
- Runner events dropped before the run id was known → client-chosen ids (D17)
- CLI reported "no folder" when a request name was ambiguous → clear ambiguity error

## Known gaps

- `insomnia.vault`, client certificates and proxy settings in scripts: NotSupported (Phases 4–5)
- `cheerio`, `xml2js` and Node built-ins are not available in scripts
- `{% response %}` template tag (reference another request's response) not implemented yet
- Runner runs requests sequentially only; no per-run environment picker in the UI (uses the active environment)
- Script completions are prefix-based from Insomnia's snippet list (no type-aware IntelliSense)

## Phase 3 prerequisites

- Script host already brokers HTTP for sandboxed code; the same pattern can expose LLM calls to scripts
- MCP client answers `sampling/createMessage` / `elicitation/create` with "not supported" — Phase 3 wires them to the LLM provider layer
