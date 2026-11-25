use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CredentialKind {
    AppToken,
    BotToken,
    ClientSecret,
    ClientState,
    ConsumerSecret,
    SecretToken,
    ServiceAccountCredentials,
    SigningSecret,
    SigningToken,
    VerificationToken,
    WebhookSecret,
    WebhookToken,
}

impl CredentialKind {
    pub const ALL: [Self; 12] = [
        Self::AppToken,
        Self::BotToken,
        Self::ClientSecret,
        Self::ClientState,
        Self::ConsumerSecret,
        Self::SecretToken,
        Self::ServiceAccountCredentials,
        Self::SigningSecret,
        Self::SigningToken,
        Self::VerificationToken,
        Self::WebhookSecret,
        Self::WebhookToken,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppToken => "app_token",
            Self::BotToken => "bot_token",
            Self::ClientSecret => "client_secret",
            Self::ClientState => "client_state",
            Self::ConsumerSecret => "consumer_secret",
            Self::SecretToken => "secret_token",
            Self::ServiceAccountCredentials => "service_account_credentials",
            Self::SigningSecret => "signing_secret",
            Self::SigningToken => "signing_token",
            Self::VerificationToken => "verification_token",
            Self::WebhookSecret => "webhook_secret",
            Self::WebhookToken => "webhook_token",
        }
    }
}

impl fmt::Display for CredentialKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
