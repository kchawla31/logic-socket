# Logic Socket evaluation

> **Status (2026-10-08):** all 17 bugs below are fixed. Their regression tests are no longer ignored and pass in `cargo test --workspace` and `npm test`. The report is kept as written at evaluation time.

Confirmed defects only. Each bug below has a command, a file location, and the output that command printed. Product code was not changed.

## 1. Environment

- Date: 2026-10-06
- OS: macOS 27.0.1 (build 26A434)
- Git revision: `deea123` (`main`)
- Rust: rustc 1.99.0 (b940084d7 2026-09-28), cargo 1.99.0 (5f94df478 2026-08-27)
- Node: v24.18.0, npm 11.16.0, from `$HOME/.local/share/fnm/aliases/default/bin` (not on the default `PATH`)
- CLI binaries used for the manual session: `./target/debug/lsock`, `./target/debug/lsock-mock-mcp` (`cargo build -p lsock-cli -p lsock-mcp`)
- Suite build: `cargo test --workspace` (test profile)
- Every CLI run used a fresh temp `LSOCK_DATA_DIR`, `LSOCK_SECRET_STORE=memory`, and `LSOCK_VAULT_KEY` (a generated 32-byte key). The OS keychain and `~/Library/Application Support/logic-socket` were not used.
- Local processes: a Python `BaseHTTP` echo on `127.0.0.1:18799`, `lsock-mock-mcp --http 13333`, `lsock mock realtime` on `18790`, `lsock mock graphql` on `18791`, `lsock mock grpc` on `15051`, `lsock llm mock-server` on `18787`. No public host is required by the committed tests.

## 2. Coverage matrix

| # | Capability | Status | What ran |
|---|---|---|---|
| 1 | HTTP send | exercised | CLI against `127.0.0.1:18799`: GET with query, POST JSON body `{"a":1}`, redirect followed to `/landed`, timeline (`-v`), cookie `sid=abc` still sent after the redirect, header `x-from-script: from-pre`. GraphQL POST to `127.0.0.1:18791/graphql` exited 0 (`Received 82 B`). Baseline `lsock-http` 16 passed, including `requests_go_through_the_proxy_unless_bypassed`. Multipart send, a form-body send, proxy, and timeout were not driven from this CLI script. |
| 2 | Templating | exercised | Baseline `lsock-templating` 12 passed. CLI `lsock render` substituted `{{ _.base_url }}`. `uuid` / `now` / `base64` / `hash` / `response` were not invoked from the CLI in this session. |
| 3 | Auth | exercised | Baseline `lsock-http` and `lsock-engine` (31 passed) cover the suites that ship with those crates. This CLI session did not send Basic, Bearer, Digest, OAuth 1, OAuth 2, or SigV4. Netrc parsing was executed by BUG-002. Implicit grant, Hawk, NTLM, and ASAP are unsupported (`docs/FEATURES.md` line 43) and are not bugs. |
| 4 | Scripts | exercised | Baseline `lsock-scripting` 23 passed. CLI pre-request script set `X-From-Script`. Runner JSON report recorded `expected 1 to deeply equal 2`. `sendRequest`, `skipRequest`, `setNextRequest`, and the per-script timeout were not driven from the CLI. BUG-013, BUG-014, and BUG-015 call the sandbox directly. |
| 5 | Runner | exercised | JSON data file produced `http://127.0.0.1:18799/w/Berlin`. Reporters: json exit 1 (failing test), junit exit 0, dot exit 1, missing collection exit 2 (`no collection or folder named or with id 'no-such-collection'`). `cargo test -p lsock-cli --test run_collection` passed (1 test) in the baseline. CSV, delay, bail, and a green `spec` run were not in this CLI script. Baseline `lsock-runner` 5 passed. |
| 6 | MCP | exercised | Streamable HTTP on `127.0.0.1:13333` and stdio `./target/debug/lsock-mock-mcp`: tools list (10 tools, protocol `2025-11-25`), `get_weather` Berlin → `Berlin: 21°C, clear sky`, resources, prompts, `mcp info --log`. Empty `city` exited 2. OAuth on HTTP MCP is unwired (`docs/FEATURES.md` line 63) and was not treated as a required pass. |
| 7 | LLM | exercised | `lsock llm mock-server` only, key `test-key`, `--key-env LSOCK_MOCK_KEY`. Chat `Say hello` → `Echo: Say hello`. Weather with `--mcp-url` and `--yes` auto-ran `get_weather`. `delete` without `--yes` returned `The user declined this tool call: no terminal to confirm (use --yes)` and `is_error true`. No live provider. Sampling (`ask_llm`) was not called. |
| 8 | Realtime | exercised | `lsock mock realtime`: WebSocket `Connected (101 Switching Protocols)` and echo, SSE `Connected (200)`, Socket.IO `Connected (101 Switching Protocols)`. BUG-008 is a parser unit test, not that echo path. |
| 9 | gRPC | exercised | `lsock mock grpc` list printed `SayHello`, `CountUp`, `Sum`, `Chat`. Unary: `{"message":"Hello, Ada!","length":11,"at":"2026-10-06T14:31:36Z"}`. CountUp values 1, 2, 3. Sum `{"value":7}`. Chat `you said: hi` / `you said: bye`. The list text does not include the word "reflection"; the reflection RPC itself was not inspected. |
| 10 | Import, export, codegen | exercised | CLI import exit 0: `crates/convert/fixtures/postman/orders-v2.1.json`, `openapi/bookstore-v3.yaml`, `openapi/bookstore-v2.json`, `har/single-request.json`, `insomnia/orders.yaml`. Export Logic Socket YAML and Postman exit 0. Codegen python, go, rust, httpie, js, curl exit 0. Exported YAML did not contain the secret value (`no-secret-in-git-yaml` on the git tree; export check `contains_secret_value` false). Field-by-field round trip beyond the existing convert suite (baseline 22 passed) was not diffed. cURL import from the CLI was not run; BUG-010 and BUG-011 call the library. |
| 11 | Vault and git | exercised | `vault status` printed `vault key: present` and `encrypted values: 1` with `LSOCK_VAULT_KEY` set. `git init --bare -b main` under the temp dir, then `lsock git open`, `config` (no `token_env`), `link`, `status`, `commit`, `push`, `clone`, all exit 0. Committed YAML had no `super-secret-token`. |
| 12 | Desktop UI | not exercised | No browser tool is connected. The web bridge was not started. `apps/desktop` `npm test` and `npm run type-check` were run. Baseline `ipc_tests`: 11 passed (`lsock_desktop`). Send, environment edit, MCP tool call, import, and runner were not clicked. Viewports 1440 and 375 were not opened. |

Docs mismatch, not filed as a product bug: `docs/FEATURES.md` line 47 lists "Timing breakdown (DNS, connect, TLS, TTFB, download)". `crates/http/src/lib.rs` lines 556–558 always set `dns_ms`, `connect_ms`, and `tls_ms` to `None`. `docs/DECISIONS.md` D10 says those phases are intentionally not split and that TTFB includes DNS, TCP, and TLS.

Unannotated MCP tools are labeled `destructive · open-world (unannotated)`. `crates/mcp/src/types.rs` documents that default. Not filed.

## 3. Baseline

Taken before any test added by this evaluation.

- `cargo test --workspace` exit 0. Sum of `test result` lines in `/tmp/lsock-cargo-test.log`: 160 passed, 0 failed, 0 ignored (29 result lines). Includes `ipc_tests` 11 passed, `run_collection` 1 passed, convert 22, engine 31, http 16, llm 10, mcp lib 5 plus `tests/client.rs` 4, realtime 6, runner 5, scripting 23, templating 12.
- `cd apps/desktop && npm test` (with the fnm `PATH` prefix): 1 file, 5 passed, exit 0 (`/tmp/lsock-npm-test.log`).
- `npm run type-check` exit 0.

After the tests in this report:

- `cargo test --workspace -- --test-threads=8` exit 0. 162 passed, 0 failed, 15 ignored (`/tmp/lsock-workspace-final.log`). The two extra passes are the unignored tests in section 5. The 15 ignored tests are BUG-001 through BUG-004 and BUG-007 through BUG-017.
- `cd apps/desktop && npm test`: 7 passed, 2 skipped, exit 0 (`/tmp/lsock-vitest-default2.log`). The two skips are BUG-005 and BUG-006.
- `npm run type-check` exit 0 after the Vitest helper stopped naming the Node `process` global (`/tmp/lsock-vitest-type2.log`).

No pre-existing suite failure was present.

## 4. Bugs

### BUG-001 — `Domain=com` is stored and sent to other hosts

- Severity: blocker
- Area: `crates/http/src/cookies.rs:68` (`domain_match`) and the domain arm at lines 34–40. `parse_set_cookie` accepts the domain when `domain_match` is true and sets `host_only` to false.
- Expected: the parser comment at line 37, "a server may not set cookies for unrelated domains." A `Domain` that is only a public suffix must not be stored. RFC 6265 §5.3 rejects a public suffix.
- Actual: `Domain=com` from `https://evil.com/` is stored and `header_for` sends `sid=secret` to `https://bank.com/account`.
- Repro: `cargo test -p lsock-http public_suffix_domain_is_not_stored_or_sent -- --ignored --nocapture`
- Evidence: exit 101. Panic text:

```
Domain=com was accepted and can be replayed to other hosts: parse=Some(Cookie { name: "sid", value: "secret", domain: "com", path: "/", expires: None, secure: false, http_only: false, host_only: false }) bank.com=Some("sid=secret")
```

- Regression: `cookies::tests::public_suffix_domain_is_not_stored_or_sent` in `crates/http/src/cookies.rs`. Command above, exit 101.
- Uncertainty: RFC 6265 §5.1.3 domain-match itself allows a suffix match. This tree has no public-suffix list. The bug is the missing §5.3 rejection, which the file's own comment describes as unrelated domains.

### BUG-002 — `.netrc` comments and `macdef` bodies become credentials

- Severity: major
- Area: `crates/engine/src/oauth2.rs:377` `parse_netrc`. Line 378 splits on whitespace. The `macdef` arm (lines 404–408) skips one token and then keeps parsing `login` / `password`.
- Expected: a `#` comment is not a machine entry, and a `macdef` body is not the machine's login. `good.com` stays `u` / `p`. `example.com` stays `user` / `secret`.
- Actual: the commented machine `evil.com` is returned as `bad` / `worse`. The `macdef` body replaces `example.com` with `attacker` / `stolen`. The `good.com` check in the same test did not fail.
- Repro: `cargo test -p lsock-engine netrc_ignores_comments_and_macdef_bodies -- --ignored --nocapture`
- Evidence: exit 101.

```
["commented machine was used: Some((\"bad\", \"worse\"))", "macdef body overwrote credentials: Some((\"attacker\", \"stolen\"))"]
```

- Regression: `oauth2::tests::netrc_ignores_comments_and_macdef_bodies` in `crates/engine/src/oauth2.rs`. Command above, exit 101.
- Uncertainty: `docs/FEATURES.md` lists netrc as implemented and does not mention comments. The failure is that a commented machine is the match that gets returned.

### BUG-003 — exclusive numeric bounds are enforced as inclusive

- Severity: major
- Area: `crates/mcp/src/schema.rs:214` (`constraints` reads `exclusiveMinimum` / `exclusiveMaximum` and prints `≤` at line 217) and `validate` at lines 335–345 (reads only `minimum` / `maximum`).
- Expected: a value equal to `exclusiveMinimum` or `exclusiveMaximum` is rejected, and the parameter table shows an exclusive bound.
- Actual: the row shows `0 ≤ value ≤ 10`. `validate` accepts 0, 10, and 5.
- Repro: `cargo test -p lsock-mcp exclusive_numeric_bounds_are_exclusive -- --ignored --nocapture`
- Evidence: exit 101.

```
shown=["0 ≤ value ≤ 10"] at_min=[] at_max=[] inside=[]
```

- Regression: `schema::tests::exclusive_numeric_bounds_are_exclusive` in `crates/mcp/src/schema.rs`. Command above, exit 101.
- Uncertainty: none on the assertion. The CLI was not given a tool whose schema uses exclusive bounds; the library test is the repro.

### BUG-004 — displayed `pattern`, `minItems`, and `uniqueItems` are not checked

- Severity: major
- Area: `crates/mcp/src/schema.rs:228` (those constraints are displayed) and `validate` (`check`, lines 329–372), which never reads `pattern`, `minItems`, `maxItems`, or `uniqueItems`. `lsock mcp call` uses this validator (`crates/cli/src/mcp_cmd.rs`).
- Expected: the rules printed in the parameter table are the rules `validate` enforces. `repo` matching `^[\w-]+/[\w-]+$` rejects `not a repo`. `labels` with `minItems: 2` and `uniqueItems: true` rejects `["a"]` and `["a","a"]`.
- Actual: the table text is present and `validate` returns no errors. The live CLI call exited 0 and the mock echoed the invalid arguments.
- Repro, library: `cargo test -p lsock-mcp displayed_pattern_items_and_uniqueness_are_enforced -- --ignored --nocapture`
- Repro, CLI (temp data dir, `LSOCK_SECRET_STORE=memory`, mock MCP on `127.0.0.1:13333`):

```
lsock mcp call --url http://127.0.0.1:13333/mcp -t create_issue -a '{"repo":"not a repo","title":"x","labels":["a","a"]}'
```

- Evidence, library, exit 101:

```
repo_c=Some(["pattern: ^[\\w-]+/[\\w-]+$"]) labels_c=Some(["≥ 2 items", "unique items"]) bad_pattern=[] dupes=[] too_few=[]
```

- Evidence, CLI, exit 0:

```
✓ success · 0.3 ms
ok: {"repo":"not a repo","title":"x","labels":["a","a"]}
```

The tools list in the same session printed `pattern: ^[\w-]+/[\w-]+$` and `unique items`. The control call `get_weather` with `city: ""` exited 2: `'city' must be at least 1 characters`.

- Regression: `schema::tests::displayed_pattern_items_and_uniqueness_are_enforced` in `crates/mcp/src/schema.rs`. Library command above, exit 101.
- Uncertainty: `example_args` emits `{"city":""}` for a `minLength: 1` string. An existing test, `example_args_fill_required_with_defaults`, locks that. The CLI hint `Example: --args '{"city":""}'` after a rejection is that behavior, not a separate bug.

### BUG-005 — pasted URLs keep the fragment on the query value

- Severity: minor
- Area: `apps/desktop/src/lib/utils.ts:101` `splitQuery`. `RequestView.tsx:146` uses it when the URL bar is empty and the paste is not a cURL command.
- Expected: `splitQuery('http://x/a?q=1#section')` is base `http://x/a#section` and query value `1`. The comment on the function is "Split `?a=1&b=2` off a URL into params."
- Actual: base `http://x/a`, value `1#section`.
- Repro, from `apps/desktop` with the fnm `PATH`: `RUN_IGNORED=1 npx vitest run -t 'BUG-005'`
- Evidence: exit 1.

```
-   "base": "http://x/a#section",
+   "base": "http://x/a",
-       "value": "1",
+       "value": "1#section",
```

- Regression: `BUG-005 fragment stays on the URL and out of the query value` in `apps/desktop/src/lib/utils.test.ts`. Command above, exit 1. `npm test` skips it.
- Uncertainty: the browser paste path was not clicked. The helper is what `onUrlPaste` calls.

### BUG-006 — path parameters are taken from the query and the fragment

- Severity: minor
- Area: `apps/desktop/src/lib/utils.ts:92` `pathParamsInUrl`. The comment says "Path params (`/:id`) present in a URL". `RequestView.tsx:93` `setUrl` copies every returned name into `pathParameters`.
- Expected: `http://x/users/:id?next=/:home#/:frag` yields `['id']`.
- Actual: `['id', 'home', 'frag']`. The regex `/\/:([^/?#:]+)/g` matches every `/:`.
- Repro: `RUN_IGNORED=1 npx vitest run -t 'BUG-006'` (same exit 1 run as BUG-005 when the filter is `BUG-00`).
- Evidence: exit 1. Expected `['id']`, received `['id', 'home', 'frag']`.
- Regression: `BUG-006 path params come from the path only` in `apps/desktop/src/lib/utils.test.ts`. Exit 1.
- Uncertainty: the URL bar was not typed in a browser. `setUrl` is the caller.

### BUG-007 — `--secret` does not parse JSON the way the value flag documents

- Severity: major
- Area: `crates/cli/src/main.rs:659`. The value help at line 175: "Value; parsed as JSON when possible, otherwise a string". The secret branch assigns `Value::String(value)` and skips `serde_json::from_str`.
- Expected: `lsock env set … secret '"quoted-value"' --secret` stores the same string as the non-secret form, `quoted-value`. `lsock render` of `{{ _.plain }}|{{ _.secret }}` prints `quoted-value|quoted-value`.
- Actual: render printed `quoted-value|"quoted-value"`. In the CLI session the same quoting on a secret header was sent as `x-trace: "super-secret-token"` (the quote characters are part of the value). `base_url`, set without `--secret`, rendered without those quotes.
- Repro: `cargo test -p lsock-cli --test secret_env_json -- --ignored --nocapture`. The test sets `LSOCK_DATA_DIR` to a temp dir, `LSOCK_SECRET_STORE=memory`, and a test `LSOCK_VAULT_KEY`.
- Evidence: exit 101.

```
stdout:
✓ plain Base Environment
✓ secret Base Environment
quoted-value|"quoted-value"
```

- Regression: `secret_values_follow_the_same_json_parsing_as_plain_values` in `crates/cli/tests/secret_env_json.rs`. Command above, exit 101.
- Uncertainty: skipping JSON parse keeps a secret whose characters are `"foo"`, `true`, or `12345` literal. The help text does not say that. The README secret example uses stdin `-`, which this test does not cover.

### BUG-008 — an SSE CRLF split after CR becomes two events

- Severity: major
- Area: `crates/realtime/src/sse.rs:28`. On any `\r` the buffer rewrites `\r\n` to `\n` and then every remaining `\r` to `\n`, including a CR whose LF has not arrived.
- Expected: `data: hello\r` + `\ndata: world\r\n\r\n` is one event with data `hello\nworld`. CR, LF, and CRLF are one line ending (`docs/FEATURES.md` line 49 marks SSE implemented). The same crate already joins a multiline event in `parses_fields_comments_and_multiline`.
- Actual: zero events from the first chunk, then two events, `hello` and `world`.
- Repro: `cargo test -p lsock-realtime crlf_split_after_the_carriage_return_stays_one_event -- --ignored --nocapture`
- Evidence: exit 101 (this filter was part of the `--no-fail-fast` run whose process exit was 101).

```
first=[] second=[SseFrame { event: None, data: "hello", id: None, retry: None, comment: false }, SseFrame { event: None, data: "world", id: None, retry: None, comment: false }]
```

- Regression: `sse::tests::crlf_split_after_the_carriage_return_stays_one_event` in `crates/realtime/src/sse.rs`.
- Uncertainty: the CLI echo against `lsock mock realtime` exited 0. That mock does not split a CRLF across chunks, so the CLI session did not hit this path.

### BUG-009 — the mock LLM panics while streaming multibyte tool arguments

- Severity: blocker
- Area: `crates/llm/src/mock.rs:185` and the same call at line 229. `input.to_string().split_at(s.len() / 2)` splits on bytes.
- Expected: `weather in 東京都` with a `get_weather` tool streams tool arguments `{"city":"東京都"}` from the Anthropic and OpenAI-compatible mock routes. `chunks()` for text already walks chars.
- Actual: the handler panics. The client sees a network error.

```
end byte index 10 is not a char boundary; it is inside '東' (bytes 9..12 of string)
Anthropic failed while streaming multibyte tool arguments: network error: error sending request for url (http://127.0.0.1:65295/v1/messages)
```

- Repro: `cargo test -p lsock-llm tool_arguments_with_multibyte_characters_stream -- --ignored --nocapture`
- Evidence: the panic and the client error above. The `--no-fail-fast` process exited 101. The test stops on the Anthropic kind, so the OpenAI-compatible route was not reached in this run. Line 229 is the same `split_at`.
- Regression: `tests::tool_arguments_with_multibyte_characters_stream` in `crates/llm/src/tests.rs`.
- Uncertainty: this is the bundled mock (`lsock llm mock-server` and `crates/llm/src/mock.rs`), which is what the CLI session used for ASCII `Berlin`. Live providers were not called. ASCII tool calls in that session succeeded.

### BUG-010 — cURL short options with the value in the same word are dropped

- Severity: major
- Area: `crates/convert/src/curl.rs:126`. The match is on the whole word (`-X`, `-d`, `-H`, `-u`). Line 188 ignores any other word that starts with `-`. `split_words` keeps `-XPUT` and `-d{"a":1}` as one word because a quote does not start a new word.
- Expected: `curl -XPUT https://ex.test/a` is PUT. The file's job, line 1, is "Parse a `curl` command line into a Request."
- Actual: method `GET`.
- Repro: `cargo test -p lsock-convert short_options_may_attach_their_value -- --ignored --nocapture`
- Evidence: exit 101 for the convert package inside the `--no-fail-fast` run.

```
assertion `left == right` failed: method GET
  left: "GET"
 right: "PUT"
```

- Regression: `curl::tests::short_options_may_attach_their_value` in `crates/convert/src/curl.rs`. The same test also asserts `-d'{"a":1}'`, `-H'K: v'`, and `-uuser:pw`. This run stopped at the method assertion, so those three were not printed.
- Uncertainty: spaced forms (`-X PUT`, `-u alice:pw`) already pass in `form_multipart_user_and_method`.

### BUG-011 — curl codegen emits `--data @…` for a text body

- Severity: major
- Area: `crates/convert/src/codegen.rs:127` uses `--data`. `sh` (lines 89–97) leaves `@` unquoted.
- Expected: a text body `@not-a-file` is sent as data. curl treats `--data @path` as a file read. `--data-raw` does not.
- Actual:

```
curl --request POST \
  --url https://ex.test/a \
  --data @not-a-file
```

- Repro: `cargo test -p lsock-convert curl_text_body_starting_with_at_is_not_a_file -- --ignored --nocapture`
- Evidence: the snippet above, from the panic at `crates/convert/src/codegen.rs:427`. Process exit 101 for that `--no-fail-fast` run.
- Regression: `codegen::tests::curl_text_body_starting_with_at_is_not_a_file` in `crates/convert/src/codegen.rs`.
- Uncertainty: the generated command was not executed. File bodies already use `--data-binary @path` at line 143.

### BUG-012 — `oneOf` is "any match", and `anyOf` skips `required`

- Severity: major
- Area: `crates/mcp/src/schema.rs:299`. The loop treats `anyOf` and `oneOf` as "at least one" and returns before later keywords.
- Expected: `oneOf` of `{type: string}` and `{minLength: 1}` rejects `"ab"` (it matches both). An object schema with `required: ["a"]` and an `anyOf` that does not itself require `a` still rejects `{}`.
- Actual: both calls return no errors (`both=[] missing=[]`).
- Repro: `cargo test -p lsock-mcp one_of_is_exclusive_and_sibling_keywords_still_apply -- --ignored --nocapture`
- Evidence: `both=[] missing=[]`. Process exit 101 for that run.
- Regression: `schema::tests::one_of_is_exclusive_and_sibling_keywords_still_apply` in `crates/mcp/src/schema.rs`.
- Uncertainty: `validate`'s own comment says "Lightweight validation". `required` is implemented a few lines later and is skipped whenever `anyOf` or `oneOf` is present.

### BUG-013 — a query string already on the URL is not the parameter list

- Severity: major
- Area: `crates/scripting/src/prelude.js:433` stores `raw.url` unchanged and a separate `_query`. `Url.toString` (lines 321–323) appends `_query` again when `_url` already contains `?`. `docs/FEATURES.md` line 105: query `add/upsert/remove` "maps to request parameters." `crates/http/src/lib.rs:136` also appends `parameters` onto a URL that already has a query.
- Expected: URL `https://ex.test/search?q=red` with an empty parameter list, then `ls.request.url.query.upsert({key:'q', value:'blue'})`, yields URL `https://ex.test/search` and a single parameter `q=blue`. `toString()` is `https://ex.test/search?q=blue`.
- Actual: the script throws `tostring https://ex.test/search?q=red&q=blue`.
- Repro: `cargo test -p lsock-scripting query_edits_apply_to_a_query_string_already_on_the_url -- --ignored --nocapture`
- Evidence:

```
script error: Some(ScriptError { message: "tostring https://ex.test/search?q=red&q=blue", line: Some(4), column: Some(58) })
```

Process exit 101 for that run.

- Regression: `tests::query_edits_apply_to_a_query_string_already_on_the_url` in `crates/scripting/src/tests.rs`.
- Uncertainty: this test stops in the script, so `apply_script_request` and the wire URL were not observed on a socket. Assigning `ls.request.url = '…?limit=5'` does split the query; that path is locked by `request_url_assignment_and_body_and_auth`.

### BUG-014 — `auth.update` with a flat parameter object stores empty credentials

- Severity: major
- Area: `crates/scripting/src/prelude.js:411` `RequestAuth.update`. The comment at line 415 accepts a nested `bearer` / `basic` object or a list. A flat `{username, password}` is copied as-is. `authFromScript` (line 393) then reads `s.basic`, which is missing, and fills `''`.
- Expected: `docs/FEATURES.md` line 109 marks `request.auth` `update` implemented. `ls.request.auth.update({ username: 'a', password: 'b' }, 'basic')` stores username `a` and password `b`.
- Actual:

```
left: Object {"type": String("basic"), "username": String(""), "password": String(""), "disabled": Bool(false)}
right: Object {"type": String("basic"), "username": String("a"), "password": String("b"), "disabled": Bool(false)}
```

- Repro: `cargo test -p lsock-scripting auth_update_accepts_a_flat_parameter_object -- --ignored --nocapture`
- Evidence: the assertion above. Process exit 101 for that run.
- Regression: `tests::auth_update_accepts_a_flat_parameter_object` in `crates/scripting/src/tests.rs`.
- Uncertainty: the nested form `auth.update({ type: 'bearer', bearer: [{ key: 'token', value }] })` is locked by `request_url_assignment_and_body_and_auth` and was not re-broken. The flat call replaces `inherit` with basic auth whose username and password are empty.

### BUG-015 — `ls.variables` lets a folder hide iteration data

- Severity: major
- Area: `crates/scripting/src/prelude.js:244`. The merge assigns iteration data and then each folder. `docs/FEATURES.md` line 103: precedence `local → iterationData → folder → environment → base → globals`. Line 35 lists the render order as folders, then iteration data, then local vars. `crates/engine/src/pipeline.rs:397` pushes iteration data after folder layers, so `{{ }}` lets the row win.
- Expected: folder `city=HQ` and row `city=Tokyo`. `ls.variables.get('city')` is `Tokyo`.
- Actual: `HQ`.
- Repro: `cargo test -p lsock-scripting iteration_data_outranks_folder_variables -- --ignored --nocapture`
- Evidence:

```
assertion `left == right` failed: folder hid the iteration row
  left: String("HQ")
 right: String("Tokyo")
```

Process exit 101 for that run.

- Regression: `tests::iteration_data_outranks_folder_variables` in `crates/scripting/src/tests.rs`.
- Uncertainty: `variables_follow_scope_precedence` (same file) currently expects `folderLevel2-value` over `iterationData-value`, and the prelude comment says folders outrank iteration data. That test and the comment disagree with both FEATURES lines and with the renderer. The ignored test asserts the FEATURES order.

### BUG-016 — OpenAPI cookie parameters are discarded

- Severity: major
- Area: `crates/convert/src/openapi.rs:409`. The `in` match handles `path`, `query`, `header`, `body`, and `formData`. Any other value, including `cookie`, hits `_ => {}`.
- Expected: `docs/FEATURES.md` lists OpenAPI 3 import as implemented. A parameter `"in": "cookie"` named `sid` is kept (header or parameter). The same importer already maps an apiKey `in: cookie` onto `add_to`.
- Actual: headers `[]`, params `[]`.
- Repro: `cargo test -p lsock-convert openapi_cookie_parameter_is_imported -- --ignored --nocapture`
- Evidence:

```
cookie parameter dropped
headers=[]
params=[]
```

The convert package failed in the `--no-fail-fast` run, exit 101. That run registered the test twice because of a duplicated `#[test]` attribute; the attribute is now once, and the assertion text is unchanged. The workspace run after the fix ignores exactly one copy (`tests::openapi_cookie_parameter_is_imported`).

- Regression: `tests::openapi_cookie_parameter_is_imported` in `crates/convert/src/tests.rs`.
- Uncertainty: the isolated command was not re-run after removing the extra attribute. The panic text is from the double-registered run and is the same assertion.

### BUG-017 — the LLM SSE parser drops events separated by bare CR

- Severity: minor
- Area: `crates/llm/src/sse.rs:17`. Only `\r\n` is rewritten. A bare CR never becomes the `\n\n` the parser searches for.
- Expected: `event: ping\rdata: hello\r\r` is one event named `ping` with data `hello`.
- Actual: `[]`.
- Repro: `cargo test -p lsock-llm bare_cr_ends_an_event -- --ignored --nocapture`
- Evidence: panic printed `[]`. Process exit 101 for that run.
- Regression: `sse::tests::bare_cr_ends_an_event` in `crates/llm/src/sse.rs`.
- Uncertainty: Anthropic frames in the existing test use `\n` and `\r\n`, and those still pass. A server that sends only CR is the case that fails. The realtime parser (BUG-008) does convert bare CR, and converts it too early.

## 5. Passing tests added

These are not ignored. They passed inside `cargo test --workspace` (exit 0, `/tmp/lsock-workspace-final.log`) and `npm test` (exit 0).

| Test | Where | Result line |
|---|---|---|
| `cookies::tests::domain_suffix_requires_a_dot_boundary_and_path_does_not_prefix_match` | `crates/http/src/cookies.rs` | `... ok`. `notexample.com` gets no cookie, path `/foo` does not match `/foobar`, `www.example.com/foo/x` sends `sid=abc`. |
| `schema::tests::additional_properties_false_rejects_unknown_keys` | `crates/mcp/src/schema.rs` | `... ok` |
| Vitest `formats sizes and durations` (`formatBytes(0)`), `colors status classes and shortens method names`, `decodes plus as a space in query values` | `apps/desktop/src/lib/utils.test.ts` | `npm test`: 7 passed, 2 skipped |

Commands:

```
cargo test -p lsock-http domain_suffix_requires_a_dot_boundary_and_path_does_not_prefix_match
cargo test -p lsock-mcp additional_properties_false_rejects_unknown_keys
cd apps/desktop && npm test
```

`npm` needs `export PATH="$HOME/.local/share/fnm/aliases/default/bin:$PATH"` on this machine.

## 6. Not verified

- Desktop UI in a browser. No browser tool is available. Not started: `cargo run -p logic-socket-desktop --example web_bridge` and `VITE_WEB_BRIDGE=1 npm run dev`. Not clicked: send, environment edit, MCP tool call, import, runner. Not viewed: 1440px or 375px.
- MCP OAuth on Streamable HTTP (documented as unwired).
- LLM sampling (`ask_llm` was not called).
- Multipart send, form-body send, request timeout, proxy, folder-inherited headers, and sub-environments from the CLI.
- Template tags `uuid`, `now`, `base64`, `hash`, `response` from the CLI.
- Script `sendRequest`, `skipRequest`, `setNextRequest`, and the per-script timeout from the CLI.
- Runner CSV, delay, bail, and a successful `spec` reporter run from the CLI. Junit XML file contents were not read; the `run-junit` process exited 0.
- gRPC reflection bytes. The method list and the four demo RPCs were run.
- cURL import from the CLI. The library parser was tested (BUG-010).
- Implicit grant, Hawk, NTLM, ASAP, WSDL, client certificates, `ls.vault`, and Node built-ins. `docs/FEATURES.md` marks these unsupported.
- Live Anthropic, OpenAI, or any other hosted model. The OS keychain was not read or written.
- A second isolated run of `openapi_cookie_parameter_is_imported` after the duplicate `#[test]` attribute was removed. The assertion output above is from the run that registered it twice.

Code that was read and not locked with a failing test is not listed as a bug.

## 7. How to re-run

Default, must stay green. Ignored tests do not run.

```
cargo test --workspace
export PATH="$HOME/.local/share/fnm/aliases/default/bin:$PATH"
cd apps/desktop && npm test && npm run type-check
```

Observed after these tests: cargo exit 0 (162 passed, 0 failed, 15 ignored). npm test exit 0 (7 passed, 2 skipped). type-check exit 0.

Ignored bug tests. Each cargo failure exits 101. The Vitest command exits 1.

```
cargo test -p lsock-http public_suffix_domain_is_not_stored_or_sent -- --ignored --nocapture
cargo test -p lsock-engine netrc_ignores_comments_and_macdef_bodies -- --ignored --nocapture
cargo test -p lsock-mcp exclusive_numeric_bounds_are_exclusive -- --ignored --nocapture
cargo test -p lsock-mcp displayed_pattern_items_and_uniqueness_are_enforced -- --ignored --nocapture
cargo test -p lsock-cli --test secret_env_json -- --ignored --nocapture
cargo test -p lsock-realtime crlf_split_after_the_carriage_return_stays_one_event -- --ignored --nocapture
cargo test -p lsock-llm tool_arguments_with_multibyte_characters_stream -- --ignored --nocapture
cargo test -p lsock-llm bare_cr_ends_an_event -- --ignored --nocapture
cargo test -p lsock-convert short_options_may_attach_their_value -- --ignored --nocapture
cargo test -p lsock-convert curl_text_body_starting_with_at_is_not_a_file -- --ignored --nocapture
cargo test -p lsock-convert openapi_cookie_parameter_is_imported -- --ignored --nocapture
cargo test -p lsock-mcp one_of_is_exclusive_and_sibling_keywords_still_apply -- --ignored --nocapture
cargo test -p lsock-scripting query_edits_apply_to_a_query_string_already_on_the_url -- --ignored --nocapture
cargo test -p lsock-scripting auth_update_accepts_a_flat_parameter_object -- --ignored --nocapture
cargo test -p lsock-scripting iteration_data_outranks_folder_variables -- --ignored --nocapture
cd apps/desktop && RUN_IGNORED=1 npx vitest run -t 'BUG-00'
```

`cargo test` stops at the first failing package when several `-p` flags are combined. Use `--no-fail-fast` to collect every ignored failure in one invocation.

Set `LSOCK_DATA_DIR` to a fresh directory, `LSOCK_SECRET_STORE=memory`, and `LSOCK_VAULT_KEY` before any manual `lsock` command. Do not point those commands at the real application-support directory.
