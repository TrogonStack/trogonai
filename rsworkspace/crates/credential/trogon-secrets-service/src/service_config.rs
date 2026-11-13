use std::env::VarError;
use std::path::{Path, PathBuf};

use trogon_audit::{AuditName, WriterId, WriterIdentity};
use trogon_credential_aggregate::CREDENTIAL_STREAM;
use trogon_nats::{IncompleteClientCertificate, NatsConfig, NatsToken, TlsMisconfiguration};
use trogon_secret_store::{OpenBaoChildTokens, OpenBaoMount, OpenBaoPolicyName, OpenBaoTokenRole};
use trogon_service_config::{NatsConfigSection, SecretReference, resolve_nats};
use trogon_std::env::ReadEnv;
use trogon_std::{ReadFile, SecretString};

use super::constants::{AUDIT_COMPONENT, AUDIT_WRITER_PREFIX};
use super::nats_section_wire::NatsSectionWire;
use super::service_config_wire::ServiceConfigWire;
use super::{AdmissionLimit, ConfigSetting, OpenBaoAddress, ServiceArgs, ServiceConfigError, TomlSyntax};

/// The configuration the secrets service runs with, after every source is
/// merged and every setting is validated (ADR#0007).
///
/// No secret is held here: the sealing key seed and the AppRole secret id are
/// named by where to read them, and a NATS secret is held the way the NATS
/// connection reads it.
#[derive(Clone, Debug)]
pub struct ServiceConfig {
    /// The connection to NATS, over TLS unless `NATS_TLS_DISABLED` says
    /// otherwise (ADR#0081 Decision 1).
    pub nats: NatsConfig,
    pub openbao_address: OpenBaoAddress,
    /// The KV mount the released credentials are read from.
    pub openbao_mount: OpenBaoMount,
    pub openbao_role_id: String,
    /// The file that holds the AppRole secret id.
    pub openbao_secret_id_file: PathBuf,
    /// The role and the policy of the child token each release reads with.
    pub child_tokens: OpenBaoChildTokens,
    pub tenant_owners_file: PathBuf,
    pub release_policies_file: PathBuf,
    /// Where the service's own sealing key seed is read from (ADR#0083).
    pub xkey_seed: SecretReference,
    /// The credential event stream a credential's state is folded from.
    pub credential_stream: NatsToken,
    pub admission_limit: AdmissionLimit,
    /// Who the service's audit writer is.
    pub audit: WriterIdentity,
}

impl ServiceConfig {
    /// Merges the built-in defaults, the configuration file named by
    /// `--config`, the environment, and the flags, each over the one before,
    /// and validates the result. An empty value counts as unset.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceConfigError`], naming the setting, when the file
    /// cannot be read or holds a key it does not define, when a required
    /// setting is unset, when a value is invalid, or when a secret is given
    /// inline rather than by reference. No error quotes a secret.
    pub fn resolve(args: &ServiceArgs, env: &impl ReadEnv, fs: &impl ReadFile) -> Result<Self, ServiceConfigError> {
        let wire = match &args.runtime.config {
            Some(path) => read_file(path, fs)?,
            None => ServiceConfigWire::default(),
        };
        let openbao = wire.openbao;
        let release = wire.release;
        let audit = wire.audit;

        let nats = nats(wire.nats, args, env)?;
        let openbao_address = OpenBaoAddress::new(
            &required(env, ConfigSetting::OpenBaoAddress, openbao.address)?,
            env_value(env, ConfigSetting::OpenBaoTlsDisabled)?.as_deref(),
        )
        .map_err(ServiceConfigError::OpenBaoAddress)?;
        let openbao_mount = OpenBaoMount::new(required(env, ConfigSetting::OpenBaoMount, openbao.mount)?)
            .map_err(ServiceConfigError::OpenBaoMount)?;
        let openbao_role_id = required(env, ConfigSetting::OpenBaoRoleId, openbao.role_id)?;
        let openbao_secret_id_file = required(env, ConfigSetting::OpenBaoSecretIdFile, openbao.secret_id_file)?.into();
        let child_tokens = OpenBaoChildTokens::new(
            OpenBaoTokenRole::new(required(
                env,
                ConfigSetting::OpenBaoChildTokenRole,
                openbao.child_token_role,
            )?)
            .map_err(ServiceConfigError::ChildTokenRole)?,
            OpenBaoPolicyName::new(required(
                env,
                ConfigSetting::OpenBaoChildTokenPolicy,
                openbao.child_token_policy,
            )?)
            .map_err(ServiceConfigError::ChildTokenPolicy)?,
        );

        let tenant_owners_file = required(env, ConfigSetting::TenantOwnersFile, release.tenant_owners_file)?.into();
        let release_policies_file = required(env, ConfigSetting::ReleasePoliciesFile, release.policies_file)?.into();
        let xkey_seed =
            reference(env, ConfigSetting::XkeySeed, release.xkey_seed)?.ok_or(ServiceConfigError::Missing {
                setting: ConfigSetting::XkeySeed,
            })?;
        let credential_stream = NatsToken::new(
            layered(env, ConfigSetting::CredentialStream, release.credential_stream)?
                .unwrap_or_else(|| CREDENTIAL_STREAM.to_owned()),
        )
        .map_err(ServiceConfigError::CredentialStream)?;
        let admission_limit = match env_value(env, ConfigSetting::AdmissionLimit)? {
            Some(raw) => Some(raw.parse().map_err(|source| ServiceConfigError::NotANumber {
                setting: ConfigSetting::AdmissionLimit,
                source,
            })?),
            None => release.admission_limit,
        }
        .map(AdmissionLimit::new)
        .transpose()
        .map_err(ServiceConfigError::AdmissionLimit)?
        .unwrap_or_default();

        let tenant = AuditName::new(required(env, ConfigSetting::AuditTenant, audit.tenant)?)
            .map_err(ServiceConfigError::AuditTenant)?;
        let project = NatsToken::new(required(env, ConfigSetting::AuditProject, audit.project)?)
            .map_err(ServiceConfigError::AuditProject)?;
        let instance = required(env, ConfigSetting::AuditInstance, audit.instance)?;
        let writer =
            WriterId::new(format!("{AUDIT_WRITER_PREFIX}{instance}")).map_err(ServiceConfigError::AuditInstance)?;
        let component = NatsToken::new(AUDIT_COMPONENT).map_err(ServiceConfigError::AuditComponent)?;

        Ok(Self {
            nats,
            openbao_address,
            openbao_mount,
            openbao_role_id,
            openbao_secret_id_file,
            child_tokens,
            tenant_owners_file,
            release_policies_file,
            xkey_seed,
            credential_stream,
            admission_limit,
            audit: WriterIdentity::new(writer, component, tenant, project),
        })
    }
}

fn read_file(path: &Path, fs: &impl ReadFile) -> Result<ServiceConfigWire, ServiceConfigError> {
    let document = fs
        .read_to_string(path)
        .map_err(|source| ServiceConfigError::ConfigUnreadable {
            path: path.to_owned(),
            source,
        })?;
    toml::from_str(&document).map_err(|error| ServiceConfigError::ConfigUnparsable {
        path: path.to_owned(),
        syntax: TomlSyntax::of(&error, &document),
    })
}

/// The NATS connection: each setting from the file, then the environment,
/// then the flags, with TLS read from `NATS_TLS_DISABLED` alone.
fn nats(wire: NatsSectionWire, args: &ServiceArgs, env: &impl ReadEnv) -> Result<NatsConfig, ServiceConfigError> {
    let section = NatsConfigSection {
        url: layered(env, ConfigSetting::NatsUrl, wire.url)?.unwrap_or_default(),
        creds: layered(env, ConfigSetting::NatsCreds, wire.creds)?,
        nkey: nats_secret(env, ConfigSetting::NatsNkey, wire.nkey)?,
        user: layered(env, ConfigSetting::NatsUser, wire.user)?,
        password: nats_secret(env, ConfigSetting::NatsPassword, wire.password)?,
        token: nats_secret(env, ConfigSetting::NatsToken, wire.token)?,
        jwt: nats_secret(env, ConfigSetting::NatsJwt, wire.jwt)?,
        seed: nats_secret(env, ConfigSetting::NatsSeed, wire.seed)?,
        tls_cert: layered(env, ConfigSetting::NatsTlsCert, wire.tls_cert)?,
        tls_key: layered(env, ConfigSetting::NatsTlsKey, wire.tls_key)?,
        tls_ca: layered(env, ConfigSetting::NatsTlsCa, wire.tls_ca)?,
    };
    let url_flag = args.runtime.nats.nats_url.as_deref().filter(|url| !url.is_empty());
    if section.url.is_empty() && url_flag.is_none() {
        return Err(ServiceConfigError::Missing {
            setting: ConfigSetting::NatsUrl,
        });
    }
    // The NATS settings read a switch that is not UTF-8 as unset, which would
    // leave TLS on; refuse it here instead, so the operator learns the switch
    // was not read.
    env_value(env, ConfigSetting::NatsTlsDisabled)?;

    let config = resolve_nats(&section, &args.runtime.nats, env).map_err(ServiceConfigError::Nats)?;
    if let Err(misconfiguration) = &config.tls {
        return Err(match misconfiguration {
            TlsMisconfiguration::IncompleteClientCertificate(IncompleteClientCertificate::MissingKey) => {
                ServiceConfigError::NatsClientCertificate {
                    present: ConfigSetting::NatsTlsCert,
                    missing: ConfigSetting::NatsTlsKey,
                }
            }
            TlsMisconfiguration::IncompleteClientCertificate(IncompleteClientCertificate::MissingCert) => {
                ServiceConfigError::NatsClientCertificate {
                    present: ConfigSetting::NatsTlsKey,
                    missing: ConfigSetting::NatsTlsCert,
                }
            }
            TlsMisconfiguration::UnrecognizedDisabledValue(value) => {
                ServiceConfigError::NatsTlsDisabledValue { value: value.clone() }
            }
            TlsMisconfiguration::DisabledAlongside(variable) => {
                ServiceConfigError::NatsTlsDisabledAlongside { variable }
            }
        });
    }
    if config.servers.is_empty() {
        return Err(ServiceConfigError::NoNatsServers);
    }
    Ok(config)
}

/// The value of `setting`'s environment variable, when it is set and not
/// empty.
fn env_value(env: &impl ReadEnv, setting: ConfigSetting) -> Result<Option<String>, ServiceConfigError> {
    match env.var(setting.env()) {
        Ok(value) if !value.is_empty() => Ok(Some(value)),
        Ok(_) | Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(ServiceConfigError::NotUnicode { setting }),
    }
}

/// The environment's value of `setting` over the file's, when either is set
/// and not empty.
fn layered(
    env: &impl ReadEnv,
    setting: ConfigSetting,
    file: Option<String>,
) -> Result<Option<String>, ServiceConfigError> {
    Ok(env_value(env, setting)?.or(file.filter(|value| !value.is_empty())))
}

fn required(env: &impl ReadEnv, setting: ConfigSetting, file: Option<String>) -> Result<String, ServiceConfigError> {
    layered(env, setting, file)?.ok_or(ServiceConfigError::Missing { setting })
}

/// Where a secret is read from: the environment's reference over the file's.
/// Both must be references; the secret itself is refused without being
/// quoted.
fn reference(
    env: &impl ReadEnv,
    setting: ConfigSetting,
    file: Option<toml::Value>,
) -> Result<Option<SecretReference>, ServiceConfigError> {
    let raw = match env_value(env, setting)? {
        Some(value) => value,
        None => match file_reference(setting, file)? {
            Some(value) => value,
            None => return Ok(None),
        },
    };
    raw.parse()
        .map(Some)
        .map_err(|source| ServiceConfigError::SecretReference { setting, source })
}

/// A NATS secret: the environment's value, a reference or the secret itself as
/// the NATS tools read it, over the file's, which must be a reference.
fn nats_secret(
    env: &impl ReadEnv,
    setting: ConfigSetting,
    file: Option<toml::Value>,
) -> Result<Option<SecretString>, ServiceConfigError> {
    if let Some(value) = env_value(env, setting)? {
        return Ok(SecretString::new(value).ok());
    }
    let Some(raw) = file_reference(setting, file)? else {
        return Ok(None);
    };
    let reference: SecretReference = raw
        .parse()
        .map_err(|source| ServiceConfigError::SecretReference { setting, source })?;
    Ok(SecretString::new(reference.to_string()).ok())
}

/// A secret's reference as the file writes it, which must be a string.
fn file_reference(setting: ConfigSetting, file: Option<toml::Value>) -> Result<Option<String>, ServiceConfigError> {
    match file {
        None => Ok(None),
        Some(toml::Value::String(value)) => Ok(Some(value).filter(|value| !value.is_empty())),
        Some(_) => Err(ServiceConfigError::SecretNotAString { setting }),
    }
}

#[cfg(test)]
mod tests;
