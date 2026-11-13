//! Command-line construction that reproduces the upstream SDKs byte for byte.
//!
//! `typescript` follows `sdk.mjs` (0.3.323) and `python` follows `subprocess_cli.py` (0.2.181). Both are pure
//! functions of the options so golden tests can pin the exact tokens.

use std::ffi::OsString;

use crate::{
    ClaudeAgentOptions, Customizations, Error, ExtraArgument, PartialMessages, ProtocolError, SessionPersistence,
    SystemPrompt, ToolSet,
    process_options::ArgvStyle,
    wire::runtime_payload::{EmbeddedMcpPayload, McpConfigPayload},
};

/// TypeScript's `--sdk-url` guard: an extra argument named `sdk-url`, `sdk-url=...`, or an executable argument.
pub(crate) fn mentions_sdk_url(options: &ClaudeAgentOptions) -> bool {
    options
        .extra_args
        .iter()
        .any(|extra| extra.flag == "sdk-url" || extra.flag.starts_with("sdk-url="))
        || options.script_runtime.arguments.iter().any(|argument| {
            let argument = argument.to_string_lossy();
            argument == "--sdk-url" || argument.starts_with("--sdk-url=")
        })
}

/// TypeScript asks the CLI to await `initialize` when it resumes or continues with the SDK-resolved executable
/// (no `pathToClaudeCodeExecutable`, no custom spawn) and no `--sdk-url`, by appending `await-initialize` to
/// `extraArgs`. Python never sends it.
pub(crate) fn request_await_initialize(options: &mut ClaudeAgentOptions) {
    if options.cli_path.is_none()
        && options.argv_style == crate::ArgvStyle::TypeScript
        && (options.resume.is_some() || options.continue_conversation == crate::ContinueConversation::Yes)
        && !mentions_sdk_url(options)
        && !options.extra_args.iter().any(|extra| extra.flag == "await-initialize")
    {
        options.extra_args.push(ExtraArgument {
            flag: "await-initialize".into(),
            value: None,
        });
    }
}

pub(crate) fn build(options: &ClaudeAgentOptions) -> Result<Vec<OsString>, Error> {
    match options.argv_style {
        ArgvStyle::TypeScript => typescript(options),
        ArgvStyle::Python => python(options),
    }
}

fn joined<I, S>(items: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    items
        .into_iter()
        .map(|item| item.as_ref().to_owned())
        .collect::<Vec<_>>()
        .join(",")
}

fn mcp_config(options: &ClaudeAgentOptions) -> Result<Option<String>, Error> {
    let mut servers = serde_json::Map::new();
    for (name, config) in &options.mcp_servers {
        servers.insert(
            name.as_str().to_owned(),
            config.to_wire_json().map_err(ProtocolError::from)?,
        );
    }
    for name in options.sdk_mcp_servers.keys() {
        servers.insert(
            name.as_str().to_owned(),
            EmbeddedMcpPayload::Sdk { name: name.as_str() }.to_value()?,
        );
    }
    if servers.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        serde_json::to_string(&McpConfigPayload { servers }).map_err(ProtocolError::from)?,
    ))
}

/// Both SDKs spell a free-form flag as one token when the value starts with `-` and as two otherwise.
fn push_spaced(arguments: &mut Vec<OsString>, flag: &str, value: impl Into<OsString>) {
    let value = value.into();
    if value.to_string_lossy().starts_with('-') {
        let mut token = OsString::from(format!("{flag}="));
        token.push(&value);
        arguments.push(token);
    } else {
        arguments.push(flag.into());
        arguments.push(value);
    }
}

fn push_extra(arguments: &mut Vec<OsString>, extra: &[ExtraArgument]) {
    for ExtraArgument { flag, value } in extra {
        match value {
            None => arguments.push(format!("--{flag}").into()),
            Some(value) => push_spaced(arguments, &format!("--{flag}"), value.as_str()),
        }
    }
}

fn equals(arguments: &mut Vec<OsString>, flag: &str, value: impl AsRef<std::ffi::OsStr>) {
    let mut token = OsString::from(format!("{flag}="));
    token.push(value.as_ref());
    arguments.push(token);
}

fn typescript(options: &ClaudeAgentOptions) -> Result<Vec<OsString>, Error> {
    let mut a: Vec<OsString> = [
        "--output-format",
        "stream-json",
        "--verbose",
        "--input-format",
        "stream-json",
    ]
    .map(OsString::from)
    .into();
    if let Some(effort) = options.effort {
        equals(&mut a, "--effort", effort.as_str());
    }
    if let Some(turns) = options.max_turns {
        equals(&mut a, "--max-turns", turns.to_string());
    }
    if let Some(budget) = &options.max_budget_usd {
        equals(&mut a, "--max-budget-usd", budget.get().to_string());
    }
    if let Some(model) = &options.model {
        equals(&mut a, "--model", model.as_str());
    }
    if let Some(agent) = &options.agent {
        equals(&mut a, "--agent", agent.as_str());
    }
    if let Some(schema) = &options.output_schema {
        equals(
            &mut a,
            "--json-schema",
            serde_json::Value::Object(schema.as_json().clone()).to_string(),
        );
    }
    if options.permission_handler.is_some() {
        a.push("--permission-prompt-tool=stdio".into());
    }
    if options.continue_conversation == crate::ContinueConversation::Yes {
        a.push("--continue".into());
    }
    if let Some(session) = &options.resume {
        equals(&mut a, "--resume", session.to_string());
    }
    if !options.allowed_tools.is_empty() {
        equals(
            &mut a,
            "--allowedTools",
            joined(options.allowed_tools.iter().map(|r| r.as_str())),
        );
    }
    if !options.disallowed_tools.is_empty() {
        equals(
            &mut a,
            "--disallowedTools",
            joined(options.disallowed_tools.iter().map(|r| r.as_str())),
        );
    }
    if let ToolSet::Only(tools) = &options.tools {
        equals(&mut a, "--tools", joined(tools.iter().map(|t| t.as_str())));
    }
    if let Some(config) = mcp_config(options)? {
        push_spaced(&mut a, "--mcp-config", config);
    }
    if let Some(sources) = &options.setting_sources {
        equals(&mut a, "--setting-sources", joined(sources.iter().map(|s| s.as_str())));
    }
    if options.mcp_discovery == crate::McpDiscovery::Disabled {
        a.push("--strict-mcp-config".into());
    }
    if let Some(mode) = options.permission_mode.as_ref() {
        equals(&mut a, "--permission-mode", mode.as_str());
    }
    if options.partial_messages == PartialMessages::Include {
        a.push("--include-partial-messages".into());
    }
    if options.session_store.is_some() {
        a.push("--session-mirror".into());
    }
    for directory in &options.additional_directories {
        equals(&mut a, "--add-dir", directory);
    }
    if crate::driver::plugins_via_initialize(options) {
        a.push("--await-initialize".into());
    } else {
        for plugin in &options.plugins {
            let flag = if plugin.skip_mcp_discovery == Some(true) {
                "--plugin-dir-no-mcp"
            } else {
                "--plugin-dir"
            };
            equals(&mut a, flag, &plugin.path);
        }
    }
    if options.fork_session == crate::SessionFork::Fork {
        a.push("--fork-session".into());
    }
    if options.session_persistence == SessionPersistence::Disabled {
        a.push("--no-session-persistence".into());
    }
    if options.customizations == Customizations::Bare {
        a.push("--bare".into());
    }
    push_extra(&mut a, &options.extra_args);
    Ok(a)
}

fn python(options: &ClaudeAgentOptions) -> Result<Vec<OsString>, Error> {
    let mut a: Vec<OsString> = ["--output-format", "stream-json", "--verbose"]
        .map(OsString::from)
        .into();
    match &options.system_prompt {
        SystemPrompt::Empty => {
            a.push("--system-prompt".into());
            a.push("".into());
        }
        SystemPrompt::Custom(prompt) => {
            a.push("--system-prompt".into());
            a.push(prompt.into());
        }
        SystemPrompt::ClaudeCode => {}
        SystemPrompt::ClaudeCodeWithAppend(prompt) => {
            a.push("--append-system-prompt".into());
            a.push(prompt.into());
        }
    }
    if let ToolSet::Only(tools) = &options.tools {
        a.push("--tools".into());
        a.push(joined(tools.iter().map(|t| t.as_str())).into());
    }
    if !options.allowed_tools.is_empty() {
        a.push("--allowedTools".into());
        a.push(joined(options.allowed_tools.iter().map(|r| r.as_str())).into());
    }
    if let Some(turns) = options.max_turns {
        a.push("--max-turns".into());
        a.push(turns.to_string().into());
    }
    if let Some(budget) = &options.max_budget_usd {
        a.push("--max-budget-usd".into());
        a.push(budget.get().to_string().into());
    }
    if !options.disallowed_tools.is_empty() {
        a.push("--disallowedTools".into());
        a.push(joined(options.disallowed_tools.iter().map(|r| r.as_str())).into());
    }
    if let Some(model) = &options.model {
        a.push("--model".into());
        a.push(model.as_str().into());
    }
    if let Some(agent) = &options.agent {
        a.push("--agent".into());
        a.push(agent.as_str().into());
    }
    if options.permission_handler.is_some() {
        a.push("--permission-prompt-tool".into());
        a.push("stdio".into());
    }
    if let Some(mode) = options.permission_mode.as_ref() {
        a.push("--permission-mode".into());
        a.push(mode.as_str().into());
    }
    if options.continue_conversation == crate::ContinueConversation::Yes {
        a.push("--continue".into());
    }
    if let Some(session) = &options.resume {
        equals(&mut a, "--resume", session.to_string());
    }
    for directory in &options.additional_directories {
        a.push("--add-dir".into());
        a.push(directory.into());
    }
    if let Some(config) = mcp_config(options)? {
        a.push("--mcp-config".into());
        a.push(config.into());
    }
    if options.partial_messages == PartialMessages::Include {
        a.push("--include-partial-messages".into());
    }
    if options.mcp_discovery == crate::McpDiscovery::Disabled {
        a.push("--strict-mcp-config".into());
    }
    if options.fork_session == crate::SessionFork::Fork {
        a.push("--fork-session".into());
    }
    if options.session_store.is_some() {
        a.push("--session-mirror".into());
    }
    if let Some(sources) = &options.setting_sources {
        equals(&mut a, "--setting-sources", joined(sources.iter().map(|s| s.as_str())));
    }
    for plugin in &options.plugins {
        a.push("--plugin-dir".into());
        a.push(plugin.path.clone().into_os_string());
    }
    if options.session_persistence == SessionPersistence::Disabled {
        a.push("--no-session-persistence".into());
    }
    if options.customizations == Customizations::Bare {
        a.push("--bare".into());
    }
    push_extra(&mut a, &options.extra_args);
    if let Some(effort) = options.effort {
        a.push("--effort".into());
        a.push(effort.as_str().into());
    }
    if let Some(schema) = &options.output_schema {
        a.push("--json-schema".into());
        a.push(serde_json::Value::Object(schema.as_json().clone()).to_string().into());
    }
    a.push("--input-format".into());
    a.push("stream-json".into());
    Ok(a)
}

#[cfg(test)]
mod tests;
