# Decisions log (overnight autonomous run, 2026-10-04)

The user waived the approval gates for this run. Every judgment call that would normally have needed sign-off is logged here for morning review.

| # | Decision | Why | Revisit if |
|---|---|---|---|
| D1 | Step 0 gates (PARITY/ARCHITECTURE approval) self-approved | User asked for an unattended overnight build | You disagree with phase boundaries in PARITY.md |
| D2 | SQLite with one generic `docs` table (JSON body) instead of a table per model | Mirrors NeDB's document model, so migrations stay additive and Insomnia import maps 1:1 | Query-heavy features (search across bodies) get slow |
| D3 | MCP client is hand-written JSON-RPC instead of the `rmcp` crate | The headline feature is a full protocol log with per-message latency, which needs every raw frame. Wrapping rmcp's transport would hide frames and pin us to its schema types. | MCP spec moves faster than we can track |
| D4 | Added small UX items beyond the Phase 1 list: cURL paste, command palette, inline variable highlighting | User asked for a UI friendlier than Postman/Hoppscotch | — |
| D5 | Toolchains installed in user dirs: rustup (~/.cargo), fnm + Node 24.18.0 (Homebrew), tauri-cli 2 | Approved by the user before the run | — |
