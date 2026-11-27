use base64::Engine;
use http_auth::ChallengeParser;
use oci_client::{Reference, manifest::OciDescriptor, secrets::RegistryAuth};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, WWW_AUTHENTICATE};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;
use trogon_sandbox_contract::Digest;

use crate::constants::{
    AUTH_HEADER_BUDGET, BLOB_REDIRECTS, MANIFEST_LIMIT, TAGS_MAX_PAGES, TAGS_PAGE_LIMIT, TAGS_PAGE_SIZE, TOKEN_LIMIT,
};
use crate::descriptor::descriptor_size;
use crate::{OciError, RegistryHost};

#[derive(Deserialize)]
struct Token {
    token: Option<String>,
    access_token: Option<String>,
}

#[derive(Deserialize)]
struct TagList {
    tags: Option<Vec<String>>,
}

fn rejected(message: impl Into<String>) -> OciError {
    OciError::RegistryRejected(message.into())
}
fn network(error: reqwest::Error) -> OciError {
    if error.is_timeout() {
        rejected("bounded OCI transport deadline exceeded")
    } else {
        rejected(format!("bounded OCI transport failed: {}", error.without_url()))
    }
}

fn authorization(auth: &RegistryAuth) -> Result<Option<HeaderValue>, OciError> {
    let value = match auth {
        RegistryAuth::Anonymous => return Ok(None),
        RegistryAuth::Basic(user, password) => format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
        ),
        RegistryAuth::Bearer(token) => format!("Bearer {token}"),
    };
    let mut value = HeaderValue::from_str(&value).map_err(|_| rejected("invalid registry authorization value"))?;
    value.set_sensitive(true);
    Ok(Some(value))
}

pub(crate) struct BoundedRegistry {
    http: reqwest::Client,
    base: reqwest::Url,
    repository: String,
    credentials: RegistryAuth,
    authorization: Option<HeaderValue>,
    deadline: Instant,
    insecure: Vec<RegistryHost>,
}

impl BoundedRegistry {
    pub(crate) fn new(
        reference: &Reference,
        credentials: RegistryAuth,
        insecure: &[RegistryHost],
        deadline: Instant,
    ) -> Result<Self, OciError> {
        let registry = RegistryHost::new(reference.resolve_registry())?;
        let scheme = if insecure.contains(&registry) { "http" } else { "https" };
        let base = registry.origin(scheme)?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(network)?;
        Ok(Self {
            http,
            base,
            repository: reference.repository().into(),
            credentials,
            authorization: None,
            deadline,
            insecure: insecure.to_vec(),
        })
    }

    fn remaining(&self) -> Result<std::time::Duration, OciError> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|value| !value.is_zero())
            .ok_or_else(|| rejected("bounded OCI transport deadline exceeded"))
    }

    fn resource(&self, kind: &str, selector: &str) -> Result<reqwest::Url, OciError> {
        let mut url = self.base.clone();
        let mut path = url.path_segments_mut().map_err(|_| rejected("invalid registry path"))?;
        path.clear().push("v2");
        for component in self.repository.split('/') {
            path.push(component);
        }
        path.push(kind).push(selector);
        drop(path);
        Ok(url)
    }

    async fn send(&self, url: reqwest::Url, accept: &str, include_auth: bool) -> Result<reqwest::Response, OciError> {
        let mut request = self.http.get(url).timeout(self.remaining()?).header(ACCEPT, accept);
        if include_auth && let Some(auth) = &self.authorization {
            request = request.header(AUTHORIZATION, auth);
        }
        let response = request.send().await.map_err(network)?;
        Ok(response)
    }

    async fn authenticate(&mut self, headers: &HeaderMap) -> Result<(), OciError> {
        let mut bearer = None;
        let mut basic = false;
        let mut bytes = 0usize;
        for header in headers.get_all(WWW_AUTHENTICATE) {
            bytes = bytes
                .checked_add(header.len())
                .ok_or_else(|| rejected("authentication header exceeds budget"))?;
            if bytes > AUTH_HEADER_BUDGET {
                return Err(rejected("authentication header exceeds budget"));
            }
            let value = header
                .to_str()
                .map_err(|_| rejected("invalid registry authentication challenge"))?;
            for challenge in ChallengeParser::new(value) {
                let challenge = challenge.map_err(|_| rejected("malformed registry authentication challenge"))?;
                if challenge.scheme.eq_ignore_ascii_case("Basic") {
                    basic = true;
                }
                if !challenge.scheme.eq_ignore_ascii_case("Bearer") || bearer.is_some() {
                    continue;
                }
                let mut realm = None;
                let mut service = None;
                for (key, value) in challenge.params {
                    if key.eq_ignore_ascii_case("realm") {
                        if realm.is_some() {
                            return Err(rejected("duplicate authentication realm"));
                        }
                        realm = Some(value.to_unescaped());
                    } else if key.eq_ignore_ascii_case("service") {
                        if service.is_some() {
                            return Err(rejected("duplicate authentication service"));
                        }
                        service = Some(value.to_unescaped());
                    }
                }
                bearer = Some((realm.ok_or_else(|| rejected("bearer challenge has no realm"))?, service));
            }
        }
        let Some((realm, service)) = bearer else {
            if basic && matches!(self.credentials, RegistryAuth::Basic(..)) {
                self.authorization = authorization(&self.credentials)?;
                return Ok(());
            }
            return Err(rejected("registry requires unsupported or missing authentication"));
        };
        let mut realm = reqwest::Url::parse(&realm).map_err(|_| rejected("invalid authentication realm URL"))?;
        if !realm.username().is_empty()
            || realm.password().is_some()
            || realm.fragment().is_some()
            || realm.host_str().is_none()
        {
            return Err(rejected("credential-bearing or malformed authentication URL"));
        }
        if realm.scheme() != "https"
            && !(realm.scheme() == "http" && self.base.scheme() == "http" && realm.origin() == self.base.origin())
        {
            return Err(rejected("insecure registry authentication downgrade"));
        }
        if realm.query_pairs().any(|(key, _)| {
            [
                "scope",
                "service",
                "username",
                "password",
                "token",
                "access_token",
                "refresh_token",
                "authorization",
            ]
            .iter()
            .any(|reserved| key.eq_ignore_ascii_case(reserved))
        }) {
            return Err(rejected("credential-bearing or ambiguous authentication URL query"));
        }
        {
            let mut query = realm.query_pairs_mut();
            query.append_pair("scope", &format!("repository:{}:pull", self.repository));
            if let Some(service) = service {
                query.append_pair("service", &service);
            }
        }
        let mut request = self.http.get(realm).timeout(self.remaining()?);
        if let Some(auth) = authorization(&self.credentials)? {
            request = request.header(AUTHORIZATION, auth);
        }
        let response = request.send().await.map_err(network)?;
        if !response.status().is_success() {
            return Err(rejected(format!(
                "authentication endpoint returned HTTP {}",
                response.status().as_u16()
            )));
        }
        let bytes = bounded_body(response, TOKEN_LIMIT, "authentication token").await?;
        let token: Token =
            serde_json::from_slice(&bytes).map_err(|_| rejected("invalid authentication token response"))?;
        let value = match (token.token, token.access_token) {
            (Some(token), Some(other)) if token == other => token,
            (Some(token), None) | (None, Some(token)) => token,
            _ => return Err(rejected("missing or conflicting authentication token")),
        };
        if value.is_empty() {
            return Err(rejected("authentication token is empty"));
        }
        self.authorization = authorization(&RegistryAuth::Bearer(value))?;
        Ok(())
    }

    async fn get_scoped(
        &mut self,
        url: reqwest::Url,
        accept: &str,
        redirects: bool,
        registry_auth: bool,
    ) -> Result<reqwest::Response, OciError> {
        let mut response = self.send(url.clone(), accept, registry_auth).await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED && registry_auth {
            let headers = response.headers().clone();
            drop(response);
            self.authenticate(&headers).await?;
            response = self.send(url, accept, true).await?;
        }
        if response.status().is_redirection() && redirects {
            return Ok(response);
        }
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(OciError::NotFound(
                oci_client::errors::OciDistributionError::ImageManifestNotFoundError(
                    "registry resource not found".into(),
                ),
            ));
        }
        if !response.status().is_success() {
            return Err(rejected(format!(
                "registry returned HTTP {}",
                response.status().as_u16()
            )));
        }
        Ok(response)
    }

    async fn blob_response(&mut self, mut url: reqwest::Url) -> Result<reqwest::Response, OciError> {
        for _ in 0..=BLOB_REDIRECTS {
            let registry_auth = url.origin() == self.base.origin();
            let response = self
                .get_scoped(url.clone(), "application/octet-stream", true, registry_auth)
                .await?;
            if !response.status().is_redirection() {
                return Ok(response);
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or_else(|| rejected("OCI blob redirect has no Location"))?
                .to_str()
                .map_err(|_| rejected("invalid OCI blob redirect Location"))?;
            if location.len() > AUTH_HEADER_BUDGET {
                return Err(rejected("OCI blob redirect URL exceeds budget"));
            }
            let next = url
                .join(location)
                .map_err(|_| rejected("invalid OCI blob redirect URL"))?;
            if !next.username().is_empty()
                || next.password().is_some()
                || next.fragment().is_some()
                || next.host_str().is_none()
            {
                return Err(rejected("credential-bearing or malformed OCI blob redirect URL"));
            }
            let allowed_http = next.scheme() == "http"
                && url.scheme() == "http"
                && self.insecure.iter().any(|registry| {
                    registry
                        .origin("http")
                        .is_ok_and(|allowed| allowed.origin() == next.origin())
                });
            if next.scheme() != "https" && !allowed_http {
                return Err(rejected("insecure OCI blob redirect downgrade"));
            }
            url = next;
        }
        Err(rejected("OCI blob redirect limit exceeded"))
    }

    pub(crate) async fn manifest(&mut self, selector: &str) -> Result<(Vec<u8>, Digest), OciError> {
        self.metadata("manifests", selector, true).await
    }

    pub(crate) async fn referrers(&mut self, digest: &str) -> Result<Vec<u8>, OciError> {
        self.metadata("referrers", digest, false).await.map(|(bytes, _)| bytes)
    }

    /// Every tag of the repository, following `Link: rel="next"` pages of the
    /// distribution tags list. A next link must stay on the registry origin
    /// and on the same tags path, and the walk is bounded.
    pub(crate) async fn tags(&mut self) -> Result<Vec<String>, OciError> {
        let first = self.resource("tags", "list")?;
        let mut url = first.clone();
        url.query_pairs_mut().append_pair("n", &TAGS_PAGE_SIZE.to_string());
        let mut out = Vec::new();
        for _ in 0..TAGS_MAX_PAGES {
            let response = self.get_scoped(url.clone(), "application/json", false, true).await?;
            let next = response
                .headers()
                .get(reqwest::header::LINK)
                .and_then(|value| value.to_str().ok())
                .and_then(next_link_target);
            let bytes = bounded_body(response, TAGS_PAGE_LIMIT, "tag list").await?;
            let page: TagList = serde_json::from_slice(&bytes).map_err(|_| rejected("invalid registry tag list"))?;
            out.extend(page.tags.unwrap_or_default());
            let Some(next) = next else {
                return Ok(out);
            };
            let next = self
                .base
                .join(&next)
                .map_err(|_| rejected("invalid registry pagination link"))?;
            if next.origin() != self.base.origin() || next.path() != first.path() {
                return Err(rejected("registry pagination link leaves the tag list"));
            }
            url = next;
        }
        Err(rejected("registry tag list exceeds page budget"))
    }

    async fn metadata(&mut self, kind: &str, selector: &str, pinned: bool) -> Result<(Vec<u8>, Digest), OciError> {
        let url = self.resource(kind, selector)?;
        let response = self.get_scoped(url,"application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.v2+json, application/vnd.docker.distribution.manifest.list.v2+json",false,true).await?;
        let reported = response
            .headers()
            .get("docker-content-digest")
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| rejected("invalid registry digest header"))?;
        let bytes = bounded_body(response, MANIFEST_LIMIT, "manifest").await?;
        let digest = Digest::new(format!("sha256:{}", hex::encode(Sha256::digest(&bytes))))?;
        if let Some(reported) = reported {
            let reported = Digest::new(reported).map_err(|_| rejected("invalid registry digest header"))?;
            if reported != digest {
                return Err(OciError::DigestMismatch {
                    expected: reported.to_string(),
                    actual: digest.to_string(),
                });
            }
        }
        if pinned && selector.starts_with("sha256:") && selector != digest.as_str() {
            return Err(OciError::DigestMismatch {
                expected: selector.into(),
                actual: digest.to_string(),
            });
        }
        Ok((bytes, digest))
    }

    pub(crate) async fn blob<T: AsyncWrite + Unpin>(
        &mut self,
        descriptor: &OciDescriptor,
        maximum: u64,
        writer: &mut T,
    ) -> Result<(), OciError> {
        if descriptor.urls.as_ref().is_some_and(|urls| !urls.is_empty()) {
            return Err(rejected("foreign OCI blob URLs are not supported"));
        }
        let size = descriptor_size(descriptor)?;
        if size > maximum {
            return Err(rejected("OCI blob descriptor exceeds byte budget"));
        }
        let expected = Digest::new(&descriptor.digest)?;
        let url = self.resource("blobs", expected.as_str())?;
        let mut response = self.blob_response(url).await?;
        if response.content_length().is_some_and(|length| length != size) {
            return Err(rejected("OCI blob Content-Length differs from descriptor"));
        }
        let mut read = 0u64;
        let mut hash = Sha256::new();
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            read = read
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| rejected("OCI blob byte budget overflow"))?;
            if read > size {
                return Err(rejected("OCI blob exceeds descriptor byte budget"));
            }
            hash.update(&chunk);
            writer.write_all(&chunk).await?;
        }
        writer.flush().await?;
        if read != size {
            return Err(rejected("OCI blob length differs from descriptor"));
        }
        let actual = format!("sha256:{}", hex::encode(hash.finalize()));
        if actual != expected.as_str() {
            return Err(OciError::DigestMismatch {
                expected: expected.to_string(),
                actual,
            });
        }
        Ok(())
    }
}

/// Target of the `rel="next"` entry of a `Link` header.
fn next_link_target(header: &str) -> Option<String> {
    header.split(',').find_map(|entry| {
        let (target, params) = entry.split_once(';')?;
        let is_next = params
            .split(';')
            .filter_map(|p| p.trim().strip_prefix("rel="))
            .any(|rel| rel.trim_matches('"').eq_ignore_ascii_case("next"));
        let target = target.trim().strip_prefix('<')?.strip_suffix('>')?;
        is_next.then(|| target.to_string())
    })
}

async fn bounded_body(mut response: reqwest::Response, maximum: u64, kind: &str) -> Result<Vec<u8>, OciError> {
    if response.content_length().is_some_and(|size| size > maximum) {
        return Err(rejected(format!("{kind} response exceeds byte budget")));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if bytes.len() as u64 + chunk.len() as u64 > maximum {
            return Err(rejected(format!("{kind} response exceeds byte budget")));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
