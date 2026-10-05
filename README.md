# insomnia-rs

A Rust rewrite of the Insomnia API client: a desktop app (Tauri 2 + React) and the `irs` CLI sharing one engine.

- **HTTP**: environments with Insomnia's variable precedence, `{{ _.var }}` + template tags, folder-inherited headers/auth, cookies, redirects with a curl-style timeline, response history
- **MCP Inspector**: stdio and Streamable HTTP servers; tools shown as readable cards (behavior badges, parameter tables with nested fields/constraints/enums), generated call forms with live schema validation, resources, prompts, notifications, and a full JSON-RPC protocol log with latencies
- **Scripts & tests**: pre-request / after-response scripts with Insomnia's `insomnia.*` API in a QuickJS sandbox (chai, lodash, crypto-js, moment, uuid, ajv, tv4), test results and console per response
- **Collection runner**: iterations, CSV/JSON data, delays, bail, `setNextRequest` flow control; spec/dot/json/JUnit reports; `irs run collection` for CI

Status and scope: see `docs/PARITY.md` (what maps to which phase), `docs/PHASE1_REPORT.md`, `docs/PHASE2_REPORT.md`, and `docs/DECISIONS.md`.

## Run the desktop app

```bash
open target/release/bundle/macos/insomnia-rs.app        # after a release build
# or build it:
cd apps/desktop && npm ci && cargo tauri build --bundles app
# dev mode with hot reload:
cd apps/desktop && cargo tauri dev
```

Shortcuts: `⌘K` command palette · `⌘N` new request · `⌘E` environments · `⌘↵` send / call tool / run · `⌘L` focus URL · paste a cURL command into the URL bar.

## CLI

```bash
cargo build --release -p irs-cli
irs workspace create "My API"
irs env set "My API" base_url '"https://httpbin.org"'
irs request add "My API" "Get JSON" GET '{{ _.base_url }}/json'
irs send "Get JSON" -i
irs request curl "My API" "curl https://httpbin.org/post -d a=1"

# MCP
irs mcp tools --stdio "npx -y @modelcontextprotocol/server-everything"
irs mcp tools --url http://localhost:3333/mcp --log
irs mcp call --url http://localhost:3333/mcp -t get_weather -a '{"city":"Berlin"}'

# Runner (exit code 0 = all passed, 1 = failures, 2 = usage/runtime error)
irs run collection "My API" -d data.csv -r junit -o report.xml
```

The CLI and the desktop app share the same database (`~/Library/Application Support/insomnia-rs` on macOS; override with `IRS_DATA_DIR`).

## Try it with the bundled mock MCP server

```bash
cargo build -p irs-cli -p irs-mcp
./target/debug/irs-mock-mcp --http 3333 &
./target/debug/irs mcp tools --url http://127.0.0.1:3333/mcp   # or: --stdio ./target/debug/irs-mock-mcp
IRS_DATA_DIR=$(mktemp -d) ./fixtures/phase2/seed.sh && ./target/debug/irs run collection "MCP smoke test" -d fixtures/phase2/cities.csv
```

## Develop

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd apps/desktop && npm run type-check && npm test
```

UI in a normal browser against the real backend (handy for UI work and headless screenshots):

```bash
cargo run -p insomnia-rs-desktop --example web_bridge     # serves the Tauri commands on :1421
cd apps/desktop && VITE_WEB_BRIDGE=1 npm run dev           # http://localhost:1420
```

## Layout

```
crates/core        models + SQLite store          crates/mcp        MCP client, protocol log, schema tools, mock server
crates/templating  Nunjucks-compatible rendering  crates/scripting  QuickJS sandbox + insomnia.* API
crates/http        HTTP engine                    crates/runner     collection runner + reporters
crates/engine      request pipeline               crates/cli        `irs`
apps/desktop       Tauri shell (src-tauri) + React UI (src)
```
