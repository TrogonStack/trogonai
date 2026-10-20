use crate::{AuthError, ObjectId};
use serde::Deserialize;
use std::collections::BTreeMap;
use url::Url;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticationConfigInput {
    pub issuers: Vec<IssuerInput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssuerInput {
    pub issuer: String,
    pub jwks_uri: String,
    pub audience: String,
    #[serde(default = "default_user_claim")]
    pub user_claim: String,
    #[serde(default = "default_client_claim")]
    pub client_claim: String,
    #[serde(default = "default_scope_claim")]
    pub scope_claim: String,
    #[serde(default)]
    pub required_scopes: Vec<String>,
    pub users: BTreeMap<String, String>,
    pub clients: BTreeMap<String, String>,
    #[serde(default)]
    pub delegation: DelegationPolicy,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationPolicy {
    #[default]
    Optional,
    Required,
    Forbidden,
}

pub(crate) struct Issuer {
    pub(crate) issuer: String,
    pub(crate) jwks_uri: Url,
    pub(crate) audience: String,
    pub(crate) user_claim: String,
    pub(crate) client_claim: String,
    pub(crate) scope_claim: String,
    pub(crate) required_scopes: Vec<String>,
    pub(crate) users: BTreeMap<String, ObjectId>,
    pub(crate) clients: BTreeMap<String, ObjectId>,
    pub(crate) delegation: DelegationPolicy,
}

impl TryFrom<IssuerInput> for Issuer {
    type Error = AuthError;

    fn try_from(input: IssuerInput) -> Result<Self, Self::Error> {
        let issuer_url = endpoint(&input.issuer)?;
        let jwks_uri = endpoint(&input.jwks_uri)?;
        if issuer_url.query().is_some()
            || input.audience.is_empty()
            || [&input.user_claim, &input.client_claim, &input.scope_claim]
                .iter()
                .any(|claim| claim.is_empty() || claim.len() > 128)
            || input
                .required_scopes
                .iter()
                .any(|scope| scope.is_empty() || scope.split_whitespace().count() != 1)
            || input.users.is_empty()
            || input.clients.is_empty()
        {
            return Err(AuthError::Configuration);
        }
        Ok(Self {
            issuer: input.issuer,
            jwks_uri,
            audience: input.audience,
            user_claim: input.user_claim,
            client_claim: input.client_claim,
            scope_claim: input.scope_claim,
            required_scopes: input.required_scopes,
            users: mappings(input.users)?,
            clients: mappings(input.clients)?,
            delegation: input.delegation,
        })
    }
}

fn mappings(input: BTreeMap<String, String>) -> Result<BTreeMap<String, ObjectId>, AuthError> {
    input
        .into_iter()
        .map(|(external, local)| {
            if external.is_empty() {
                return Err(AuthError::Configuration);
            }
            Ok((external, ObjectId::new(local).map_err(|_| AuthError::Configuration)?))
        })
        .collect()
}

fn endpoint(input: &str) -> Result<Url, AuthError> {
    let url = Url::parse(input).map_err(|_| AuthError::Configuration)?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(AuthError::Configuration);
    }
    Ok(url)
}

fn default_user_claim() -> String {
    "sub".to_owned()
}
fn default_client_claim() -> String {
    "client_id".to_owned()
}
fn default_scope_claim() -> String {
    "scope".to_owned()
}
