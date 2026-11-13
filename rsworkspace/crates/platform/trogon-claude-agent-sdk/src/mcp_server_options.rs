use crate::{ConfigValue, Environment, JsonObject, McpServerToolPolicy, McpUrl};
use serde_json::Value;
use std::collections::BTreeMap;

/// An MCP server the CLI connects to itself, sent to it the way both upstream SDKs send their server
/// configs: every field the caller set, in declaration order, followed by `extra` keys verbatim.
#[derive(Clone, Debug)]
pub enum McpServerOptions {
    Stdio(McpStdioServerOptions),
    Sse(McpRemoteServerOptions),
    Http(McpRemoteServerOptions),
    /// An `sdk` server config without an in-process instance, passed through untouched like upstream does.
    Sdk(McpSdkServerOptions),
}

/// TypeScript `McpStdioServerConfig` / Python `McpStdioServerConfig`. Arguments and environment values
/// are sensitive unless marked plain, so debug output stays safe.
#[derive(Clone, Debug, Default)]
pub struct McpStdioServerOptions {
    pub command: std::path::PathBuf,
    /// Both SDKs make `type: "stdio"` optional; `true` writes it.
    pub declare_type: bool,
    pub args: Option<Vec<ConfigValue>>,
    pub env: Option<Environment>,
    pub timeout: Option<u64>,
    pub always_load: Option<bool>,
    pub extra: JsonObject,
}

/// TypeScript `McpHttpServerConfig` and `McpSSEServerConfig`; the variant picks the `type`.
#[derive(Clone, Debug)]
pub struct McpRemoteServerOptions {
    pub url: McpUrl,
    pub headers: Option<BTreeMap<String, ConfigValue>>,
    pub tools: Option<Vec<McpServerToolPolicy>>,
    pub timeout: Option<u64>,
    pub always_load: Option<bool>,
    pub extra: JsonObject,
}

/// TypeScript `McpSdkServerConfig` without `instance`.
#[derive(Clone, Debug, Default)]
pub struct McpSdkServerOptions {
    pub name: String,
    pub timeout: Option<u64>,
    pub tools: Option<Vec<McpServerToolPolicy>>,
    pub extra: JsonObject,
}

impl McpRemoteServerOptions {
    pub fn new(url: impl Into<McpUrl>) -> Self {
        Self {
            url: url.into(),
            headers: None,
            tools: None,
            timeout: None,
            always_load: None,
            extra: JsonObject::new(),
        }
    }
}

impl McpServerOptions {
    pub fn to_wire_json(&self) -> Result<Value, serde_json::Error> {
        let mut object = JsonObject::new();
        let extra = match self {
            Self::Stdio(options) => {
                if options.declare_type {
                    object.insert("type".into(), "stdio".into());
                }
                object.insert("command".into(), options.command.to_string_lossy().into_owned().into());
                if let Some(args) = &options.args {
                    object.insert(
                        "args".into(),
                        args.iter()
                            .map(|arg| Value::from(arg.expose()))
                            .collect::<Vec<_>>()
                            .into(),
                    );
                }
                if let Some(env) = &options.env {
                    object.insert(
                        "env".into(),
                        Value::Object(
                            env.iter()
                                .map(|(key, value)| (key.to_owned(), value.expose().into()))
                                .collect(),
                        ),
                    );
                }
                insert_optional(&mut object, "timeout", options.timeout);
                insert_optional(&mut object, "alwaysLoad", options.always_load);
                &options.extra
            }
            Self::Sse(options) => remote(&mut object, "sse", options)?,
            Self::Http(options) => remote(&mut object, "http", options)?,
            Self::Sdk(options) => {
                object.insert("type".into(), "sdk".into());
                object.insert("name".into(), options.name.clone().into());
                insert_optional(&mut object, "timeout", options.timeout);
                insert_tools(&mut object, options.tools.as_deref())?;
                &options.extra
            }
        };
        for (key, value) in extra {
            if !object.contains_key(key) {
                object.insert(key.clone(), value.clone());
            }
        }
        Ok(Value::Object(object))
    }
}

fn remote<'a>(
    object: &mut JsonObject,
    kind: &str,
    options: &'a McpRemoteServerOptions,
) -> Result<&'a JsonObject, serde_json::Error> {
    object.insert("type".into(), kind.into());
    object.insert("url".into(), options.url.as_str().into());
    if let Some(headers) = &options.headers {
        object.insert(
            "headers".into(),
            Value::Object(
                headers
                    .iter()
                    .map(|(key, value)| (key.clone(), value.expose().into()))
                    .collect(),
            ),
        );
    }
    insert_tools(object, options.tools.as_deref())?;
    insert_optional(object, "timeout", options.timeout);
    insert_optional(object, "alwaysLoad", options.always_load);
    Ok(&options.extra)
}

fn insert_tools(object: &mut JsonObject, tools: Option<&[McpServerToolPolicy]>) -> Result<(), serde_json::Error> {
    if let Some(tools) = tools {
        object.insert(
            "tools".into(),
            tools
                .iter()
                .map(McpServerToolPolicy::to_wire_json)
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        );
    }
    Ok(())
}

fn insert_optional<T: Into<Value>>(object: &mut JsonObject, key: &str, value: Option<T>) {
    if let Some(value) = value {
        object.insert(key.into(), value.into());
    }
}
