//! Opening a NATS connection from a single URL that may carry credentials.

use trogon_nats::{CredentialValue, NatsAuth, NatsConfig, NatsTls, connect_once};

use trogon_std::secret_string::SecretString;

use crate::constants::CONNECT_TIMEOUT;
use crate::nats_connect_error::NatsConnectError;

/// Connects to the server at `nats_url`.
///
/// Credentials in the URL's userinfo authenticate as user and password and
/// are removed from the URL before it is handed to the client, so they never
/// reach logs or error messages. A user without a password is refused. Transport security follows the scheme: `tls`
/// requires TLS and every other scheme connects in the clear.
pub async fn connect(nats_url: &str) -> Result<async_nats::Client, NatsConnectError> {
    let url = url::Url::parse(nats_url)?;
    let tls = if url.scheme() == "tls" {
        NatsTls::default()
    } else {
        NatsTls::Disabled
    };
    let username = url.username();
    let auth = if username.is_empty() {
        NatsAuth::None
    } else {
        NatsAuth::UserPassword {
            user: username.to_owned(),
            password: CredentialValue::Literal(SecretString::new(url.password().unwrap_or(""))?),
        }
    };
    let mut clean = url.clone();
    // Both setters fail only for a URL that cannot carry userinfo, which has none to remove.
    let _ = clean.set_username("");
    let _ = clean.set_password(None);
    let config = NatsConfig::new(vec![clean.to_string()], NatsAuth::None).with_tls(tls);
    Ok(connect_once(&config, auth, CONNECT_TIMEOUT).await?)
}
