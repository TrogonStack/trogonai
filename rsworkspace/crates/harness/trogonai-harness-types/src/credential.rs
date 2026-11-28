//! Authorization for one provider request.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ContentHash, ContentHasher, Credential, CredentialSource, TeamId};

/// Owned authorization selected for provider work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderAuthorization {
    /// The runtime may lend this credential to a native provider implementation.
    Direct(Credential),
    /// The embedding host retains every credential byte.
    HostManaged,
}

impl ProviderAuthorization {
    /// Borrows the credential only when the runtime owns it.
    #[must_use]
    pub const fn credential(&self) -> Option<&Credential> {
        match self {
            Self::Direct(credential) => Some(credential),
            Self::HostManaged => None,
        }
    }

    /// Replaces direct authority after a successful refresh.
    pub fn replace_direct(&mut self, credential: Credential) -> bool {
        match self {
            Self::Direct(active) => {
                *active = credential;
                true
            }
            Self::HostManaged => false,
        }
    }

    /// Whether authority is retained by the embedding host.
    #[must_use]
    pub const fn is_host_managed(&self) -> bool {
        matches!(self, Self::HostManaged)
    }
}

impl From<Credential> for ProviderAuthorization {
    fn from(value: Credential) -> Self {
        Self::Direct(value)
    }
}

/// Why an account identifier was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("account identifier must be 1 to 1024 printable ASCII characters and no spaces")]
pub struct CredentialAccountIdError;

/// The account a subscription credential speaks for.
///
/// Subscription routes put this on the wire, so a value that would need
/// escaping, or that a store could not write back, is refused here rather than
/// sent and rejected.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CredentialAccountId(String);

impl CredentialAccountId {
    const MAX_LEN: usize = 1024;

    /// Parses an account identifier.
    pub fn parse(raw: &str) -> Result<Self, CredentialAccountIdError> {
        if raw.is_empty() || raw.len() > Self::MAX_LEN {
            return Err(CredentialAccountIdError);
        }
        if !raw.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
            return Err(CredentialAccountIdError);
        }
        Ok(Self(raw.to_owned()))
    }

    /// The identifier as the provider wrote it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CredentialAccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for CredentialAccountId {
    type Error = CredentialAccountIdError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::parse(&raw)
    }
}

impl From<CredentialAccountId> for String {
    fn from(value: CredentialAccountId) -> Self {
        value.0
    }
}

/// What a request may show a provider to authorize itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirectCredentialLease<'a> {
    /// The secret the request may send.
    pub secret: Option<&'a Credential>,
    /// Which credential answered, when the resolution said.
    pub source: Option<CredentialSource>,
    /// The account the credential speaks for.
    pub account_id: Option<&'a CredentialAccountId>,
    /// The team the request is made on behalf of.
    pub tenant_context: Option<&'a TeamId>,
}

/// Borrowed authorization for one provider request.
///
/// A host-managed request is authorized outside the runtime, so it has no local
/// authority to lend: every accessor below answers with nothing rather than
/// letting credential or account bytes reach an embedded runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialLease<'a> {
    /// The runtime holds the authority and lends it for this request.
    Direct(DirectCredentialLease<'a>),
    /// The host authorizes the request and the runtime never sees how.
    HostManaged,
}

impl Default for CredentialLease<'_> {
    fn default() -> Self {
        Self::Direct(DirectCredentialLease::default())
    }
}

impl<'a> CredentialLease<'a> {
    /// The secret to send, when this request has one to send.
    #[must_use]
    pub const fn secret(&self) -> Option<&'a Credential> {
        match self {
            Self::Direct(direct) => direct.secret,
            Self::HostManaged => None,
        }
    }

    /// Which credential authorized the request.
    #[must_use]
    pub const fn credential_source(&self) -> Option<CredentialSource> {
        match self {
            Self::Direct(direct) => direct.source,
            Self::HostManaged => Some(CredentialSource::HostManaged),
        }
    }

    /// The account the credential speaks for.
    #[must_use]
    pub const fn account_id(&self) -> Option<&'a CredentialAccountId> {
        match self {
            Self::Direct(direct) => direct.account_id,
            Self::HostManaged => None,
        }
    }

    /// The team the request is made on behalf of.
    #[must_use]
    pub const fn tenant(&self) -> Option<&'a TeamId> {
        match self {
            Self::Direct(direct) => direct.tenant_context,
            Self::HostManaged => None,
        }
    }
}

/// Whose authority a record was written under.
///
/// Not a secret and not reversible, which is the point: a session file can hold this and a
/// later process can tell whether the credential it now has is the one that earned the record,
/// without the file ever holding a key.
///
/// What identifies an authority differs by route, and that is the whole of the
/// logic. A gateway credential identifies the slot it was resolved from, so
/// rotating the key behind one slot leaves the authority alone. A subscription
/// identifies the account, because the same person signing in again is the same
/// authority and a different account never is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialAuthority(ContentHash);

/// What the digest is domained under, so an authority cannot collide with any
/// other digest the runtime writes down.
const AUTHORITY_DOMAIN: &[u8] = b"trogonai-credential-authority-v1\x00";

impl CredentialAuthority {
    /// The authority a credential from `source` speaks with.
    ///
    /// `None` when a subscription route was not told which account it is signed in as: an
    /// authority that stood for "some ChatGPT account" would match every other one, which is
    /// worse than having none.
    #[must_use]
    pub fn derive(source: CredentialSource, account: Option<&CredentialAccountId>) -> Option<Self> {
        let mut hasher = ContentHasher::new();
        hasher.update(AUTHORITY_DOMAIN);
        hasher.update(source.label().as_bytes());
        match source {
            CredentialSource::VercelOidcToken
            | CredentialSource::AiGatewayApiKey
            | CredentialSource::TrogonaiLogin
            | CredentialSource::StoredKey
            | CredentialSource::HostManaged
            | CredentialSource::Configured => hasher.update(b"\x00slot\x00"),
            CredentialSource::ChatgptSubscription | CredentialSource::GrokSubscription => {
                hasher.update(b"\x00account\x00");
                hasher.update(account?.as_str().as_bytes());
            }
        }
        Some(Self(hasher.finish()))
    }

    /// The authority as it is written down.
    #[must_use]
    pub fn digest(&self) -> ContentHash {
        self.0
    }
}

impl fmt::Display for CredentialAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl CredentialAuthority {
    /// Derives credential authority for an independently admitted host provider.
    #[must_use]
    pub fn derive_host_provider(binding: crate::HostProviderBinding) -> Self {
        let mut hasher = ContentHasher::new();
        hasher.update(b"trogonai-host-provider-credential-authority-v1\x00");
        hasher.update(binding.digest().as_bytes());
        Self(hasher.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(raw: &str) -> CredentialAccountId {
        CredentialAccountId::parse(raw).expect("a printable account id")
    }

    #[test]
    fn a_subscription_speaks_for_its_account_and_for_no_other() {
        let first = CredentialAuthority::derive(CredentialSource::ChatgptSubscription, Some(&account("r-acct")));
        let signed_in_again =
            CredentialAuthority::derive(CredentialSource::ChatgptSubscription, Some(&account("r-acct")));
        let somebody_else =
            CredentialAuthority::derive(CredentialSource::ChatgptSubscription, Some(&account("s-acct")));

        assert_eq!(first, signed_in_again);
        assert_ne!(first, somebody_else);
        assert_eq!(
            CredentialAuthority::derive(CredentialSource::GrokSubscription, None),
            None,
            "an authority standing for any account would match every account"
        );
    }

    #[test]
    fn a_gateway_credential_speaks_for_its_slot_and_not_for_its_key() {
        let api_key = CredentialAuthority::derive(CredentialSource::AiGatewayApiKey, None);
        let same_slot =
            CredentialAuthority::derive(CredentialSource::AiGatewayApiKey, Some(&account("ignored-account")));
        let stored_key = CredentialAuthority::derive(CredentialSource::StoredKey, None);

        assert_eq!(api_key, same_slot, "a rotated key is the same authority");
        assert_ne!(api_key, stored_key);
        for source in [
            CredentialSource::VercelOidcToken,
            CredentialSource::TrogonaiLogin,
            CredentialSource::HostManaged,
        ] {
            assert!(CredentialAuthority::derive(source, None).is_some());
        }
    }

    #[test]
    fn a_slot_authority_is_the_digest_the_journal_fixtures_record() {
        // Recovery checkpoints persist this value, so a domain change orphans every one
        // written before it. The portable journal fixtures hold the same digest.
        let authority = CredentialAuthority::derive(CredentialSource::AiGatewayApiKey, None).expect("a slot authority");
        assert_eq!(
            authority.to_string(),
            "a783c5a6289f498c7585a1450131a999106d3a763a3f94056d4a145d6c09c893"
        );
    }

    #[test]
    fn an_authority_written_down_reads_back_as_the_same_one() {
        let authority = CredentialAuthority::derive(CredentialSource::TrogonaiLogin, None).expect("a slot authority");
        let written = serde_json::to_string(&authority).expect("a digest serializes");

        assert_eq!(authority.to_string(), authority.digest().to_string());
        assert_eq!(
            serde_json::from_str::<CredentialAuthority>(&written).expect("a digest reads back"),
            authority
        );
    }

    #[test]
    fn a_host_managed_lease_carries_no_local_authority() {
        let lease = CredentialLease::HostManaged;

        assert!(lease.secret().is_none());
        assert!(lease.account_id().is_none());
        assert!(lease.tenant().is_none());
        assert_eq!(lease.credential_source(), Some(CredentialSource::HostManaged));
    }

    #[test]
    fn a_lease_that_was_lent_nothing_says_so_rather_than_naming_a_source() {
        let lease = CredentialLease::default();

        assert!(lease.secret().is_none());
        assert!(lease.credential_source().is_none());
    }

    #[test]
    fn an_account_identifier_a_provider_could_not_have_written_is_refused() {
        assert_eq!(CredentialAccountId::parse(""), Err(CredentialAccountIdError));
        assert_eq!(
            CredentialAccountId::parse("acct with space"),
            Err(CredentialAccountIdError)
        );
        assert_eq!(
            CredentialAccountId::parse(&"a".repeat(1_025)),
            Err(CredentialAccountIdError)
        );

        let account =
            CredentialAccountId::parse("org-01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("a printable account identifier");
        assert_eq!(account.as_str(), "org-01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(
            CredentialAccountId::parse(&"a".repeat(1_024)).map(|id| id.as_str().len()),
            Ok(1_024)
        );
    }
}
