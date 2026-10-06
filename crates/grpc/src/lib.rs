//! Dynamic gRPC client: schemas from `.proto` sources (compiled in-process by
//! protox) or server reflection, JSON ↔ protobuf via prost-reflect, and all
//! four call kinds without code generation.

pub mod demo;

use std::time::{Duration, Instant};

use futures::StreamExt;
use lsock_realtime::{Direction, EventLog};
use prost::Message as _;
use prost_reflect::{
    DescriptorPool, DeserializeOptions, DynamicMessage, MessageDescriptor, MethodDescriptor,
    SerializeOptions,
};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Code, Status};

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum GrpcError {
    #[error("proto error: {0}")]
    Proto(String),
    #[error("reflection: {0}")]
    Reflection(String),
    #[error("could not connect to {0}: {1}")]
    Connect(String, String),
    #[error("no method {0} in the schema")]
    UnknownMethod(String),
    #[error("request body does not match {0}: {1}")]
    Body(String, String),
    #[error("{0}")]
    Other(String),
}

// ---------------------------------------------------------------- schema

#[derive(Debug, Clone)]
pub struct Schema {
    pub pool: DescriptorPool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MethodInfo {
    pub name: String,
    /// `/package.Service/Method`
    pub path: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
    pub input_type: String,
    pub output_type: String,
    /// JSON template for the request message (all fields with defaults).
    pub example: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfo {
    pub name: String,
    pub methods: Vec<MethodInfo>,
}

impl Schema {
    /// Compile `.proto` sources: `(file name, contents)`; imports resolve among them
    /// and Google well-known types.
    pub fn from_protos(files: &[(String, String)]) -> Result<Schema, GrpcError> {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "lsock-protos-{}-{nanos}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).map_err(|e| GrpcError::Proto(e.to_string()))?;
        let result = (|| {
            for (name, contents) in files {
                let p = dir.join(name);
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| GrpcError::Proto(e.to_string()))?;
                }
                std::fs::write(&p, contents).map_err(|e| GrpcError::Proto(e.to_string()))?;
            }
            let names: Vec<&String> = files.iter().map(|(n, _)| n).collect();
            let fds = protox::compile(names, [&dir])
                .map_err(|e| GrpcError::Proto(describe_proto_error(&e, files)))?;
            let pool = DescriptorPool::from_file_descriptor_set(fds)
                .map_err(|e| GrpcError::Proto(e.to_string()))?;
            Ok(Schema { pool })
        })();
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    pub fn from_file_descriptor_set(bytes: &[u8]) -> Result<Schema, GrpcError> {
        Ok(Schema {
            pool: DescriptorPool::decode(bytes).map_err(|e| GrpcError::Proto(e.to_string()))?,
        })
    }

    pub fn services(&self) -> Vec<ServiceInfo> {
        self.pool
            .services()
            .filter(|s| !s.full_name().starts_with("grpc.reflection."))
            .map(|s| ServiceInfo {
                name: s.full_name().to_string(),
                methods: s
                    .methods()
                    .map(|m| MethodInfo {
                        name: m.name().to_string(),
                        path: format!("/{}/{}", s.full_name(), m.name()),
                        client_streaming: m.is_client_streaming(),
                        server_streaming: m.is_server_streaming(),
                        input_type: m.input().full_name().to_string(),
                        output_type: m.output().full_name().to_string(),
                        example: example_json(&m.input()),
                    })
                    .collect(),
            })
            .collect()
    }

    pub fn method(&self, path: &str) -> Result<MethodDescriptor, GrpcError> {
        let p = path.trim_start_matches('/');
        let (svc, m) = p
            .rsplit_once('/')
            .ok_or_else(|| GrpcError::UnknownMethod(path.into()))?;
        self.pool
            .get_service_by_name(svc)
            .and_then(|s| s.methods().find(|x| x.name() == m))
            .ok_or_else(|| GrpcError::UnknownMethod(path.into()))
    }
}

/// `file.proto:LINE:COL: message` using the diagnostic's first label.
fn describe_proto_error(e: &protox::Error, files: &[(String, String)]) -> String {
    let file = e.file().map(str::to_string);
    let offset = miette::Diagnostic::labels(e)
        .and_then(|mut l| l.next())
        .map(|l| l.offset());
    let pos = match (&file, offset) {
        (Some(f), Some(off)) => files.iter().find(|(n, _)| n == f).map(|(_, src)| {
            let before = &src[..off.min(src.len())];
            let line = before.matches('\n').count() + 1;
            let col = before
                .rsplit('\n')
                .next()
                .map(|l| l.chars().count() + 1)
                .unwrap_or(1);
            format!(":{line}:{col}")
        }),
        _ => None,
    };
    match file {
        Some(f) => format!("{f}{}: {e}", pos.unwrap_or_default()),
        None => e.to_string(),
    }
}

fn ser_opts() -> SerializeOptions {
    SerializeOptions::new()
        .skip_default_fields(false)
        .stringify_64_bit_integers(false)
}

/// A JSON object with every field of `desc` set to its default (one level of nesting).
pub fn example_json(desc: &MessageDescriptor) -> Value {
    let msg = DynamicMessage::new(desc.clone());
    serde_json::to_value(SerWrap(&msg)).unwrap_or(Value::Object(Default::default()))
}

struct SerWrap<'a>(&'a DynamicMessage);
impl Serialize for SerWrap<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize_with_options(s, &ser_opts())
    }
}

pub fn to_json(msg: &DynamicMessage) -> Value {
    serde_json::to_value(SerWrap(msg)).unwrap_or(Value::Null)
}

pub fn from_json(desc: &MessageDescriptor, json: &str) -> Result<DynamicMessage, GrpcError> {
    let text = if json.trim().is_empty() { "{}" } else { json };
    let mut de = serde_json::Deserializer::from_str(text);
    let msg = DynamicMessage::deserialize_with_options(
        desc.clone(),
        &mut de,
        &DeserializeOptions::new().deny_unknown_fields(true),
    )
    .map_err(|e| GrpcError::Body(desc.full_name().to_string(), e.to_string()))?;
    de.end()
        .map_err(|e| GrpcError::Body(desc.full_name().to_string(), e.to_string()))?;
    Ok(msg)
}

// ---------------------------------------------------------------- codec

/// Encodes any DynamicMessage; decodes into `decode`'s message type.
#[derive(Clone)]
pub struct DynamicCodec {
    decode: MessageDescriptor,
}

impl DynamicCodec {
    pub fn new(decode: MessageDescriptor) -> Self {
        Self { decode }
    }
}

pub struct DynEncoder;
pub struct DynDecoder(MessageDescriptor);

impl Encoder for DynEncoder {
    type Item = DynamicMessage;
    type Error = Status;
    fn encode(&mut self, item: DynamicMessage, dst: &mut EncodeBuf<'_>) -> Result<(), Status> {
        item.encode(dst)
            .map_err(|e| Status::internal(e.to_string()))
    }
}

impl Decoder for DynDecoder {
    type Item = DynamicMessage;
    type Error = Status;
    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<DynamicMessage>, Status> {
        DynamicMessage::decode(self.0.clone(), src)
            .map(Some)
            .map_err(|e| Status::internal(e.to_string()))
    }
}

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynEncoder;
    type Decoder = DynDecoder;
    fn encoder(&mut self) -> DynEncoder {
        DynEncoder
    }
    fn decoder(&mut self) -> DynDecoder {
        DynDecoder(self.decode.clone())
    }
}

// ---------------------------------------------------------------- connection

/// `grpc://` / `http://` = plaintext, `grpcs://` / `https://` = TLS (webpki roots).
pub async fn connect(url: &str) -> Result<Channel, GrpcError> {
    let (scheme, rest) = url.split_once("://").unwrap_or(("grpc", url));
    let tls = matches!(scheme, "grpcs" | "https");
    let http_url = format!("{}://{rest}", if tls { "https" } else { "http" });
    let mut ep = Endpoint::from_shared(http_url.clone())
        .map_err(|e| GrpcError::Connect(url.into(), e.to_string()))?
        .connect_timeout(Duration::from_secs(15));
    if tls {
        ep = ep
            .tls_config(ClientTlsConfig::new().with_webpki_roots())
            .map_err(|e| GrpcError::Connect(url.into(), e.to_string()))?;
    }
    ep.connect()
        .await
        .map_err(|e| GrpcError::Connect(url.into(), source_chain(&e)))
}

fn source_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push_str(&format!(": {c}"));
        cur = c.source();
    }
    s
}

fn add_metadata<T>(
    req: &mut tonic::Request<T>,
    metadata: &[(String, String)],
) -> Result<(), GrpcError> {
    for (k, v) in metadata {
        let key = tonic::metadata::MetadataKey::from_bytes(k.to_ascii_lowercase().as_bytes())
            .map_err(|e| GrpcError::Other(format!("metadata key {k}: {e}")))?;
        let val = v
            .parse()
            .map_err(|e| GrpcError::Other(format!("metadata {k}: {e}")))?;
        req.metadata_mut().insert(key, val);
    }
    Ok(())
}

fn mk_req<T>(body: T, metadata: &[(String, String)]) -> Result<tonic::Request<T>, Status> {
    let mut r = tonic::Request::new(body);
    add_metadata(&mut r, metadata).map_err(|e| Status::invalid_argument(e.to_string()))?;
    Ok(r)
}

fn md_to_vec(md: &tonic::metadata::MetadataMap) -> Vec<(String, String)> {
    md.iter()
        .map(|kv| match kv {
            tonic::metadata::KeyAndValueRef::Ascii(k, v) => {
                (k.to_string(), v.to_str().unwrap_or("").to_string())
            }
            tonic::metadata::KeyAndValueRef::Binary(k, _) => (k.to_string(), "<binary>".into()),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatusInfo {
    /// gRPC status code number and name (0 OK).
    pub code: i32,
    pub code_name: String,
    pub message: String,
}

impl StatusInfo {
    pub fn ok() -> Self {
        Self {
            code: 0,
            code_name: "OK".into(),
            message: String::new(),
        }
    }
    fn from(s: &Status) -> Self {
        Self {
            code: s.code() as i32,
            code_name: format!("{:?}", s.code()),
            message: s.message().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UnaryResult {
    pub status: StatusInfo,
    pub response: Option<Value>,
    pub headers: Vec<(String, String)>,
    pub trailers: Vec<(String, String)>,
    pub latency_ms: f64,
}

async fn grpc_client(channel: Channel) -> Result<tonic::client::Grpc<Channel>, GrpcError> {
    let mut g = tonic::client::Grpc::new(channel);
    g.ready()
        .await
        .map_err(|e| GrpcError::Other(e.to_string()))?;
    Ok(g)
}

fn path_of(m: &MethodDescriptor) -> Result<http::uri::PathAndQuery, GrpcError> {
    http::uri::PathAndQuery::try_from(format!("/{}/{}", m.parent_service().full_name(), m.name()))
        .map_err(|e| GrpcError::Other(e.to_string()))
}

/// Unary call with a JSON body.
pub async fn unary(
    channel: Channel,
    method: &MethodDescriptor,
    body: &str,
    metadata: &[(String, String)],
    timeout: Duration,
) -> Result<UnaryResult, GrpcError> {
    let msg = from_json(&method.input(), body)?;
    let mut req = tonic::Request::new(msg);
    add_metadata(&mut req, metadata)?;
    req.set_timeout(timeout);
    let mut g = grpc_client(channel).await?;
    let t = Instant::now();
    let res = g
        .unary(req, path_of(method)?, DynamicCodec::new(method.output()))
        .await;
    let latency_ms = t.elapsed().as_secs_f64() * 1000.0;
    Ok(match res {
        Ok(r) => {
            let headers = md_to_vec(r.metadata());
            UnaryResult {
                status: StatusInfo::ok(),
                response: Some(to_json(r.get_ref())),
                headers,
                trailers: vec![],
                latency_ms,
            }
        }
        Err(s) => UnaryResult {
            status: StatusInfo::from(&s),
            response: None,
            headers: md_to_vec(s.metadata()),
            trailers: vec![],
            latency_ms,
        },
    })
}

/// A streaming call (server, client or bidirectional). Messages and status are
/// recorded in `log`; send with [`StreamCall::send`], finish sending with
/// [`StreamCall::commit`].
pub struct StreamCall {
    pub log: EventLog,
    tx: Option<mpsc::UnboundedSender<DynamicMessage>>,
    input: MessageDescriptor,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    cancel: tokio::task::AbortHandle,
}

impl StreamCall {
    pub fn is_done(&self) -> bool {
        self.done.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Send one JSON message (client/bidi streaming).
    pub fn send(&self, body: &str) -> Result<(), GrpcError> {
        let tx = self.tx.as_ref().ok_or_else(|| {
            GrpcError::Other("this method does not accept streamed requests".into())
        })?;
        let msg = from_json(&self.input, body)?;
        let text = to_json(&msg).to_string();
        tx.send(msg)
            .map_err(|_| GrpcError::Other("the request stream is closed".into()))?;
        let n = text.len();
        self.log.push(Direction::Out, "message", None, text, n);
        Ok(())
    }

    /// Close the request stream (half-close); the server then finishes.
    pub fn commit(&mut self) {
        if self.tx.take().is_some() {
            self.log.info("info", "Request stream committed");
        }
    }

    pub fn cancel(&self) {
        self.cancel.abort();
        self.log.info("close", "Cancelled by client");
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Start a streaming call. For server-streaming methods `first` is the single request.
pub async fn start_stream(
    channel: Channel,
    method: &MethodDescriptor,
    first: Option<&str>,
    metadata: &[(String, String)],
) -> Result<StreamCall, GrpcError> {
    let log = EventLog::default();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let path = path_of(method)?;
    let codec = DynamicCodec::new(method.output());
    let mut g = grpc_client(channel).await?;
    let started = Instant::now();
    let (tx, client_side) = if method.is_client_streaming() {
        let (tx, rx) = mpsc::unbounded_channel::<DynamicMessage>();
        (
            Some(tx),
            Some(tokio_stream::wrappers::UnboundedReceiverStream::new(rx)),
        )
    } else {
        (None, None)
    };
    let log2 = log.clone();
    let done2 = done.clone();
    let server_streaming = method.is_server_streaming();
    let first_msg = match first {
        Some(b) if !method.is_client_streaming() => Some(from_json(&method.input(), b)?),
        _ => None,
    };
    if let Some(m) = &first_msg {
        let t = to_json(m).to_string();
        let n = t.len();
        log.push(Direction::Out, "message", None, t, n);
    }
    let md = metadata.to_vec();
    let handle = tokio::spawn(async move {
        let finish = |log: &EventLog,
                      s: Result<(), Status>,
                      trailers: Option<tonic::metadata::MetadataMap>| {
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            match s {
                Ok(()) => log.info(
                    "close",
                    format!(
                        "Status OK in {ms:.0} ms{}",
                        trailers
                            .map(|t| format!(" · trailers {:?}", md_to_vec(&t)))
                            .unwrap_or_default()
                    ),
                ),
                Err(e) => log.error(format!(
                    "Status {:?} ({}): {} after {ms:.0} ms",
                    e.code(),
                    e.code() as i32,
                    e.message()
                )),
            }
        };
        let result: Result<(), Status> = async {
            match (client_side, server_streaming) {
                (None, true) => {
                    let resp = g
                        .server_streaming(mk_req(first_msg.unwrap(), &md)?, path, codec)
                        .await?;
                    log2.info("open", format!("Headers {:?}", md_to_vec(resp.metadata())));
                    let mut stream = resp.into_inner();
                    while let Some(m) = stream.message().await? {
                        let t = to_json(&m).to_string();
                        let n = t.len();
                        log2.push(Direction::In, "message", None, t, n);
                    }
                    if let Some(t) = stream.trailers().await? {
                        log2.info("info", format!("Trailers {:?}", md_to_vec(&t)));
                    }
                    Ok(())
                }
                (Some(rs), false) => {
                    let resp = g.client_streaming(mk_req(rs, &md)?, path, codec).await?;
                    let t = to_json(resp.get_ref()).to_string();
                    let n = t.len();
                    log2.push(Direction::In, "message", None, t, n);
                    Ok(())
                }
                (Some(rs), true) => {
                    let resp = g.streaming(mk_req(rs, &md)?, path, codec).await?;
                    let mut stream = resp.into_inner();
                    while let Some(m) = stream.message().await? {
                        let t = to_json(&m).to_string();
                        let n = t.len();
                        log2.push(Direction::In, "message", None, t, n);
                    }
                    Ok(())
                }
                (None, false) => Err(Status::new(
                    Code::InvalidArgument,
                    "use unary() for unary methods",
                )),
            }
        }
        .await;
        finish(&log2, result, None);
        done2.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    log.info(
        "open",
        format!(
            "Started {} {}",
            if method.is_client_streaming() && server_streaming {
                "bidirectional stream"
            } else if method.is_client_streaming() {
                "client stream"
            } else {
                "server stream"
            },
            method.full_name()
        ),
    );
    Ok(StreamCall {
        log,
        tx,
        input: method.input(),
        done,
        cancel: handle.abort_handle(),
    })
}

// ---------------------------------------------------------------- reflection

/// Build a schema from the server's reflection service (v1, falling back to v1alpha).
pub async fn reflect(channel: Channel) -> Result<Schema, GrpcError> {
    match reflect_v1(channel.clone()).await {
        Ok(s) => Ok(s),
        Err(e) if e.to_string().contains("Unimplemented") => reflect_v1alpha(channel).await,
        Err(e) => Err(e),
    }
}

macro_rules! reflection_impl {
    ($name:ident, $pb:path) => {
        async fn $name(channel: Channel) -> Result<Schema, GrpcError> {
            use pb::server_reflection_client::ServerReflectionClient;
            use pb::server_reflection_request::MessageRequest;
            use pb::server_reflection_response::MessageResponse;
            use $pb as pb;
            let mut client = ServerReflectionClient::new(channel);
            let (tx, rx) = mpsc::unbounded_channel();
            let req = |m: MessageRequest| pb::ServerReflectionRequest {
                host: String::new(),
                message_request: Some(m),
            };
            tx.send(req(MessageRequest::ListServices(String::new())))
                .ok();
            let mut stream = client
                .server_reflection_info(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
                .await
                .map_err(|e| GrpcError::Reflection(format!("{:?}: {}", e.code(), e.message())))?
                .into_inner();
            let mut files: Vec<prost_types::FileDescriptorProto> = vec![];
            let mut seen = std::collections::HashSet::new();
            let mut pending = 1usize;
            while pending > 0 {
                let Some(msg) = stream.next().await else {
                    break;
                };
                pending -= 1;
                let msg = msg.map_err(|e| {
                    GrpcError::Reflection(format!("{:?}: {}", e.code(), e.message()))
                })?;
                match msg.message_response {
                    Some(MessageResponse::ListServicesResponse(l)) => {
                        for s in l
                            .service
                            .into_iter()
                            .filter(|s| !s.name.starts_with("grpc.reflection."))
                        {
                            tx.send(req(MessageRequest::FileContainingSymbol(s.name)))
                                .ok();
                            pending += 1;
                        }
                    }
                    Some(MessageResponse::FileDescriptorResponse(f)) => {
                        for bytes in f.file_descriptor_proto {
                            let fd = prost_types::FileDescriptorProto::decode(bytes.as_slice())
                                .map_err(|e| GrpcError::Reflection(e.to_string()))?;
                            let name = fd.name.clone().unwrap_or_default();
                            if seen.insert(name) {
                                for dep in &fd.dependency {
                                    if !seen.contains(dep) {
                                        tx.send(req(MessageRequest::FileByFilename(dep.clone())))
                                            .ok();
                                        pending += 1;
                                    }
                                }
                                files.push(fd);
                            }
                        }
                    }
                    // a missing well-known import is fine if another file provided it
                    Some(MessageResponse::ErrorResponse(e))
                        if !e.error_message.contains("google/protobuf") =>
                    {
                        return Err(GrpcError::Reflection(e.error_message));
                    }
                    _ => {}
                }
            }
            drop(tx);
            let mut pool = DescriptorPool::new();
            // dependencies first
            let mut remaining = files;
            for _ in 0..64 {
                if remaining.is_empty() {
                    break;
                }
                let mut next = vec![];
                for f in remaining {
                    let deps_ok = f
                        .dependency
                        .iter()
                        .all(|d| pool.get_file_by_name(d).is_some());
                    if deps_ok {
                        pool.add_file_descriptor_proto(f)
                            .map_err(|e| GrpcError::Reflection(e.to_string()))?;
                    } else {
                        next.push(f);
                    }
                }
                remaining = next;
            }
            if !remaining.is_empty() {
                let names: Vec<String> = remaining.iter().filter_map(|f| f.name.clone()).collect();
                return Err(GrpcError::Reflection(format!(
                    "unresolved imports in {}",
                    names.join(", ")
                )));
            }
            Ok(Schema { pool })
        }
    };
}

reflection_impl!(reflect_v1, tonic_reflection::pb::v1);
reflection_impl!(reflect_v1alpha, tonic_reflection::pb::v1alpha);

#[cfg(test)]
mod tests;
