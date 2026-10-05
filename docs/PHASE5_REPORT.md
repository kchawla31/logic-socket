# Phase 5 report: import/export, Git sync, vault

## What was built

| Area | Where | Notes |
|---|---|---|
| Importers | `crates/convert` | **Insomnia v5** YAML (collection, spec, environment, MCP client), **Insomnia v4/v3** JSON/YAML exports (incl. WebSocket payloads, gRPC + proto directories), **Postman** v2.0/v2.1 collections (auth of every kind, scripts, variables, form/multipart/GraphQL/file bodies, path variables, dynamic variables) and environments (secret type), **OpenAPI 3** and **Swagger 2** (folders per tag, `:path` parameters, generated example bodies, `base_url` + server variables, security schemes → auth with environment placeholders), **HAR**, and one or more **cURL** commands. Format is detected automatically |
| Exporters | `crates/convert` | Insomnia v5 YAML (everything; Insomnia can open it), Postman v2.1 (scripts translated `insomnia.*` → `pm.*`), HAR |
| Script translation | `convert::postman` | Insomnia's Postman rules (legacy `tests[…]`, `environment.x`, `postman.*`, `responseBody`, …) then `pm.` → `insomnia.`; Rust `regex` has no look-behind, so the guard is a captured prefix |
| `{% faker %}` tag | `crates/templating` | 38 Postman dynamic variables (`randomEmail`, `randomInt`, `guid`, …) so imported `{{$…}}` keep working |
| Code generation | `convert::codegen`, `engine::code_request` | curl, HTTPie, JavaScript fetch, Python requests, Go net/http, Rust reqwest, from the fully rendered request (environment, inherited headers, auth, OAuth 2 token) |
| Vault | `engine::vault` | AES-256-GCM key in the OS keychain (or `IRS_VAULT_KEY` for CI). Secret values are stored as `vault:v1:…` and decrypted only for rendering and scripts. Every write path re-seals; OAuth 2 tokens are encrypted too. Recovery-key export/import (refuses a key that can't decrypt existing data) and reset |
| Import/export into the store | `engine::transfer` | **Copy** (fresh ids; MCP references re-keyed), **Replace** (keep ids, overwrite in place, remove what the file dropped; keeps local secrets, private environments, response/run history and this machine's active environment), **merge into a workspace** |
| Git sync | `engine::git` | One v5 file per workspace (Insomnia's layout) via the system `git`: open/init, clone, link/unlink, status, diff, commit, pull (refuses over uncommitted edits; reports conflicts; resolve with mine/theirs; abort), push, discard, log, branches. HTTPS token in the keychain, passed through `GIT_CONFIG_*` env (never argv). Secret values, private envs and cookies are never written |
| CLI | `irs import`, `export`, `code`, `vault`, `git`, `env set --secret`, `env reveal` | |
| Desktop | `ImportExport.tsx`, `GitPanel.tsx`, `Modals.tsx` | Import dialog (file / drop, paste, URL) with a live preview and warnings; export with format choice → Downloads; "Generate code" next to Send; Secrets panel in Environments (masked, reveal, copy, make plain); Settings → Secrets vault; Git sync panel (connect, changes with diffs, commit, pull/push with ahead/behind, conflict resolution, branches, history, remote/author/token, synced workspaces) |

## Verification (actual results)

- `cargo clippy --workspace --all-targets -D warnings`: clean
- `cargo test --workspace`: **157 passed, 0 failed** (Phase 4: 129). New tests:
  - convert (20), checked against Insomnia's own importer fixtures (copied into `crates/convert/fixtures`):
    - detection of every format, Postman v2.0/v2.1 and every auth kind;
    - script translation rules, variables and faker, Postman environments and secrets, a Postman export round trip;
    - OpenAPI petstore, security schemes and server variables, Swagger 2 forms and uploads;
    - HAR (full and bare-request), multi-command cURL;
    - **v5 round trips** of Insomnia's fixtures and of a bundle containing every item kind;
    - v5 output Insomnia can load (ID prefixes, `reflectionApi`, the extension block), MCP client files, a hand-built v4 export, code snippets (generated curl parses back);
  - engine (+10):
    - secrets encrypted at rest and used when sending;
    - scripts reading and writing secrets;
    - moving the key between machines, with a wrong key refused;
    - OAuth 2 tokens sealed;
    - Copy, Replace and merge imports;
    - Postman and HAR exports, code generation from the rendered request;
    - **a two-teammate Git flow** against a bare remote: share, clone, rename, pull, refuse a dirty pull, discard, conflict → take theirs, branches. It checks that no secret reaches the repository and that a fresh clone shows no phantom diffs;
  - templating (+1) for faker and bracket syntax; desktop IPC (+1) for every new command.
- `tsc --noEmit` and `vitest`: clean
- CLI end to end:
  - imported OpenAPI, Postman and v5 files;
  - generated code; set and revealed secrets;
  - exported to Postman, checking that the secret was absent;
  - ran `git open`, `config`, `link`, `commit` and `push` against a bare remote, then `git clone` on a second data directory.
- Desktop UI driven through the web bridge with Playwright:
  - import preview and import; the generated-code dialog; environment secrets; export;
  - Git connect → link → commit → edit → diff → history; Settings → vault.

  I reviewed the screenshots.

## Bugs found and fixed during verification

- Postman script rules referenced the wrong capture groups (the prefix guard is group 1)
- PKCE was switched on for every Postman authorization-code grant
- OAuth 2 `audience`/`resource` arrived as Postman's internal objects
- OpenAPI `*/*` bodies weren't treated as JSON; one implicit-flow warning per operation instead of per scheme
- HAR form params without a MIME type were dropped; bare-request HAR files weren't recognised
- MCP-client exports lost sampling/cwd settings; MCP ids were re-keyed in Replace mode
- The workspace timestamp and secret-only edits caused phantom Git diffs
- `IRS_SECRET_STORE=memory` made every CLI process use a different vault key → added `IRS_VAULT_KEY`
- Desktop: a slow refresh for the previous workspace could overwrite the sidebar after an import/switch

## Known gaps

- `insomnia.vault.get()` in scripts is not implemented (secrets are available as ordinary variables)
- Mock servers, unit-test suites, API spec documents and CA/client certificates in Insomnia files are skipped with a warning (no models yet); WSDL import is not supported
- Hawk, NTLM, ASAP and Akamai auth from Postman/Insomnia become "No auth" with a warning
- Git: no interactive per-hunk merge (whole-file mine/theirs), no SSH-key management UI (uses your SSH agent), no automatic background fetch
- Exports save to `~/Downloads` (no native save dialog yet)
