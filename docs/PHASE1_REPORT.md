# Phase 1 report — core, HTTP, environments, MCP inspector

## What was built

| Area | Where | Notes |
|---|---|---|
| Data model + SQLite store | `crates/core` | Generic `docs` table, typed models (Workspace, Folder, Request, Environment, McpServer, Response, CookieJar, Settings), batched transactional writes with one change event per batch, additive migrations |
| Templating | `crates/templating` | Insomnia precedence (global base → global sub → base → sub → folders → extra layers), self-recursive values, 3-pass cross references, `{{ _.x }}` and `{{ x }}`, tags `uuid now base64 hash timestamp urlencode`, unresolved variables reported by name |
| HTTP engine | `crates/http` | Path/query params, JSON/form/multipart/file/raw bodies, Basic/Bearer/API-key auth, manual redirects (303→GET, 307 keeps body, auth stripped cross-host), RFC 6265 cookie jar incl. intermediate hops, curl-style timeline, timeouts |
| Engine | `crates/engine` | Request pipeline (render + folder header/auth inheritance + send + persist), history pruning, large bodies to disk, bounded concurrent `send_many`, cURL parser, saved-MCP-server resolution |
| MCP client | `crates/mcp` | JSON-RPC over stdio and Streamable HTTP (sessions, SSE, DELETE on close), paginated list calls, tools/call, resources, templates, prompts, ping, roots, cancellation on timeout, protocol log with per-response latency, schema → readable parameter rows, argument validation, example args, mock server binary `lsock-mock-mcp` |
| CLI | `crates/cli` (`lsock`) | `send`, `workspace`, `request` (add/curl/folder/history), `env`, `render`, `mcp info/tools/call/resources/prompts/add` with readable tool cards and `--log` |
| Desktop | `apps/desktop` | Tauri 2 + React. Sidebar tree (drag & drop, rename, duplicate, context menu), tabs, ⌘K palette, environments modal, settings, request editor (variable highlighting + autocomplete, cURL paste, live URL preview), response viewer (JSON tree with copy-path, raw, headers, cookies, timeline, timing), folder settings, MCP Inspector (server card, tool list with badges, parameter tables, generated call form + JSON mode with live validation, destructive-tool confirmation, results with content blocks + structured content, resources, prompts, notifications, protocol log with filters and frame detail) |

## Verification (actual results)

- `cargo fmt --check` + `cargo clippy --workspace --all-targets -D warnings`: clean
- `cargo test --workspace`: 56 passed, 0 failed (core 8, templating 12, http 11, mcp 9, engine 10, cli 2, desktop IPC contract 4)
- Frontend: `tsc --noEmit` clean, `vitest` 5 passed, `vite build` ok
- `cargo tauri build --debug --bundles app`: builds `logic-socket.app`
- Manual E2E: CLI against the mock server over stdio and HTTP; desktop UI driven headlessly through the web bridge with screenshots in light and dark themes (bugs found and fixed: editor height, variable-overlay alignment, duplicate log frames, tab restore race, palette focus, log toolbar overflow)

## Known gaps

- Auth: digest, OAuth 1/2, AWS IAM, Hawk, NTLM, ASAP not implemented (Phase 4); MCP OAuth flow not implemented
- Local argument validation ignores `pattern` (no regex engine); server still validates
- Timing does not split DNS/connect/TLS (D10)
- No proxy or client-certificate settings yet; no GET SSE stream for server-initiated MCP messages (only responses/notifications on POST streams)
- MCP sampling/elicitation requests are answered with "not supported" until Phase 3
- Tree keyboard navigation is basic (click/drag only)

## Phase 2 prerequisites

- Engine pipeline has hook points (`prepare` extra layers, `execute`) for pre/after scripts
- Response history exists for the `{% response %}` tag and `insomnia.response`
- Runner can build on `send_many` + `request_ids_in`
