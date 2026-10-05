#!/usr/bin/env bash
# Seed the Phase 2 sample collection into $IRS_DATA_DIR (see README.md).
set -euo pipefail
cd "$(dirname "$0")/../.."
IRS=${IRS:-./target/debug/irs}
F=fixtures/phase2
$IRS workspace create "MCP smoke test" >/dev/null
$IRS env set "MCP smoke test" base_url '"http://127.0.0.1:3333"' >/dev/null
$IRS request folder "MCP smoke test" "JSON-RPC" >/dev/null
$IRS request script "JSON-RPC" --pre $F/folder-pre.js >/dev/null
$IRS request add "JSON-RPC" "Initialize" POST '{{ _.base_url }}/mcp' --json '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"irs-runner","version":"1"}}}' >/dev/null
$IRS request script "Initialize" --after $F/initialize-after.js >/dev/null
$IRS request add "JSON-RPC" "List tools" POST '{{ _.base_url }}/mcp' --json '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' >/dev/null
$IRS request script "List tools" --after $F/list-after.js >/dev/null
$IRS request add "JSON-RPC" "Weather" POST '{{ _.base_url }}/mcp' --json '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_weather","arguments":{"city":"{{ city }}","units":"{{ units }}"}}}' >/dev/null
$IRS request script "Weather" --after $F/weather-after.js >/dev/null
$IRS workspace tree "MCP smoke test"
