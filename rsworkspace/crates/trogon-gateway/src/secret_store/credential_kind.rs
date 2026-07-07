use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CredentialKind {
    AppToken,
    BotToken,
    ClientSecret,
    ClientState,
    ConsumerSecret,
    SigningSecret,
    SigningToken,
    VerificationToken,
    WebhookSecret,
}

impl CredentialKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppToken => "app_token",
            Self::BotToken => "bot_token",
            Self::ClientSecret => "client_secret",
            Self::ClientState => "client_state",
            Self::ConsumerSecret => "consumer_secret",
            Self::SigningSecret => "signing_secret",
            Self::SigningToken => "signing_token",
            Self::VerificationToken => "verification_token",
            Self::WebhookSecret => "webhook_secret",
        }
    }
}

impl fmt::Display for CredentialKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
