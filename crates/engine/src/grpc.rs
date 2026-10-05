//! gRPC requests: schema (workspace protos or server reflection) and rendering.

use irs_core::{Doc, GrpcRequest, ProtoFile};
use irs_grpc::Schema;
use irs_templating::Mode;

use crate::{Engine, EngineError, Result};

pub struct GrpcPrepared {
    pub url: String,
    pub method: String,
    pub body: String,
    pub metadata: Vec<(String, String)>,
    pub timeout: std::time::Duration,
}

impl Engine {
    pub fn proto_files(&self, workspace_id: &str) -> Result<Vec<Doc<ProtoFile>>> {
        Ok(self.store.children::<ProtoFile>(workspace_id)?)
    }

    fn render(&self, id: &str, field: &str, text: &str) -> Result<String> {
        let ctx = self.context(id)?;
        self.renderer().render_str(text, &ctx, Mode::Throw).map_err(|source| EngineError::Render { field: field.into(), source })
    }

    /// Render URL, body and metadata of a gRPC request.
    pub fn grpc_prepare(&self, id: &str) -> Result<GrpcPrepared> {
        let r: Doc<GrpcRequest> = self.store.get(id)?;
        let mut metadata = vec![];
        for m in r.metadata.iter().filter(|m| !m.disabled && !m.name.trim().is_empty()) {
            metadata.push((self.render(id, "metadata name", &m.name)?, self.render(id, &format!("metadata '{}'", m.name), &m.value)?));
        }
        Ok(GrpcPrepared {
            url: self.render(id, "URL", &r.url)?,
            method: r.method.clone(),
            body: self.render(id, "message", &r.message)?,
            metadata,
            timeout: std::time::Duration::from_millis(r.timeout_ms.max(100)),
        })
    }

    /// Load the schema for a gRPC request: compile the workspace's proto files,
    /// or ask the server via reflection.
    pub async fn grpc_schema(&self, id: &str) -> Result<Schema> {
        let r: Doc<GrpcRequest> = self.store.get(id)?;
        if r.schema_source == "protos" {
            let ws = self.workspace_of(id)?;
            let files: Vec<(String, String)> = self.proto_files(ws.id())?.into_iter().map(|f| (f.body.name.clone(), f.body.contents.clone())).collect();
            if files.is_empty() {
                return Err(EngineError::Message("No proto files in this collection — add one or switch to server reflection".into()));
            }
            return Schema::from_protos(&files).map_err(|e| EngineError::Message(e.to_string()));
        }
        let url = self.render(id, "URL", &r.url)?;
        let channel = irs_grpc::connect(&url).await.map_err(|e| EngineError::Message(e.to_string()))?;
        irs_grpc::reflect(channel).await.map_err(|e| {
            let hint = if e.to_string().contains("Unimplemented") { " — the server has no reflection service; add its .proto files instead" } else { "" };
            EngineError::Message(format!("{e}{hint}"))
        })
    }
}
