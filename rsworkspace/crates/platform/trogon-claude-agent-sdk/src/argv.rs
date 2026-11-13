//! Command-line construction that reproduces the upstream SDKs byte for byte.
//!
//! `typescript` follows `sdk.mjs` (0.3.323) and `python` follows `subprocess_cli.py` (0.2.181). Both are pure
//! functions of the options so golden tests can pin the exact tokens.

use std::{ffi::OsString, path::Path};

use crate::{
    ClaudeAgentOptions, Customizations, DangerousPermissionSkip, DebugOutput, Error, ExtraArgument, HookEvents,
    OptionError, PartialMessages, ProtocolError, SessionPersistence, ThinkingConfig, ToolSet,
    adapters::{NativeHost, ReadFile},
    process_options::ArgvStyle,
    skills::python_repr,
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

/// `derived_debug_file` is the TypeScript default debug file, which only a native spawn derives.
pub(crate) fn build(options: &ClaudeAgentOptions, derived_debug_file: Option<&Path>) -> Result<Vec<OsString>, Error> {
    build_with(options, derived_debug_file, &NativeHost)
}

/// [`build`] with the file reader Python's settings merge uses for a settings file path.
pub(crate) fn build_with<R>(
    options: &ClaudeAgentOptions,
    derived_debug_file: Option<&Path>,
    files: &R,
) -> Result<Vec<OsString>, Error>
where
    R: ReadFile<Error = std::io::Error>,
{
    crate::shadow_warning::reject_handler_with_tool_name(options)?;
    if options.argv_style == ArgvStyle::TypeScript {
        options.system_prompt.reject_for_typescript()?;
    }
    match options.argv_style {
        ArgvStyle::TypeScript => typescript(options, derived_debug_file),
        ArgvStyle::Python => python(options, files),
    }
}

/// Python `_reject_windows_cmd_metacharacters`: on Windows, a value that carries a `cmd.exe` metacharacter or a
/// line break never reaches the command line.
pub(crate) fn reject_windows_metacharacters(name: &str, value: &str, windows: bool) -> Result<(), OptionError> {
    if !windows {
        return Ok(());
    }
    let mut bad: Vec<char> = value
        .chars()
        .filter(|c| crate::constants::WINDOWS_CMD_METACHARACTERS.contains(*c) || matches!(c, '\r' | '\n'))
        .collect();
    bad.sort_unstable();
    bad.dedup();
    if bad.is_empty() {
        return Ok(());
    }
    let listed: Vec<String> = bad.iter().map(|c| python_repr(&c.to_string())).collect();
    Err(OptionError::new(format!(
        "{name} value {} contains characters that are unsafe to pass on a Windows command line: [{}]",
        python_repr(value),
        listed.join(", ")
    )))
}

/// Python rejects Windows metacharacters in the four values an application commonly takes from outside.
fn reject_windows_values(options: &ClaudeAgentOptions) -> Result<(), OptionError> {
    let windows = cfg!(windows);
    if let Some(session) = &options.resume {
        reject_windows_metacharacters("resume", session.as_str(), windows)?;
    }
    if let Some(session) = options.session_id.as_ref().filter(|id| !id.as_str().is_empty()) {
        reject_windows_metacharacters("session_id", session.as_str(), windows)?;
    }
    if let Some(at) = options.resume_session_at.as_deref().filter(|at| !at.is_empty()) {
        reject_windows_metacharacters("resume_session_at", at, windows)?;
    }
    if let Some(turn) = options.resume_drops_turn.as_deref() {
        reject_windows_metacharacters("resume_drops_turn", turn, windows)?;
    }
    Ok(())
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

/// The `--mcp-config` JSON. TypeScript sends only the servers the CLI connects to itself (in-process servers go
/// in `initialize`), as compact JSON. Python also lists each in-process server as `{"type": "sdk", "name": ...}`
/// and writes `json.dumps` text. An in-process server wins a name shared with an external one, as an object
/// spread does upstream.
fn mcp_config(options: &ClaudeAgentOptions, style: ArgvStyle) -> Result<Option<String>, Error> {
    let mut servers = serde_json::Map::new();
    for (name, config) in &options.mcp_servers {
        if style == ArgvStyle::TypeScript && options.sdk_mcp_servers.contains_key(name) {
            continue;
        }
        servers.insert(
            name.as_str().to_owned(),
            config.to_wire_json().map_err(ProtocolError::from)?,
        );
    }
    if style == ArgvStyle::Python {
        for name in options.sdk_mcp_servers.keys() {
            servers.insert(
                name.as_str().to_owned(),
                EmbeddedMcpPayload::Sdk { name: name.as_str() }.to_value()?,
            );
        }
    }
    if servers.is_empty() {
        return Ok(None);
    }
    let payload = McpConfigPayload { servers };
    Ok(Some(
        match style {
            ArgvStyle::TypeScript => serde_json::to_string(&payload),
            ArgvStyle::Python => crate::python_json::to_string(&payload),
        }
        .map_err(ProtocolError::from)?,
    ))
}

/// Both SDKs spell a free-form flag as one token when the value starts with `-` and as two otherwise.
fn push_spaced(arguments: &mut Vec<OsString>, flag: &str, value: impl Into<OsString>) {
    let value = value.into();
    // `yE`: a lone `-` is still a separate token.
    if value.len() > 1 && value.to_string_lossy().starts_with('-') {
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

fn push_entries(arguments: &mut Vec<OsString>, entries: Vec<crate::settings_option::ExtraEntry>) {
    for (flag, value) in entries {
        match value {
            None => arguments.push(format!("--{flag}").into()),
            Some(value) => push_spaced(arguments, &format!("--{flag}"), value),
        }
    }
}

fn equals(arguments: &mut Vec<OsString>, flag: &str, value: impl AsRef<std::ffi::OsStr>) {
    let mut token = OsString::from(format!("{flag}="));
    token.push(value.as_ref());
    arguments.push(token);
}

fn typescript_thinking(a: &mut Vec<OsString>, options: &ClaudeAgentOptions) {
    // `maxThinkingTokens` only applies when `thinking` is absent: zero disables, any other value is a budget.
    let thinking = options.thinking.or(match options.max_thinking_tokens {
        Some(0) => Some(ThinkingConfig::Disabled),
        Some(budget) => Some(ThinkingConfig::Enabled {
            budget_tokens: Some(budget),
            display: None,
        }),
        None => None,
    });
    let Some(thinking) = thinking else {
        return;
    };
    let display = match thinking {
        ThinkingConfig::Enabled {
            budget_tokens: Some(budget),
            display,
        } => {
            equals(a, "--max-thinking-tokens", budget.to_string());
            display
        }
        ThinkingConfig::Enabled {
            budget_tokens: None,
            display,
        }
        | ThinkingConfig::Adaptive { display } => {
            a.push("--thinking=adaptive".into());
            display
        }
        ThinkingConfig::Disabled => {
            a.push("--thinking=disabled".into());
            None
        }
    };
    if let Some(display) = display {
        equals(a, "--thinking-display", display.as_str());
    }
}

fn typescript(options: &ClaudeAgentOptions, derived_debug_file: Option<&Path>) -> Result<Vec<OsString>, Error> {
    let allowed_tools = crate::skills::effective_allowed_tools(options)?;
    let mut a: Vec<OsString> = [
        "--output-format",
        "stream-json",
        "--verbose",
        "--input-format",
        "stream-json",
    ]
    .map(OsString::from)
    .into();
    typescript_thinking(&mut a, options);
    if let Some(effort) = options.effort {
        equals(&mut a, "--effort", effort.as_str());
    }
    if let Some(turns) = options.max_turns {
        equals(&mut a, "--max-turns", turns.to_string());
    }
    if let Some(budget) = &options.max_budget_usd {
        equals(&mut a, "--max-budget-usd", budget.typescript_text());
    }
    if let Some(budget) = options.task_budget {
        equals(&mut a, "--task-budget", budget.total.to_string());
    }
    if let Some(model) = &options.model {
        equals(&mut a, "--model", model.as_str());
    }
    if let Some(agent) = &options.agent {
        equals(&mut a, "--agent", agent.as_str());
    }
    if !options.betas.is_empty() {
        equals(
            &mut a,
            "--betas",
            joined(options.betas.iter().map(|beta| beta.as_str())),
        );
    }
    if let Some(schema) = &options.output_schema {
        equals(
            &mut a,
            "--json-schema",
            serde_json::Value::Object(schema.as_json().clone()).to_string(),
        );
    }
    let debug_file = options.debug_file.as_ref().filter(|file| !file.as_os_str().is_empty());
    if let Some(file) = debug_file {
        equals(&mut a, "--debug-file", file);
    } else if options.debug == DebugOutput::On {
        a.push("--debug".into());
    }
    if debug_file.is_none()
        && let Some(file) = derived_debug_file
    {
        equals(&mut a, "--debug-file", file);
    }
    if options.permission_handler.is_some() {
        a.push("--permission-prompt-tool=stdio".into());
    } else if let Some(name) = options
        .permission_prompt_tool_name
        .as_deref()
        .filter(|name| !name.is_empty())
    {
        equals(&mut a, "--permission-prompt-tool", name);
    }
    if let Some(prompts) = options.permission_prompts {
        equals(&mut a, "--permission-prompts", prompts.as_str());
    }
    if options.continue_conversation == crate::ContinueConversation::Yes {
        a.push("--continue".into());
    }
    if let Some(session) = &options.resume {
        equals(&mut a, "--resume", session.to_string());
    }
    for channel in &options.channels {
        equals(&mut a, "--channels", channel);
    }
    if !allowed_tools.is_empty() {
        equals(&mut a, "--allowedTools", joined(&allowed_tools));
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
    if let Some(config) = mcp_config(options, ArgvStyle::TypeScript)? {
        push_spaced(&mut a, "--mcp-config", config);
    }
    if let Some(config) = &options.mcp_config {
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
    if options.allow_dangerously_skip_permissions == DangerousPermissionSkip::Allow {
        a.push("--allow-dangerously-skip-permissions".into());
    }
    if let Some(fallback) = options
        .fallback_model
        .as_ref()
        .filter(|model| !model.as_str().is_empty())
    {
        if options
            .model
            .as_ref()
            .is_some_and(|model| !model.as_str().is_empty() && model.as_str() == fallback.as_str())
        {
            return Err(OptionError::new(
                "Fallback model cannot be the same as the main model. Please specify a different model for fallbackModel option.",
            )
            .into());
        }
        equals(&mut a, "--fallback-model", fallback.as_str());
    }
    if options.include_hook_events == HookEvents::Include {
        a.push("--include-hook-events".into());
    }
    if options.partial_messages == PartialMessages::Include {
        a.push("--include-partial-messages".into());
    }
    if options.session_store.is_some() {
        a.push("--session-mirror".into());
    }
    if let Some(root) = &options.project_config_root {
        equals(&mut a, "--project-config-root", root);
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
    if let Some(at) = options.resume_session_at.as_deref().filter(|at| !at.is_empty()) {
        equals(&mut a, "--resume-session-at", at);
    }
    if let Some(turn) = options.resume_drops_turn.as_deref() {
        equals(&mut a, "--resume-drops-turn", turn);
    }
    if let Some(session) = options.session_id.as_ref().filter(|id| !id.as_str().is_empty()) {
        equals(&mut a, "--session-id", session.as_str());
    }
    if options.session_persistence == SessionPersistence::Disabled {
        a.push("--no-session-persistence".into());
    }
    if crate::settings_option::managed_via_initialize(options) {
        if !crate::driver::plugins_via_initialize(options) {
            a.push("--await-initialize".into());
        }
        a.push("--managed-settings-in-initialize".into());
    } else if let Some(managed) = crate::settings_option::managed_settings_json(options)? {
        push_spaced(&mut a, "--managed-settings", managed);
    }
    if options.customizations == Customizations::Bare {
        a.push("--bare".into());
    }
    push_entries(&mut a, crate::settings_option::typescript_extra_entries(options)?);
    Ok(a)
}

fn python_thinking(a: &mut Vec<OsString>, options: &ClaudeAgentOptions) -> Result<(), Error> {
    // `thinking` takes precedence over the deprecated `max_thinking_tokens`.
    let Some(thinking) = options.thinking else {
        if let Some(budget) = options.max_thinking_tokens {
            a.push("--max-thinking-tokens".into());
            a.push(budget.to_string().into());
        }
        return Ok(());
    };
    let display = match thinking {
        ThinkingConfig::Adaptive { display } => {
            a.push("--thinking".into());
            a.push("adaptive".into());
            display
        }
        ThinkingConfig::Enabled { budget_tokens, display } => {
            // Python's `budget_tokens` is a required key, so its absence is a `KeyError` upstream.
            let budget = budget_tokens.ok_or_else(|| OptionError::new("'budget_tokens'"))?;
            a.push("--max-thinking-tokens".into());
            a.push(budget.to_string().into());
            display
        }
        ThinkingConfig::Disabled => {
            a.push("--thinking".into());
            a.push("disabled".into());
            None
        }
    };
    if let Some(display) = display {
        a.push("--thinking-display".into());
        a.push(display.as_str().into());
    }
    Ok(())
}

fn python<R>(options: &ClaudeAgentOptions, files: &R) -> Result<Vec<OsString>, Error>
where
    R: ReadFile<Error = std::io::Error>,
{
    let allowed_tools = crate::skills::effective_allowed_tools(options)?;
    let setting_sources = crate::skills::effective_setting_sources(options);
    let mut a: Vec<OsString> = ["--output-format", "stream-json", "--verbose"]
        .map(OsString::from)
        .into();
    a.extend(options.system_prompt.python_argv()?);
    if let ToolSet::Only(tools) = &options.tools {
        a.push("--tools".into());
        a.push(joined(tools.iter().map(|t| t.as_str())).into());
    }
    if !allowed_tools.is_empty() {
        a.push("--allowedTools".into());
        a.push(joined(&allowed_tools).into());
    }
    if let Some(turns) = options.max_turns {
        a.push("--max-turns".into());
        a.push(turns.to_string().into());
    }
    if let Some(budget) = &options.max_budget_usd {
        a.push("--max-budget-usd".into());
        a.push(budget.python_text().into());
    }
    if !options.disallowed_tools.is_empty() {
        a.push("--disallowedTools".into());
        a.push(joined(options.disallowed_tools.iter().map(|r| r.as_str())).into());
    }
    if let Some(budget) = options.task_budget {
        a.push("--task-budget".into());
        a.push(budget.total.to_string().into());
    }
    if let Some(model) = &options.model {
        a.push("--model".into());
        a.push(model.as_str().into());
    }
    if let Some(agent) = &options.agent {
        a.push("--agent".into());
        a.push(agent.as_str().into());
    }
    if let Some(fallback) = options
        .fallback_model
        .as_ref()
        .filter(|model| !model.as_str().is_empty())
    {
        a.push("--fallback-model".into());
        a.push(fallback.as_str().into());
    }
    if !options.betas.is_empty() {
        a.push("--betas".into());
        a.push(joined(options.betas.iter().map(|beta| beta.as_str())).into());
    }
    // Python routes a `can_use_tool` callback through `permission_prompt_tool_name="stdio"`.
    if options.permission_handler.is_some() {
        a.push("--permission-prompt-tool".into());
        a.push("stdio".into());
    } else if let Some(name) = options
        .permission_prompt_tool_name
        .as_deref()
        .filter(|name| !name.is_empty())
    {
        a.push("--permission-prompt-tool".into());
        a.push(name.into());
    }
    if let Some(mode) = options.permission_mode.as_ref() {
        a.push("--permission-mode".into());
        a.push(mode.as_str().into());
    }
    if options.continue_conversation == crate::ContinueConversation::Yes {
        a.push("--continue".into());
    }
    reject_windows_values(options)?;
    if let Some(session) = &options.resume {
        equals(&mut a, "--resume", session.to_string());
    }
    if let Some(session) = options.session_id.as_ref().filter(|id| !id.as_str().is_empty()) {
        equals(&mut a, "--session-id", session.as_str());
    }
    if let Some(settings) = crate::settings_option::python_settings_value(options, files)? {
        a.push("--settings".into());
        a.push(settings.into());
    }
    for directory in &options.additional_directories {
        a.push("--add-dir".into());
        a.push(directory.into());
    }
    if let Some(config) = mcp_config(options, ArgvStyle::Python)? {
        a.push("--mcp-config".into());
        a.push(config.into());
    }
    if let Some(config) = &options.mcp_config {
        a.push("--mcp-config".into());
        a.push(config.into());
    }
    if options.partial_messages == PartialMessages::Include {
        a.push("--include-partial-messages".into());
    }
    if options.include_hook_events == HookEvents::Include {
        a.push("--include-hook-events".into());
    }
    if options.mcp_discovery == crate::McpDiscovery::Disabled {
        a.push("--strict-mcp-config".into());
    }
    if options.fork_session == crate::SessionFork::Fork {
        a.push("--fork-session".into());
    }
    if let Some(at) = options.resume_session_at.as_deref().filter(|at| !at.is_empty()) {
        equals(&mut a, "--resume-session-at", at);
    }
    if let Some(turn) = options.resume_drops_turn.as_deref() {
        equals(&mut a, "--resume-drops-turn", turn);
    }
    if options.session_store.is_some() {
        a.push("--session-mirror".into());
    }
    if let Some(sources) = &setting_sources {
        equals(&mut a, "--setting-sources", joined(sources));
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
    python_thinking(&mut a, options)?;
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
mod runtime_tests;
#[cfg(test)]
mod tests;
