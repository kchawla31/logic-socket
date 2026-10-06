# Phase 4 report — protocols and auth

## What was built

| Area | Where | Notes |
|---|---|---|
| WebSocket | `crates/realtime/src/ws.rs` | tokio-tungstenite with rustls; headers, subprotocols, text/binary/ping/pong, close reasons, handshake timing; event log with direction, size and timestamps |
| Server-Sent Events | `crates/realtime/src/sse.rs` | Spec-compliant parser (multi-line `data`, `event`, `id`, `retry`, comments, CRLF); GET or POST with a body |
| Socket.IO | `crates/realtime/src/socketio.rs` | v4 over engine.io WebSocket: namespaces, connect `auth`, emit with JSON args, acks, ping/pong, server disconnect reasons |
| GraphQL | `apps/desktop` GraphQLEditor, `engine::graphql_query` | Query + variables editors, introspection through the normal request pipeline (auth, env, proxy), schema explorer, completion and validation (`cm6-graphql`), operation picker |
| gRPC | `crates/grpc` | Schema from server reflection (v1, falling back to v1alpha) or from `.proto` files compiled in-process (protox) with file:line:col errors; unary, server, client and bidi streaming through a dynamic codec; metadata, deadlines, TLS (`grpcs://`); JSON examples generated per method |
| Auth | `crates/http/src/sign.rs`, `crates/engine/src/oauth2.rs` | Digest (MD5/SHA-256, qop auth, 401 challenge retry), OAuth 1.0a (HMAC-SHA1/256, PLAINTEXT; header/query/body), AWS SigV4 (incl. session token, service/region), OAuth 2 (client credentials, password, authorization code with PKCE via a localhost redirect catcher, refresh tokens; tokens cached per request/folder), netrc |
| Proxy | `crates/http`, Settings | Proxy URL + no-proxy list in Settings; system env proxies are used when unset |
| CLI | `lsock rt`, `lsock grpc`, `lsock mock` | Realtime connect/send/emit; gRPC list/unary/streaming with `--proto`; local demo servers (`mock realtime / graphql / grpc / llm / mcp`) |
| Desktop | `apps/desktop` | Single "New…" menu; Realtime view (connect panel, message composer, filterable event log); gRPC view (method picker, proto files manager, Start/Send/Commit/Cancel, streamed responses, status + trailers); auth editor covering every auth kind with an OAuth 2 token panel |

## Verification (actual results)

- `cargo clippy --workspace --all-targets -D warnings`: clean
- `cargo test --workspace`: **129 passed, 0 failed** (Phase 3: 102). New tests:
  - realtime (6): WebSocket echo/binary/close, SSE parsing and streaming, Socket.IO namespaces, emits and acks
  - grpc (4): all four call kinds against a dynamic demo server, reflection vs proto files, proto compile errors
  - http (+3): Digest against a verifying server, OAuth 1 RFC/Twitter vectors, SigV4 headers, proxy and bypass
  - engine (+): OAuth 2 against a mock auth server (client credentials, password, PKCE code flow, refresh, cache/clear), netrc, script round-trip keeping OAuth2/IAM config
  - desktop IPC (+): realtime and gRPC commands, and a guard that every emitted event reaches the web bridge
- `tsc --noEmit` and `vitest`: clean
- CLI end-to-end against `lsock mock realtime|graphql|grpc`
- Desktop UI driven headlessly through the web bridge (GraphQL explorer and completion, WebSocket/SSE/Socket.IO logs, gRPC unary + server/bidi streaming, OAuth 2 editor). I reviewed the screenshots.

## Bugs found and fixed during verification

- The web bridge didn't forward `rt-event`/`grpc-event` → one shared `APP_EVENTS` list plus a test (desktop-only issue in dev mode; the Tauri app was unaffected)
- Editing a request in a script reset OAuth 2 / AWS IAM auth to "inherit" → scripts keep the raw config they don't model
- A field named `body` clashed with the flattened `Doc.body` → gRPC field renamed `message`
- When the method changed, the gRPC message kept the old method's example → it's replaced when untouched
- The MCP timeout test was flaky under full-suite load → wider margin

## Known gaps

- Client certificates / custom CA (`http::tls`) are not implemented
- NTLM, Hawk and ASAP auth; the OAuth 2 implicit grant
- GraphQL subscriptions (over WebSocket)
- gRPC binary (`-bin`) metadata values are shown as `<binary>` and can't be sent
- Socket.IO binary attachments; no auto-reconnect for realtime connections
- Realtime and gRPC sessions are not saved as response history and aren't part of collection runs
