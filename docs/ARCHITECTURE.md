# Architecture

## Crate layout

```
insomnia-rs/
  Cargo.toml                 workspace
  crates/
    core/        models (serde), SQLite store, migrations, batched writes
    templating/  render context layering + minijinja Nunjucks-compatible renderer + tags
    http/        request building, auth, cookies, send, timings, timeline
    mcp/         JSON-RPC 2.0 client: stdio + Streamable HTTP transports, protocol log
    engine/      orchestration: load request + ancestors + envs → render → (scripts) → send → persist
    scripting/   (Phase 2) rquickjs sandbox + insomnia.* API
    runner/      (Phase 2) collection runner, event stream, reporters
    llm/         (Phase 3) provider abstraction, streaming, MCP bridge
    cli/         `irs` binary (clap) — thin layer over engine/mcp/runner
  apps/desktop/
    src-tauri/   Tauri 2 shell: commands = thin wrappers over engine/mcp
    src/         React + TypeScript + Tailwind UI (Vite)
```

Dependency direction: `core ← templating ← http ← engine ← {cli, desktop}`, `mcp` depends only on `core` types it needs (headers/env) and is used by engine, cli and desktop. The UI never talks to SQLite or the network directly — everything goes through Tauri commands so the CLI and the desktop share the exact same behavior.

## Storage

A single SQLite file (`<data_dir>/insomnia-rs/insomnia.db`, override with `IRS_DATA_DIR`).

```sql
CREATE TABLE docs (
  id        TEXT PRIMARY KEY,      -- prefix_uuid, e.g. req_..., fld_..., env_...
  type      TEXT NOT NULL,         -- Workspace | Folder | Request | Environment | McpServer | Response | CookieJar | Settings
  parent_id TEXT,                  -- hierarchy edge (NULL for workspaces/settings)
  sort_key  REAL NOT NULL DEFAULT 0,
  created   INTEGER NOT NULL,      -- ms epoch
  modified  INTEGER NOT NULL,
  data      TEXT NOT NULL          -- JSON of the typed model body
);
CREATE INDEX docs_parent ON docs(parent_id, type);
CREATE INDEX docs_type   ON docs(type);
CREATE TABLE schema_version (version INTEGER NOT NULL);
```

A generic document table mirrors Insomnia's NeDB document model, keeps migrations additive, and makes import/export (Phase 5) a near-direct mapping. Typed access goes through `core::Store` (`get::<Request>(id)`, `children::<Folder>(parent)`, `ancestors(id)`, `upsert`, `delete_tree`). Bulk writes use `Store::batch(|tx| ...)` — one transaction, one change notification (Insomnia's `bufferChanges/flushChanges`). Response bodies above 1 MB are stored as files next to the DB; smaller bodies inline (base64 in `data`).

## Request pipeline (engine)

1. Load request, ancestors (folders → workspace), base + active sub env, global env, cookie jar.
2. Build render context (`templating::Context`) using Insomnia's precedence.
3. (Phase 2) folder pre-scripts outer→inner, request pre-script; mutations applied to the context/request.
4. Render URL, params, path params, headers, body, auth fields.
5. Merge inherited folder headers/auth (nearest folder wins; request overrides).
6. `http::send` → `HttpResponse { status, headers, body, timings, timeline, size }`.
7. (Phase 2) after-scripts request → folders inner→outer.
8. Persist Response + cookie updates in one batch; prune history to 20 per request.

`engine::send_many(ids, concurrency)` runs step 1–8 for many requests with a `tokio::Semaphore`.

## MCP client

Hand-written JSON-RPC 2.0 (see DECISIONS.md D3). `Transport` trait with `Stdio` (child process, newline-delimited JSON on stdin/stdout, stderr captured into log) and `StreamableHttp` (POST with `Accept: application/json, text/event-stream`, SSE or JSON responses, `Mcp-Session-Id` header, `MCP-Protocol-Version` header, DELETE on close). Every message in either direction is appended to a `ProtocolLog` (`{ seq, direction, timestamp, latency_ms (responses), method, json }`) exposed to the UI as a stream.

## Tauri command surface (IPC)

| Command | Purpose |
|---|---|
| `workspace_list/create/rename/delete` | workspaces |
| `tree_get(workspace_id)` | folders + requests + mcp servers as a tree |
| `folder_create/update/delete`, `request_create/update/duplicate/delete`, `item_move` | tree editing |
| `env_list/get/upsert/delete`, `env_set_active` | environments |
| `request_send(request_id)` → Response | send through engine |
| `response_list(request_id)`, `response_get(id)` | history |
| `render_preview(workspace_id, text)` | live variable preview / unresolved warnings |
| `curl_import(text)` | paste cURL into a request (UX) |
| `mcp_connect(server_id)` → ServerInfo | initialize |
| `mcp_list(server_id, kind)` | tools / resources / templates / prompts (all pages) |
| `mcp_call_tool(server_id, name, args)`, `mcp_read_resource`, `mcp_get_prompt` | primitives |
| `mcp_disconnect(server_id)`, `mcp_log(server_id)` + event `mcp://log` | protocol log |
| `settings_get/update` | settings |

## UI principles (the "friendlier than Postman/Hoppscotch" bar)

- Three-pane layout: sidebar tree · request editor · response, resizable, collapses to two panes under 1100px.
- Cmd/Ctrl+K command palette for every action; Cmd+Enter send; Cmd+S save is implicit (autosave).
- Variables highlighted inline: green resolved, red unresolved, hover shows value + source layer.
- Paste a cURL command into the URL bar → request is populated.
- Response viewer: pretty/raw/headers/timeline/timing waterfall; JSON tree with search and copy-path.
- Dark and light theme from OS, no flash on load.
- MCP Inspector: readable-first (schema tables, badges, generated forms), raw JSON one click away.
