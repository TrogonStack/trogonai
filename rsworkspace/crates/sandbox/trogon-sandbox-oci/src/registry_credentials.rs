use secrecy::SecretString;

/// Basic-auth credentials for one registry; the password is redacted from `Debug`.
#[derive(Debug, Clone)]
pub struct RegistryCredentials {
    pub username: String,
    pub password: SecretString,
}

impl RegistryCredentials {
    pub fn new(username: impl Into<String>, password: SecretString) -> Self {
        Self {
            username: username.into(),
            password,
        }
    }
}
