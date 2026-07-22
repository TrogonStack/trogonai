use std::io::Write as _;
use std::process::Command;

use thiserror::Error;
use trogon_std::env::{ReadEnv as _, SystemEnv};

/// Failure verifying that a compiled component declares no imports.
#[derive(Debug, Error)]
pub enum ImportCheckError {
    /// The component's core wasm import section names an import (wasmparser fallback path).
    #[error("component declares import '{module}/{name}'")]
    ImportPresent {
        /// The imported module name.
        module: String,
        /// The imported item name within `module`.
        name: String,
    },
    /// The component's decompiled WIT declares an `import` line (wasm-tools path).
    #[error("component WIT declares import: {line}")]
    WorldImport {
        /// The offending `import` line, trimmed of leading whitespace.
        line: String,
    },
    /// The wasm bytes could not be parsed as a module or component.
    #[error("failed to parse wasm module: {0}")]
    Parse(String),
    /// The `wasm-tools` binary could not be located or invoked.
    #[error("wasm-tools not available: {0}")]
    WasmToolsUnavailable(String),
}

/// Returns Ok when `wasm-tools component wit` shows no root-world `import` lines.
///
/// Matches AC-0.2 literally. wit-bindgen guest artifacts embed component types inside
/// a core module; `wasmparser` misreads those as imports, so we delegate to wasm-tools.
pub fn assert_zero_imports(bytes: &[u8]) -> Result<(), ImportCheckError> {
    match assert_zero_imports_via_wasm_tools(bytes) {
        Ok(()) => Ok(()),
        // The wasmparser fallback only understands core modules; falling back for a component
        // when wasm-tools is missing/broken would return Ok and silently skip the check. Only
        // retry when the input actually is a core module, otherwise surface the failure.
        Err(_) if is_core_wasm_module(bytes) => assert_zero_imports_via_wasmparser(bytes),
        Err(error) => Err(error),
    }
}

fn is_core_wasm_module(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes[0..4] == *b"\0asm" && bytes[4..8] == [1, 0, 0, 0]
}

fn assert_zero_imports_via_wasm_tools(bytes: &[u8]) -> Result<(), ImportCheckError> {
    let mut temp = tempfile::Builder::new()
        .suffix(".wasm")
        .tempfile()
        .map_err(|err| ImportCheckError::Parse(err.to_string()))?;
    temp.write_all(bytes)
        .map_err(|err| ImportCheckError::Parse(err.to_string()))?;

    let wasm_tools = SystemEnv.var("WASM_TOOLS").unwrap_or_else(|_| "wasm-tools".into());
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
    // Mirror the CI guard (`grep -E '^[[:space:]]*import '`): a zero-import
    // component must not declare any `import` anywhere in its WIT. Scoping the
    // scan to a single `world root {` block missed imports listed under a
    // differently named world (the contract defines `world decider`) or outside
    // that block entirely.
    for line in wit.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("import ") {
            return Err(ImportCheckError::WorldImport {
                line: trimmed.to_string(),
            });
        }
    }
    Ok(())
}

fn assert_zero_imports_via_wasmparser(bytes: &[u8]) -> Result<(), ImportCheckError> {
    if bytes.len() >= 8 && &bytes[0..4] == b"\0asm" && bytes[4] == 1 {
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            let payload = payload.map_err(|err| ImportCheckError::Parse(err.to_string()))?;
            if let wasmparser::Payload::ImportSection(reader) = payload
                && let Some(import) = reader.into_imports().next()
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
