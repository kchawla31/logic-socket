//! Demo gRPC server (`demo.Greeter`) with server reflection — used by tests
//! and `irs mock grpc`. Implemented with dynamic messages, no codegen.

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::{Stream, StreamExt};
use prost_reflect::{DynamicMessage, MessageDescriptor, Value as PValue};
use tonic::body::Body;
use tonic::server::NamedService;
use tonic::{Request, Response, Status, Streaming};

use crate::{DynamicCodec, Schema};

pub const DEMO_PROTO: &str = include_str!("../protos/demo.proto");

pub fn schema() -> Schema {
    Schema::from_protos(&[("demo.proto".into(), DEMO_PROTO.into())]).expect("demo.proto compiles")
}

type BoxStream = Pin<Box<dyn Stream<Item = Result<DynamicMessage, Status>> + Send>>;
type BoxFut<T> = Pin<Box<dyn std::future::Future<Output = Result<T, Status>> + Send>>;

#[derive(Clone)]
pub struct Greeter {
    schema: Schema,
}

impl NamedService for Greeter {
    const NAME: &'static str = "demo.Greeter";
}

fn desc(s: &Schema, name: &str) -> MessageDescriptor {
    s.pool.get_message_by_name(name).expect("demo message")
}

fn msg(s: &Schema, name: &str, fields: &[(&str, PValue)]) -> DynamicMessage {
    let mut m = DynamicMessage::new(desc(s, name));
    for (k, v) in fields {
        m.set_field_by_name(k, v.clone());
    }
    m
}

fn get_str(m: &DynamicMessage, f: &str) -> String {
    m.get_field_by_name(f).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn get_i64(m: &DynamicMessage, f: &str) -> i64 {
    m.get_field_by_name(f).and_then(|v| v.as_i64().or(v.as_i32().map(i64::from))).unwrap_or(0)
}

struct Unary(Schema);
impl tonic::server::UnaryService<DynamicMessage> for Unary {
    type Response = DynamicMessage;
    type Future = BoxFut<Response<DynamicMessage>>;
    fn call(&mut self, req: Request<DynamicMessage>) -> Self::Future {
        let s = self.0.clone();
        Box::pin(async move {
            if req.metadata().get("x-fail").is_some() {
                return Err(Status::permission_denied("x-fail header set"));
            }
            let name = get_str(req.get_ref(), "name");
            if name.is_empty() {
                return Err(Status::invalid_argument("name is required"));
            }
            let text = format!("Hello, {name}!");
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
            let ts = msg(&s, "google.protobuf.Timestamp", &[("seconds", PValue::I64(now.as_secs() as i64)), ("nanos", PValue::I32(0))]);
            let reply = msg(&s, "demo.HelloReply", &[("message", PValue::String(text.clone())), ("length", PValue::I32(text.len() as i32)), ("at", PValue::Message(ts))]);
            let mut r = Response::new(reply);
            r.metadata_mut().insert("x-served-by", "irs-demo".parse().unwrap());
            Ok(r)
        })
    }
}

struct Count(Schema);
impl tonic::server::ServerStreamingService<DynamicMessage> for Count {
    type Response = DynamicMessage;
    type ResponseStream = BoxStream;
    type Future = BoxFut<Response<BoxStream>>;
    fn call(&mut self, req: Request<DynamicMessage>) -> Self::Future {
        let s = self.0.clone();
        Box::pin(async move {
            let to = get_i64(req.get_ref(), "to").clamp(0, 100);
            let items: Vec<Result<DynamicMessage, Status>> = (1..=to).map(|i| Ok(msg(&s, "demo.Number", &[("value", PValue::I64(i))]))).collect();
            Ok(Response::new(Box::pin(futures::stream::iter(items)) as BoxStream))
        })
    }
}

struct Sum(Schema);
impl tonic::server::ClientStreamingService<DynamicMessage> for Sum {
    type Response = DynamicMessage;
    type Future = BoxFut<Response<DynamicMessage>>;
    fn call(&mut self, req: Request<Streaming<DynamicMessage>>) -> Self::Future {
        let s = self.0.clone();
        Box::pin(async move {
            let mut stream = req.into_inner();
            let mut total = 0i64;
            while let Some(m) = stream.message().await? {
                total += get_i64(&m, "value");
            }
            Ok(Response::new(msg(&s, "demo.Number", &[("value", PValue::I64(total))])))
        })
    }
}

struct Chat(Schema);
impl tonic::server::StreamingService<DynamicMessage> for Chat {
    type Response = DynamicMessage;
    type ResponseStream = BoxStream;
    type Future = BoxFut<Response<BoxStream>>;
    fn call(&mut self, req: Request<Streaming<DynamicMessage>>) -> Self::Future {
        let s = self.0.clone();
        Box::pin(async move {
            let stream = req.into_inner().map(move |m| {
                let m = m?;
                Ok(msg(&s, "demo.ChatMessage", &[("user", PValue::String("bot".into())), ("text", PValue::String(format!("you said: {}", get_str(&m, "text"))))]))
            });
            Ok(Response::new(Box::pin(stream) as BoxStream))
        })
    }
}

impl<B> tower_service::Service<http::Request<B>> for Greeter
where
    B: http_body::Body + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>> + Send + 'static,
{
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        let s = self.schema.clone();
        let path = req.uri().path().to_string();
        Box::pin(async move {
            let input = |m: &str| DynamicCodec::new(desc(&s, m));
            let res = match path.as_str() {
                "/demo.Greeter/SayHello" => tonic::server::Grpc::new(input("demo.HelloRequest")).unary(Unary(s.clone()), req).await,
                "/demo.Greeter/CountUp" => tonic::server::Grpc::new(input("demo.CountRequest")).server_streaming(Count(s.clone()), req).await,
                "/demo.Greeter/Sum" => tonic::server::Grpc::new(input("demo.Number")).client_streaming(Sum(s.clone()), req).await,
                "/demo.Greeter/Chat" => tonic::server::Grpc::new(input("demo.ChatMessage")).streaming(Chat(s.clone()), req).await,
                _ => Status::unimplemented(format!("no method {path}")).into_http(),
            };
            Ok(res)
        })
    }
}

/// Serve the demo service (+ reflection) on 127.0.0.1:`port` (0 = random). Returns `grpc://host:port`.
pub async fn spawn(port: u16, with_reflection: bool) -> std::io::Result<String> {
    let schema = schema();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;
    let fds = schema.pool.encode_to_vec();
    let greeter = Greeter { schema };
    tokio::spawn(async move {
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        let mut router = tonic::transport::Server::builder().add_service(greeter);
        if with_reflection {
            let reflection = tonic_reflection::server::Builder::configure().register_encoded_file_descriptor_set(&fds).build_v1().expect("reflection");
            router = router.add_service(reflection);
        }
        let _ = router.serve_with_incoming(incoming).await;
    });
    Ok(format!("grpc://{addr}"))
}
