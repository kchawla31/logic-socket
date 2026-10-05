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
