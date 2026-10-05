//! `irs grpc` — list and call gRPC methods (reflection or .proto files).

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use clap::Args;
use irs_core::GrpcRequest;
use irs_engine::Engine;
use irs_grpc::Schema;
use irs_realtime::Direction;

use crate::out::*;

#[derive(Args, Debug)]
pub struct GrpcArgs {
    /// Server URL (grpc://host:port or grpcs://…) or a saved gRPC request (name/id)
    pub target: String,
    /// .proto files to use instead of server reflection (repeatable)
    #[arg(long = "proto")]
    pub protos: Vec<std::path::PathBuf>,
    /// List services and methods
    #[arg(short, long)]
    pub list: bool,
    /// Method: Service/Method or package.Service/Method
    #[arg(short, long)]
    pub method: Option<String>,
    /// JSON request (unary / server streaming)
    #[arg(short, long, default_value = "{}")]
    pub data: String,
    /// Message to send on a client/bidi stream (repeatable, sent in order)
    #[arg(long)]
    pub send: Vec<String>,
    /// Metadata "key: value" (repeatable)
    #[arg(short = 'H', long = "header")]
    pub headers: Vec<String>,
}

pub async fn run(engine: &Engine, a: GrpcArgs) -> Result<()> {
    let saved = crate::find::<GrpcRequest>(engine, &a.target).ok();
    let (url, schema, method, data, metadata) = match &saved {
        Some(r) => {
            let p = engine.grpc_prepare(r.id())?;
            (
                p.url,
                engine.grpc_schema(r.id()).await?,
                a.method.clone().unwrap_or(p.method),
                p.body,
                p.metadata,
            )
        }
        None => {
            let schema = if a.protos.is_empty() {
                irs_grpc::reflect(irs_grpc::connect(&a.target).await?)
                    .await
                    .map_err(|e| {
                        anyhow!("{e} (pass --proto files if the server has no reflection)")
                    })?
            } else {
                let dir = a.protos[0]
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default();
                let files = a
                    .protos
                    .iter()
                    .map(|p| {
                        let name = p
                            .strip_prefix(&dir)
                            .unwrap_or(p)
                            .to_string_lossy()
                            .into_owned();
                        Ok((name, std::fs::read_to_string(p)?))
                    })
                    .collect::<std::io::Result<Vec<_>>>()?;
                Schema::from_protos(&files)?
            };
            let md = a
                .headers
                .iter()
                .map(|h| {
                    h.split_once(':')
                        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                        .ok_or_else(|| anyhow!("metadata must be 'key: value'"))
                })
                .collect::<Result<Vec<_>>>()?;
            (
                a.target.clone(),
                schema,
                a.method.clone().unwrap_or_default(),
                a.data.clone(),
                md,
            )
        }
    };

    if a.list || method.is_empty() {
        for s in schema.services() {
            println!("{}", bold(&s.name));
            for m in s.methods {
                let kind = match (m.client_streaming, m.server_streaming) {
                    (true, true) => magenta("bidi"),
                    (true, false) => blue("client stream"),
                    (false, true) => green("server stream"),
                    _ => dim("unary"),
                };
                println!(
                    "  {} {kind}  {} → {}",
                    cyan(&m.name),
                    dim(&m.input_type),
                    dim(&m.output_type)
                );
                println!("    {}", dim(&format!("example: {}", m.example)));
            }
        }
        return Ok(());
    }

    let path = if method.matches('/').count() == 1 && !method.contains('.') {
        // Service/Method without package: resolve by suffix
        let (svc, m) = method.split_once('/').unwrap();
        schema
            .services()
            .into_iter()
            .find(|s| s.name.ends_with(svc))
            .map(|s| format!("{}/{m}", s.name))
            .unwrap_or(method.clone())
    } else {
        method.clone()
    };
    let m = schema.method(&path)?;
    let channel = irs_grpc::connect(&url).await?;
    if !m.is_client_streaming() && !m.is_server_streaming() {
        let r = irs_grpc::unary(channel, &m, &data, &metadata, Duration::from_secs(30)).await?;
        let status = format!("{} {}", r.status.code, r.status.code_name);
        eprintln!(
            "{} {} {}",
            if r.status.code == 0 {
                green(&status)
            } else {
                red(&status)
            },
            dim(&format!("{:.0} ms", r.latency_ms)),
            r.status.message
        );
        if let Some(resp) = r.response {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        if r.status.code != 0 {
            std::process::exit(1);
        }
        return Ok(());
    }
    let mut call = irs_grpc::start_stream(channel, &m, Some(&data), &metadata).await?;
    let mut rx = call.log.subscribe();
    let earlier = call.log.entries();
    let printer = tokio::spawn(async move {
        let mut failed = false;
        let mut last = 0;
        let mut queue: std::collections::VecDeque<irs_realtime::RtEvent> = earlier.into();
        loop {
            let e = match queue.pop_front() {
                Some(e) => e,
                None => match rx.recv().await {
                    Ok(e) => e,
                    Err(_) => break,
                },
            };
            if e.seq <= last {
                continue;
            }
            last = e.seq;
            match e.direction {
                Direction::In => println!("{} {}", green("↓"), e.data),
                Direction::Out => println!("{} {}", blue("↑"), e.data),
                Direction::Error => {
                    failed = true;
                    eprintln!("{} {}", red("✗"), e.data)
                }
                Direction::Info => eprintln!("{}", dim(&e.data)),
            }
            if e.kind == "close" || e.direction == Direction::Error {
                break;
            }
        }
        failed
    });
    if m.is_client_streaming() {
        for s in &a.send {
            call.send(s)?;
        }
        call.commit();
    }
    for _ in 0..600 {
        if call.is_done() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let failed = tokio::time::timeout(Duration::from_secs(2), printer)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or(false);
    if failed {
        bail!("stream ended with an error");
    }
    Ok(())
}
