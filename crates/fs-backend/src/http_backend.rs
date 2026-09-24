//! [`HttpFilesystemBackend`] — the external durable fs backend (ADR 0024 phase 2e),
//! behind `feature = "http"`. Configured via `ADRIANE_FS_BACKEND_URL` (+ optional
//! `ADRIANE_FS_BACKEND_TOKEN` bearer), it POSTs each operation to an external service
//! that holds the filesystem durably — so fs content **survives a suspend/resume**
//! across the napi boundary (the in-memory [`crate::ArtifactFsBackend`] does not).
//!
//! Mirrors the [`adriane_llm_gateway`] redactor/compressor seam shape, but is
//! **fail-CLOSED**: a transport/parse error becomes [`FsError::ServiceUnavailable`]
//! (never a silent pass-through), because a missing or unconfirmed fs op is a semantic
//! error the agent must reason about.

use adriane_artifact_store::ArtifactVersion;
use adriane_graph_core::RunId;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::backend::FilesystemBackend;
use crate::types::{EditOp, FileContent, FileEntry, FsError, FsWriteCtx, GrepMatch};

/// Connect/read timeouts so an fs service that accepts the connection and never answers fails
/// the op (fail-closed, like any transport error) instead of hanging the run forever.
fn fs_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .read_timeout(std::time::Duration::from_secs(120))
        .build()
        .expect("static HTTP client configuration is valid")
}

/// A run-scoped governed fs backed by an external HTTP service. Every op POSTs a
/// `{ op, runId, ... }` body to the configured URL; a `200` response is either the op's
/// result or `{ "error": <FsError> }` (a semantic error the service reports), and any
/// transport / non-2xx / parse failure is surfaced fail-closed.
pub struct HttpFilesystemBackend {
    url: String,
    token: Option<String>,
    run_id: RunId,
    client: reqwest::Client,
}

impl HttpFilesystemBackend {
    /// Build from env for `run_id`, or `None` when `ADRIANE_FS_BACKEND_URL` is unset (the
    /// caller then falls back to the in-memory backend).
    pub fn from_env(run_id: RunId) -> Option<Self> {
        let url = std::env::var("ADRIANE_FS_BACKEND_URL")
            .ok()
            .filter(|value| !value.is_empty())?;
        let token = std::env::var("ADRIANE_FS_BACKEND_TOKEN")
            .ok()
            .filter(|value| !value.is_empty());
        Some(Self {
            url,
            token,
            run_id,
            client: fs_http_client(),
        })
    }

    /// POST one op and decode its result, fail-closed.
    async fn request<T: DeserializeOwned>(&self, op: &str, mut body: Value) -> Result<T, FsError> {
        if let Value::Object(map) = &mut body {
            map.insert("op".to_owned(), Value::String(op.to_owned()));
            map.insert("runId".to_owned(), Value::String(self.run_id.0.clone()));
        }
        let mut builder = self.client.post(&self.url).json(&body);
        if let Some(token) = &self.token {
            builder = builder.bearer_auth(token);
        }
        let unavailable = |reason: String| FsError::ServiceUnavailable { reason };
        let response = builder
            .send()
            .await
            .map_err(|error| unavailable(error.to_string()))?
            .error_for_status()
            .map_err(|error| unavailable(error.to_string()))?;
        let value: Value = response.json().await.map_err(|error| FsError::Backend {
            reason: error.to_string(),
        })?;
        // A service-reported semantic error (NotFound, PermissionDenied, …) rides a 200
        // with an `error` envelope; surface it verbatim.
        if let Some(error) = value.get("error") {
            return Err(serde_json::from_value::<FsError>(error.clone()).unwrap_or(
                FsError::Backend {
                    reason: error.to_string(),
                },
            ));
        }
        serde_json::from_value::<T>(value).map_err(|error| FsError::Backend {
            reason: error.to_string(),
        })
    }
}

#[async_trait]
impl FilesystemBackend for HttpFilesystemBackend {
    async fn read(
        &self,
        path: &str,
        version: Option<ArtifactVersion>,
    ) -> Result<FileContent, FsError> {
        self.request("read", json!({ "path": path, "version": version }))
            .await
    }

    async fn write(
        &self,
        path: &str,
        content: String,
        media_type: adriane_artifact_store::ArtifactMediaType,
        ctx: &FsWriteCtx,
    ) -> Result<adriane_artifact_store::ArtifactRef, FsError> {
        self.request(
            "write",
            json!({ "path": path, "content": content, "mediaType": media_type, "principal": ctx.principal, "nodeId": ctx.node_id.0 }),
        )
        .await
    }

    async fn edit(
        &self,
        path: &str,
        patches: Vec<EditOp>,
        ctx: &FsWriteCtx,
    ) -> Result<adriane_artifact_store::ArtifactRef, FsError> {
        self.request(
            "edit",
            json!({ "path": path, "patches": patches, "principal": ctx.principal, "nodeId": ctx.node_id.0 }),
        )
        .await
    }

    async fn delete(&self, path: &str, ctx: &FsWriteCtx) -> Result<(), FsError> {
        let _: Value = self
            .request(
                "delete",
                json!({ "path": path, "principal": ctx.principal, "nodeId": ctx.node_id.0 }),
            )
            .await?;
        Ok(())
    }

    async fn rename(
        &self,
        from: &str,
        to: &str,
        ctx: &FsWriteCtx,
    ) -> Result<adriane_artifact_store::ArtifactRef, FsError> {
        self.request(
            "rename",
            json!({ "from": from, "to": to, "principal": ctx.principal, "nodeId": ctx.node_id.0 }),
        )
        .await
    }

    async fn ls(&self, prefix: &str) -> Result<Vec<FileEntry>, FsError> {
        self.request("ls", json!({ "prefix": prefix })).await
    }

    async fn glob(&self, pattern: &str) -> Result<Vec<String>, FsError> {
        self.request("glob", json!({ "pattern": pattern })).await
    }

    async fn grep(&self, pattern: &str, paths: Vec<String>) -> Result<Vec<GrepMatch>, FsError> {
        self.request("grep", json!({ "pattern": pattern, "paths": paths }))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    fn backend_at(url: String) -> HttpFilesystemBackend {
        HttpFilesystemBackend {
            url,
            token: None,
            run_id: RunId::from("run-1"),
            client: reqwest::Client::new(),
        }
    }

    #[test]
    fn from_env_is_none_without_a_url() {
        // Not set in the test env → None (caller falls back to the in-memory backend).
        std::env::remove_var("ADRIANE_FS_BACKEND_URL");
        assert!(HttpFilesystemBackend::from_env(RunId::from("run-1")).is_none());
    }

    #[tokio::test]
    async fn read_round_trips_a_file_content_response() {
        // A one-shot mock service: accept the POST, reply with a FileContent JSON.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf); // drain the request (we don't need it)
            let body = r#"{"path":"notes.md","content":"hi","mediaType":"text/markdown","version":2,"createdAt":"t"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        let backend = backend_at(format!("http://{addr}"));
        let file = backend.read("notes.md", None).await.expect("reads");
        assert_eq!(file.content, "hi");
        assert_eq!(file.version, 2);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn transport_error_is_fail_closed() {
        // An unroutable address → ServiceUnavailable (never a silent success).
        let backend = backend_at("http://127.0.0.1:1/fs".to_owned());
        let err = backend.read("x", None).await.expect_err("must fail closed");
        assert!(
            matches!(err, FsError::ServiceUnavailable { .. }),
            "got: {err:?}"
        );
    }
}
