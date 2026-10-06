# Logic Socket

An API client for HTTP, GraphQL, gRPC, realtime protocols, MCP and AI, written in Rust: a desktop app (Tauri 2 + React) and the `lsock` CLI sharing one engine.

- **HTTP**: layered environments (global → base → sub-environment → folders), `{{ _.var }}` + template tags, folder-inherited headers/auth, cookies, redirects with a curl-style timeline, response history
- **MCP Inspector**: stdio and Streamable HTTP servers; tools shown as readable cards (behavior badges, parameter tables with nested fields/constraints/enums), generated call forms with live schema validation, resources, prompts, notifications, and a full JSON-RPC protocol log with latencies
- **Scripts & tests**: pre-request / after-response scripts with the `ls.*` API (`pm.*` works too) in a QuickJS sandbox (chai, lodash, crypto-js, moment, uuid, ajv, tv4), test results and console per response
- **AI requests (LLM ↔ MCP)**: Anthropic, OpenAI, Ollama or any OpenAI-compatible endpoint; streaming answers, thinking, and MCP tools the model can call — read-only tools run automatically, risky ones wait for your approval; token usage, time-to-first-token and the exact provider payloads; MCP sampling backed by your provider. Keys stay in the OS keychain
- **Protocols & auth**: GraphQL (schema explorer, completion), gRPC (reflection or .proto files, all four call types), WebSocket, Server-Sent Events, Socket.IO; Basic, Bearer, API key, Digest, OAuth 1, OAuth 2 (incl. PKCE), AWS SigV4, netrc; HTTP proxy
- **Bring your collections**: import Postman (collections, environments, scripts), OpenAPI 3, Swagger 2, HAR, cURL and Insomnia files; export to Logic Socket YAML, Postman v2.1, HAR or Insomnia; generate code (curl, HTTPie, fetch, Python, Go, Rust)
- **Secrets & Git**: secret variables encrypted at rest (key in the OS keychain); Git sync with one readable YAML file per workspace, diffs, commit/pull/push, branches and conflict resolution — secret values never leave your machine
- **Collection runner**: iterations, CSV/JSON data, delays, bail, `setNextRequest` flow control; spec/dot/json/JUnit reports; `lsock run collection` for CI

Status and scope: see `docs/FEATURES.md` (what lives where, by phase), the phase reports `docs/PHASE1_REPORT.md` … `docs/PHASE5_REPORT.md`, and `docs/DECISIONS.md`.

## Run the desktop app

```bash
open "target/release/bundle/macos/Logic Socket.app"        # after a release build
# or build it:
cd apps/desktop && npm ci && cargo tauri build --bundles app
# dev mode with hot reload:
cd apps/desktop && cargo tauri dev
```

Shortcuts: `⌘K` command palette · `⌘N` new request · `⌘E` environments · `⌘↵` send / call tool / run · `⌘L` focus URL · paste a cURL command into the URL bar.

## CLI

```bash
cargo build --release -p lsock-cli
lsock workspace create "My API"
lsock env set "My API" base_url '"https://httpbin.org"'
lsock request add "My API" "Get JSON" GET '{{ _.base_url }}/json'
lsock send "Get JSON" -i
lsock request curl "My API" "curl https://httpbin.org/post -d a=1"

# MCP
lsock mcp tools --stdio "npx -y @modelcontextprotocol/server-everything"
lsock mcp tools --url http://localhost:3333/mcp --log
lsock mcp call --url http://localhost:3333/mcp -t get_weather -a '{"city":"Berlin"}'

# AI (keys: OS keychain by default, or --key-env / --key-template)
lsock llm provider add Claude --kind anthropic --model claude-sonnet-5-5
lsock llm chat "What's the weather in Berlin?" --mcp-url http://localhost:3333/mcp
lsock llm request add "My API" "Summarise issues" --prompt "Summarise open issues in {{ repo }}" --mcp GitHub
lsock llm request run "Summarise issues"
lsock llm mock-server            # offline mock (key: test-key) for trying it without an account

# Realtime and gRPC (try them against the local demo servers)
lsock mock realtime &            # ws://localhost:8790/ws, http://localhost:8790/events, Socket.IO on :8790
lsock rt ws://localhost:8790/ws -s hello
lsock rt http://localhost:8790/socket.io/ -k socketio --emit chat --args '["hi"]'
lsock mock grpc &                # demo.Greeter with reflection on :50051
lsock grpc grpc://localhost:50051 -l
lsock grpc grpc://localhost:50051 -m demo.Greeter/Chat --send '{"user":"a","text":"hi"}' --send '{"user":"a","text":"bye"}'

# Import / export / code
lsock import petstore.yaml                     # Logic Socket, Postman, OpenAPI 3, Swagger 2, HAR, Insomnia, curl
lsock import prod.postman_environment.json --into "My API"
lsock export "My API" -f postman -o my-api.postman_collection.json
lsock code "Get JSON" --lang python            # curl, httpie, js, python, go, rust

# Secrets (encrypted with a key in your keychain; LSOCK_VAULT_KEY for CI)
lsock env set "My API" token - --secret         # reads the value from stdin
lsock vault export-key                          # move secrets to another machine

# Git sync (one logic-socket.<name>.yaml per workspace; secrets never committed)
lsock git clone https://github.com/acme/api-collections.git ~/api-collections
lsock git open ~/my-repo && lsock git link my-repo "My API"
lsock git status my-repo && lsock git commit my-repo -m "Add orders" && lsock git push my-repo

# Runner (exit code 0 = all passed, 1 = failures, 2 = usage/runtime error)
lsock run collection "My API" -d data.csv -r junit -o report.xml
```

The CLI and the desktop app share the same database (`~/Library/Application Support/logic-socket` on macOS; override with `LSOCK_DATA_DIR`).

## Try it with the bundled mock MCP server

```bash
cargo build -p lsock-cli -p lsock-mcp
./target/debug/lsock-mock-mcp --http 3333 &
./target/debug/lsock mcp tools --url http://127.0.0.1:3333/mcp   # or: --stdio ./target/debug/lsock-mock-mcp
LSOCK_DATA_DIR=$(mktemp -d) ./fixtures/phase2/seed.sh && ./target/debug/lsock run collection "MCP smoke test" -d fixtures/phase2/cities.csv
```

## Develop

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd apps/desktop && npm run type-check && npm test
```

UI in a normal browser against the real backend (handy for UI work and headless screenshots):

```bash
cargo run -p logic-socket-desktop --example web_bridge     # serves the Tauri commands on :1421
cd apps/desktop && VITE_WEB_BRIDGE=1 npm run dev           # http://localhost:1420
```

## Layout

```
crates/core        models + SQLite store          crates/mcp        MCP client, protocol log, schema tools, mock server
crates/templating  Nunjucks-compatible rendering  crates/scripting  QuickJS sandbox + ls.* API
crates/http        HTTP engine                    crates/runner     collection runner + reporters
crates/engine      request pipeline               crates/cli        `lsock`
apps/desktop       Tauri shell (src-tauri) + React UI (src)
```
