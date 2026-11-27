//! Test doubles that stand in for the in-guest agent behind the worker's transport.

use crate::config::VsockPort;
use crate::vsock::VsockEndpoint;
use axum::Router;
use axum::http::HeaderValue;
use axum::response::Response as AxumResponse;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener};

use super::*;

/// Stands in for a current agent: every answer carries the agent's protocol version.
pub(crate) async fn stamp_version(mut response: AxumResponse) -> AxumResponse {
    response.headers_mut().insert(
        trogon_sandbox_actor_agent::AGENT_PROTOCOL_VERSION_HEADER,
        HeaderValue::from(trogon_sandbox_actor_agent::GUEST_PROTOCOL_VERSION),
    );
    response
}

/// Fronts a real axum server with a fake Firecracker vsock socket: it answers the
/// `CONNECT <port>` line (and checks the port) and then splices the raw bytes through,
/// so the backend sees exactly the HTTP/1.1 traffic a guest agent would.
///
/// Behind an agent that announces its protocol version.
pub(crate) async fn serve(router: Router) -> anyhow::Result<(GuestAgent, tempfile::TempDir)> {
    serve_unversioned(router.layer(axum::middleware::map_response(stamp_version))).await
}

/// The vsock socket with `router` answering as-is: a guest image that predates the
/// handshake when `router` adds no version header.
pub(crate) async fn serve_unversioned(router: Router) -> anyhow::Result<(GuestAgent, tempfile::TempDir)> {
    let backend = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let backend_port = backend.local_addr()?.port();
    tokio::spawn(async move {
        let _ = axum::serve(backend, router).await;
    });

    let tmp = tempfile::tempdir()?;
    let socket: PathBuf = tmp.path().join("v.sock");
    let listener = UnixListener::bind(&socket)?;
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut line = Vec::new();
                let mut byte = [0u8; 1];
                loop {
                    if stream.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    if byte[0] == b'\n' {
                        break;
                    }
                    line.push(byte[0]);
                }
                if line != b"CONNECT 8110" {
                    return;
                }
                let _ = stream.write_all(b"OK 1073741824\n").await;
                let Ok(mut upstream) = TcpStream::connect(("127.0.0.1", backend_port)).await else {
                    return;
                };
                let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
            });
        }
    });

    let agent = GuestAgent::new(VsockEndpoint::new(socket, VsockPort::new(8110)?));
    Ok((agent, tmp))
}
