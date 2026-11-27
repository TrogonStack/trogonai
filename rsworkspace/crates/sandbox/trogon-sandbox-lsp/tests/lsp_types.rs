use serde_json::{Value, json};
use trogon_sandbox_lsp::{
    CompletionResponse, ConfigurationItem, DocumentSymbols, InitializeResult, LspPreset, LspPresetName, LspServerSpec,
    Position,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn position_serializes_as_lsp_expects() -> TestResult {
    assert_eq!(
        serde_json::to_value(Position::new(5, 8))?,
        json!({ "line": 5, "character": 8 })
    );
    Ok(())
}

#[test]
fn completion_response_accepts_list_and_array() -> TestResult {
    let list: CompletionResponse =
        serde_json::from_value(json!({ "isIncomplete": false, "items": [{ "label": "a" }] }))?;
    let array: CompletionResponse = serde_json::from_value(json!([{ "label": "a" }]))?;
    assert_eq!(list.into_items()[0].label, "a");
    assert_eq!(array.into_items()[0].label, "a");
    Ok(())
}

#[test]
fn document_symbols_accept_hierarchical_and_flat() -> TestResult {
    let range = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } });
    let hier: DocumentSymbols = serde_json::from_value(json!([
        { "name": "greet", "kind": 12, "range": range, "selectionRange": range }
    ]))?;
    let flat: DocumentSymbols = serde_json::from_value(json!([
        { "name": "msg", "kind": 13, "location": { "uri": "file:///a", "range": range } }
    ]))?;
    assert_eq!(hier.names(), vec!["greet"]);
    assert_eq!(flat.names(), vec!["msg"]);
    Ok(())
}

#[test]
fn initialize_result_keeps_the_server_info() -> TestResult {
    let result: InitializeResult = serde_json::from_value(json!({
        "capabilities": { "hoverProvider": true },
        "serverInfo": { "name": "ls", "version": "1.0" }
    }))?;
    let info = result.server_info.ok_or("no server info")?;
    assert_eq!((info.name.as_str(), info.version.as_deref()), ("ls", Some("1.0")));
    assert_eq!(result.capabilities["hoverProvider"], Value::Bool(true));
    Ok(())
}

#[test]
fn configuration_items_default_to_no_scope_and_no_section() -> TestResult {
    let item: ConfigurationItem = serde_json::from_value(json!({}))?;
    assert_eq!(item, ConfigurationItem::default());
    Ok(())
}

#[test]
fn presets_expand_to_specs() {
    let ts = LspServerSpec::from(LspPresetName::TypeScript);
    assert_eq!(ts.command, "typescript-language-server");
    assert_eq!(ts.args, vec!["--stdio"]);
    assert_eq!(ts.language_id.as_deref(), Some("typescript"));
    let py = LspServerSpec::from(LspPresetName::Python);
    assert_eq!(py.command, "pyright-langserver");
    assert_eq!(py.language_id.as_deref(), Some("python"));
    assert_eq!(LspServerSpec::from(&LspPreset::PYTHON), py);
}

#[test]
fn a_custom_spec_is_built_from_its_command() {
    let spec = LspServerSpec::new("rust-analyzer").args(["--log"]).language_id("rust");
    assert_eq!(spec.command, "rust-analyzer");
    assert_eq!(spec.args, vec!["--log"]);
    assert_eq!(spec.language_id.as_deref(), Some("rust"));
}
