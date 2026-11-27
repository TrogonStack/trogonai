//! The preview reverse proxy. The upstream is resolved through the broker on every request, so a revoked lease or a
//! moved port is noticed at once, and the broker credential never travels inside the token.

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{GetSandboxDomainResponse, LeaseId};

use crate::constants::{API_KEY_HEADER, HOP_BY_HOP, PREVIEW_ROUTE_PREFIX};
use crate::encode_segment::encode_segment;
use crate::preview_location::preview_location;
use crate::session_record::now_secs;
use crate::upstream::{access_headers, upstream_base};
use crate::{AppState, PreviewError, PreviewState, PreviewTokenError};

fn forwardable(name: &HeaderName) -> bool {
    !HOP_BY_HOP.contains(&name.as_str())
}

pub async fn preview_root(
    State(state): State<AppState>,
    Path((lease_id, port, token)): Path<(String, u16, String)>,
    request: Request,
) -> Response {
    proxy(state, lease_id, port, token, String::new(), request).await
}

pub async fn preview_path(
    State(state): State<AppState>,
    Path((lease_id, port, token, rest)): Path<(String, u16, String, String)>,
    request: Request,
) -> Response {
    proxy(state, lease_id, port, token, rest, request).await
}

async fn proxy(
    state: AppState,
    lease_id: String,
    port: u16,
    token: String,
    rest: String,
    request: Request,
) -> Response {
    match forward(state, lease_id, port, token, rest, request).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn forward(
    state: AppState,
    lease_id: String,
    port: u16,
    token: String,
    rest: String,
    request: Request,
) -> Result<Response, PreviewError> {
    let preview = state.preview.as_ref().ok_or(PreviewError::Disabled)?;
    let lease = LeaseId::new(lease_id);
    let claims = preview
        .keys()
        .verify_for(&token, &lease, port, now_secs())
        .map_err(|error| match error {
            PreviewTokenError::Expired => PreviewError::Expired,
            _ => PreviewError::InvalidToken,
        })?;
    let grant_key = PreviewState::grant_key(&token).map_err(|_| PreviewError::InvalidToken)?;
    let grant = match state.sessions.lookup(&grant_key).await? {
        Some(grant) if grant.tenant == claims.tenant => grant,
        _ => return Err(PreviewError::InvalidToken),
    };
    if request.headers().contains_key(header::UPGRADE) {
        return Err(PreviewError::UpgradeUnsupported);
    }

    let (endpoint, access) = match state
        .backend
        .get_sandbox_domain(&grant.api_key, &claims.lease_id, claims.port)
        .await
    {
        Ok(GetSandboxDomainResponse::Available { endpoint, access, .. }) => (endpoint, access),
        Ok(GetSandboxDomainResponse::NotReady { .. }) => return Err(PreviewError::UpstreamNotReady),
        Ok(GetSandboxDomainResponse::PortNotExposed) => return Err(PreviewError::PortNotExposed),
        Ok(GetSandboxDomainResponse::Unsupported) => return Err(PreviewError::Unsupported),
        Ok(GetSandboxDomainResponse::Denied(_)) | Err(ClientError::Denied(_)) => return Err(PreviewError::Denied),
        Err(error) => return Err(PreviewError::Resolution(error)),
    };

    let (parts, body) = request.into_parts();
    let upstream_root = upstream_base(&endpoint, access.as_ref());
    let base_path = reqwest::Url::parse(&upstream_root)
        .map(|url| url.path().trim_end_matches('/').to_string())
        .unwrap_or_default();
    let mut target = format!("{upstream_root}/{rest}");
    if let Some(query) = parts.uri.query() {
        target.push('?');
        target.push_str(query);
    }

    let mut headers = HeaderMap::new();
    for (name, value) in &parts.headers {
        if forwardable(name) && name != header::HOST && !name.as_str().eq_ignore_ascii_case(API_KEY_HEADER) {
            headers.append(name.clone(), value.clone());
        }
    }
    headers.extend(access_headers(access.as_ref()));

    let upstream = preview
        .client()
        .request(parts.method, &target)
        .headers(headers)
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await
        .map_err(PreviewError::Unreachable)?;

    let status = upstream.status();
    let upstream_url = upstream.url().clone();
    let upstream_headers = upstream.headers().clone();
    let mut response = Response::new(Body::from_stream(upstream.bytes_stream()));
    *response.status_mut() = status;
    let prefix = format!(
        "{PREVIEW_ROUTE_PREFIX}/{}/{port}/{token}",
        encode_segment(claims.lease_id.as_str())
    );
    let out = response.headers_mut();
    for (name, value) in &upstream_headers {
        if forwardable(name) {
            let value = if name == header::LOCATION {
                preview_location(value, &upstream_url, &base_path, &prefix)
            } else {
                value.clone()
            };
            out.append(name.clone(), value);
        }
    }
    // The token sits in the URL, so keep it out of Referer headers.
    out.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    Ok(response)
}
