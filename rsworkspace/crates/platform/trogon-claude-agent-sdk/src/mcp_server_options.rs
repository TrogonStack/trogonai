use crate::{ConfigValue, Environment, McpUrl};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// External MCP server transports. Arguments and header values are sensitive
/// unless marked plain, so debug output stays safe without a hand-written list.
#[derive(Clone, Debug)]
pub enum McpServerOptions {
    Stdio {
        command: std::path::PathBuf,
        args: Vec<ConfigValue>,
        env: Environment,
    },
    Http {
        url: McpUrl,
        headers: BTreeMap<String, ConfigValue>,
    },
    Sse {
        url: McpUrl,
        headers: BTreeMap<String, ConfigValue>,
    },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum McpServerOptionsWire<'a> {
    Stdio {
        command: &'a std::path::Path,
        args: Vec<&'a str>,
        env: BTreeMap<&'a str, &'a str>,
    },
    Http {
        url: &'a str,
        headers: BTreeMap<&'a str, &'a str>,
    },
    Sse {
        url: &'a str,
        headers: BTreeMap<&'a str, &'a str>,
    },
}

impl McpServerOptions {
    pub fn to_wire_json(&self) -> Result<Value, serde_json::Error> {
        let wire = match self {
            Self::Stdio { command, args, env } => McpServerOptionsWire::Stdio {
                command: command.as_path(),
                args: args.iter().map(ConfigValue::expose).collect(),
                env: env.iter().map(|(key, value)| (key, value.expose())).collect(),
            },
            Self::Http { url, headers } => McpServerOptionsWire::Http {
                url: url.as_str(),
                headers: exposed(headers),
            },
            Self::Sse { url, headers } => McpServerOptionsWire::Sse {
                url: url.as_str(),
                headers: exposed(headers),
            },
        };
        serde_json::to_value(wire)
    }
}

fn exposed(values: &BTreeMap<String, ConfigValue>) -> BTreeMap<&str, &str> {
    values
        .iter()
        .map(|(key, value)| (key.as_str(), value.expose()))
        .collect()
}
