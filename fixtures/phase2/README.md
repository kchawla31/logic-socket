# Phase 2 sample collection

Exercises scripts, tests, data-driven iterations and the JUnit reporter by
testing the bundled mock MCP server over raw HTTP JSON-RPC.

```bash
cargo build -p lsock-cli -p lsock-mcp
./target/debug/lsock-mock-mcp --http 3333 &          # from logic-socket/
export LSOCK_DATA_DIR=$(mktemp -d)
./fixtures/phase2/seed.sh
./target/debug/lsock run collection "MCP smoke test" -d fixtures/phase2/cities.csv
./target/debug/lsock run collection "MCP smoke test" -d fixtures/phase2/cities.csv -r junit -o report.xml
```
