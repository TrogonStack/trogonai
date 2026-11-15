use super::*;

#[test]
fn a_setting_reads_as_its_file_key_and_the_variable_that_overrides_it() {
    for (setting, named) in [
        (ConfigSetting::NatsUrl, "nats.url (NATS_URL)"),
        (ConfigSetting::NatsCreds, "nats.creds (NATS_CREDS)"),
        (ConfigSetting::NatsNkey, "nats.nkey (NATS_NKEY)"),
        (ConfigSetting::NatsUser, "nats.user (NATS_USER)"),
        (ConfigSetting::NatsPassword, "nats.password (NATS_PASSWORD)"),
        (ConfigSetting::NatsToken, "nats.token (NATS_TOKEN)"),
        (ConfigSetting::NatsJwt, "nats.jwt (NATS_JWT)"),
        (ConfigSetting::NatsSeed, "nats.seed (NATS_SEED)"),
        (ConfigSetting::NatsTlsCert, "nats.tls_cert (NATS_CERT)"),
        (ConfigSetting::NatsTlsKey, "nats.tls_key (NATS_KEY)"),
        (ConfigSetting::NatsTlsCa, "nats.tls_ca (NATS_CA)"),
        (ConfigSetting::OpenBaoAddress, "openbao.address (OPENBAO_ADDR)"),
        (ConfigSetting::OpenBaoMount, "openbao.mount (OPENBAO_MOUNT)"),
        (ConfigSetting::OpenBaoRoleId, "openbao.role_id (OPENBAO_ROLE_ID)"),
        (
            ConfigSetting::OpenBaoSecretIdFile,
            "openbao.secret_id_file (OPENBAO_SECRET_ID_FILE)",
        ),
        (
            ConfigSetting::OpenBaoChildTokenRole,
            "openbao.child_token_role (OPENBAO_CHILD_TOKEN_ROLE)",
        ),
        (
            ConfigSetting::OpenBaoChildTokenPolicy,
            "openbao.child_token_policy (OPENBAO_CHILD_TOKEN_POLICY)",
        ),
        (
            ConfigSetting::TenantOwnersFile,
            "release.tenant_owners_file (TROGON_SECRETS_TENANT_OWNERS_FILE)",
        ),
        (
            ConfigSetting::ReleasePoliciesFile,
            "release.policies_file (TROGON_SECRETS_RELEASE_POLICIES_FILE)",
        ),
        (ConfigSetting::XkeySeed, "release.xkey_seed (TROGON_SECRETS_XKEY_SEED)"),
        (
            ConfigSetting::CredentialStream,
            "release.credential_stream (TROGON_SECRETS_CREDENTIAL_STREAM)",
        ),
        (
            ConfigSetting::AdmissionLimit,
            "release.admission_limit (TROGON_SECRETS_ADMISSION_LIMIT)",
        ),
        (ConfigSetting::AuditTenant, "audit.tenant (TROGON_SECRETS_AUDIT_TENANT)"),
        (
            ConfigSetting::AuditProject,
            "audit.project (TROGON_SECRETS_AUDIT_PROJECT)",
        ),
        (
            ConfigSetting::AuditInstance,
            "audit.instance (TROGON_SECRETS_AUDIT_INSTANCE)",
        ),
    ] {
        assert_eq!(format!("{setting}"), named);
    }
}

#[test]
fn a_setting_only_the_environment_may_hold_reads_as_its_variable() {
    for (setting, named) in [
        (ConfigSetting::NatsTlsDisabled, "NATS_TLS_DISABLED"),
        (ConfigSetting::OpenBaoTlsDisabled, "OPENBAO_TLS_DISABLED"),
    ] {
        assert_eq!(format!("{setting}"), named);
    }
}
