use trogon_sandbox_contract::{CloneAuth, SecretRef};
use url::Url;

use crate::WorkspaceError;
use crate::constants::{ALLOWED_CLONE_SCHEMES, BEARER_USERNAME};

/// A repository url git may fetch from, carrying the credentials the lease supplied.
///
/// Only network and file schemes are accepted, which keeps transports such as `ext::` out of reach.
#[derive(Clone)]
pub(crate) struct CloneUrl(Url);

impl CloneUrl {
    pub(crate) fn new(url: &str, auth: &CloneAuth) -> Result<Self, WorkspaceError> {
        let mut url = Url::parse(url)?;
        if !ALLOWED_CLONE_SCHEMES.contains(&url.scheme()) {
            return Err(WorkspaceError::CloneScheme(url.scheme().to_owned()));
        }
        match auth {
            CloneAuth::None(_) => {}
            CloneAuth::BearerToken { token_ref } => credentials(&mut url, BEARER_USERNAME, token_ref)?,
            CloneAuth::Basic { username, password_ref } => credentials(&mut url, username, password_ref)?,
        }
        Ok(Self(url))
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

fn credentials(url: &mut Url, username: &str, secret: &SecretRef) -> Result<(), WorkspaceError> {
    let password = match secret {
        SecretRef::Literal { value } => value,
        SecretRef::NatsKv { .. } => return Err(WorkspaceError::SecretUnresolved),
    };
    url.set_username(username)
        .map_err(|()| WorkspaceError::CloneCredentials)?;
    url.set_password(Some(password))
        .map_err(|()| WorkspaceError::CloneCredentials)
}
