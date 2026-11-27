//! Constants shared by the credential client.

use std::time::Duration;

/// Reply timeout used until [`crate::CredentialClient::with_timeout`] overrides it.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
