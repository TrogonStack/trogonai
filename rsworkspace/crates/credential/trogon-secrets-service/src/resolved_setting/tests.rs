use super::*;

#[test]
fn a_resolved_setting_reads_as_its_name_its_value_and_its_source() {
    for (resolved, line) in [
        (
            ResolvedSetting {
                setting: ConfigSetting::AuditInstance,
                value: SettingValue::Shown("secrets-0".to_owned()),
                source: SettingSource::Environment,
            },
            "audit.instance (TROGON_SECRETS_AUDIT_INSTANCE) = secrets-0 (from environment)",
        ),
        (
            ResolvedSetting {
                setting: ConfigSetting::NatsTlsDisabled,
                value: SettingValue::Shown("false".to_owned()),
                source: SettingSource::Default,
            },
            "NATS_TLS_DISABLED = false (from default)",
        ),
        (
            ResolvedSetting {
                setting: ConfigSetting::NatsCreds,
                value: SettingValue::Unset,
                source: SettingSource::Default,
            },
            "nats.creds (NATS_CREDS) = <unset> (from default)",
        ),
    ] {
        assert_eq!(format!("{resolved}"), line);
    }
}
