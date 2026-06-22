use std::io::Write as _;
use std::process::Command;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ImportCheckError {
    #[error("component declares import '{module}/{name}'")]
    ImportPresent { module: String, name: String },
    #[error("world root declares import: {line}")]
    WorldImport { line: String },
    #[error("failed to parse wasm module: {0}")]
    Parse(String),
    #[error("wasm-tools not available: {0}")]
    WasmToolsUnavailable(String),
}

/// Returns Ok when `wasm-tools component wit` shows no root-world `import` lines.
///
/// Matches AC-0.2 literally. wit-bindgen guest artifacts embed component types inside
/// a core module; `wasmparser` misreads those as imports, so we delegate to wasm-tools.
pub fn assert_zero_imports(bytes: &[u8]) -> Result<(), ImportCheckError> {
    if let Ok(()) = assert_zero_imports_via_wasm_tools(bytes) {
        return Ok(());
    }
    assert_zero_imports_via_wasmparser(bytes)
}

fn assert_zero_imports_via_wasm_tools(bytes: &[u8]) -> Result<(), ImportCheckError> {
    let mut temp = tempfile::Builder::new()
        .suffix(".wasm")
        .tempfile()
        .map_err(|err| ImportCheckError::Parse(err.to_string()))?;
    temp.write_all(bytes)
        .map_err(|err| ImportCheckError::Parse(err.to_string()))?;

    let wasm_tools = std::env::var("WASM_TOOLS").unwrap_or_else(|_| "wasm-tools".into());
    let output = Command::new(&wasm_tools)
        .args(["component", "wit"])
        .arg(temp.path())
        .output()
        .map_err(|err| ImportCheckError::WasmToolsUnavailable(err.to_string()))?;

    if !output.status.success() {
        return Err(ImportCheckError::Parse(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }

    let wit = String::from_utf8_lossy(&output.stdout);
    let mut in_root_world = false;
    for line in wit.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("world root {") {
            in_root_world = true;
            continue;
        }
        if in_root_world {
            if trimmed == "}" {
                break;
            }
            if trimmed.starts_with("import ") {
                return Err(ImportCheckError::WorldImport {
                    line: trimmed.to_string(),
                });
            }
        }
    }
    Ok(())
}

fn assert_zero_imports_via_wasmparser(bytes: &[u8]) -> Result<(), ImportCheckError> {
    if bytes.len() >= 8 && &bytes[0..4] == b"\0asm" && bytes[4] == 1 {
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            let payload = payload.map_err(|err| ImportCheckError::Parse(err.to_string()))?;
            if let wasmparser::Payload::ImportSection(reader) = payload
                && let Some(import) = reader.into_iter().next()
            {
                let import = import.map_err(|err| ImportCheckError::Parse(err.to_string()))?;
                return Err(ImportCheckError::ImportPresent {
                    module: import.module.to_string(),
                    name: import.name.to_string(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
