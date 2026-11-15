use std::io;
use std::num::ParseIntError;
use std::path::PathBuf;

use trogon_audit::AuditNameError;
use trogon_nats::SubjectTokenViolationError;
use trogon_nats::constants::TLS_DISABLED_VALUES;
use trogon_secret_store::{OpenBaoMountError, OpenBaoNameError};
use trogon_service_config::{NatsArgsError, SecretReferenceError};

use super::{AdmissionLimitError, ConfigSetting, OpenBaoAddressError, TomlSyntax};

/// Why the service configuration was refused. The service does not start on
/// any of them: every setting it reads has a use, so one it cannot read is
/// one it cannot run without, and a guess at it could release a credential
/// the operator did not mean to.
#[derive(Debug, thiserror::Error)]
pub enum ServiceConfigError {
    #[error("configuration file {} could not be read", path.display())]
    ConfigUnreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("configuration file {} is not a valid configuration file: {syntax}", path.display())]
    ConfigUnparsable { path: PathBuf, syntax: TomlSyntax },
    #[error("{setting} is not set")]
    Missing { setting: ConfigSetting },
    #[error("{setting} is not valid UTF-8")]
    NotUnicode { setting: ConfigSetting },
    #[error("{setting} is not a number")]
    NotANumber {
        setting: ConfigSetting,
        #[source]
        source: ParseIntError,
    },
    #[error("{setting} must be a reference, env://NAME or file:///path, written as a string")]
    SecretNotAString { setting: ConfigSetting },
    #[error("{setting} is not a usable reference")]
    SecretReference {
        setting: ConfigSetting,
        #[source]
        source: SecretReferenceError,
    },
    #[error("the NATS flags cannot be used together")]
    Nats(#[source] NatsArgsError),
    #[error("{} lists no NATS server", ConfigSetting::NatsUrl)]
    NoNatsServers,
    #[error(
        "{} is {value:?}, which is none of {}",
        ConfigSetting::NatsTlsDisabled,
        TLS_DISABLED_VALUES.join(", ")
    )]
    NatsTlsDisabledValue { value: String },
    #[error(
        "{} disables TLS, but {variable} is set, which only TLS uses",
        ConfigSetting::NatsTlsDisabled
    )]
    NatsTlsDisabledAlongside { variable: &'static str },
    #[error("{present} is set without {missing}; a client certificate needs both")]
    NatsClientCertificate {
        present: ConfigSetting,
        missing: ConfigSetting,
    },
    #[error("invalid {}", ConfigSetting::OpenBaoAddress)]
    OpenBaoAddress(#[source] OpenBaoAddressError),
    #[error("invalid {}", ConfigSetting::OpenBaoMount)]
    OpenBaoMount(#[source] OpenBaoMountError),
    #[error("invalid {}", ConfigSetting::OpenBaoChildTokenRole)]
    ChildTokenRole(#[source] OpenBaoNameError),
    #[error("invalid {}", ConfigSetting::OpenBaoChildTokenPolicy)]
    ChildTokenPolicy(#[source] OpenBaoNameError),
    #[error("invalid {}", ConfigSetting::CredentialStream)]
    CredentialStream(#[source] SubjectTokenViolationError),
    #[error("invalid {}", ConfigSetting::AdmissionLimit)]
    AdmissionLimit(#[source] AdmissionLimitError),
    #[error("invalid {}", ConfigSetting::AuditTenant)]
    AuditTenant(#[source] AuditNameError),
    #[error("invalid {}", ConfigSetting::AuditProject)]
    AuditProject(#[source] SubjectTokenViolationError),
    #[error("invalid {}", ConfigSetting::AuditInstance)]
    AuditInstance(#[source] AuditNameError),
    #[error("the audit component is not a subject token")]
    AuditComponent(#[source] SubjectTokenViolationError),
}
