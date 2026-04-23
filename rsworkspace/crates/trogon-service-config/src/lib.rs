use std::path::{Path, PathBuf};

use clap::Args;
use confique::Config;
use trogon_nats::{NatsAuth, NatsConfig};

#[derive(Args, Clone, Debug, Default)]
pub struct RuntimeConfigArgs {
    #[arg(long, short, global = true)]
    pub config: Option<PathBuf>,

    #[command(flatten)]
    pub nats: NatsArgs,
}

#[derive(Args, Clone, Debug, Default)]
pub struct NatsArgs {
    #[arg(long, global = true)]
    pub nats_url: Option<String>,
    #[arg(long, global = true)]
    pub nats_creds: Option<String>,
    #[arg(long, global = true)]
    pub nats_nkey: Option<String>,
    #[arg(long, global = true)]
    pub nats_user: Option<String>,
    #[arg(long, global = true)]
    pub nats_password: Option<String>,
    #[arg(long, global = true)]
    pub nats_token: Option<String>,
}

#[derive(Config, Clone, Debug)]
pub struct NatsConfigSection {
    #[config(env = "NATS_URL", default = "localhost:4222")]
    pub url: String,
    #[config(env = "NATS_CREDS")]
    pub creds: Option<String>,
    #[config(env = "NATS_NKEY")]
    pub nkey: Option<String>,
    #[config(env = "NATS_USER")]
    pub user: Option<String>,
    #[config(env = "NATS_PASSWORD")]
    pub password: Option<String>,
    #[config(env = "NATS_TOKEN")]
    pub token: Option<String>,
}

pub fn load_config<T: Config>(config_path: Option<&Path>) -> Result<T, confique::Error> {
    let mut builder = T::builder();
    if let Some(path) = config_path {
        builder = builder.file(path);
    }
    builder.env().load()
}

pub fn resolve_nats(section: &NatsConfigSection, overrides: &NatsArgs) -> NatsConfig {
    let auth = if let Some(creds) = first_non_empty(overrides.nats_creds.as_ref(), section.creds.as_ref()) {
        NatsAuth::Credentials(creds.clone().into())
    } else if let Some(nkey) = first_non_empty(overrides.nats_nkey.as_ref(), section.nkey.as_ref()) {
        NatsAuth::NKey(nkey.clone())
    } else if let (Some(user), Some(password)) = (
        first_non_empty(overrides.nats_user.as_ref(), section.user.as_ref()),
        first_non_empty(overrides.nats_password.as_ref(), section.password.as_ref()),
    ) {
        NatsAuth::UserPassword {
            user: user.clone(),
            password: password.clone(),
        }
    } else if let Some(token) = first_non_empty(overrides.nats_token.as_ref(), section.token.as_ref()) {
        NatsAuth::Token(token.clone())
    } else {
        NatsAuth::None
    };

    let raw_url = first_non_empty(overrides.nats_url.as_ref(), Some(&section.url))
        .map(|value| value.as_str())
        .unwrap_or("localhost:4222");

    let servers = raw_url
        .split(',')
        .map(str::trim)
        .filter(|server| !server.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    NatsConfig::new(servers, auth)
}

fn first_non_empty<'a>(primary: Option<&'a String>, fallback: Option<&'a String>) -> Option<&'a String> {
    primary
        .filter(|value| !value.is_empty())
        .or_else(|| fallback.filter(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Config)]
    struct DummyConfig {
        #[config(env = "DUMMY_VALUE", default = "default")]
        value: String,
    }

    #[test]
    fn resolve_nats_uses_section_when_no_overrides() {
        let section = NatsConfigSection {
            url: "host1:4222, host2:4222".to_string(),
            creds: None,
            nkey: None,
            user: None,
            password: None,
            token: Some("section-token".to_string()),
        };

        let resolved = resolve_nats(&section, &NatsArgs::default());

        assert_eq!(resolved.servers, vec!["host1:4222", "host2:4222"]);
        assert!(matches!(resolved.auth, NatsAuth::Token(ref token) if token == "section-token"));
    }

    #[test]
    fn resolve_nats_prefers_cli_overrides() {
        let section = NatsConfigSection {
            url: "host1:4222".to_string(),
            creds: None,
            nkey: None,
            user: None,
            password: None,
            token: Some("section-token".to_string()),
        };
        let overrides = NatsArgs {
            nats_url: Some("override1:4222,override2:4222".to_string()),
            nats_token: Some("override-token".to_string()),
            ..Default::default()
        };

        let resolved = resolve_nats(&section, &overrides);

        assert_eq!(resolved.servers, vec!["override1:4222", "override2:4222"]);
        assert!(matches!(resolved.auth, NatsAuth::Token(ref token) if token == "override-token"));
    }

    #[test]
    fn resolve_nats_keeps_auth_priority_with_overrides() {
        let section = NatsConfigSection {
            url: "host1:4222".to_string(),
            creds: None,
            nkey: None,
            user: None,
            password: None,
            token: Some("section-token".to_string()),
        };
        let overrides = NatsArgs {
            nats_creds: Some("/tmp/nats.creds".to_string()),
            nats_token: Some("override-token".to_string()),
            ..Default::default()
        };

        let resolved = resolve_nats(&section, &overrides);

        assert!(matches!(resolved.auth, NatsAuth::Credentials(ref path) if path == Path::new("/tmp/nats.creds")));
    }

    #[test]
    fn load_config_reads_optional_file() {
        let file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
        std::fs::write(file.path(), "value = 'from-file'\n").unwrap();

        let loaded: DummyConfig = load_config(Some(file.path())).unwrap();

        assert_eq!(loaded.value, "from-file");
    }
}
