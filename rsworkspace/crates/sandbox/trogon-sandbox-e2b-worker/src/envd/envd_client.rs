use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::Serialize;
use serde_json::Value;
use trogon_sandbox_contract::{FileOp, FileRecursion, FileSha256};
use trogon_sandbox_file_ops::StreamHasher;
use trogon_sandbox_protocol::CappedOutput;

use super::constants::{
    ACCESS_TOKEN_HEADER, CONNECT_STREAM_CONTENT_TYPE, CONNECT_TIMEOUT, CONNECT_TIMEOUT_HEADER, CONNECT_VERSION,
    CONNECT_VERSION_HEADER, FALLBACK_UPLOAD_NAME, KEEPALIVE_HEADER, KEEPALIVE_SECS, MAX_FILESYSTEM_REPLY_BYTES,
    PTY_LIFETIME, REQUEST_GRACE, SANDBOX_GONE_MARKER, SIGKILL, UNARY_TIMEOUT,
};
use super::frame::encode_frame;
use super::process_stream::parse_process_stream;
use super::start_body::{PtyBody, PtySizeBody, StartBody};
use super::unary_bodies::{InputBody, SendInputBody, SendSignalBody, UpdateBody};
use super::watcher_bodies::{CreateWatcherBody, WatcherIdBody};
use super::{EnvdTarget, ProcessRequest, ProcessResult, ProcessSelector, PtyRequest, PtyStream, WatcherBatch};
use crate::api::{ApiError, error_message};

/// Client for `envd`, the daemon E2B runs inside every sandbox.
///
/// Commands use envd's Connect protocol (`process.Process/Start`, a server stream framed as
/// `1 flag byte + 4 byte big endian length + JSON`), files use its plain HTTP `/files`
/// endpoint. Both are authenticated with the sandbox's own access token, not the API key.
///
/// Interactive terminals use the same service's PTY mode. The message and field names come
/// from E2B's public spec, `spec/envd/process/process.proto` in `github.com/e2b-dev/E2B`:
/// `Start` takes `pty{size{cols,rows}}` and a `tag`, output arrives as `data.pty` events,
/// `SendInput` carries `input{pty}`, `Update` carries `pty{size}` for a resize, and
/// `SendSignal` carries `SIGNAL_SIGKILL`. A process is selected by `pid` or `tag`.
/// `CloseStdin` does not work on a PTY (the spec says to send Ctrl+D, 0x04, instead).
/// This behaviour is verified against an in-process fake envd only, never a live sandbox.
#[derive(Clone)]
pub struct Envd {
    http: reqwest::Client,
}

impl Envd {
    pub fn new(timeout: Duration) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .build()
            .map_err(ApiError::transport)?;
        Ok(Self { http })
    }

    fn auth(request: RequestBuilder, target: &EnvdTarget, user: &str) -> RequestBuilder {
        let request = request.basic_auth(user, None::<&str>);
        match target.access_header() {
            Some(token) => request.header(ACCESS_TOKEN_HEADER, token),
            None => request,
        }
    }

    async fn failure(response: Response) -> ApiError {
        let status = response.status();
        let body = response.bytes().await.unwrap_or_default();
        // The edge answers 502 with a JSON body when the sandbox does not exist any more.
        if status == StatusCode::BAD_GATEWAY
            && let Some(message) =
                error_message(&body).filter(|message| message.to_lowercase().contains(SANDBOX_GONE_MARKER))
        {
            return ApiError::NotFound(message);
        }
        ApiError::from_status(status, None, &body)
    }

    async fn checked(request: RequestBuilder) -> Result<Response, ApiError> {
        let response = request.send().await.map_err(ApiError::transport)?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(Self::failure(response).await)
        }
    }

    /// Posts a `process.Process/Start` stream request.
    async fn start(
        &self,
        target: &EnvdTarget,
        user: &str,
        body: &StartBody<'_>,
        lifetime: Duration,
        keepalive: bool,
    ) -> Result<Response, ApiError> {
        let mut builder = self
            .http
            .post(target.url("/process.Process/Start"))
            .header(reqwest::header::CONTENT_TYPE, CONNECT_STREAM_CONTENT_TYPE)
            .header(CONNECT_VERSION_HEADER, CONNECT_VERSION)
            .header(CONNECT_TIMEOUT_HEADER, lifetime.as_millis().max(1).to_string())
            // The stream may legitimately stay quiet up to the command timeout.
            .timeout(lifetime + REQUEST_GRACE)
            .body(encode_frame(body)?);
        if keepalive {
            builder = builder.header(KEEPALIVE_HEADER, KEEPALIVE_SECS);
        }
        Self::checked(Self::auth(builder, target, user)).await
    }

    /// Runs `argv` to completion through `bash -l -c 'exec "$@"'`, which keeps the argv
    /// exact (no quoting) and reports a missing program as exit 127.
    pub async fn run(&self, target: &EnvdTarget, request: &ProcessRequest) -> Result<ProcessResult, ApiError> {
        let response = self
            .start(
                target,
                &request.user,
                &StartBody::command(request),
                request.timeout,
                false,
            )
            .await?;
        let bytes = response.bytes().await.map_err(ApiError::transport)?;
        parse_process_stream(&bytes)
    }

    /// Starts `request` on a PTY and returns its event stream. The stream's first event
    /// should be `PtyEvent::Started`.
    pub async fn start_pty(&self, target: &EnvdTarget, request: &PtyRequest) -> Result<PtyStream, ApiError> {
        let response = self
            .start(target, &request.user, &StartBody::pty(request), PTY_LIFETIME, true)
            .await?;
        Ok(PtyStream::new(response))
    }

    async fn unary<B: Serialize>(
        &self,
        target: &EnvdTarget,
        user: &str,
        service: &str,
        rpc: &str,
        body: &B,
    ) -> Result<Response, ApiError> {
        let builder = self
            .http
            .post(target.url(&format!("/{service}/{rpc}")))
            .header(CONNECT_VERSION_HEADER, CONNECT_VERSION)
            .timeout(UNARY_TIMEOUT)
            .json(body);
        Self::checked(Self::auth(builder, target, user)).await
    }

    /// Types `bytes` into a PTY process.
    pub async fn send_pty_input(
        &self,
        target: &EnvdTarget,
        user: &str,
        process: &ProcessSelector,
        bytes: &[u8],
    ) -> Result<(), ApiError> {
        let body = SendInputBody {
            process,
            input: InputBody {
                pty: STANDARD.encode(bytes),
            },
        };
        self.unary(target, user, "process.Process", "SendInput", &body)
            .await
            .map(|_| ())
    }

    /// Resizes a PTY process.
    pub async fn resize_pty(
        &self,
        target: &EnvdTarget,
        user: &str,
        process: &ProcessSelector,
        cols: u16,
        rows: u16,
    ) -> Result<(), ApiError> {
        let body = UpdateBody {
            process,
            pty: PtyBody {
                size: PtySizeBody { cols, rows },
            },
        };
        self.unary(target, user, "process.Process", "Update", &body)
            .await
            .map(|_| ())
    }

    /// Kills a process with `SIGKILL`.
    pub async fn kill_process(
        &self,
        target: &EnvdTarget,
        user: &str,
        process: &ProcessSelector,
    ) -> Result<(), ApiError> {
        let body = SendSignalBody {
            process,
            signal: SIGKILL,
        };
        self.unary(target, user, "process.Process", "SendSignal", &body)
            .await
            .map(|_| ())
    }

    /// One unary call of `filesystem.Filesystem` (Connect, plain JSON). The reply is read
    /// with a size ceiling so a misbehaving peer cannot make the worker buffer without bound.
    pub(crate) async fn filesystem_unary<B: Serialize>(
        &self,
        target: &EnvdTarget,
        user: &str,
        rpc: &str,
        body: B,
    ) -> Result<Value, ApiError> {
        let mut response = self.unary(target, user, "filesystem.Filesystem", rpc, &body).await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(ApiError::transport)? {
            if bytes.len() + chunk.len() > MAX_FILESYSTEM_REPLY_BYTES {
                return Err(ApiError::Transport(
                    "envd filesystem reply exceeds the size limit".to_owned(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|error| ApiError::Transport(format!("unexpected envd reply: {error}")))
    }

    /// `CreateWatcher`: starts an envd-side directory watcher and returns its id.
    pub async fn create_watcher(
        &self,
        target: &EnvdTarget,
        user: &str,
        path: &str,
        recursion: FileRecursion,
    ) -> Result<String, ApiError> {
        let body = CreateWatcherBody {
            path,
            recursive: recursion.is_recursive(),
            include_entry: true,
        };
        let reply = self.filesystem_unary(target, user, "CreateWatcher", body).await?;
        reply
            .get("watcherId")
            .or_else(|| reply.get("watcher_id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| ApiError::Transport("envd returned no watcher id".to_owned()))
    }

    /// `GetWatcherEvents`: the events envd buffered since the previous call (envd drops
    /// them once returned). At most `MAX_WATCHER_EVENTS_PER_FETCH` are kept; the rest of
    /// an oversized batch is counted in `dropped`.
    pub async fn watcher_events(
        &self,
        target: &EnvdTarget,
        user: &str,
        watcher_id: &str,
    ) -> Result<WatcherBatch, ApiError> {
        let reply = self
            .filesystem_unary(target, user, "GetWatcherEvents", WatcherIdBody { watcher_id })
            .await?;
        Ok(WatcherBatch::from_reply(&reply))
    }

    /// `RemoveWatcher`.
    pub async fn remove_watcher(&self, target: &EnvdTarget, user: &str, watcher_id: &str) -> Result<(), ApiError> {
        self.filesystem_unary(target, user, "RemoveWatcher", WatcherIdBody { watcher_id })
            .await
            .map(|_| ())
    }

    /// `GET /files`; `None` when the file does not exist.
    async fn open_file(&self, target: &EnvdTarget, user: &str, path: &str) -> Result<Option<Response>, ApiError> {
        let builder = self
            .http
            .get(target.url("/files"))
            .query(&[("path", path), ("username", user)]);
        let response = Self::auth(builder, target, user)
            .send()
            .await
            .map_err(ApiError::transport)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Self::failure(response).await);
        }
        Ok(Some(response))
    }

    /// `Ok(None)` when the file does not exist.
    pub async fn read_file(&self, target: &EnvdTarget, user: &str, path: &str) -> Result<Option<Vec<u8>>, ApiError> {
        let Some(response) = self.open_file(target, user, path).await? else {
            return Ok(None);
        };
        let bytes = response.bytes().await.map_err(ApiError::transport)?;
        Ok(Some(bytes.to_vec()))
    }

    pub(crate) async fn read_file_capped(
        &self,
        target: &EnvdTarget,
        user: &str,
        path: &str,
        cap: usize,
    ) -> Result<Option<(Vec<u8>, bool)>, ApiError> {
        let Some(mut response) = self.open_file(target, user, path).await? else {
            return Ok(None);
        };
        let mut output = CappedOutput::new(cap);
        while let Some(chunk) = response.chunk().await.map_err(ApiError::transport)? {
            output.push(&chunk);
        }
        let truncated = output.truncation().is_truncated();
        Ok(Some((output.into_bytes(), truncated)))
    }

    /// Reads a bounded range without retaining bytes before the requested offset.
    pub(crate) async fn read_file_range(
        &self,
        target: &EnvdTarget,
        user: &str,
        path: &str,
        mut offset: u64,
        count: u64,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        let limit = count.min(FileOp::MAX_READ_BYTES) as usize;
        if limit == 0 {
            return Ok(Some(Vec::new()));
        }
        let Some(mut response) = self.open_file(target, user, path).await? else {
            return Ok(None);
        };
        let mut bytes = Vec::with_capacity(limit);
        while let Some(chunk) = response.chunk().await.map_err(ApiError::transport)? {
            let skipped = offset.min(chunk.len() as u64) as usize;
            offset -= skipped as u64;
            let remaining = &chunk[skipped..];
            let take = remaining.len().min(limit - bytes.len());
            bytes.extend_from_slice(&remaining[..take]);
            if bytes.len() == limit {
                break;
            }
        }
        Ok(Some(bytes))
    }

    pub(crate) async fn hash_file_range(
        &self,
        target: &EnvdTarget,
        user: &str,
        path: &str,
        mut offset: u64,
        count: u64,
    ) -> Result<Option<(FileSha256, u64)>, ApiError> {
        let mut hasher = StreamHasher::new();
        let mut bytes = 0;
        if count > 0 {
            let Some(mut response) = self.open_file(target, user, path).await? else {
                return Ok(None);
            };
            while let Some(chunk) = response.chunk().await.map_err(ApiError::transport)? {
                let skipped = offset.min(chunk.len() as u64) as usize;
                offset -= skipped as u64;
                let remaining = &chunk[skipped..];
                let take = (remaining.len() as u64).min(count - bytes) as usize;
                hasher.update(&remaining[..take]);
                bytes += take as u64;
                if bytes == count {
                    break;
                }
            }
        }
        let digest = hasher
            .finish()
            .map_err(|error| ApiError::Transport(error.to_string()))?;
        Ok(Some((digest, bytes)))
    }

    /// Writes `content` to `path`, creating parent directories.
    pub async fn write_file(
        &self,
        target: &EnvdTarget,
        user: &str,
        path: &str,
        content: Vec<u8>,
    ) -> Result<(), ApiError> {
        let name = path.rsplit('/').next().unwrap_or(FALLBACK_UPLOAD_NAME).to_owned();
        let part = reqwest::multipart::Part::bytes(content).file_name(name);
        let form = reqwest::multipart::Form::new().part("file", part);
        let builder = self
            .http
            .post(target.url("/files"))
            .query(&[("path", path), ("username", user)])
            .multipart(form);
        Self::checked(Self::auth(builder, target, user)).await.map(|_| ())
    }
}
