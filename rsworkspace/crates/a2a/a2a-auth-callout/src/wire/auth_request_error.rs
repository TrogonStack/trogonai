use crate::admission_keys::XkeyExchangeError;

#[derive(Debug, thiserror::Error)]
pub enum AuthRequestError {
    #[error("authorization request arrived in plaintext although the callout xkey is configured")]
    UnexpectedPlaintext,
    #[error("authorization request is encrypted but the callout has no xkey to open it")]
    UnexpectedEncryption,
    #[error("encrypted authorization request does not name the server xkey that sealed it")]
    MissingServerXkey,
    #[error("authorization request names a server xkey that is not a curve public key")]
    InvalidServerXkey,
    #[error("encrypted authorization request does not open with the callout xkey")]
    Unsealable(#[source] XkeyExchangeError),
    #[error("authorization request is not a signed JWT")]
    NotAJwt,
    #[error("authorization request is not signed by a server key")]
    IssuerNotAServer,
    #[error("authorization request signature does not verify")]
    BadSignature,
    #[error("authorization request claims do not decode")]
    Claims(#[source] serde_json::Error),
    #[error("authorization request is not addressed to the authorization callout")]
    WrongAudience,
    #[error("authorization request was not issued for the AUTH account")]
    WrongAccount,
    #[error("authorization request names a server id other than the key that signed it")]
    ServerIdMismatch,
    #[error("authorization request names a server xkey other than the one that sealed it")]
    ServerXkeyMismatch,
    #[error("plaintext authorization request asks for an encrypted response")]
    PlaintextAsksForEncryption,
}
