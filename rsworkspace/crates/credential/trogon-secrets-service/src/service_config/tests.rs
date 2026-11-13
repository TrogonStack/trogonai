use std::env::VarError;
use std::ffi::OsString;
use std::path::Path;

use clap::Parser;
use trogon_nats::{NatsAuth, NatsTls};
use trogon_secret_store::{OpenBaoChildTokens, OpenBaoPolicyName, OpenBaoTokenRole, OpenBaoTransport};
use trogon_service_config::{NatsArgsError, SecretReference, SecretReferenceError};
use trogon_std::env::{InMemoryEnv, ReadEnv};
use trogon_std::fs::MemFs;

use super::*;
use crate::{AdmissionLimit, AdmissionLimitError, ConfigSetting, OpenBaoAddressError, ServiceArgs};

const CONFIG_PATH: &str = "/etc/trogon/secrets-service.toml";

/// Every setting the service cannot start without, with a value it accepts.
const REQUIRED: [(ConfigSetting, &str); 13] = [
    (ConfigSetting::NatsUrl, "nats.internal:4222"),
    (ConfigSetting::OpenBaoAddress, "https://openbao.internal:8200"),
    (ConfigSetting::OpenBaoMount, "secret"),
    (ConfigSetting::OpenBaoRoleId, "fake-role-id"),
    (ConfigSetting::OpenBaoSecretIdFile, "/run/secrets/fake-secret-id"),
    (ConfigSetting::OpenBaoChildTokenRole, "secrets-release"),
    (ConfigSetting::OpenBaoChildTokenPolicy, "secrets-release-read"),
    (ConfigSetting::TenantOwnersFile, "/etc/trogon/tenant-owners.toml"),
    (ConfigSetting::ReleasePoliciesFile, "/etc/trogon/release-policies.toml"),
    (ConfigSetting::XkeySeed, "file:///run/secrets/fake-xkey-seed"),
    (ConfigSetting::AuditTenant, "acme"),
    (ConfigSetting::AuditProject, "platform"),
    (ConfigSetting::AuditInstance, "secrets-0"),
];

/// A configuration file that sets every setting the environment sets in
/// [`complete_env`].
const COMPLETE_FILE: &str = r#"
[nats]
url = "nats.internal:4222"

[openbao]
address = "https://openbao.internal:8200"
mount = "secret"
role_id = "fake-role-id"
secret_id_file = "/run/secrets/fake-secret-id"
child_token_role = "secrets-release"
child_token_policy = "secrets-release-read"

[release]
tenant_owners_file = "/etc/trogon/tenant-owners.toml"
policies_file = "/etc/trogon/release-policies.toml"
xkey_seed = "file:///run/secrets/fake-xkey-seed"

[audit]
tenant = "acme"
project = "platform"
instance = "secrets-0"
"#;

/// An environment that reports one variable as not valid UTF-8 and reads every
/// other one from an [`InMemoryEnv`].
struct NotUnicodeEnv {
    env: InMemoryEnv,
    variable: &'static str,
}

impl ReadEnv for NotUnicodeEnv {
    fn var(&self, key: &str) -> Result<String, VarError> {
        if key == self.variable {
            return Err(VarError::NotUnicode(OsString::from("fake-not-unicode")));
        }
        self.env.var(key)
    }
}

fn args(flags: &[&str]) -> ServiceArgs {
    ServiceArgs::parse_from(["trogon-secrets-service"].iter().chain(flags))
}

fn with_config_file() -> ServiceArgs {
    args(&["--config", CONFIG_PATH])
}

fn complete_env() -> InMemoryEnv {
    let env = InMemoryEnv::new();
    for (setting, value) in REQUIRED {
        env.set(setting.env(), value);
    }
    env
}

fn file(contents: &str) -> MemFs {
    let fs = MemFs::new();
    fs.insert(CONFIG_PATH, contents);
    fs
}

fn refused(env: &impl ReadEnv) -> ServiceConfigError {
    ServiceConfig::resolve(&args(&[]), env, &MemFs::new()).unwrap_err()
}

fn refused_with_file(contents: &str, env: &InMemoryEnv) -> ServiceConfigError {
    ServiceConfig::resolve(&with_config_file(), env, &file(contents)).unwrap_err()
}

fn resolved(env: &InMemoryEnv) -> ServiceConfig {
    ServiceConfig::resolve(&args(&[]), env, &MemFs::new()).unwrap()
}

#[test]
fn a_required_setting_left_unset_refuses_the_configuration() {
    for (setting, _) in REQUIRED {
        let env = complete_env();
        env.remove(setting.env());

        let refused = refused(&env);

        assert!(
            matches!(refused, ServiceConfigError::Missing { setting: missing } if missing == setting),
            "{setting}: {refused:?}"
        );
    }
}

#[test]
fn a_required_setting_set_empty_counts_as_unset() {
    for (setting, _) in REQUIRED {
        let env = complete_env();
        env.set(setting.env(), "");

        let refused = refused(&env);

        assert!(
            matches!(refused, ServiceConfigError::Missing { setting: missing } if missing == setting),
            "{setting}: {refused:?}"
        );
    }
}

#[test]
fn a_setting_that_is_not_unicode_is_refused_by_name() {
    for setting in [
        ConfigSetting::NatsPassword,
        ConfigSetting::NatsTlsDisabled,
        ConfigSetting::OpenBaoTlsDisabled,
        ConfigSetting::XkeySeed,
        ConfigSetting::AdmissionLimit,
        ConfigSetting::AuditInstance,
    ] {
        let env = NotUnicodeEnv {
            env: complete_env(),
            variable: setting.env(),
        };

        let refused = refused(&env);

        assert!(
            matches!(refused, ServiceConfigError::NotUnicode { setting: named } if named == setting),
            "{setting}: {refused:?}"
        );
    }
}

#[test]
fn a_configuration_file_that_cannot_be_read_is_refused() {
    let refused = ServiceConfig::resolve(&with_config_file(), &complete_env(), &MemFs::new()).unwrap_err();

    assert!(
        matches!(&refused, ServiceConfigError::ConfigUnreadable { path, .. } if path == Path::new(CONFIG_PATH)),
        "{refused:?}"
    );
}

#[test]
fn a_configuration_file_with_a_key_no_section_defines_is_refused() {
    for contents in [
        "[release]\nxkey_sed = \"file:///run/secrets/fake-xkey-seed\"\n",
        "[vault]\n",
    ] {
        let refused = refused_with_file(contents, &complete_env());

        assert!(
            matches!(&refused, ServiceConfigError::ConfigUnparsable { path, .. } if path == Path::new(CONFIG_PATH)),
            "{contents}: {refused:?}"
        );
    }
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "display redaction is the contract under test; typed variants separately prove error classification"
    )
)]
fn a_configuration_file_that_is_not_toml_never_quotes_its_lines() {
    let refused = refused_with_file("[nats]\npassword = \"fake-quoted-password\n", &complete_env());

    assert!(
        matches!(refused, ServiceConfigError::ConfigUnparsable { .. }),
        "{refused:?}"
    );
    assert!(!refused.to_string().contains("fake-quoted-password"), "{refused}");
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "display redaction is the contract under test; typed variants separately prove error classification"
    )
)]
fn a_secret_written_into_the_configuration_file_is_refused_without_quoting_it() {
    for (section, key, setting) in [
        ("nats", "nkey", ConfigSetting::NatsNkey),
        ("nats", "password", ConfigSetting::NatsPassword),
        ("nats", "token", ConfigSetting::NatsToken),
        ("nats", "jwt", ConfigSetting::NatsJwt),
        ("nats", "seed", ConfigSetting::NatsSeed),
        ("release", "xkey_seed", ConfigSetting::XkeySeed),
    ] {
        let env = complete_env();
        env.remove(ConfigSetting::XkeySeed.env());

        let refused = refused_with_file(&format!("[{section}]\n{key} = \"fake-inline-secret\"\n"), &env);

        assert!(
            matches!(
                refused,
                ServiceConfigError::SecretReference {
                    setting: named,
                    source: SecretReferenceError::NotAReference,
                } if named == setting
            ),
            "{setting}: {refused:?}"
        );
        assert!(!refused.to_string().contains("fake-inline-secret"), "{refused}");
    }
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "display redaction is the contract under test; typed variants separately prove error classification"
    )
)]
fn a_secret_written_into_the_configuration_file_as_another_type_is_refused_without_quoting_it() {
    for value in [
        "8675309",
        "{ value = \"fake-inline-secret\" }",
        "[\"fake-inline-secret\"]",
    ] {
        let env = complete_env();
        env.remove(ConfigSetting::XkeySeed.env());

        let refused = refused_with_file(&format!("[release]\nxkey_seed = {value}\n"), &env);

        assert!(
            matches!(
                refused,
                ServiceConfigError::SecretNotAString {
                    setting: ConfigSetting::XkeySeed
                }
            ),
            "{value}: {refused:?}"
        );
        let shown = refused.to_string();
        assert!(
            !shown.contains("8675309") && !shown.contains("fake-inline-secret"),
            "{shown}"
        );
    }
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "display redaction is the contract under test; typed variants separately prove error classification"
    )
)]
fn a_sealing_key_seed_given_inline_is_refused_without_echoing_it() {
    let env = complete_env();
    env.set(ConfigSetting::XkeySeed.env(), "SXAFAKEINLINESEED");

    let refused = refused(&env);

    assert!(
        matches!(
            refused,
            ServiceConfigError::SecretReference {
                setting: ConfigSetting::XkeySeed,
                source: SecretReferenceError::NotAReference,
            }
        ),
        "{refused:?}"
    );
    assert!(!refused.to_string().contains("SXAFAKEINLINESEED"), "{refused}");
}

#[test]
fn a_sealing_key_seed_reference_of_another_scheme_is_refused() {
    let env = complete_env();
    env.set(ConfigSetting::XkeySeed.env(), "op://fake-vault/xkey-seed");

    let refused = refused(&env);

    assert!(
        matches!(
            &refused,
            ServiceConfigError::SecretReference {
                setting: ConfigSetting::XkeySeed,
                source: SecretReferenceError::UnsupportedScheme(scheme),
            } if scheme == "op"
        ),
        "{refused:?}"
    );
}

#[test]
fn a_nats_tls_switch_holding_an_unknown_value_is_refused() {
    let env = complete_env();
    env.set(ConfigSetting::NatsTlsDisabled.env(), "maybe");

    let refused = refused(&env);

    assert!(
        matches!(&refused, ServiceConfigError::NatsTlsDisabledValue { value } if value == "maybe"),
        "{refused:?}"
    );
}

#[test]
fn nats_tls_disabled_alongside_a_tls_file_is_refused() {
    let env = complete_env();
    env.set(ConfigSetting::NatsTlsDisabled.env(), "true");
    env.set(ConfigSetting::NatsTlsCa.env(), "/etc/trogon/nats-ca.pem");

    let refused = refused(&env);

    assert!(
        matches!(
            refused,
            ServiceConfigError::NatsTlsDisabledAlongside { variable: "NATS_CA" }
        ),
        "{refused:?}"
    );
}

#[test]
fn half_a_nats_client_certificate_is_refused() {
    for (present, missing) in [
        (ConfigSetting::NatsTlsCert, ConfigSetting::NatsTlsKey),
        (ConfigSetting::NatsTlsKey, ConfigSetting::NatsTlsCert),
    ] {
        let env = complete_env();
        env.set(present.env(), "/etc/trogon/nats-client.pem");

        let refused = refused(&env);

        assert!(
            matches!(
                refused,
                ServiceConfigError::NatsClientCertificate { present: shown, missing: lacking }
                    if shown == present && lacking == missing
            ),
            "{present}: {refused:?}"
        );
    }
}

#[test]
fn a_nats_url_that_lists_no_server_is_refused() {
    let env = complete_env();
    env.set(ConfigSetting::NatsUrl.env(), " , ");

    assert!(matches!(refused(&env), ServiceConfigError::NoNatsServers));
}

#[test]
fn a_nats_user_flag_without_a_password_is_refused() {
    let refused =
        ServiceConfig::resolve(&args(&["--nats-user", "fake-user"]), &complete_env(), &MemFs::new()).unwrap_err();

    assert!(
        matches!(refused, ServiceConfigError::Nats(NatsArgsError::UserSentAsToken)),
        "{refused:?}"
    );
}

#[test]
fn a_plaintext_openbao_address_is_refused_without_the_tls_switch() {
    let env = complete_env();
    env.set(ConfigSetting::OpenBaoAddress.env(), "http://openbao.internal:8200");

    let refused = refused(&env);

    assert!(
        matches!(
            refused,
            ServiceConfigError::OpenBaoAddress(OpenBaoAddressError::Plaintext)
        ),
        "{refused:?}"
    );
}

#[test]
fn an_invalid_name_is_refused_by_its_setting() {
    for (setting, value) in [
        (ConfigSetting::OpenBaoMount, "secret/../sys"),
        (ConfigSetting::OpenBaoChildTokenRole, "secrets release"),
        (ConfigSetting::OpenBaoChildTokenPolicy, "secrets/release"),
        (ConfigSetting::CredentialStream, "CREDENTIAL.EVENTS"),
        (ConfigSetting::AuditTenant, "acme corp"),
        (ConfigSetting::AuditProject, "plat.form"),
        (ConfigSetting::AuditInstance, "secrets 0"),
    ] {
        let env = complete_env();
        env.set(setting.env(), value);

        let refused = refused(&env);

        let named = match refused {
            ServiceConfigError::OpenBaoMount(_) => ConfigSetting::OpenBaoMount,
            ServiceConfigError::ChildTokenRole(_) => ConfigSetting::OpenBaoChildTokenRole,
            ServiceConfigError::ChildTokenPolicy(_) => ConfigSetting::OpenBaoChildTokenPolicy,
            ServiceConfigError::CredentialStream(_) => ConfigSetting::CredentialStream,
            ServiceConfigError::AuditTenant(_) => ConfigSetting::AuditTenant,
            ServiceConfigError::AuditProject(_) => ConfigSetting::AuditProject,
            ServiceConfigError::AuditInstance(_) => ConfigSetting::AuditInstance,
            other => panic!("{setting}: {other:?}"),
        };
        assert_eq!(named, setting);
    }
}

#[test]
fn an_admission_limit_that_is_not_a_number_is_refused() {
    let env = complete_env();
    env.set(ConfigSetting::AdmissionLimit.env(), "many");

    let refused = refused(&env);

    assert!(
        matches!(
            refused,
            ServiceConfigError::NotANumber {
                setting: ConfigSetting::AdmissionLimit,
                ..
            }
        ),
        "{refused:?}"
    );
}

#[test]
fn a_zero_admission_limit_is_refused_from_any_source() {
    let env = complete_env();
    env.set(ConfigSetting::AdmissionLimit.env(), "0");
    assert!(matches!(
        refused(&env),
        ServiceConfigError::AdmissionLimit(AdmissionLimitError::Zero)
    ));

    let refused = refused_with_file("[release]\nadmission_limit = 0\n", &complete_env());
    assert!(
        matches!(refused, ServiceConfigError::AdmissionLimit(AdmissionLimitError::Zero)),
        "{refused:?}"
    );
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "naming the setting is the contract under test; typed variants separately prove error classification"
    )
)]
fn a_refusal_names_the_setting_by_its_key_and_its_variable() {
    let env = complete_env();
    env.remove(ConfigSetting::OpenBaoSecretIdFile.env());
    assert_eq!(
        refused(&env).to_string(),
        "openbao.secret_id_file (OPENBAO_SECRET_ID_FILE) is not set"
    );

    for (setting, value, named) in [
        (ConfigSetting::NatsTlsDisabled, "maybe", "NATS_TLS_DISABLED"),
        (ConfigSetting::NatsTlsCert, "/etc/trogon/nats-client.pem", "NATS_KEY"),
        (ConfigSetting::NatsUrl, ",", "NATS_URL"),
        (
            ConfigSetting::OpenBaoAddress,
            "http://openbao.internal:8200",
            "OPENBAO_ADDR",
        ),
        (ConfigSetting::OpenBaoMount, "", "OPENBAO_MOUNT"),
        (ConfigSetting::OpenBaoMount, "a/../b", "OPENBAO_MOUNT"),
        (ConfigSetting::OpenBaoChildTokenRole, "a b", "OPENBAO_CHILD_TOKEN_ROLE"),
        (
            ConfigSetting::OpenBaoChildTokenPolicy,
            "a b",
            "OPENBAO_CHILD_TOKEN_POLICY",
        ),
        (ConfigSetting::XkeySeed, "SXAFAKEINLINESEED", "TROGON_SECRETS_XKEY_SEED"),
        (
            ConfigSetting::CredentialStream,
            "A.B",
            "TROGON_SECRETS_CREDENTIAL_STREAM",
        ),
        (ConfigSetting::AdmissionLimit, "many", "TROGON_SECRETS_ADMISSION_LIMIT"),
        (ConfigSetting::AdmissionLimit, "0", "TROGON_SECRETS_ADMISSION_LIMIT"),
        (ConfigSetting::AuditTenant, "a b", "TROGON_SECRETS_AUDIT_TENANT"),
        (ConfigSetting::AuditProject, "a.b", "TROGON_SECRETS_AUDIT_PROJECT"),
        (ConfigSetting::AuditInstance, "a b", "TROGON_SECRETS_AUDIT_INSTANCE"),
    ] {
        let env = complete_env();
        env.set(setting.env(), value);

        let shown = refused(&env).to_string();

        assert!(shown.contains(named), "{setting}: {shown}");
    }

    let env = complete_env();
    env.set(ConfigSetting::NatsTlsDisabled.env(), "true");
    env.set(ConfigSetting::NatsTlsCa.env(), "/etc/trogon/nats-ca.pem");
    let shown = refused(&env).to_string();
    assert!(
        shown.contains("NATS_TLS_DISABLED") && shown.contains("NATS_CA"),
        "{shown}"
    );

    let env = NotUnicodeEnv {
        env: complete_env(),
        variable: ConfigSetting::AuditInstance.env(),
    };
    assert!(refused(&env).to_string().contains("TROGON_SECRETS_AUDIT_INSTANCE"));

    let env = complete_env();
    env.remove(ConfigSetting::XkeySeed.env());
    let shown = refused_with_file("[release]\nxkey_seed = 1\n", &env).to_string();
    assert!(shown.contains("TROGON_SECRETS_XKEY_SEED"), "{shown}");

    let shown = ServiceConfig::resolve(&with_config_file(), &complete_env(), &MemFs::new())
        .unwrap_err()
        .to_string();
    assert!(shown.contains(CONFIG_PATH), "{shown}");
    let shown = refused_with_file("[vault]\n", &complete_env()).to_string();
    assert!(shown.contains(CONFIG_PATH), "{shown}");
    let shown = ServiceConfig::resolve(&args(&["--nats-user", "fake-user"]), &complete_env(), &MemFs::new())
        .unwrap_err()
        .to_string();
    assert!(shown.contains("NATS"), "{shown}");
}

#[test]
fn the_environment_alone_configures_the_service_with_its_defaults() {
    let config = resolved(&complete_env());

    assert_eq!(config.nats.servers, ["nats.internal:4222"]);
    assert!(matches!(config.nats.auth, NatsAuth::None));
    assert!(
        matches!(
            config.nats.tls,
            Ok(NatsTls::Enabled {
                ca: None,
                client_cert: None
            })
        ),
        "{:?}",
        config.nats.tls
    );
    assert_eq!(config.openbao_address.as_str(), "https://openbao.internal:8200/");
    assert_eq!(config.openbao_address.transport(), OpenBaoTransport::TlsRequired);
    assert_eq!(config.openbao_mount.as_str(), "secret");
    assert_eq!(config.openbao_role_id, "fake-role-id");
    assert_eq!(config.openbao_secret_id_file, Path::new("/run/secrets/fake-secret-id"));
    assert_eq!(
        config.child_tokens,
        OpenBaoChildTokens::new(
            OpenBaoTokenRole::new("secrets-release").unwrap(),
            OpenBaoPolicyName::new("secrets-release-read").unwrap()
        )
    );
    assert_eq!(config.tenant_owners_file, Path::new("/etc/trogon/tenant-owners.toml"));
    assert_eq!(
        config.release_policies_file,
        Path::new("/etc/trogon/release-policies.toml")
    );
    assert_eq!(
        config.xkey_seed,
        SecretReference::File("/run/secrets/fake-xkey-seed".into())
    );
    assert_eq!(config.credential_stream.as_str(), "CREDENTIAL_EVENTS");
    assert_eq!(config.admission_limit, AdmissionLimit::default());
    assert_eq!(config.audit.writer().as_str(), "secrets-service:secrets-0");
    assert_eq!(config.audit.component().as_str(), "secrets_service");
    assert_eq!(config.audit.tenant().as_str(), "acme");
    assert_eq!(config.audit.project().as_str(), "platform");
}

#[test]
fn the_configuration_file_alone_configures_the_service() {
    let config = ServiceConfig::resolve(&with_config_file(), &InMemoryEnv::new(), &file(COMPLETE_FILE)).unwrap();

    assert_eq!(config.nats.servers, ["nats.internal:4222"]);
    assert_eq!(config.openbao_address.as_str(), "https://openbao.internal:8200/");
    assert_eq!(config.openbao_secret_id_file, Path::new("/run/secrets/fake-secret-id"));
    assert_eq!(
        config.xkey_seed,
        SecretReference::File("/run/secrets/fake-xkey-seed".into())
    );
    assert_eq!(config.audit.writer().as_str(), "secrets-service:secrets-0");
}

#[test]
fn the_environment_overrides_the_file_and_a_flag_overrides_the_environment() {
    let env = InMemoryEnv::new();
    env.set(ConfigSetting::OpenBaoMount.env(), "kv");
    env.set(ConfigSetting::NatsUrl.env(), "nats-env.internal:4222");
    env.set(ConfigSetting::XkeySeed.env(), "env://FAKE_XKEY_SEED");
    env.set(ConfigSetting::CredentialStream.env(), "CREDENTIALS_ENV");
    let contents = format!("{COMPLETE_FILE}\n");
    let contents = contents.replace(
        "[release]\n",
        "[release]\ncredential_stream = \"CREDENTIALS_FILE\"\nadmission_limit = 8\n",
    );

    let from_env = ServiceConfig::resolve(&with_config_file(), &env, &file(&contents)).unwrap();
    let from_flag = ServiceConfig::resolve(
        &args(&["--config", CONFIG_PATH, "--nats-url", "nats-flag.internal:4222"]),
        &env,
        &file(&contents),
    )
    .unwrap();

    assert_eq!(from_env.openbao_mount.as_str(), "kv");
    assert_eq!(from_env.nats.servers, ["nats-env.internal:4222"]);
    assert_eq!(
        from_env.xkey_seed,
        SecretReference::Env(trogon_service_config::EnvVarName::new("FAKE_XKEY_SEED").unwrap())
    );
    assert_eq!(from_env.credential_stream.as_str(), "CREDENTIALS_ENV");
    assert_eq!(from_env.admission_limit, AdmissionLimit::new(8).unwrap());
    assert_eq!(from_flag.nats.servers, ["nats-flag.internal:4222"]);
}

#[test]
fn a_nats_url_given_only_as_a_flag_is_enough() {
    let env = complete_env();
    env.remove(ConfigSetting::NatsUrl.env());

    let config =
        ServiceConfig::resolve(&args(&["--nats-url", "nats-flag.internal:4222"]), &env, &MemFs::new()).unwrap();

    assert_eq!(config.nats.servers, ["nats-flag.internal:4222"]);
}

#[test]
fn an_admission_limit_from_the_environment_replaces_the_default() {
    let env = complete_env();
    env.set(ConfigSetting::AdmissionLimit.env(), "16");

    assert_eq!(resolved(&env).admission_limit, AdmissionLimit::new(16).unwrap());
}

#[test]
fn a_nats_secret_in_the_file_is_a_reference_the_connection_reads() {
    let env = complete_env();
    let contents = "[nats]\nuser = \"secrets-service\"\npassword = \"env://FAKE_NATS_PASSWORD\"\n";

    let config = ServiceConfig::resolve(&with_config_file(), &env, &file(contents)).unwrap();

    assert!(
        matches!(&config.nats.auth, NatsAuth::UserPassword { user, .. } if user == "secrets-service"),
        "{:?}",
        config.nats.auth
    );
}

#[test]
fn every_nats_setting_is_read_from_the_file_and_the_environment() {
    let contents = "[nats]\ncreds = \"/etc/trogon/nats.creds\"\ntls_ca = \"/etc/trogon/nats-ca.pem\"\n\
        tls_cert = \"/etc/trogon/nats-client.pem\"\ntls_key = \"/etc/trogon/nats-client.key\"\n\
        nkey = \"file:///run/secrets/fake-nkey\"\ntoken = \"env://FAKE_NATS_TOKEN\"\n\
        jwt = \"env://FAKE_NATS_JWT\"\nseed = \"env://FAKE_NATS_SEED\"\n";

    let config = ServiceConfig::resolve(&with_config_file(), &complete_env(), &file(contents)).unwrap();

    assert!(
        matches!(config.nats.auth, NatsAuth::Credentials(_)),
        "{:?}",
        config.nats.auth
    );
    assert!(
        matches!(
            config.nats.tls,
            Ok(NatsTls::Enabled {
                ca: Some(_),
                client_cert: Some(_)
            })
        ),
        "{:?}",
        config.nats.tls
    );

    let env = complete_env();
    env.set(ConfigSetting::NatsToken.env(), "fake-nats-token");
    let config = resolved(&env);
    assert!(matches!(config.nats.auth, NatsAuth::Token(_)), "{:?}", config.nats.auth);
}

#[test]
fn nats_tls_is_disabled_only_by_its_switch() {
    let env = complete_env();
    env.set(ConfigSetting::NatsTlsDisabled.env(), "true");

    assert!(matches!(resolved(&env).nats.tls, Ok(NatsTls::Disabled)));
}

#[test]
fn a_plaintext_openbao_address_is_allowed_by_the_tls_switch() {
    let env = complete_env();
    env.set(ConfigSetting::OpenBaoAddress.env(), "http://127.0.0.1:8200");
    env.set(ConfigSetting::OpenBaoTlsDisabled.env(), "true");

    assert_eq!(
        resolved(&env).openbao_address.transport(),
        OpenBaoTransport::PlaintextAllowed
    );
}
