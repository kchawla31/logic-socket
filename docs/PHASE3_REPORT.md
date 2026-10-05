# Phase 3 report — LLM providers and the LLM ↔ MCP bridge

## What was built

| Area | Where | Notes |
|---|---|---|
| Providers | `crates/llm` | Streaming Anthropic Messages (text, tool_use, thinking + signature replay, usage, stop reasons, errors) and OpenAI Chat Completions (OpenAI, Ollama, OpenRouter, Gemini OpenAI endpoint, LM Studio, vLLM); `max_completion_tokens` vs `max_tokens`; model listing; readable API errors with hints |
| Agent (bridge) | `crates/llm/src/agent.rs` | MCP tools exposed with unique, valid names (server prefix when several), descriptions tagged read-only/destructive, approval gate (`none` / `read-only` / `all`), unknown-tool and failure handling, max-turn guard that keeps transcripts valid, cancel, per-turn request bodies |
| Sampling | `irs-mcp` + `LlmSampler` | Client advertises `sampling` and answers `sampling/createMessage` from a task (dispatcher never blocks); opt-in per saved server with provider/model/max tokens; readable refusal when disabled |
| Storage | `irs-core` | `LlmProvider` (key source: keychain / env var / env template / none), `LlmRequest` (prompt with `{{ vars }}`, model, tools, approval policy, limits), `LlmRun` history (transcript, tool calls + results, tokens, TTFT, total, exact payloads), `McpServer.sampling` |
| Engine | `crates/engine/src/llm.rs` | Key resolution with fix-it messages, prompt rendering, MCP tool sources, run + persist with history pruning; keychain via `keyring` (memory store for tests / `IRS_SECRET_STORE=memory`) |
| CLI | `irs llm` | `provider add/list/remove`, `models`, `chat` (streaming, tool cards, terminal approval, `--mcp`, `--mcp-url`, `--mcp-stdio`, `--yes`, `--json`), `request add/run`, `mock-server` |
| Desktop | `apps/desktop` | AI providers dialog (keychain key entry, test connection, model suggestions), "AI request" item (sidebar, palette, welcome), editor (provider/model, system, multi-message prompt, MCP tool checklist, approval policy, limits), live conversation (streaming markdown, thinking, tool cards with arguments/results/latency, Approve/Deny for risky tools, stop), history, request trace, usage bar; MCP server Sampling settings tab |

## Verification (actual results)

- `cargo clippy --workspace --all-targets -D warnings`: clean
- `cargo test --workspace`: **102 passed, 0 failed** — new: llm 10 (both wire formats streaming, tool round-trip incl. thinking signature, auth errors + key never in body, model list, agent with real MCP server over HTTP for both providers, approval/denial, multi-server naming, max turns, sampling over SSE enabled/disabled), engine 3 (key sources + messages, rendered AI request with MCP tools persisted without secrets, saved-server sampling), desktop IPC 1 (providers never expose keys, run with UI approval of a destructive tool)
- CLI end-to-end against the mock LLM + mock MCP server (both formats, auto tool call, non-interactive denial, saved request with `{{ city }}`)
- Desktop UI driven headlessly through the web bridge: AI request run with tool call, request trace, destructive-tool approval → approve → answer, providers dialog, dark theme — screenshots reviewed
- **Not verified against real providers**: no API key or Ollama on this machine (D23)

## Bugs found and fixed during verification

- Event enums serialized variant fields in snake_case (`needs_approval`) → approvals never showed in the UI (D24); also fixed latent runner `requestId`
- Mock MCP needed real server→client requests over SSE to test sampling (implemented properly rather than stubbing)
- User bubble showed the unrendered `{{ template }}` after a run → rebuilt from the stored, rendered transcript

## Known gaps

- MCP elicitation (`elicitation/create`) still answered with "not supported"
- No image/audio inputs to models; tool image outputs are summarised as text for the model
- No per-run cost estimates (token counts only); no prompt caching controls
- Scripts can't call LLMs yet (`insomnia.llm`), and AI requests aren't part of collection runs
- Thinking/effort controls for Claude 5 models and OpenAI reasoning effort are not exposed in the UI

## Try it with a real key

```bash
irs llm provider add Claude --kind anthropic --model claude-sonnet-5-5     # prompts for the key → OS keychain
irs llm chat "Which tools do you have?" --mcp-stdio "npx -y @modelcontextprotocol/server-everything"
```
