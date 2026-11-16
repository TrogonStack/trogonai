use std::fmt;

/// The canonical name of one setting of the service configuration, spelt the
/// way an operator sets it: by its key in the configuration file and by the
/// environment variable that overrides that key (ADR#0007). The switches that
/// disable TLS have no key, because only the environment may disable TLS
/// (ADR#0081).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigSetting {
    NatsUrl,
    NatsCreds,
    NatsNkey,
    NatsUser,
    NatsPassword,
    NatsToken,
    NatsJwt,
    NatsSeed,
    NatsTlsCert,
    NatsTlsKey,
    NatsTlsCa,
    NatsTlsDisabled,
    OpenBaoAddress,
    OpenBaoTlsDisabled,
    OpenBaoMount,
    OpenBaoRoleId,
    OpenBaoSecretIdFile,
    OpenBaoChildTokenRole,
    OpenBaoChildTokenPolicy,
    TenantOwnersFile,
    ReleasePoliciesFile,
    XkeySeed,
    CredentialStream,
    AdmissionLimit,
    AuditTenant,
    AuditProject,
    AuditInstance,
}

impl ConfigSetting {
    /// The key of the setting in the configuration file, when the file may
    /// hold it.
    pub fn key(self) -> Option<&'static str> {
        Some(match self {
            Self::NatsUrl => "nats.url",
            Self::NatsCreds => "nats.creds",
            Self::NatsNkey => "nats.nkey",
            Self::NatsUser => "nats.user",
            Self::NatsPassword => "nats.password",
            Self::NatsToken => "nats.token",
            Self::NatsJwt => "nats.jwt",
            Self::NatsSeed => "nats.seed",
            Self::NatsTlsCert => "nats.tls_cert",
            Self::NatsTlsKey => "nats.tls_key",
            Self::NatsTlsCa => "nats.tls_ca",
            Self::NatsTlsDisabled | Self::OpenBaoTlsDisabled => return None,
            Self::OpenBaoAddress => "openbao.address",
            Self::OpenBaoMount => "openbao.mount",
            Self::OpenBaoRoleId => "openbao.role_id",
            Self::OpenBaoSecretIdFile => "openbao.secret_id_file",
            Self::OpenBaoChildTokenRole => "openbao.child_token_role",
            Self::OpenBaoChildTokenPolicy => "openbao.child_token_policy",
            Self::TenantOwnersFile => "release.tenant_owners_file",
            Self::ReleasePoliciesFile => "release.policies_file",
            Self::XkeySeed => "release.xkey_seed",
            Self::CredentialStream => "release.credential_stream",
            Self::AdmissionLimit => "release.admission_limit",
            Self::AuditTenant => "audit.tenant",
            Self::AuditProject => "audit.project",
            Self::AuditInstance => "audit.instance",
        })
    }

    /// The environment variable that sets it.
    pub fn env(self) -> &'static str {
        match self {
            Self::NatsUrl => "NATS_URL",
            Self::NatsCreds => "NATS_CREDS",
            Self::NatsNkey => "NATS_NKEY",
            Self::NatsUser => "NATS_USER",
            Self::NatsPassword => "NATS_PASSWORD",
            Self::NatsToken => "NATS_TOKEN",
            Self::NatsJwt => "NATS_JWT",
            Self::NatsSeed => "NATS_SEED",
            Self::NatsTlsCert => "NATS_CERT",
            Self::NatsTlsKey => "NATS_KEY",
            Self::NatsTlsCa => "NATS_CA",
            Self::NatsTlsDisabled => "NATS_TLS_DISABLED",
            Self::OpenBaoAddress => "OPENBAO_ADDR",
            Self::OpenBaoTlsDisabled => "OPENBAO_TLS_DISABLED",
            Self::OpenBaoMount => "OPENBAO_MOUNT",
            Self::OpenBaoRoleId => "OPENBAO_ROLE_ID",
            Self::OpenBaoSecretIdFile => "OPENBAO_SECRET_ID_FILE",
            Self::OpenBaoChildTokenRole => "OPENBAO_CHILD_TOKEN_ROLE",
            Self::OpenBaoChildTokenPolicy => "OPENBAO_CHILD_TOKEN_POLICY",
            Self::TenantOwnersFile => "TROGON_SECRETS_TENANT_OWNERS_FILE",
            Self::ReleasePoliciesFile => "TROGON_SECRETS_RELEASE_POLICIES_FILE",
            Self::XkeySeed => "TROGON_SECRETS_XKEY_SEED",
            Self::CredentialStream => "TROGON_SECRETS_CREDENTIAL_STREAM",
            Self::AdmissionLimit => "TROGON_SECRETS_ADMISSION_LIMIT",
            Self::AuditTenant => "TROGON_SECRETS_AUDIT_TENANT",
            Self::AuditProject => "TROGON_SECRETS_AUDIT_PROJECT",
            Self::AuditInstance => "TROGON_SECRETS_AUDIT_INSTANCE",
        }
    }
}

impl fmt::Display for ConfigSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.key() {
            Some(key) => write!(f, "{key} ({})", self.env()),
            None => f.write_str(self.env()),
        }
    }
}

#[cfg(test)]
mod tests;
