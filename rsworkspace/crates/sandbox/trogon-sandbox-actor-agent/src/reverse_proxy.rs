use std::num::NonZeroU16;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;

/// Forwards non-agent traffic to the port the tenant workload listens on.
///
/// The router addresses an actor as a single HTTP origin, so the agent and the tenant's own server have to share one
/// hostname. The agent owns the reserved prefix and the readiness path and hands everything else to the workload
/// unchanged.
#[derive(Clone)]
pub struct ReverseProxy {
    client: Client<HttpConnector, Body>,
}

impl ReverseProxy {
    pub fn new() -> Self {
        Self {
            client: Client::builder(TokioExecutor::new()).build_http(),
        }
    }

    pub async fn forward(&self, port: NonZeroU16, mut request: Request) -> Response {
        let path_and_query = request.uri().path_and_query().map_or("/", |value| value.as_str());
        // A target that does not parse cannot be reached either, so it falls through to the unreachable answer below.
        let upstream = format!("http://127.0.0.1:{port}{path_and_query}")
            .parse::<Uri>()
            .unwrap_or_else(|_| request.uri().clone());
        *request.uri_mut() = upstream;

        match self.client.request(request).await {
            Ok(response) => response.into_response(),
            Err(error) => (
                StatusCode::BAD_GATEWAY,
                format!("exposed port {port} is not reachable: {error}"),
            )
                .into_response(),
        }
    }
}

impl Default for ReverseProxy {
    fn default() -> Self {
        Self::new()
    }
}
