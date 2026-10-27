use crate::Error;
use serde::Deserialize;
use std::net::SocketAddr;
use url::Url;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpTransportConfigInput {
    pub bind: SocketAddr,
    pub resource_url: String,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

pub struct HttpTransportConfig {
    bind: SocketAddr,
    resource_url: Url,
    allowed_origins: Vec<String>,
    transport_origins: Vec<String>,
}

impl HttpTransportConfig {
    pub fn new(input: HttpTransportConfigInput) -> Result<Self, Error> {
        let resource_url = Url::parse(&input.resource_url).map_err(|_| Error::Configuration)?;
        if !secure_url(&resource_url) || resource_url.path() != "/mcp" || resource_url.as_str() != input.resource_url {
            return Err(Error::Configuration);
        }
        let mut allowed_origins = Vec::new();
        let mut transport_origins = Vec::new();
        for origin in input.allowed_origins {
            let url = Url::parse(&origin).map_err(|_| Error::Configuration)?;
            if !secure_url(&url) || url.path() != "/" || url.origin().ascii_serialization() != origin {
                return Err(Error::Configuration);
            }
            transport_origins.push(format!(
                "{}://{}:{}",
                url.scheme(),
                &url[url::Position::BeforeHost..url::Position::AfterHost],
                url.port_or_known_default().ok_or(Error::Configuration)?
            ));
            allowed_origins.push(origin);
        }
        Ok(Self {
            bind: input.bind,
            resource_url,
            allowed_origins,
            transport_origins,
        })
    }

    pub fn bind(&self) -> SocketAddr {
        self.bind
    }
    pub fn resource_url(&self) -> &str {
        self.resource_url.as_str()
    }
    pub(crate) fn allowed_origins(&self) -> &[String] {
        &self.allowed_origins
    }
    pub(crate) fn transport_origins(&self) -> &[String] {
        &self.transport_origins
    }
    pub(crate) fn authority(&self) -> String {
        self.resource_url[url::Position::BeforeHost..url::Position::AfterPort].to_owned()
    }
    pub(crate) fn metadata_url(&self) -> String {
        let mut url = self.resource_url.clone();
        url.set_path("/.well-known/oauth-protected-resource/mcp");
        url.to_string()
    }
}

fn secure_url(url: &Url) -> bool {
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    (url.scheme() == "https" || (url.scheme() == "http" && local))
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}
