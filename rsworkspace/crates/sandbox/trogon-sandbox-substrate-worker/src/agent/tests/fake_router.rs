//! Test doubles that stand in for the router and the in-guest agent behind the worker's
//! transport.

use std::error::Error;

use axum::Router;
use axum::response::Response as AxumResponse;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use trogon_std::env::InMemoryEnv;

use crate::agent::{ActorAgent, ActorTarget};
use crate::config::RouterEndpoint;

/// Stands in for a current agent: every answer carries the agent's protocol version.
async fn stamp_version(mut response: AxumResponse) -> AxumResponse {
    response.headers_mut().insert(
        trogon_sandbox_actor_agent::AGENT_PROTOCOL_VERSION_HEADER,
        trogon_sandbox_actor_agent::GUEST_PROTOCOL_VERSION.into(),
    );
    response
}

/// Reads one CONNECT request head, byte by byte, up to the blank line that ends it.
async fn read_head(socket: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        if socket.read(&mut byte).await.ok()? == 0 {
            return None;
        }
        buffer.push(byte[0]);
    }
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

/// Fronts a real axum server with a fake CONNECT-accepting listener, so tests exercise
/// the same tunnel handshake the real router speaks without needing a substrate cluster.
///
/// Validates the `ate-target-actor` header and the CONNECT authority's port against
/// `target`, matching what the real router keys routing on, then splices the raw bytes
/// through to the backend rather than re-parsing HTTP, so the backend still sees exactly
/// the plain HTTP/1.1 traffic `router` was written to answer.
///
/// Behind an agent that announces its protocol version.
pub(crate) async fn serve(target: ActorTarget, router: Router) -> Result<ActorAgent, Box<dyn Error>> {
    serve_unversioned(target, router.layer(axum::middleware::map_response(stamp_version))).await
}

/// The tunnel with `router` answering as-is: an agent image that predates the handshake
/// when `router` adds no version header.
pub(crate) async fn serve_unversioned(target: ActorTarget, router: Router) -> Result<ActorAgent, Box<dyn Error>> {
    let backend_listener = TcpListener::bind("127.0.0.1:0").await?;
    let backend_port = backend_listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = axum::serve(backend_listener, router).await;
    });

    let tunnel_listener = TcpListener::bind("127.0.0.1:0").await?;
    let tunnel_port = tunnel_listener.local_addr()?.port();
    let expected_header = format!("ate-target-actor: {}", target.header_value());
    let expected_port = format!(":{}", target.port());

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = tunnel_listener.accept().await {
            let expected_header = expected_header.clone();
            let expected_port = expected_port.clone();
            tokio::spawn(async move {
                let Some(head) = read_head(&mut socket).await else {
                    return;
                };
                let request_line = head.lines().next().unwrap_or_default();
                let authority = request_line.split_whitespace().nth(1).unwrap_or_default();
                let has_header = head.lines().any(|line| line.eq_ignore_ascii_case(&expected_header));

                if !has_header || !authority.ends_with(&expected_port) {
                    let _ = socket.write_all(b"HTTP/1.1 421 Misdirected Request\r\n\r\n").await;
                    return;
                }
                let _ = socket.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await;

                let Ok(mut backend) = TcpStream::connect(("127.0.0.1", backend_port)).await else {
                    return;
                };
                let _ = tokio::io::copy_bidirectional(&mut socket, &mut backend).await;
            });
        }
    });

    let router_endpoint: RouterEndpoint = format!("127.0.0.1:{tunnel_port}").parse()?;
    Ok(ActorAgent::new(router_endpoint, target, &InMemoryEnv::new())?)
}

/// A router that answers every CONNECT with exactly `reply` and then hangs up.
pub(crate) async fn serve_raw_reply(reply: &'static [u8]) -> Result<RouterEndpoint, Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                if read_head(&mut socket).await.is_some() {
                    let _ = socket.write_all(reply).await;
                }
            });
        }
    });
    Ok(format!("127.0.0.1:{port}").parse()?)
}
