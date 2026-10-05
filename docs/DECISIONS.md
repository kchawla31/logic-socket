# Decisions log (overnight autonomous run, 2026-10-04)

The user waived the approval gates for this run. Every judgment call that would normally have needed sign-off is logged here for morning review.

| # | Decision | Why | Revisit if |
|---|---|---|---|
| D1 | Step 0 gates (PARITY/ARCHITECTURE approval) self-approved | User asked for an unattended overnight build | You disagree with phase boundaries in PARITY.md |
| D2 | SQLite with one generic `docs` table (JSON body) instead of a table per model | Mirrors NeDB's document model, so migrations stay additive and Insomnia import maps 1:1 | Query-heavy features (search across bodies) get slow |
| D3 | MCP client is hand-written JSON-RPC instead of the `rmcp` crate | The headline feature is a full protocol log with per-message latency, which needs every raw frame. Wrapping rmcp's transport would hide frames and pin us to its schema types. | MCP spec moves faster than we can track |
| D4 | Added small UX items beyond the Phase 1 list: cURL paste, command palette, inline variable highlighting | User asked for a UI friendlier than Postman/Hoppscotch | — |
| D5 | Toolchains installed in user dirs: rustup (~/.cargo), fnm + Node 24.18.0 (Homebrew), tauri-cli 2 | Approved by the user before the run | — |
| D6 | Extra Rust crates beyond the pre-approved list: `sha2`, `md5` (hash tag), `base64`, `url`, `futures`, `indexmap`, `axum` (mock MCP server binary + test servers), `tower-http` (dev-only, web bridge CORS) | All small, widely used; avoided hand-rolling crypto/URL parsing | — |
| D7 | Frontend: plain React + Tailwind v4 + native elements (`<dialog>`, `<select>`) instead of React Aria; CodeMirror 6 for editors; `react-markdown` for MCP descriptions; `lucide-react` icons | Fewer dependencies for a greenfield app; native elements are accessible by default | Accessibility audit finds gaps (menus/tree keyboard nav) |
| D8 | TypeScript pinned to 5.9 (npm `latest` is 7.x native port) | TS 7 tooling compatibility with Vite/vitest not verified overnight | Upgrade once verified |
| D9 | Dev-only "web bridge" (`cargo run -p insomnia-rs-desktop --example web_bridge` + `VITE_WEB_BRIDGE=1 npm run dev`) runs the real Tauri commands over HTTP so the UI runs in a browser | Screen capture is blocked in this environment; this let me drive and screenshot the real UI headlessly. Also enables Playwright E2E later | — |
| D10 | Timing breakdown shows client prep / TTFB (incl. DNS+TCP+TLS) / download; DNS, connect and TLS are not split out | reqwest doesn't expose per-phase timings without a custom connector | Users need curl-level phase timings |
| D11 | Each HTTP send uses a fresh client (no connection reuse) and redirects are followed manually | Matches curl/Insomnia semantics, gives per-hop timeline + cookies from intermediate hops | Throughput matters for the runner (Phase 2 may pool) |
| D12 | Native `window.prompt/confirm` replaced by in-app dialogs | WKWebView doesn't reliably implement them | — |
| D13 | Phase 2 gate self-approved. Scripting reimplements the `insomnia.*` object model as a JS prelude in QuickJS instead of bundling Insomnia's TS objects | Those objects import Insomnia internals (libcurl network, tough-cookie, constants, faker) and Node APIs; a prelude keeps the sandbox small and auditable | A compatibility gap shows up in real scripts → port that object |
| D14 | `require` supports chai, lodash, uuid, crypto-js, moment, tv4, ajv, atob/btoa; cheerio, xml2js and Node built-ins throw `NotSupported` | Pure-JS libs vendor cleanly; the rest need a Node runtime | Users need XML/HTML parsing in scripts |
| D15 | Scripts run on a dedicated 32 MB-stack thread with its own current-thread Tokio runtime; QuickJS stack cap 16 MB | Evaluating chai/crypto-js/ajv overflowed the default 2 MB stack (found by tests) | Script throughput becomes a bottleneck in large runs (pool threads) |
| D16 | With no active sub-environment, `insomnia.environment` is the base environment (writes land in the base) | Matches Insomnia; found by the end-to-end fixture run | — |
| D17 | Runner run ids are chosen by the UI before `runner_start` | Events can arrive before the start call returns (found via screenshots) | — |
| D18 | Extra crates: `rquickjs` (approved), `csv` (approved), `roxmltree` (dev-only, JUnit validation in tests), `@codemirror/autocomplete` (script completions) | Small, purpose-specific | — |
| D19 | Two LLM wire protocols: Anthropic Messages and OpenAI Chat Completions (also used for Ollama, OpenRouter, Gemini's OpenAI endpoint, LM Studio, vLLM) | Chat Completions is the common denominator for compatible servers; Anthropic needs its own format for tool use + thinking signatures | OpenAI-only features (Responses API, built-in tools) are needed |
| D20 | API keys live in the OS keychain (keyring 3.x), an env var, or an environment template — never in SQLite; the UI only sees "key stored" | Avoid plaintext secrets in the DB and in IPC payloads | Vault (Phase 5) becomes the preferred store |
| D21 | Tool approval default: read-only tools run automatically, everything else asks (per-request setting: none / read-only / all); CLI denies when there is no terminal unless `--yes` | Safe by default without being tedious; uses MCP tool annotations | — |
| D22 | MCP sampling is opt-in per saved server, backed by a chosen provider/model, and recorded in the protocol log | Servers can't spend the user's tokens without explicit consent | Elicitation (still unsupported) arrives |
| D23 | Verified only against a scripted mock LLM (both wire formats) — no API key or Ollama was available on this machine | Honest scope of verification | Run `irs llm chat` with a real key to confirm |
| D24 | Serde `rename_all_fields = "camelCase"` on event enums | Variant fields were snake_case while the UI expected camelCase (found by the IPC test: approvals never showed) | — |
