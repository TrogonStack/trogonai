use trogon_nats::constants::TLS_DISABLED_VALUES;

use super::ConfigSetting;

/// Why the OpenBao address, read together with the switch that disables TLS,
/// was refused. The service reaches OpenBao over `https` unless TLS is
/// disabled by name (ADR#0081 Decision 1), so an address and a switch that
/// disagree are a misconfiguration rather than a choice to make for the
/// operator.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum OpenBaoAddressError {
    #[error("the OpenBao address is not a URL")]
    NotAUrl(#[source] url::ParseError),
    #[error("the OpenBao address carries credentials; the service logs in with its AppRole")]
    Credentials,
    #[error("the OpenBao address uses scheme {0:?}; use https")]
    UnsupportedScheme(String),
    #[error(
        "the OpenBao address uses http, which only {} allows; use https",
        ConfigSetting::OpenBaoTlsDisabled
    )]
    Plaintext,
    #[error(
        "the OpenBao address uses https, but {} disables TLS",
        ConfigSetting::OpenBaoTlsDisabled
    )]
    TlsDisabledForHttps,
    #[error(
        "{} is {value:?}, which is none of {}",
        ConfigSetting::OpenBaoTlsDisabled,
        TLS_DISABLED_VALUES.join(", ")
    )]
    UnrecognizedTlsDisabledValue { value: String },
}
