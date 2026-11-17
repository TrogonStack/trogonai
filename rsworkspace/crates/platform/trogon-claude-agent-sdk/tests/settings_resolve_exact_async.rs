//! The async exact resolves read the project settings in a temporary directory through the native host with the
//! module state of this process, and return what the synchronous exact resolve returns for the same files with a
//! state that keeps the same launch directory, every lone surrogate kept.

use std::{fs, path::Path};

use serde_json::json;
use trogon_claude_agent_sdk::{
    PythonJsonValue, ResolveSettingsOptions, ResolverState, SettingsDialect,
    adapters::{NativeHost, capture_launch_directory},
    js_stringify_exact, python_json_dumps, resolve_settings_exact, resolve_settings_exact_in_dialect,
    resolve_settings_exact_in_dialect_with_state,
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// A project settings file whose `x` member holds a lone surrogate, a `null` and a boolean.
const PROJECT_SETTINGS: &str = r#"{"x":["a\udc00",null,true]}"#;

/// Resolve options that read only the project settings of `project`.
fn options(project: &Path) -> Result<ResolveSettingsOptions, Box<dyn std::error::Error + Send + Sync>> {
    let cwd = project.to_str().ok_or("the project path is not UTF-8")?;
    Ok(serde_json::from_value(
        json!({"cwd": cwd, "settingSources": ["project"]}),
    )?)
}

/// A temporary project whose `.claude/settings.json` holds [`PROJECT_SETTINGS`].
fn project() -> Result<tempfile::TempDir, std::io::Error> {
    let project = tempfile::tempdir()?;
    fs::create_dir(project.path().join(".claude"))?;
    fs::write(project.path().join(".claude/settings.json"), PROJECT_SETTINGS)?;
    Ok(project)
}

/// The synchronous exact resolve in `dialect` with the state the async resolves share, as a process that starts
/// now holds it.
fn resolve_now(
    options: &ResolveSettingsOptions,
    dialect: SettingsDialect,
) -> Result<PythonJsonValue, Box<dyn std::error::Error + Send + Sync>> {
    let state = ResolverState::new().with_launch_directory(capture_launch_directory());
    Ok(resolve_settings_exact_in_dialect_with_state(
        &NativeHost,
        options,
        dialect,
        &state,
    )?)
}

fn member_x(resolved: &PythonJsonValue) -> Option<&PythonJsonValue> {
    resolved.get("effective").and_then(|effective| effective.get("x"))
}

/// `resolve_settings_exact` resolves in the TypeScript dialect, so `x` prints as `JSON.stringify` prints it.
#[tokio::test]
async fn the_async_exact_resolve_keeps_lone_surrogates_as_the_typescript_resolve_does() -> TestResult {
    let project = project()?;
    let options = options(project.path())?;
    let exact = resolve_settings_exact(&options).await?;
    assert_eq!(
        member_x(&exact).map(js_stringify_exact),
        Some(r#"["a\udc00",null,true]"#.to_owned())
    );
    assert_eq!(exact, resolve_now(&options, SettingsDialect::TypeScript)?);
    Ok(())
}

/// `resolve_settings_exact_in_dialect` resolves in the chosen dialect: `x` prints as `JSON.stringify` prints it in
/// the TypeScript dialect and as `json.dumps` prints it in the Python dialect.
#[tokio::test]
async fn the_async_exact_resolve_in_each_dialect_is_the_synchronous_result() -> TestResult {
    let project = project()?;
    let options = options(project.path())?;
    let typescript = resolve_settings_exact_in_dialect(&options, SettingsDialect::TypeScript).await?;
    let python = resolve_settings_exact_in_dialect(&options, SettingsDialect::Python).await?;
    assert_eq!(
        (
            member_x(&typescript).map(js_stringify_exact),
            member_x(&python).map(python_json_dumps)
        ),
        (
            Some(r#"["a\udc00",null,true]"#.to_owned()),
            Some(r#"["a\udc00", null, true]"#.to_owned())
        )
    );
    assert_eq!(
        (typescript, python),
        (
            resolve_now(&options, SettingsDialect::TypeScript)?,
            resolve_now(&options, SettingsDialect::Python)?
        )
    );
    Ok(())
}
