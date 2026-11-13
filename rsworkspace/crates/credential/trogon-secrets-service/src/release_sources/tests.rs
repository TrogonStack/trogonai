use std::env::VarError;
use std::error::Error;
use std::ffi::OsString;
use std::path::Path;

use nkeys::{KeyPair, XKey};
use trogon_secret_store::OpenBaoAppRoleError;
use trogon_std::env::{InMemoryEnv, ReadEnv};
use trogon_std::fs::MemFs;

use super::*;
use crate::{
    ConfigSetting, ReleasePoliciesFileError, ReleaseSourcesError, ServiceArgs, TenantOwnersFileError, XkeySealerError,
};

const TENANT_OWNERS_FILE: &str = "/etc/trogon/tenant-owners.toml";
const RELEASE_POLICIES_FILE: &str = "/etc/trogon/release-policies.toml";
const SECRET_ID_FILE: &str = "/run/secrets/fake-secret-id";
const SEED_FILE: &str = "/run/secrets/fake-xkey-seed";
const SEED_VARIABLE: &str = "FAKE_SERVICE_XKEY_SEED";
const SECRET_ID: &str = "fake-secret-id-value";

const RELEASE_POLICIES: &str = r#"
[[policies]]
owner_id = "project-1"
credential_id = "0190a5b4-7c1e-7d2a-9f3b-6c8d4e2a1b0f"
runtime_services = ["billing-runner"]
hosts = ["api.github.com"]
headers = ["authorization"]
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

/// The configuration the sources are read from, with the sealing key seed
/// named by `xkey_seed`.
fn config(xkey_seed: &str) -> ServiceConfig {
    let env = InMemoryEnv::new();
    for (setting, value) in [
        (ConfigSetting::NatsUrl, "nats.internal:4222"),
        (ConfigSetting::OpenBaoAddress, "https://openbao.internal:8200"),
        (ConfigSetting::OpenBaoMount, "secret"),
        (ConfigSetting::OpenBaoRoleId, "fake-role-id"),
        (ConfigSetting::OpenBaoSecretIdFile, SECRET_ID_FILE),
        (ConfigSetting::OpenBaoChildTokenRole, "secrets-release"),
        (ConfigSetting::OpenBaoChildTokenPolicy, "secrets-release-read"),
        (ConfigSetting::TenantOwnersFile, TENANT_OWNERS_FILE),
        (ConfigSetting::ReleasePoliciesFile, RELEASE_POLICIES_FILE),
        (ConfigSetting::XkeySeed, xkey_seed),
        (ConfigSetting::AuditTenant, "acme"),
        (ConfigSetting::AuditProject, "platform"),
        (ConfigSetting::AuditInstance, "secrets-0"),
    ] {
        env.set(setting.env(), value);
    }
    ServiceConfig::resolve(&ServiceArgs::default(), &env, &MemFs::new()).unwrap()
}

fn seed_in_file() -> ServiceConfig {
    config(&format!("file://{SEED_FILE}"))
}

fn seed_in_env() -> ServiceConfig {
    config(&format!("env://{SEED_VARIABLE}"))
}

/// Every file the sources are read from, each holding what the service
/// accepts, with `seed` in the sealing key seed file.
fn contents(seed: &str) -> [(&'static str, String); 4] {
    [
        (
            TENANT_OWNERS_FILE,
            format!(
                "[tenants]\n\"{}\" = [\"project-1\"]\n",
                KeyPair::new_account().public_key()
            ),
        ),
        (RELEASE_POLICIES_FILE, RELEASE_POLICIES.to_owned()),
        (SECRET_ID_FILE, format!("{SECRET_ID}\n")),
        (SEED_FILE, format!("{seed}\n")),
    ]
}

fn files(seed: &str) -> MemFs {
    let fs = MemFs::new();
    for (path, contents) in contents(seed) {
        fs.insert(path, contents);
    }
    fs
}

/// [`files`] with no file at `missing`.
fn without(seed: &str, missing: &str) -> MemFs {
    let fs = MemFs::new();
    for (path, contents) in contents(seed) {
        if path != missing {
            fs.insert(path, contents);
        }
    }
    fs
}

fn with(fs: MemFs, path: &str, contents: &str) -> MemFs {
    fs.insert(path, contents);
    fs
}

fn seed() -> String {
    XKey::new().seed().unwrap()
}

fn refused(config: &ServiceConfig, env: &impl ReadEnv, fs: &MemFs) -> ReleaseSourcesError {
    ReleaseSources::load(config, env, fs).unwrap_err()
}

/// The error and every error under it, as the service logs them.
fn chain(error: &dyn Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[test]
fn a_tenant_owners_file_that_cannot_be_read_refuses_to_start() {
    let refused = refused(
        &seed_in_file(),
        &InMemoryEnv::new(),
        &without(&seed(), TENANT_OWNERS_FILE),
    );

    assert!(
        matches!(
            refused,
            ReleaseSourcesError::TenantOwners(TenantOwnersFileError::Unreadable { .. })
        ),
        "{refused:?}"
    );
}

#[test]
fn a_tenant_owners_file_that_names_no_tenants_refuses_to_start() {
    let refused = refused(
        &seed_in_file(),
        &InMemoryEnv::new(),
        &with(files(&seed()), TENANT_OWNERS_FILE, "[tenants]\n"),
    );

    assert!(
        matches!(
            refused,
            ReleaseSourcesError::TenantOwners(TenantOwnersFileError::NoTenants { .. })
        ),
        "{refused:?}"
    );
}

#[test]
fn a_release_policies_file_that_cannot_be_read_refuses_to_start() {
    let refused = refused(
        &seed_in_file(),
        &InMemoryEnv::new(),
        &without(&seed(), RELEASE_POLICIES_FILE),
    );

    assert!(
        matches!(
            refused,
            ReleaseSourcesError::ReleasePolicies(ReleasePoliciesFileError::Unreadable { .. })
        ),
        "{refused:?}"
    );
}

#[test]
fn a_release_policies_file_that_names_no_policies_refuses_to_start() {
    let refused = refused(
        &seed_in_file(),
        &InMemoryEnv::new(),
        &with(files(&seed()), RELEASE_POLICIES_FILE, "policies = []\n"),
    );

    assert!(
        matches!(
            refused,
            ReleaseSourcesError::ReleasePolicies(ReleasePoliciesFileError::NoPolicies { .. })
        ),
        "{refused:?}"
    );
}

#[test]
fn a_sealing_key_seed_file_that_cannot_be_read_refuses_to_start() {
    let refused = refused(&seed_in_file(), &InMemoryEnv::new(), &without(&seed(), SEED_FILE));

    let ReleaseSourcesError::XkeySeedUnreadable { path, .. } = refused else {
        panic!("an unreadable seed file was refused as {refused:?}");
    };
    assert_eq!(path, Path::new(SEED_FILE));
}

#[test]
fn a_sealing_key_seed_variable_that_is_not_set_refuses_to_start() {
    let refused = refused(&seed_in_env(), &InMemoryEnv::new(), &files(&seed()));

    let ReleaseSourcesError::XkeySeedUnset { variable } = refused else {
        panic!("an unset seed variable was refused as {refused:?}");
    };
    assert_eq!(variable.as_str(), SEED_VARIABLE);
}

#[test]
fn a_sealing_key_seed_variable_that_is_not_unicode_refuses_to_start() {
    let env = NotUnicodeEnv {
        env: InMemoryEnv::new(),
        variable: SEED_VARIABLE,
    };

    let refused = refused(&seed_in_env(), &env, &files(&seed()));

    let ReleaseSourcesError::XkeySeedNotUnicode { variable } = refused else {
        panic!("a seed variable that is not unicode was refused as {refused:?}");
    };
    assert_eq!(variable.as_str(), SEED_VARIABLE);
}

#[test]
fn an_empty_sealing_key_seed_refuses_to_start() {
    let env = InMemoryEnv::new();
    env.set(SEED_VARIABLE, " \n");

    for (config, env) in [(seed_in_file(), InMemoryEnv::new()), (seed_in_env(), env)] {
        let refused = refused(&config, &env, &files(" "));

        assert!(
            matches!(refused, ReleaseSourcesError::XkeySeedEmpty { ref reference } if *reference == config.xkey_seed),
            "{refused:?}"
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
fn a_sealing_key_seed_that_is_not_a_curve_seed_refuses_to_start_without_quoting_it() {
    let account_seed = KeyPair::new_account().seed().unwrap();

    for wrong in ["fake-xkey-seed-not-a-curve-seed", account_seed.as_str()] {
        let refused = refused(&seed_in_file(), &InMemoryEnv::new(), &files(wrong));

        assert!(
            matches!(
                refused,
                ReleaseSourcesError::XkeySeed {
                    source: XkeySealerError::NotACurveSeed,
                    ..
                }
            ),
            "{refused:?}"
        );
        assert!(!chain(&refused).contains(wrong), "{}", chain(&refused));
        assert!(!format!("{refused:?}").contains(wrong), "{refused:?}");
    }
}

#[test]
fn an_empty_role_id_refuses_to_start() {
    let mut config = seed_in_file();
    config.openbao_role_id = String::new();

    let refused = refused(&config, &InMemoryEnv::new(), &files(&seed()));

    assert!(
        matches!(
            refused,
            ReleaseSourcesError::AppRole {
                setting: ConfigSetting::OpenBaoRoleId,
                source: OpenBaoAppRoleError::EmptyRoleId,
            }
        ),
        "{refused:?}"
    );
}

#[test]
fn a_secret_id_file_that_cannot_be_read_refuses_to_start() {
    let refused = refused(&seed_in_file(), &InMemoryEnv::new(), &without(&seed(), SECRET_ID_FILE));

    assert!(
        matches!(
            refused,
            ReleaseSourcesError::AppRole {
                setting: ConfigSetting::OpenBaoSecretIdFile,
                source: OpenBaoAppRoleError::SecretIdUnreadable { .. },
            }
        ),
        "{refused:?}"
    );
}

#[test]
fn an_empty_secret_id_file_refuses_to_start() {
    let refused = refused(
        &seed_in_file(),
        &InMemoryEnv::new(),
        &with(files(&seed()), SECRET_ID_FILE, "\n"),
    );

    assert!(
        matches!(
            refused,
            ReleaseSourcesError::AppRole {
                setting: ConfigSetting::OpenBaoSecretIdFile,
                source: OpenBaoAppRoleError::EmptySecretId { .. },
            }
        ),
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
fn every_refusal_names_its_setting() {
    let unset_seed = refused(&seed_in_env(), &InMemoryEnv::new(), &files(&seed()));
    let cases = [
        (
            refused(
                &seed_in_file(),
                &InMemoryEnv::new(),
                &without(&seed(), TENANT_OWNERS_FILE),
            ),
            ConfigSetting::TenantOwnersFile,
        ),
        (
            refused(
                &seed_in_file(),
                &InMemoryEnv::new(),
                &without(&seed(), RELEASE_POLICIES_FILE),
            ),
            ConfigSetting::ReleasePoliciesFile,
        ),
        (
            refused(&seed_in_file(), &InMemoryEnv::new(), &without(&seed(), SEED_FILE)),
            ConfigSetting::XkeySeed,
        ),
        (unset_seed, ConfigSetting::XkeySeed),
        (
            refused(&seed_in_file(), &InMemoryEnv::new(), &files("fake-xkey-seed")),
            ConfigSetting::XkeySeed,
        ),
        (
            refused(&seed_in_file(), &InMemoryEnv::new(), &without(&seed(), SECRET_ID_FILE)),
            ConfigSetting::OpenBaoSecretIdFile,
        ),
    ];

    for (refused, setting) in cases {
        assert!(refused.to_string().contains(&setting.to_string()), "{refused}");
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
fn no_refusal_quotes_the_secret_id() {
    let mut config = seed_in_file();
    config.openbao_role_id = String::new();

    let refused = refused(&config, &InMemoryEnv::new(), &files(&seed()));

    assert!(!chain(&refused).contains(SECRET_ID), "{}", chain(&refused));
}

#[test]
fn the_sources_load_from_the_files_the_configuration_names() {
    let seed = seed();
    let service = XKey::from_seed(&seed).unwrap();

    let sources = ReleaseSources::load(&seed_in_file(), &InMemoryEnv::new(), &files(&seed)).unwrap();

    assert_eq!(sources.sealer.public_key(), service.public_key());
    assert_eq!(sources.approle.role_id(), "fake-role-id");
}

#[test]
fn the_sealing_key_seed_is_read_from_the_variable_its_reference_names() {
    let seed = seed();
    let service = XKey::from_seed(&seed).unwrap();
    let env = InMemoryEnv::new();
    env.set(SEED_VARIABLE, format!("{seed}\n"));

    let sources = ReleaseSources::load(&seed_in_env(), &env, &without("unused", SEED_FILE)).unwrap();

    assert_eq!(sources.sealer.public_key(), service.public_key());
}
