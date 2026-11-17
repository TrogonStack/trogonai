use std::fmt;

use trogon_nats::{NatsTls, redact_server_userinfo};
use trogon_secret_store::OpenBaoTransport;
use trogon_service_config::SecretReference;
use trogon_std::ReadFile;
use trogon_std::env::ReadEnv;

use super::service_config::env_value;
use super::service_config_wire::ServiceConfigWire;
use super::{
    ConfigSetting, ResolvedSetting, ServiceArgs, ServiceConfig, ServiceConfigError, SettingSource, SettingValue,
};

/// Every setting of the service configuration with its effective value and
/// the source it comes from, for `config dump --resolved` (ADR#0007).
///
/// The dump resolves the configuration the way startup does, and refuses it
/// with the error startup refuses it with, but it reads only the configuration
/// file and the environment variables of the settings: no secret, no file a
/// setting names, and no connection to NATS or OpenBao. A secret is shown by
/// the reference it is read from, a secret given inline is redacted, key
/// material given where a NATS credentials or TLS file path belongs is
/// redacted, and the credentials a NATS server address carries are redacted.
#[derive(Clone, Debug)]
pub struct ConfigDump {
    settings: Vec<ResolvedSetting>,
}

impl ConfigDump {
    /// Merges the built-in defaults, the configuration file named by
    /// `--config`, the environment, and the flags, each over the one before,
    /// validates the result, and records where each setting comes from.
    ///
    /// # Errors
    ///
    /// Returns the [`ServiceConfigError`] that [`ServiceConfig::resolve`]
    /// refuses the same configuration with.
    pub fn resolve(args: &ServiceArgs, env: &impl ReadEnv, fs: &impl ReadFile) -> Result<Self, ServiceConfigError> {
        let wire = ServiceConfigWire::read(args.runtime.config.as_deref(), fs)?;
        let config = ServiceConfig::from_wire(wire.clone(), args, env)?;
        let settings = ConfigSetting::ALL
            .into_iter()
            .map(|setting| {
                let found = match flag(setting, args) {
                    Some(value) => Some((value, SettingSource::Flag)),
                    None => match env_value(env, setting)? {
                        Some(value) => Some((value, SettingSource::Environment)),
                        None => file(setting, &wire).map(|value| (value, SettingSource::File)),
                    },
                };
                let source = found.as_ref().map_or(SettingSource::Default, |(_, source)| *source);
                Ok(ResolvedSetting {
                    setting,
                    value: shown(setting, found.map(|(value, _)| value), &config),
                    source,
                })
            })
            .collect::<Result<_, ServiceConfigError>>()?;
        Ok(Self { settings })
    }

    /// Every setting, in the order [`ConfigSetting::ALL`] lists them.
    pub fn settings(&self) -> &[ResolvedSetting] {
        &self.settings
    }
}

impl fmt::Display for ConfigDump {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for setting in &self.settings {
            writeln!(f, "{setting}")?;
        }
        Ok(())
    }
}

/// The value of `setting` as the dump shows it: from `raw`, the value its
/// source sets, or from `config` where the service derives the value it runs
/// with.
fn shown(setting: ConfigSetting, raw: Option<String>, config: &ServiceConfig) -> SettingValue {
    match setting {
        ConfigSetting::NatsUrl => SettingValue::Shown(
            config
                .nats
                .servers
                .iter()
                .map(|server| redact_server_userinfo(server))
                .collect::<Vec<_>>()
                .join(","),
        ),
        ConfigSetting::NatsTlsDisabled => {
            SettingValue::Shown(matches!(config.nats.tls, Ok(NatsTls::Disabled)).to_string())
        }
        ConfigSetting::OpenBaoTlsDisabled => SettingValue::Shown(
            matches!(config.openbao_address.transport(), OpenBaoTransport::PlaintextAllowed).to_string(),
        ),
        ConfigSetting::XkeySeed => SettingValue::Reference(config.xkey_seed.clone()),
        ConfigSetting::CredentialStream => SettingValue::Shown(config.credential_stream.to_string()),
        ConfigSetting::AdmissionLimit => SettingValue::Shown(config.admission_limit.get().to_string()),
        ConfigSetting::InvalidationStreamReplicas => {
            SettingValue::Shown(config.invalidation_stream_replicas.get().to_string())
        }
        // The environment may give a NATS secret inline, the way the NATS
        // tools read it, and such a secret is never shown.
        ConfigSetting::NatsNkey
        | ConfigSetting::NatsPassword
        | ConfigSetting::NatsToken
        | ConfigSetting::NatsJwt
        | ConfigSetting::NatsSeed => raw.map_or(SettingValue::Unset, |raw| {
            raw.parse::<SecretReference>()
                .map_or(SettingValue::Redacted, SettingValue::Reference)
        }),
        // These name a file, but key material pasted in place of the path
        // would otherwise be printed as it is.
        ConfigSetting::NatsCreds
        | ConfigSetting::NatsTlsCert
        | ConfigSetting::NatsTlsKey
        | ConfigSetting::NatsTlsCa => raw.map_or(SettingValue::Unset, path),
        _ => raw.map_or(SettingValue::Unset, SettingValue::Shown),
    }
}

/// A setting that names a file, shown as it is unless it holds inline
/// material, a line break or a PEM block, which is redacted.
fn path(value: String) -> SettingValue {
    if value.contains(['\n', '\r']) || value.contains("-----BEGIN") {
        SettingValue::Redacted
    } else {
        SettingValue::Shown(value)
    }
}

/// The value the command line flag of `setting` sets, when the setting has a
/// flag and it is set and not empty.
fn flag(setting: ConfigSetting, args: &ServiceArgs) -> Option<String> {
    let flags = &args.runtime.nats;
    let plain = |value: &Option<String>| value.clone().filter(|value| !value.is_empty());
    let reference = |value: &Option<SecretReference>| value.as_ref().map(ToString::to_string);
    match setting {
        ConfigSetting::NatsUrl => plain(&flags.nats_url),
        ConfigSetting::NatsCreds => plain(&flags.nats_creds),
        ConfigSetting::NatsNkey => reference(&flags.nats_nkey),
        ConfigSetting::NatsUser => plain(&flags.nats_user),
        ConfigSetting::NatsPassword => reference(&flags.nats_password),
        ConfigSetting::NatsToken => reference(&flags.nats_token),
        ConfigSetting::NatsJwt => reference(&flags.nats_jwt),
        ConfigSetting::NatsSeed => reference(&flags.nats_seed),
        ConfigSetting::NatsTlsCert => plain(&flags.nats_tls_cert),
        ConfigSetting::NatsTlsKey => plain(&flags.nats_tls_key),
        ConfigSetting::NatsTlsCa => plain(&flags.nats_tls_ca),
        ConfigSetting::InvalidationStreamReplicas => {
            args.invalidation_stream_replicas.map(|replicas| replicas.to_string())
        }
        _ => None,
    }
}

/// The value the configuration file sets for `setting`, when it sets one that
/// is not empty. The file holds no TLS switch.
fn file(setting: ConfigSetting, wire: &ServiceConfigWire) -> Option<String> {
    let text = |value: &Option<toml::Value>| value.as_ref().and_then(toml::Value::as_str).map(str::to_owned);
    let (nats, openbao, release, audit) = (&wire.nats, &wire.openbao, &wire.release, &wire.audit);
    match setting {
        ConfigSetting::NatsUrl => nats.url.clone(),
        ConfigSetting::NatsCreds => nats.creds.clone(),
        ConfigSetting::NatsNkey => text(&nats.nkey),
        ConfigSetting::NatsUser => nats.user.clone(),
        ConfigSetting::NatsPassword => text(&nats.password),
        ConfigSetting::NatsToken => text(&nats.token),
        ConfigSetting::NatsJwt => text(&nats.jwt),
        ConfigSetting::NatsSeed => text(&nats.seed),
        ConfigSetting::NatsTlsCert => nats.tls_cert.clone(),
        ConfigSetting::NatsTlsKey => nats.tls_key.clone(),
        ConfigSetting::NatsTlsCa => nats.tls_ca.clone(),
        ConfigSetting::NatsTlsDisabled | ConfigSetting::OpenBaoTlsDisabled => None,
        ConfigSetting::OpenBaoAddress => openbao.address.clone(),
        ConfigSetting::OpenBaoMount => openbao.mount.clone(),
        ConfigSetting::OpenBaoRoleId => openbao.role_id.clone(),
        ConfigSetting::OpenBaoSecretIdFile => openbao.secret_id_file.clone(),
        ConfigSetting::OpenBaoChildTokenRole => openbao.child_token_role.clone(),
        ConfigSetting::OpenBaoChildTokenPolicy => openbao.child_token_policy.clone(),
        ConfigSetting::TenantOwnersFile => release.tenant_owners_file.clone(),
        ConfigSetting::ReleasePoliciesFile => release.policies_file.clone(),
        ConfigSetting::XkeySeed => text(&release.xkey_seed),
        ConfigSetting::CredentialStream => release.credential_stream.clone(),
        ConfigSetting::AdmissionLimit => release.admission_limit.map(|limit| limit.to_string()),
        ConfigSetting::InvalidationStreamReplicas => {
            release.invalidation_stream_replicas.map(|count| count.to_string())
        }
        ConfigSetting::AuditTenant => audit.tenant.clone(),
        ConfigSetting::AuditProject => audit.project.clone(),
        ConfigSetting::AuditInstance => audit.instance.clone(),
    }
    .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests;
