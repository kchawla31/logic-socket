# Parity map: Insomnia (TypeScript) → insomnia-rs

Source paths are relative to `packages/`. Phase: 1 core/HTTP/env/MCP inspector · 2 scripting/runner/CLI · 3 LLM · 4 other protocols · 5 import/export/sync/vault.

## Data model

| Capability | Insomnia source | Rust target | Phase |
|---|---|---|---|
| Base doc (`_id`, `type`, `parentId`, `created`, `modified`, `name`) | insomnia-data/src/models/base-types.ts | core::model::Meta | 1 |
| Workspace (scope collection/design/mcp/...) | models/workspace.ts | core::model::Workspace | 1 |
| Folder (RequestGroup) + folder env, headers, auth, scripts | models/request-group.ts | core::model::Folder | 1 (scripts 2) |
| HTTP Request (url, method, params, path params, headers, body, auth, settings) | models/request.ts | core::model::Request | 1 |
| Environment (base + sub, JSON / kv-pair, secret type) | models/environment.ts | core::model::Environment | 1 (secret vault 5 ✅) |
| Global environments (environment workspaces) | models/workspace.ts scope `environment` | core::model::Environment w/ global flag | 1 |
| Cookie jar | models/cookie-jar.ts | core::model::CookieJar + http::cookies | 1 |
| Response (status, headers, timings, body, test results) | models/response.ts | core::model::Response | 1 |
| Request history / versions | models/request-version.ts | core::model::Response.request_snapshot | 2 |
| MCP request (url, transport stdio/streamable-http, headers, env, roots) | models/mcp-request.ts | core::model::McpServer | 1 |
| MCP response / event log | models/mcp-response.ts | mcp::log (in-memory + persisted JSONL) | 1 |
| Runner test results | models/runner-test-result.ts | core::model::RunResult | 2 |
| Unit test suites | models/unit-test*.ts | covered by runner + scripts | 2 |
| gRPC request / proto files | models/grpc-request.ts, proto-*.ts | core::model::GrpcRequest, ProtoFile | 4 ✅ |
| WebSocket / Socket.IO requests & payloads | models/websocket-*.ts, socket-io-*.ts | core::model::RealtimeRequest (websocket / sse / socketio) | 4 ✅ |
| API spec (design docs) | models/api-spec.ts | spec text kept on import only | not modelled yet |
| Mock servers / routes | models/mock-*.ts | deferred (post-5) | — |
| Git repository / credentials | models/git-*.ts, insomnia-vcs | core::model::GitRepo + engine::git | 5 ✅ |
| Client / CA certificates | models/client-certificate.ts, ca-certificate.ts | http::tls | deferred (D29) |
| Settings | models/settings.ts | core::model::Settings | 1 |
| Organization / Project / cloud sync / user session | models/organization.ts, project.ts | local-only projects in 1; cloud out of scope | 1 / — |

## Request execution & templating

| Capability | Insomnia source | Rust target | Phase |
|---|---|---|---|
| Render context precedence (global base → global sub → base → sub → folders outer→inner → iteration data → local vars) | insomnia/src/common/render.ts `buildRenderContext` | templating::Context | 1 |
| Self-recursive vars (`base_url: '{{ base_url }}/x'`) | render.ts `renderSubContext` | templating::Context::layer | 1 |
| `{{ _.var }}` + bare `{{ var }}` Nunjucks syntax | insomnia/src/templating | templating (minijinja) | 1 |
| Template tags (`{% uuid %}`, `{% now %}`, `{% base64 %}`, `{% hash %}`, `{% response %}`) | templating/ + plugins | templating::tags | 1 (response tag 2) |
| Path params `/:id` | models/request.ts `applyPathParametersToUrl` | http::url | 1 |
| Query params, URL encoding settings | network/network.ts | http::build | 1 |
| Body: JSON, raw, form-urlencoded, multipart, file, GraphQL | network/network.ts, main/network/multipart.ts | http::body | 1 (GraphQL UI 4 ✅) |
| Auth: none, basic, bearer, API key | main/network/get-auth-header.ts | http::auth | 1 |
| Auth: digest, OAuth 1, OAuth 2 (all grants), AWS IAM, Hawk, NTLM, ASAP, netrc | get-auth-header.ts, o-auth-1/, o-auth-2/ | http::sign + engine::oauth2 | 4 ✅ digest, OAuth 1, OAuth 2 (client credentials, password, code + PKCE, refresh), AWS SigV4, netrc · ❌ implicit grant, Hawk, NTLM, ASAP |
| Folder-inherited headers & auth | network/network.ts | engine::prepare | 1 |
| Cookies (store/send settings) | network/network.ts, cookie-jar | http::cookies | 1 |
| Redirects (global/on/off), timeout, TLS validation, proxy | main/network/libcurl-promise.ts | http::Options | 1 (proxy 4 ✅) |
| Timing breakdown (DNS, connect, TLS, TTFB, download) | main/network/request-timing.ts | http::timing | 1 |
| Timeline (verbose request/response log) | libcurl-promise.ts debug | http::timeline | 1 |
| SSE / event-stream requests | models/request.ts `isEventStreamRequest` | realtime::sse | 4 ✅ |
| Concurrent requests | — (single send in UI) | engine::send_many (bounded) | 1 |

## MCP

| Capability | Insomnia source | Rust target | Phase |
|---|---|---|---|
| Transports: stdio, Streamable HTTP (+ session id) | insomnia/src/main/network/mcp.ts | mcp::transport | 1 |
| initialize / capabilities / serverInfo | mcp.ts | mcp::Client::connect | 1 |
| tools/list (paginated), tools/call | mcp.ts | mcp::Client | 1 |
| resources/list, resources/templates/list, resources/read | mcp.ts | mcp::Client | 1 |
| prompts/list, prompts/get | mcp.ts | mcp::Client | 1 |
| Roots, resource subscriptions, notifications tab | ui/components/mcp/mcp-roots-panel.tsx, mcp-notification-tab.tsx | mcp::Client + UI | 1 (subscriptions 3) |
| Elicitation form, sampling form (server → client requests) | ui/components/mcp/elicitation-form.tsx, sampling-form.tsx | mcp::server_requests + UI | 3 (needs LLM) |
| MCP auth (OAuth `mcp_auth_flow`) | request.ts AuthTypeOAuth2 | engine::oauth2 | 4 (OAuth 2 for HTTP MCP servers not wired yet) |
| Event log / timeline | ui/components/mcp/event-view.tsx | mcp::log + Protocol Log panel | 1 |
| Readable tool inspector (schema table, annotation badges, generated forms) | not present (Insomnia shows raw JSON) | desktop MCP Inspector | 1 (new) |

## Scripting, runner, CLI

| Capability | Insomnia source | Rust target | Phase |
|---|---|---|---|
| `insomnia.*` script API (environment, variables, request, response, cookies, sendRequest, test, expect, execution) | insomnia-scripting-environment/src/objects/ | scripting (rquickjs) | 2 |
| Pre-request / after-response on request + folders | network/network.ts | engine::pipeline | 2 |
| Collection runner (iterations, delay, data file, bail, flow control) | routes/*.debug.runner.tsx | runner | 2 |
| `inso run collection` CLI + reporters | insomnia-inso/src/commands/run-collection/ | cli `irs run collection` | 2 |
| `inso lint spec` / `export spec` | insomnia-inso/src/commands/*-specification.ts | — | 5 |

## AI / LLM

| Capability | Insomnia source | Rust target | Phase |
|---|---|---|---|
| AI settings (providers, keys) | ui/components/settings/ai-settings.tsx | core::LlmProvider + keychain | 3 ✅ |
| LLM request type (chat, streaming, tool calls) | — (new) | core::LlmRequest/LlmRun + LlmView | 3 ✅ |
| LLM ↔ MCP tool-calling bridge | — (new) | llm::agent (approvals) | 3 ✅ |
| MCP sampling responses via LLM | main/mcp-generate-sampling-response.mjs | mcp::SamplingHandler + llm::agent::LlmSampler | 3 ✅ |

## Import / export / sync

| Capability | Insomnia source | Rust target | Phase |
|---|---|---|---|
| Insomnia v4/v5 export import & export | insomnia/src/common/insomnia-v5.ts, main/importers | convert::insomnia | 5 ✅ (v5 import+export, v4/v3 import) |
| Postman, OpenAPI, HAR, cURL importers | main/importers/importers/ | convert::{postman,openapi,har,curl} | 5 ✅ (WSDL ✗) |
| Git sync | insomnia-vcs, insomnia/src/sync | engine::git (system git) | 5 ✅ |
| Vault secrets (AES-GCM) | account/, environment SECRET type | engine::vault | 5 ✅ |

## Phase 2 API matrix (`insomnia.*` in scripts)

Source: `packages/insomnia-scripting-environment/src/objects/` + `packages/insomnia/src/script-executor.ts` and `scripting/require-interceptor.ts`. Runtime: QuickJS (rquickjs) with a JS prelude reimplementing the object model; host functions in Rust for HTTP, templating, timers, crypto randomness.

| Member | Status | Notes |
|---|---|---|
| `insomnia` / `$` / `pm` aliases | implement | `$` as in Insomnia; `pm` alias eases Postman script reuse |
| `environment`, `baseEnvironment`, `collectionVariables` (= base), `globals`, `iterationData` | implement | `name, has, get, set, unset, clear, replaceIn, toObject`; mutations persisted after the run (iterationData read-only to storage) |
| `variables` | implement | `has, get, set, replaceIn, toObject`; precedence local → iterationData → folder → environment → base → globals; `set` writes script-local vars (highest render precedence) |
| `vault` | NotSupported | secrets are ordinary variables in scripts; `insomnia.vault` not yet |
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
| `setTimeout/clearTimeout`, `queueMicrotask` | implement | host timers; `setImmediate` NotSupported (as in Insomnia) |
| `eval` | implement | available, runs inside the same sandbox (no host access) |

| `require(...)` module | Status | How |
|---|---|---|
| `chai` | implement | vendored chai 4 UMD (embedded) |
| `lodash` (+ global `_`) | implement | vendored lodash 4 (Insomnia uses es-toolkit/compat, API-compatible) |
| `uuid` | implement | small JS shim (`v4`, `validate`, `NIL`) over host randomness |
| `crypto-js` | implement | vendored UMD |
| `moment` | implement | vendored UMD |
| `tv4` | implement | vendored UMD |
| `ajv` | implement | bundled with esbuild to one IIFE |
| `atob`, `btoa` | implement | host base64 |
| `cheerio`, `xml2js` | NotSupported | depend on Node streams/events; clear error |
| `path, url, querystring, util, buffer, events, stream, assert, timers, punycode, string_decoder` | NotSupported | Node built-ins (no Node runtime in the sandbox) |

Sandbox: no filesystem/process/network globals; network only via `sendRequest`; per-script timeout (default 5 s) via interrupt handler; memory limit (default 64 MB) via QuickJS allocator limit.
