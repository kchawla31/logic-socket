//! Demo MCP server. `irs-mock-mcp` speaks stdio; `irs-mock-mcp --http 3333`
//! serves Streamable HTTP at http://127.0.0.1:3333/mcp
//! (add `--token secret` to require `Authorization: Bearer secret`).

use irs_mcp::mock::{HttpState, handle, spawn_http};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if let Some(port) = flag("--http") {
        let state = flag("--token")
            .map(HttpState::with_token)
            .unwrap_or_default();
        let url = spawn_http(state, port.parse().unwrap_or(3333)).await?;
        eprintln!("irs-mock-mcp listening on {url}");
        tokio::signal::ctrl_c().await?;
        return Ok(());
    }

    eprintln!("irs-mock-mcp ready on stdio");
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    let write = async |v: &Value, out: &mut tokio::io::Stdout| -> std::io::Result<()> {
        out.write_all(format!("{v}\n").as_bytes()).await?;
        out.flush().await
    };
    while let Some(line) = lines.next_line().await? {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            eprintln!("ignoring non-JSON input");
            continue;
        };
        if msg["method"] == "tools/call" && msg["params"]["name"] == "ask_roots" {
            write(
                &json!({"jsonrpc": "2.0", "id": "srv-1", "method": "roots/list"}),
                &mut out,
            )
            .await?;
            let mut roots = Value::Null;
            while let Some(l) = lines.next_line().await? {
                let v: Value = serde_json::from_str(&l).unwrap_or(Value::Null);
                if v["id"] == "srv-1" {
                    roots = v["result"]["roots"].clone();
                    break;
                }
            }
            let text = roots.to_string();
            write(&json!({"jsonrpc": "2.0", "id": msg["id"], "result": {"content": [{"type": "text", "text": text}]}}), &mut out)
                .await?;
            continue;
        }
        if msg["method"] == "tools/call" && msg["params"]["name"] == "ask_llm" {
            let prompt = msg["params"]["arguments"]["prompt"]
                .as_str()
                .unwrap_or("hello")
                .to_string();
            write(
                &json!({"jsonrpc": "2.0", "id": "srv-2", "method": "sampling/createMessage", "params": {
                    "messages": [{"role": "user", "content": {"type": "text", "text": prompt}}],
                    "systemPrompt": "Answer briefly.", "maxTokens": 200
                }}),
                &mut out,
            )
            .await?;
            let mut answer =
                json!({"content": [{"type": "text", "text": "no answer"}], "isError": true});
            while let Some(l) = lines.next_line().await? {
                let v: Value = serde_json::from_str(&l).unwrap_or(Value::Null);
                if v["id"] == "srv-2" {
                    answer = match v.get("result") {
                        Some(r) => {
                            json!({"content": [{"type": "text", "text": format!("LLM said: {}", r["content"]["text"].as_str().unwrap_or(""))}]})
                        }
                        None => {
                            json!({"content": [{"type": "text", "text": format!("sampling failed: {}", v["error"]["message"])}], "isError": true})
                        }
                    };
                    break;
                }
            }
            write(
                &json!({"jsonrpc": "2.0", "id": msg["id"], "result": answer}),
                &mut out,
            )
            .await?;
            continue;
        }
        let reply = handle(&msg).await;
        for n in &reply.before {
            write(n, &mut out).await?;
        }
        if let Some(r) = reply.response {
            write(&r, &mut out).await?;
        }
    }
    Ok(())
}
