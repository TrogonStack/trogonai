use super::*;

#[test]
fn a_secret_reads_as_its_reference_or_as_redacted() {
    for (value, shown) in [
        (SettingValue::Shown("secrets-0".to_owned()), "secrets-0"),
        (
            SettingValue::Reference("env://FAKE_NATS_PASSWORD".parse().unwrap()),
            "env://FAKE_NATS_PASSWORD",
        ),
        (
            SettingValue::Reference("file:///run/secrets/fake-xkey-seed".parse().unwrap()),
            "file:///run/secrets/fake-xkey-seed",
        ),
        (SettingValue::Redacted, "<redacted>"),
        (SettingValue::Unset, "<unset>"),
    ] {
        assert_eq!(format!("{value}"), shown);
    }
}
