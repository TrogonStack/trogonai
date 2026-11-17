use std::cell::RefCell;
use std::env::VarError;
use std::io;
use std::path::{Path, PathBuf};

use clap::Parser;
use trogon_std::ReadFile;
use trogon_std::env::{InMemoryEnv, ReadEnv};
use trogon_std::fs::MemFs;

use super::*;
use crate::fixtures::service_settings::{CONFIG_PATH, REQUIRED, complete_env};
use crate::{ServiceArgs, ServiceConfig, SettingSource, SettingValue};

const SECRET_ID_FILE: &str = "/run/secrets/fake-secret-id";
const SEED_FILE: &str = "/run/secrets/fake-xkey-seed";
const SEED_VARIABLE: &str = "FAKE_XKEY_SEED";
const PASSWORD_VARIABLE: &str = "FAKE_NATS_PASSWORD";
/// What every secret in these tests holds. No dump may show it.
const SECRET: &str = "fake-secret-content";

/// The dump of [`complete_env`], with no configuration file and no flag.
const COMPLETE_DUMP: &str = "\
nats.url (NATS_URL) = nats.internal:4222 (from environment)
nats.creds (NATS_CREDS) = <unset> (from default)
nats.nkey (NATS_NKEY) = <unset> (from default)
nats.user (NATS_USER) = <unset> (from default)
nats.password (NATS_PASSWORD) = <unset> (from default)
nats.token (NATS_TOKEN) = <unset> (from default)
nats.jwt (NATS_JWT) = <unset> (from default)
nats.seed (NATS_SEED) = <unset> (from default)
nats.tls_cert (NATS_CERT) = <unset> (from default)
nats.tls_key (NATS_KEY) = <unset> (from default)
nats.tls_ca (NATS_CA) = <unset> (from default)
NATS_TLS_DISABLED = false (from default)
openbao.address (OPENBAO_ADDR) = https://openbao.internal:8200 (from environment)
OPENBAO_TLS_DISABLED = false (from default)
openbao.mount (OPENBAO_MOUNT) = secret (from environment)
openbao.role_id (OPENBAO_ROLE_ID) = fake-role-id (from environment)
openbao.secret_id_file (OPENBAO_SECRET_ID_FILE) = /run/secrets/fake-secret-id (from environment)
openbao.child_token_role (OPENBAO_CHILD_TOKEN_ROLE) = secrets-release (from environment)
openbao.child_token_policy (OPENBAO_CHILD_TOKEN_POLICY) = secrets-release-read (from environment)
release.tenant_owners_file (TROGON_SECRETS_TENANT_OWNERS_FILE) = /etc/trogon/tenant-owners.toml (from environment)
release.policies_file (TROGON_SECRETS_RELEASE_POLICIES_FILE) = /etc/trogon/release-policies.toml (from environment)
release.xkey_seed (TROGON_SECRETS_XKEY_SEED) = file:///run/secrets/fake-xkey-seed (from environment)
release.credential_stream (TROGON_SECRETS_CREDENTIAL_STREAM) = CREDENTIAL_EVENTS (from default)
release.admission_limit (TROGON_SECRETS_ADMISSION_LIMIT) = 64 (from default)
release.invalidation_stream_replicas (TROGON_SECRETS_INVALIDATION_STREAM_REPLICAS) = 3 (from default)
audit.tenant (TROGON_SECRETS_AUDIT_TENANT) = acme (from environment)
audit.project (TROGON_SECRETS_AUDIT_PROJECT) = platform (from environment)
audit.instance (TROGON_SECRETS_AUDIT_INSTANCE) = secrets-0 (from environment)
";

/// A configuration file that sets every setting a file may set.
const FULL_FILE: &str = r#"
[nats]
url = "nats://nats.internal:4222"
creds = "/etc/trogon/fake.creds"
nkey = "env://FAKE_NATS_NKEY"
user = "secrets-service"
password = "env://FAKE_NATS_PASSWORD"
token = "env://FAKE_NATS_TOKEN"
jwt = "file:///run/secrets/fake-nats-jwt"
seed = "file:///run/secrets/fake-nats-seed"
tls_cert = "/etc/trogon/nats-client.pem"
tls_key = "/run/secrets/fake-nats-client-key.pem"
tls_ca = "/etc/trogon/nats-ca.pem"

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
credential_stream = "FAKE_CREDENTIALS"
admission_limit = 16
invalidation_stream_replicas = 5

[audit]
tenant = "acme"
project = "platform"
instance = "secrets-0"
"#;

/// The dump of [`FULL_FILE`] with nothing set in the environment.
const FULL_FILE_DUMP: &str = "\
nats.url (NATS_URL) = nats://nats.internal:4222 (from file)
nats.creds (NATS_CREDS) = /etc/trogon/fake.creds (from file)
nats.nkey (NATS_NKEY) = env://FAKE_NATS_NKEY (from file)
nats.user (NATS_USER) = secrets-service (from file)
nats.password (NATS_PASSWORD) = env://FAKE_NATS_PASSWORD (from file)
nats.token (NATS_TOKEN) = env://FAKE_NATS_TOKEN (from file)
nats.jwt (NATS_JWT) = file:///run/secrets/fake-nats-jwt (from file)
nats.seed (NATS_SEED) = file:///run/secrets/fake-nats-seed (from file)
nats.tls_cert (NATS_CERT) = /etc/trogon/nats-client.pem (from file)
nats.tls_key (NATS_KEY) = /run/secrets/fake-nats-client-key.pem (from file)
nats.tls_ca (NATS_CA) = /etc/trogon/nats-ca.pem (from file)
NATS_TLS_DISABLED = false (from default)
openbao.address (OPENBAO_ADDR) = https://openbao.internal:8200 (from file)
OPENBAO_TLS_DISABLED = false (from default)
openbao.mount (OPENBAO_MOUNT) = secret (from file)
openbao.role_id (OPENBAO_ROLE_ID) = fake-role-id (from file)
openbao.secret_id_file (OPENBAO_SECRET_ID_FILE) = /run/secrets/fake-secret-id (from file)
openbao.child_token_role (OPENBAO_CHILD_TOKEN_ROLE) = secrets-release (from file)
openbao.child_token_policy (OPENBAO_CHILD_TOKEN_POLICY) = secrets-release-read (from file)
release.tenant_owners_file (TROGON_SECRETS_TENANT_OWNERS_FILE) = /etc/trogon/tenant-owners.toml (from file)
release.policies_file (TROGON_SECRETS_RELEASE_POLICIES_FILE) = /etc/trogon/release-policies.toml (from file)
release.xkey_seed (TROGON_SECRETS_XKEY_SEED) = file:///run/secrets/fake-xkey-seed (from file)
release.credential_stream (TROGON_SECRETS_CREDENTIAL_STREAM) = FAKE_CREDENTIALS (from file)
release.admission_limit (TROGON_SECRETS_ADMISSION_LIMIT) = 16 (from file)
release.invalidation_stream_replicas (TROGON_SECRETS_INVALIDATION_STREAM_REPLICAS) = 5 (from file)
audit.tenant (TROGON_SECRETS_AUDIT_TENANT) = acme (from file)
audit.project (TROGON_SECRETS_AUDIT_PROJECT) = platform (from file)
audit.instance (TROGON_SECRETS_AUDIT_INSTANCE) = secrets-0 (from file)
";

/// A value for the environment variable of every setting the file may set,
/// each different from [`FULL_FILE`], and how the dump shows it.
fn environment_overrides() -> [(ConfigSetting, &'static str, SettingValue); 26] {
    [
        (
            ConfigSetting::NatsUrl,
            "nats://nats-env.internal:4222",
            shown("nats://nats-env.internal:4222"),
        ),
        (
            ConfigSetting::NatsCreds,
            "/etc/trogon/env.creds",
            shown("/etc/trogon/env.creds"),
        ),
        (
            ConfigSetting::NatsNkey,
            "env://FAKE_ENV_NKEY",
            reference("env://FAKE_ENV_NKEY"),
        ),
        (ConfigSetting::NatsUser, "env-user", shown("env-user")),
        (
            ConfigSetting::NatsPassword,
            "fake-inline-password",
            SettingValue::Redacted,
        ),
        (
            ConfigSetting::NatsToken,
            "env://FAKE_ENV_TOKEN",
            reference("env://FAKE_ENV_TOKEN"),
        ),
        (
            ConfigSetting::NatsJwt,
            "file:///run/secrets/fake-env-jwt",
            reference("file:///run/secrets/fake-env-jwt"),
        ),
        (ConfigSetting::NatsSeed, "fake-inline-seed", SettingValue::Redacted),
        (
            ConfigSetting::NatsTlsCert,
            "/etc/trogon/env-client.pem",
            shown("/etc/trogon/env-client.pem"),
        ),
        (
            ConfigSetting::NatsTlsKey,
            "/run/secrets/fake-env-client-key.pem",
            shown("/run/secrets/fake-env-client-key.pem"),
        ),
        (
            ConfigSetting::NatsTlsCa,
            "/etc/trogon/env-ca.pem",
            shown("/etc/trogon/env-ca.pem"),
        ),
        (
            ConfigSetting::OpenBaoAddress,
            "https://openbao-env.internal:8200",
            shown("https://openbao-env.internal:8200"),
        ),
        (ConfigSetting::OpenBaoMount, "kv", shown("kv")),
        (
            ConfigSetting::OpenBaoRoleId,
            "fake-env-role-id",
            shown("fake-env-role-id"),
        ),
        (
            ConfigSetting::OpenBaoSecretIdFile,
            "/run/secrets/fake-env-secret-id",
            shown("/run/secrets/fake-env-secret-id"),
        ),
        (
            ConfigSetting::OpenBaoChildTokenRole,
            "env-release",
            shown("env-release"),
        ),
        (
            ConfigSetting::OpenBaoChildTokenPolicy,
            "env-release-read",
            shown("env-release-read"),
        ),
        (
            ConfigSetting::TenantOwnersFile,
            "/etc/trogon/env-owners.toml",
            shown("/etc/trogon/env-owners.toml"),
        ),
        (
            ConfigSetting::ReleasePoliciesFile,
            "/etc/trogon/env-policies.toml",
            shown("/etc/trogon/env-policies.toml"),
        ),
        (
            ConfigSetting::XkeySeed,
            "env://FAKE_ENV_XKEY_SEED",
            reference("env://FAKE_ENV_XKEY_SEED"),
        ),
        (
            ConfigSetting::CredentialStream,
            "FAKE_ENV_CREDENTIALS",
            shown("FAKE_ENV_CREDENTIALS"),
        ),
        (ConfigSetting::AdmissionLimit, "8", shown("8")),
        (ConfigSetting::InvalidationStreamReplicas, "2", shown("2")),
        (ConfigSetting::AuditTenant, "globex", shown("globex")),
        (ConfigSetting::AuditProject, "runtime", shown("runtime")),
        (ConfigSetting::AuditInstance, "secrets-1", shown("secrets-1")),
    ]
}

/// A flag for every setting that has one, and how the dump shows it.
fn flag_overrides() -> [(ConfigSetting, &'static str, &'static str, SettingValue); 12] {
    [
        (
            ConfigSetting::NatsUrl,
            "--nats-url",
            "nats://nats-flag.internal:4222",
            shown("nats://nats-flag.internal:4222"),
        ),
        (
            ConfigSetting::NatsCreds,
            "--nats-creds",
            "/etc/trogon/flag.creds",
            shown("/etc/trogon/flag.creds"),
        ),
        (
            ConfigSetting::NatsNkey,
            "--nats-nkey",
            "env://FAKE_FLAG_NKEY",
            reference("env://FAKE_FLAG_NKEY"),
        ),
        (ConfigSetting::NatsUser, "--nats-user", "flag-user", shown("flag-user")),
        (
            ConfigSetting::NatsPassword,
            "--nats-password",
            "env://FAKE_FLAG_PASSWORD",
            reference("env://FAKE_FLAG_PASSWORD"),
        ),
        (
            ConfigSetting::NatsToken,
            "--nats-token",
            "file:///run/secrets/fake-flag-token",
            reference("file:///run/secrets/fake-flag-token"),
        ),
        (
            ConfigSetting::NatsJwt,
            "--nats-jwt",
            "env://FAKE_FLAG_JWT",
            reference("env://FAKE_FLAG_JWT"),
        ),
        (
            ConfigSetting::NatsSeed,
            "--nats-seed",
            "env://FAKE_FLAG_SEED",
            reference("env://FAKE_FLAG_SEED"),
        ),
        (
            ConfigSetting::NatsTlsCert,
            "--nats-tls-cert",
            "/etc/trogon/flag-client.pem",
            shown("/etc/trogon/flag-client.pem"),
        ),
        (
            ConfigSetting::NatsTlsKey,
            "--nats-tls-key",
            "/run/secrets/fake-flag-client-key.pem",
            shown("/run/secrets/fake-flag-client-key.pem"),
        ),
        (
            ConfigSetting::NatsTlsCa,
            "--nats-tls-ca",
            "/etc/trogon/flag-ca.pem",
            shown("/etc/trogon/flag-ca.pem"),
        ),
        (
            ConfigSetting::InvalidationStreamReplicas,
            "--invalidation-stream-replicas",
            "1",
            shown("1"),
        ),
    ]
}

/// An environment that records every variable it is asked for.
struct RecordingEnv {
    env: InMemoryEnv,
    read: RefCell<Vec<String>>,
}

impl ReadEnv for RecordingEnv {
    fn var(&self, key: &str) -> Result<String, VarError> {
        self.read.borrow_mut().push(key.to_owned());
        self.env.var(key)
    }
}

/// A file system that records every file it is asked for.
struct RecordingFs {
    fs: MemFs,
    read: RefCell<Vec<PathBuf>>,
}

impl ReadFile for RecordingFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.read.borrow_mut().push(path.to_owned());
        self.fs.read_to_string(path)
    }
}

fn args(flags: &[&str]) -> ServiceArgs {
    ServiceArgs::parse_from(["trogon-secrets-service"].iter().chain(flags))
}

fn shown(value: &str) -> SettingValue {
    SettingValue::Shown(value.to_owned())
}

fn reference(value: &str) -> SettingValue {
    SettingValue::Reference(value.parse().unwrap())
}

/// A file system that holds the secrets the configuration points at, and the
/// configuration file when `contents` is given.
fn files(contents: Option<&str>) -> MemFs {
    let fs = MemFs::new();
    fs.insert(SECRET_ID_FILE, SECRET);
    fs.insert(SEED_FILE, SECRET);
    if let Some(contents) = contents {
        fs.insert(CONFIG_PATH, contents);
    }
    fs
}

fn dumped(args: &ServiceArgs, env: &impl ReadEnv, fs: &impl ReadFile) -> ConfigDump {
    ConfigDump::resolve(args, env, fs).unwrap()
}

fn resolved_setting(dump: &ConfigDump, setting: ConfigSetting) -> &ResolvedSetting {
    dump.settings()
        .iter()
        .find(|resolved| resolved.setting == setting)
        .unwrap()
}

/// Refuses the configuration through the dump and through startup, and
/// returns the dump's refusal once both are proven alike.
fn refused_alike(args: &ServiceArgs, env: &InMemoryEnv, fs: &MemFs) -> ServiceConfigError {
    let dump = ConfigDump::resolve(args, env, fs).unwrap_err();
    let startup = ServiceConfig::resolve(args, env, fs).unwrap_err();
    assert_eq!(format!("{dump:?}"), format!("{startup:?}"));
    dump
}

#[test]
fn the_sealing_key_seed_and_the_approle_secret_id_are_never_printed() {
    for seed in [format!("env://{SEED_VARIABLE}"), format!("file://{SEED_FILE}")] {
        let env = complete_env();
        env.set(ConfigSetting::XkeySeed.env(), &seed);
        env.set(SEED_VARIABLE, SECRET);

        let printed = format!("{}", dumped(&args(&[]), &env, &files(None)));

        assert_eq!(
            printed,
            COMPLETE_DUMP.replace(
                "= file:///run/secrets/fake-xkey-seed (from environment)",
                &format!("= {seed} (from environment)"),
            ),
        );
        assert!(!printed.contains(SECRET), "{printed}");
    }
}

#[test]
fn a_nats_secret_given_inline_is_never_printed() {
    for setting in [
        ConfigSetting::NatsNkey,
        ConfigSetting::NatsPassword,
        ConfigSetting::NatsToken,
        ConfigSetting::NatsJwt,
        ConfigSetting::NatsSeed,
    ] {
        let env = complete_env();
        env.set(ConfigSetting::NatsUser.env(), "secrets-service");
        env.set(setting.env(), SECRET);

        let dump = dumped(&args(&[]), &env, &files(None));

        assert_eq!(
            resolved_setting(&dump, setting),
            &ResolvedSetting {
                setting,
                value: SettingValue::Redacted,
                source: SettingSource::Environment,
            },
        );
        let printed = format!("{dump}");
        assert!(!printed.contains(SECRET), "{setting}: {printed}");
    }
}

#[test]
fn the_credentials_a_nats_url_carries_are_never_printed() {
    let env = complete_env();
    env.set(
        ConfigSetting::NatsUrl.env(),
        "nats://fake-user:fake-password@nats-0.internal:4222, nats://fake-token@nats-1.internal:4222",
    );

    let dump = dumped(&args(&[]), &env, &files(None));

    assert_eq!(
        resolved_setting(&dump, ConfigSetting::NatsUrl),
        &ResolvedSetting {
            setting: ConfigSetting::NatsUrl,
            value: shown("nats://<redacted>@nats-0.internal:4222,nats://<redacted>@nats-1.internal:4222"),
            source: SettingSource::Environment,
        },
    );
    let printed = format!("{dump}");
    for credential in ["fake-user", "fake-password", "fake-token"] {
        assert!(!printed.contains(credential), "{credential}: {printed}");
    }
}

#[test]
fn the_dump_reads_the_configuration_file_and_nothing_it_points_at() {
    let env = RecordingEnv {
        env: complete_env(),
        read: RefCell::new(Vec::new()),
    };
    env.env
        .set(ConfigSetting::XkeySeed.env(), format!("env://{SEED_VARIABLE}"));
    env.env.set(SEED_VARIABLE, SECRET);
    env.env.set(ConfigSetting::NatsUser.env(), "secrets-service");
    env.env
        .set(ConfigSetting::NatsPassword.env(), format!("env://{PASSWORD_VARIABLE}"));
    env.env.set(PASSWORD_VARIABLE, SECRET);
    let fs = RecordingFs {
        fs: files(Some("[audit]\ninstance = \"secrets-0\"\n")),
        read: RefCell::new(Vec::new()),
    };

    dumped(&args(&["--config", CONFIG_PATH]), &env, &fs);

    assert_eq!(fs.read.into_inner(), [PathBuf::from(CONFIG_PATH)]);
    let read = env.read.into_inner();
    for variable in [SEED_VARIABLE, PASSWORD_VARIABLE] {
        assert!(!read.iter().any(|key| key == variable), "{variable}: {read:?}");
    }
}

#[test]
fn a_configuration_the_service_refuses_is_refused_by_the_dump_with_the_same_error() {
    for (setting, _) in REQUIRED {
        let env = complete_env();
        env.remove(setting.env());

        let refused = refused_alike(&args(&[]), &env, &files(None));

        assert!(
            matches!(refused, ServiceConfigError::Missing { setting: missing } if missing == setting),
            "{setting}: {refused:?}"
        );
    }

    let env = complete_env();
    env.set(ConfigSetting::XkeySeed.env(), SECRET);
    let refused = refused_alike(&args(&[]), &env, &files(None));
    assert!(
        matches!(
            refused,
            ServiceConfigError::SecretReference {
                setting: ConfigSetting::XkeySeed,
                ..
            }
        ),
        "{refused:?}"
    );

    for setting in [ConfigSetting::NatsTlsDisabled, ConfigSetting::OpenBaoTlsDisabled] {
        let env = complete_env();
        env.set(setting.env(), "fake-maybe");
        let refused = refused_alike(&args(&[]), &env, &files(None));
        assert!(
            matches!(
                refused,
                ServiceConfigError::NatsTlsDisabledValue { .. } | ServiceConfigError::OpenBaoAddress(_)
            ),
            "{setting}: {refused:?}"
        );
    }

    let env = complete_env();
    env.set(ConfigSetting::AdmissionLimit.env(), "0");
    let refused = refused_alike(&args(&[]), &env, &files(None));
    assert!(matches!(refused, ServiceConfigError::AdmissionLimit(_)), "{refused:?}");

    let refused = refused_alike(&args(&["--config", CONFIG_PATH]), &complete_env(), &files(None));
    assert!(
        matches!(refused, ServiceConfigError::ConfigUnreadable { .. }),
        "{refused:?}"
    );

    let refused = refused_alike(
        &args(&["--config", CONFIG_PATH]),
        &complete_env(),
        &files(Some("[nats]\ntls_disabled = \"true\"\n")),
    );
    assert!(
        matches!(refused, ServiceConfigError::ConfigUnparsable { .. }),
        "{refused:?}"
    );
}

#[test]
fn every_setting_is_dumped_once_in_the_order_it_is_declared() {
    let dump = dumped(&args(&[]), &complete_env(), &files(None));

    let settings: Vec<ConfigSetting> = dump.settings().iter().map(|resolved| resolved.setting).collect();

    assert_eq!(settings, ConfigSetting::ALL);
}

#[test]
fn a_setting_left_to_its_default_shows_the_default() {
    let printed = format!("{}", dumped(&args(&[]), &complete_env(), &files(None)));

    assert_eq!(printed, COMPLETE_DUMP);
}

#[test]
fn a_setting_from_the_file_shows_the_file_as_its_source() {
    let printed = format!(
        "{}",
        dumped(
            &args(&["--config", CONFIG_PATH]),
            &InMemoryEnv::new(),
            &files(Some(FULL_FILE))
        )
    );

    assert_eq!(printed, FULL_FILE_DUMP);
}

#[test]
fn the_environment_wins_over_the_file() {
    let env = InMemoryEnv::new();
    for (setting, value, _) in environment_overrides() {
        env.set(setting.env(), value);
    }

    let dump = dumped(&args(&["--config", CONFIG_PATH]), &env, &files(Some(FULL_FILE)));

    for (setting, _, value) in environment_overrides() {
        assert_eq!(
            resolved_setting(&dump, setting),
            &ResolvedSetting {
                setting,
                value,
                source: SettingSource::Environment,
            },
        );
    }
}

#[test]
fn a_flag_wins_over_the_environment() {
    let env = InMemoryEnv::new();
    for (setting, value, _) in environment_overrides() {
        env.set(setting.env(), value);
    }
    let mut flags = vec!["--config", CONFIG_PATH];
    for (_, flag, value, _) in flag_overrides() {
        flags.extend([flag, value]);
    }

    let dump = dumped(&args(&flags), &env, &files(Some(FULL_FILE)));

    for (setting, _, _, value) in flag_overrides() {
        assert_eq!(
            resolved_setting(&dump, setting),
            &ResolvedSetting {
                setting,
                value,
                source: SettingSource::Flag,
            },
        );
    }
}

#[test]
fn an_empty_value_falls_through_to_the_source_below() {
    let env = InMemoryEnv::new();
    env.set(ConfigSetting::NatsUrl.env(), "nats://nats-env.internal:4222");
    env.set(ConfigSetting::AuditInstance.env(), "");

    let dump = dumped(
        &args(&["--config", CONFIG_PATH, "--nats-url", ""]),
        &env,
        &files(Some(FULL_FILE)),
    );

    assert_eq!(
        resolved_setting(&dump, ConfigSetting::NatsUrl),
        &ResolvedSetting {
            setting: ConfigSetting::NatsUrl,
            value: shown("nats://nats-env.internal:4222"),
            source: SettingSource::Environment,
        },
    );
    assert_eq!(
        resolved_setting(&dump, ConfigSetting::AuditInstance),
        &ResolvedSetting {
            setting: ConfigSetting::AuditInstance,
            value: shown("secrets-0"),
            source: SettingSource::File,
        },
    );
}

#[test]
fn a_tls_switch_shows_whether_the_environment_disabled_tls() {
    let env = complete_env();
    env.set(ConfigSetting::NatsTlsDisabled.env(), "YES");
    env.set(ConfigSetting::OpenBaoAddress.env(), "http://openbao.internal:8200");
    env.set(ConfigSetting::OpenBaoTlsDisabled.env(), "1");

    let dump = dumped(&args(&[]), &env, &files(None));

    for setting in [ConfigSetting::NatsTlsDisabled, ConfigSetting::OpenBaoTlsDisabled] {
        assert_eq!(
            resolved_setting(&dump, setting),
            &ResolvedSetting {
                setting,
                value: shown("true"),
                source: SettingSource::Environment,
            },
        );
    }
}

#[test]
fn the_invalidation_stream_replicas_show_the_value_and_the_source_startup_uses() {
    let replicas = |flags: &[&str], env: &InMemoryEnv, contents: Option<&str>| {
        let args = args(flags);
        let dump = dumped(&args, env, &files(contents));
        let config = ServiceConfig::resolve(&args, env, &files(contents)).unwrap();
        let resolved = resolved_setting(&dump, ConfigSetting::InvalidationStreamReplicas).clone();
        assert_eq!(resolved.value, shown(&config.invalidation_stream_replicas.to_string()));
        resolved
    };
    let expected = |value: &str, source| ResolvedSetting {
        setting: ConfigSetting::InvalidationStreamReplicas,
        value: shown(value),
        source,
    };
    let file = Some(FULL_FILE);
    let from_file = ["--config", CONFIG_PATH];
    let with_flag = ["--config", CONFIG_PATH, "--invalidation-stream-replicas", "1"];
    let env = complete_env();

    assert_eq!(replicas(&[], &env, None), expected("3", SettingSource::Default));
    assert_eq!(replicas(&from_file, &env, file), expected("5", SettingSource::File));

    env.set(ConfigSetting::InvalidationStreamReplicas.env(), "2");
    assert_eq!(
        replicas(&from_file, &env, file),
        expected("2", SettingSource::Environment)
    );
    assert_eq!(replicas(&with_flag, &env, file), expected("1", SettingSource::Flag));

    env.set(ConfigSetting::InvalidationStreamReplicas.env(), "many");
    assert_eq!(replicas(&with_flag, &env, file), expected("1", SettingSource::Flag));
}

#[test]
fn invalidation_stream_replicas_startup_refuses_are_refused_by_the_dump_too() {
    let env = complete_env();
    env.set(ConfigSetting::InvalidationStreamReplicas.env(), "many");
    let refused = refused_alike(&args(&[]), &env, &files(None));
    assert!(
        matches!(
            refused,
            ServiceConfigError::NotANumber {
                setting: ConfigSetting::InvalidationStreamReplicas,
                ..
            }
        ),
        "{refused:?}"
    );

    let refused = refused_alike(
        &args(&["--invalidation-stream-replicas", "0"]),
        &complete_env(),
        &files(None),
    );
    assert!(
        matches!(
            refused,
            ServiceConfigError::Zero {
                setting: ConfigSetting::InvalidationStreamReplicas
            }
        ),
        "{refused:?}"
    );
}
