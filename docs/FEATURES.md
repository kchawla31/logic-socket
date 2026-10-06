# Logic Socket feature matrix

Where each capability lives in the code, and the phase that delivered it. Phase: 1 core/HTTP/env/MCP inspector · 2 scripting/runner/CLI · 3 LLM · 4 other protocols · 5 import/export/sync/vault.

## Data model

| Capability | Rust target | Phase |
|---|---|---|
| Base doc (`_id`, `type`, `parentId`, `created`, `modified`, `name`) | core::model::Meta | 1 |
| Workspace (scope collection/design/mcp/...) | core::model::Workspace | 1 |
| Folder (RequestGroup) + folder env, headers, auth, scripts | core::model::Folder | 1 (scripts 2) |
| HTTP Request (url, method, params, path params, headers, body, auth, settings) | core::model::Request | 1 |
| Environment (base + sub, JSON / kv-pair, secret type) | core::model::Environment | 1 (secret vault 5 ✅) |
| Global environments (environment workspaces) | core::model::Environment w/ global flag | 1 |
| Cookie jar | core::model::CookieJar + http::cookies | 1 |
| Response (status, headers, timings, body, test results) | core::model::Response | 1 |
| Request history / versions | core::model::Response.request_snapshot | 2 |
| MCP request (url, transport stdio/streamable-http, headers, env, roots) | core::model::McpServer | 1 |
| MCP response / event log | mcp::log (in-memory + persisted JSONL) | 1 |
| Runner test results | core::model::RunResult | 2 |
| Unit test suites | covered by runner + scripts | 2 |
| gRPC request / proto files | core::model::GrpcRequest, ProtoFile | 4 ✅ |
| WebSocket / Socket.IO requests & payloads | core::model::RealtimeRequest (websocket / sse / socketio) | 4 ✅ |
| API spec (design docs) | spec text kept on import only | not modelled yet |
| Mock servers / routes | deferred (post-5) | — |
| Git repository / credentials | core::model::GitRepo + engine::git | 5 ✅ |
| Client / CA certificates | http::tls | deferred (D29) |
| Settings | core::model::Settings | 1 |
| Organization / Project / cloud sync / user session | local-only projects in 1; cloud out of scope | 1 / — |

## Request execution & templating

| Capability | Rust target | Phase |
|---|---|---|
| Render context precedence (global base → global sub → base → sub → folders outer→inner → iteration data → local vars) | templating::Context | 1 |
| Self-recursive vars (`base_url: '{{ base_url }}/x'`) | templating::Context::layer | 1 |
| `{{ _.var }}` + bare `{{ var }}` Nunjucks syntax | templating (minijinja) | 1 |
| Template tags (`{% uuid %}`, `{% now %}`, `{% base64 %}`, `{% hash %}`, `{% response %}`) | templating::tags | 1 (response tag 2) |
| Path params `/:id` | http::url | 1 |
| Query params, URL encoding settings | http::build | 1 |
| Body: JSON, raw, form-urlencoded, multipart, file, GraphQL | http::body | 1 (GraphQL UI 4 ✅) |
| Auth: none, basic, bearer, API key | http::auth | 1 |
| Auth: digest, OAuth 1, OAuth 2 (all grants), AWS IAM, Hawk, NTLM, ASAP, netrc | http::sign + engine::oauth2 | 4 ✅ digest, OAuth 1, OAuth 2 (client credentials, password, code + PKCE, refresh), AWS SigV4, netrc · ❌ implicit grant, Hawk, NTLM, ASAP |
| Folder-inherited headers & auth | engine::prepare | 1 |
| Cookies (store/send settings) | http::cookies | 1 |
| Redirects (global/on/off), timeout, TLS validation, proxy | http::Options | 1 (proxy 4 ✅) |
| Timing breakdown (DNS, connect, TLS, TTFB, download) | http::timing | 1 |
| Timeline (verbose request/response log) | http::timeline | 1 |
| SSE / event-stream requests | realtime::sse | 4 ✅ |
| Concurrent requests | engine::send_many (bounded) | 1 |

## MCP

| Capability | Rust target | Phase |
|---|---|---|
| Transports: stdio, Streamable HTTP (+ session id) | mcp::transport | 1 |
| initialize / capabilities / serverInfo | mcp::Client::connect | 1 |
| tools/list (paginated), tools/call | mcp::Client | 1 |
| mcp.ts | mcp::Client | 1 |
| mcp.ts | mcp::Client | 1 |
| ui/components/mcp/mcp-roots-panel.tsx, mcp-notification-tab.tsx | mcp::Client + UI | 1 (subscriptions 3) |
| ui/components/mcp/elicitation-form.tsx, sampling-form.tsx | mcp::server_requests + UI | 3 (needs LLM) |
| request.ts AuthTypeOAuth2 | engine::oauth2 | 4 (OAuth 2 for HTTP MCP servers not wired yet) |
| ui/components/mcp/event-view.tsx | mcp::log + Protocol Log panel | 1 |
| desktop MCP Inspector | 1 |

## Scripting, runner, CLI

| Capability | Rust target | Phase |
|---|---|---|
| `ls.*` script API (environment, variables, request, response, cookies, sendRequest, test, expect, execution) | scripting (rquickjs) | 2 |
| Pre-request / after-response on request + folders | engine::pipeline | 2 |
| Collection runner (iterations, delay, data file, bail, flow control) | runner | 2 |
| `inso run collection` CLI + reporters | cli `lsock run collection` | 2 |
| `inso lint spec` / `export spec` | — | 5 |

## AI / LLM

| Capability | Rust target | Phase |
|---|---|---|
| AI settings (providers, keys) | core::LlmProvider + keychain | 3 ✅ |
| LLM request type (chat, streaming, tool calls) | core::LlmRequest/LlmRun + LlmView | 3 ✅ |
| LLM ↔ MCP tool-calling bridge | llm::agent (approvals) | 3 ✅ |
| MCP sampling responses via LLM | mcp::SamplingHandler + llm::agent::LlmSampler | 3 ✅ |

## Import / export / sync

| Capability | Rust target | Phase |
|---|---|---|
| Logic Socket YAML (exports and Git sync) | convert::native | 5 ✅ |
| Insomnia files (v5 import + export, v4/v3 import) | convert::insomnia | 5 ✅ |
| Postman, OpenAPI, HAR, cURL importers | convert::{postman,openapi,har,curl} | 5 ✅ (WSDL ✗) |
| Git sync | engine::git (system git) | 5 ✅ |
| Vault secrets (AES-GCM) | engine::vault | 5 ✅ |

## Script API matrix (`ls.*`)


| Member | Status | Notes |
|---|---|---|
| `ls` / `$` / `pm` aliases | implement | `pm` alias eases Postman script reuse |
| `environment`, `baseEnvironment`, `collectionVariables` (= base), `globals`, `iterationData` | implement | `name, has, get, set, unset, clear, replaceIn, toObject`; mutations persisted after the run (iterationData read-only to storage) |
| `variables` | implement | `has, get, set, replaceIn, toObject`; precedence local → iterationData → folder → environment → base → globals; `set` writes script-local vars (highest render precedence) |
| `vault` | NotSupported | secrets are ordinary variables in scripts; `ls.vault` not yet |
| `request.url` (string or Url: `toString, getHost, getPath, getQueryString, query.add/upsert/remove/get/toObject, addQueryParams`) | implement | query maps to request parameters |
| `request.method`, `request.name`, `request.id` | implement | |
| `request.headers` (`add, upsert, remove, get, has, each, toObject, all`), `request.addHeader/removeHeader/upsertHeader` | implement | |
| `request.body` (`mode` raw/urlencoded/formdata/file/graphql, `raw`, `urlencoded`, `formdata`, `update()`) | implement | maps to body mime types |
| `request.auth` (`type, update, parameters().get`) | implement | none/inherit/basic/bearer/apikey |
| `request.certificate`, `request.proxy` | NotSupported | proxy via Settings (4); certificates deferred |
| `response` (`code, status, headers, body, text(), json(), responseTime, size(), reason(), cookies`) | implement | after-response only |
| `response.to.have.status/header/body/jsonBody`, `response.to.be.ok/success/error/json` | implement | chai plugin |
| `cookies` (request URL cookies: `get, has, toObject`), `cookies.jar()` (`set, get, getAll, unset, clear`, callback style) | implement | jar changes persisted |
| `info` (`eventName, iteration, iterationCount, requestName, requestId`) | implement | |
| `execution.skipRequest()`, `execution.setNextRequest()`, `execution.location` | implement | runner honors flow control |
| `test(name, fn)` (sync/async), `test.skip` | implement | results: name, passed/failed/skipped, error, duration |
| `expect` | implement | chai 4 `expect` |
| `sendRequest(urlOrRequest, cb)` (callback + Promise) | implement | via Rust HTTP engine, honors timeout |
| `settings`, `clientCertificates`, `parentFolders` | partial / NotSupported | `parentFolders.get(name).environment` read-only; certificates Phase 4 |
| `console.log/info/warn/error/debug` | implement | captured with level + timestamp |
| `setTimeout/clearTimeout`, `queueMicrotask` | implement | host timers; `setImmediate` NotSupported |
| `eval` | implement | available, runs inside the same sandbox (no host access) |

| `require(...)` module | Status | How |
|---|---|---|
| `chai` | implement | vendored chai 4 UMD (embedded) |
| `lodash` (+ global `_`) | implement | vendored lodash 4 |
| `uuid` | implement | small JS shim (`v4`, `validate`, `NIL`) over host randomness |
| `crypto-js` | implement | vendored UMD |
| `moment` | implement | vendored UMD |
| `tv4` | implement | vendored UMD |
| `ajv` | implement | bundled with esbuild to one IIFE |
| `atob`, `btoa` | implement | host base64 |
| `cheerio`, `xml2js` | NotSupported | depend on Node streams/events; clear error |
| `path, url, querystring, util, buffer, events, stream, assert, timers, punycode, string_decoder` | NotSupported | Node built-ins (no Node runtime in the sandbox) |

Sandbox: no filesystem/process/network globals; network only via `sendRequest`; per-script timeout (default 5 s) via interrupt handler; memory limit (default 64 MB) via QuickJS allocator limit.
