use trogon_std::env::InMemoryEnv;

use crate::ConfigSetting;

/// Where the tests that pass `--config` keep the configuration file.
pub(crate) const CONFIG_PATH: &str = "/etc/trogon/secrets-service.toml";

/// Every setting the service cannot start without, with a value it accepts.
pub(crate) const REQUIRED: [(ConfigSetting, &str); 13] = [
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

/// An environment that sets every setting in [`REQUIRED`].
pub(crate) fn complete_env() -> InMemoryEnv {
    let env = InMemoryEnv::new();
    for (setting, value) in REQUIRED {
        env.set(setting.env(), value);
    }
    env
}
