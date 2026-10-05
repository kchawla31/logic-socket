# Phase 2 sample collection

Exercises scripts, tests, data-driven iterations and the JUnit reporter by
testing the bundled mock MCP server over raw HTTP JSON-RPC.

```bash
cargo build -p irs-cli -p irs-mcp
./target/debug/irs-mock-mcp --http 3333 &          # from insomnia-rs/
export IRS_DATA_DIR=$(mktemp -d)
./fixtures/phase2/seed.sh
./target/debug/irs run collection "MCP smoke test" -d fixtures/phase2/cities.csv
./target/debug/irs run collection "MCP smoke test" -d fixtures/phase2/cities.csv -r junit -o report.xml
```
