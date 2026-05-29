//! Bridges `HostEvalContext` onto generated WIT `host` imports via `StoreHost`.

use crate::cel_builtins::CelBuiltinsError;

use super::bindings::LogLevel;
use super::runtime::trogon::mcp_policy::host::{HostFailure as WitHostFailure, HostWithStore, LogLevel as WitLogLevel};
use super::wasi_stub::StoreHost;

impl HostWithStore for StoreHost {
    fn spicedb_check<T>(
        mut host: wasmtime::component::Access<T, Self>,
        subject: String,
        permission: String,
        object_id: String,
    ) -> Result<bool, WitHostFailure> {
        let state = host.get();
        state
            .with_host(|ctx| ctx.spicedb_check(&subject, &permission, &object_id))
            .map_err(map_host_failure)?
            .map_err(map_cel_error)
    }

    fn cache_get<T>(
        mut host: wasmtime::component::Access<T, Self>,
        key: String,
    ) -> Option<Vec<u8>> {
        let state = host.get();
        state
            .with_host(|ctx| {
                ctx.cache_get(&key)
                    .ok()
                    .flatten()
                    .and_then(|value| serde_json::to_vec(&value).ok())
            })
            .ok()
            .flatten()
    }

    fn cache_set<T>(
        mut host: wasmtime::component::Access<T, Self>,
        key: String,
        value: Vec<u8>,
        ttl_secs: u32,
    ) -> Result<(), WitHostFailure> {
        let state = host.get();
        state
            .with_host(|ctx| {
                let json = serde_json::from_slice::<serde_json::Value>(&value)
                    .map_err(|err| cel_policy_fault("cache.set", err.to_string()))?;
                ctx.cache_set(&key, json, u64::from(ttl_secs))
                    .map(|_| ())
            })
            .map_err(map_host_failure)?
            .map_err(map_cel_error)
    }

    fn audit_emit<T>(
        mut host: wasmtime::component::Access<T, Self>,
        category: String,
        fields_json: String,
    ) -> Result<(), WitHostFailure> {
        let state = host.get();
        state
            .with_host(|ctx| {
                let value: serde_json::Value = serde_json::from_str(&fields_json)
                    .map_err(|err| cel_policy_fault("audit.emit", err.to_string()))?;
                let map = value.as_object().cloned().ok_or_else(|| {
                    cel_policy_fault("audit.emit", "fields-json must be a JSON object")
                })?;
                let mut fields = std::collections::BTreeMap::new();
                fields.insert("category".into(), serde_json::Value::String(category));
                for (key, val) in map {
                    fields.insert(key, val);
                }
                ctx.audit_emit(fields).map(|_| ())
            })
            .map_err(map_host_failure)?
            .map_err(map_cel_error)
    }

    fn time_now<T>(mut host: wasmtime::component::Access<T, Self>) -> u64 {
        let state = host.get();
        state
            .with_host(|ctx| ctx.now_unix_ms().max(0) as u64)
            .unwrap_or(0)
    }

    fn rate_acquire<T>(
        mut host: wasmtime::component::Access<T, Self>,
        scope: String,
        key: String,
        budget: u32,
        window_secs: u32,
    ) -> Result<bool, WitHostFailure> {
        let state = host.get();
        state
            .with_host(|ctx| {
                ctx.rate_acquire(
                    &scope,
                    &key,
                    budget,
                    std::time::Duration::from_secs(u64::from(window_secs)),
                )
            })
            .map_err(map_host_failure)?
            .map_err(map_cel_error)
    }

    fn jsonpath_read<T>(
        mut host: wasmtime::component::Access<T, Self>,
        document_json: String,
        path: String,
    ) -> Result<String, WitHostFailure> {
        let state = host.get();
        state
            .with_host(|_| jsonpath_read_impl(&document_json, &path))
            .map_err(map_host_failure)?
    }

    fn jsonpath_has<T>(
        mut host: wasmtime::component::Access<T, Self>,
        document_json: String,
        path: String,
    ) -> Result<bool, WitHostFailure> {
        let state = host.get();
        state
            .with_host(|_| jsonpath_has_impl(&document_json, &path))
            .map_err(map_host_failure)?
    }

    fn log<T>(
        mut host: wasmtime::component::Access<T, Self>,
        level: WitLogLevel,
        message: String,
    ) -> Result<(), WitHostFailure> {
        let mapped = map_log_level(level);
        let state = host.get();
        let component_id = std::sync::Arc::clone(&state.component_id);
        let instance_id = state.instance_id;
        state
            .with_host(|_| {
                match mapped {
                    LogLevel::Trace => tracing::event!(
                        target: "trogon_mcp_gateway::wasm",
                        tracing::Level::TRACE,
                        component_id = %component_id,
                        instance_id = instance_id,
                        "{message}"
                    ),
                    LogLevel::Debug => tracing::event!(
                        target: "trogon_mcp_gateway::wasm",
                        tracing::Level::DEBUG,
                        component_id = %component_id,
                        instance_id = instance_id,
                        "{message}"
                    ),
                    LogLevel::Info => tracing::event!(
                        target: "trogon_mcp_gateway::wasm",
                        tracing::Level::INFO,
                        component_id = %component_id,
                        instance_id = instance_id,
                        "{message}"
                    ),
                    LogLevel::Warn => tracing::event!(
                        target: "trogon_mcp_gateway::wasm",
                        tracing::Level::WARN,
                        component_id = %component_id,
                        instance_id = instance_id,
                        "{message}"
                    ),
                    LogLevel::Error => tracing::event!(
                        target: "trogon_mcp_gateway::wasm",
                        tracing::Level::ERROR,
                        component_id = %component_id,
                        instance_id = instance_id,
                        "{message}"
                    ),
                }
                Ok(())
            })
            .map_err(map_host_failure)?
            .map_err(map_host_failure_inner)
    }

    fn span_attribute_set<T>(
        mut host: wasmtime::component::Access<T, Self>,
        _key: String,
        _value: String,
    ) -> Result<(), WitHostFailure> {
        let state = host.get();
        state.with_host(|_| Ok(())).map_err(map_host_failure)?
    }

    fn span_event<T>(
        mut host: wasmtime::component::Access<T, Self>,
        _name: String,
        _attributes_json: String,
    ) -> Result<(), WitHostFailure> {
        let state = host.get();
        state.with_host(|_| Ok(())).map_err(map_host_failure)?
    }
}

fn map_log_level(level: WitLogLevel) -> LogLevel {
    match level {
        WitLogLevel::Trace => LogLevel::Trace,
        WitLogLevel::Debug => LogLevel::Debug,
        WitLogLevel::Info => LogLevel::Info,
        WitLogLevel::Warn => LogLevel::Warn,
        WitLogLevel::Error => LogLevel::Error,
    }
}

fn map_host_failure(err: super::bindings::HostFailure) -> WitHostFailure {
    WitHostFailure {
        code: err.code,
        message: err.message,
    }
}

fn map_host_failure_inner(err: super::bindings::HostFailure) -> WitHostFailure {
    map_host_failure(err)
}

fn map_cel_error(err: CelBuiltinsError) -> WitHostFailure {
    WitHostFailure {
        code: match err.host_failure() {
            Some(crate::cel_builtins::HostFailure::Transient) => "spicedb_unavailable".into(),
            Some(crate::cel_builtins::HostFailure::Permanent) => "policy_fault".into(),
            Some(crate::cel_builtins::HostFailure::NotApplicable) => "not_applicable".into(),
            None => "policy_fault".into(),
        },
        message: err.to_string(),
    }
}

fn cel_policy_fault(name: &'static str, detail: impl Into<String>) -> CelBuiltinsError {
    CelBuiltinsError::policy_fault(name, detail)
}

fn jsonpath_read_impl(document_json: &str, path: &str) -> Result<String, WitHostFailure> {
    let doc: serde_json::Value = serde_json::from_str(document_json).map_err(|err| WitHostFailure {
        code: "jsonpath_invalid_document".into(),
        message: err.to_string(),
    })?;
    let matches = eval_jsonpath(&doc, path).map_err(|msg| WitHostFailure {
        code: "jsonpath_invalid".into(),
        message: msg,
    })?;
    match matches.len() {
        0 => Err(WitHostFailure {
            code: "jsonpath_no_match".into(),
            message: "jsonpath no match".into(),
        }),
        1 => serde_json::to_string(matches.first().expect("one match")).map_err(|err| WitHostFailure {
            code: "jsonpath_encode".into(),
            message: err.to_string(),
        }),
        _ => Err(WitHostFailure {
            code: "jsonpath_ambiguous".into(),
            message: "jsonpath_ambiguous".into(),
        }),
    }
}

fn jsonpath_has_impl(document_json: &str, path: &str) -> Result<bool, WitHostFailure> {
    let doc: serde_json::Value = serde_json::from_str(document_json).map_err(|err| WitHostFailure {
        code: "jsonpath_invalid_document".into(),
        message: err.to_string(),
    })?;
    let matches = eval_jsonpath(&doc, path).map_err(|msg| WitHostFailure {
        code: "jsonpath_invalid".into(),
        message: msg,
    })?;
    Ok(!matches.is_empty())
}

fn eval_jsonpath(doc: &serde_json::Value, path: &str) -> Result<Vec<serde_json::Value>, String> {
    if !path.starts_with('$') {
        return Err("jsonpath must start with $".into());
    }
    if path == "$" {
        return Ok(vec![doc.clone()]);
    }
    let pointer = to_json_pointer(path).ok_or_else(|| "invalid jsonpath".to_string())?;
    Ok(match doc.pointer(&pointer) {
        Some(value) => vec![value.clone()],
        None => Vec::new(),
    })
}

fn to_json_pointer(path: &str) -> Option<String> {
    let stripped = path.strip_prefix('$')?;
    if stripped.is_empty() || stripped == "." {
        return Some(String::new());
    }
    let stripped = stripped.strip_prefix('.').unwrap_or(stripped);
    let mut pointer = String::from("/");
    for segment in stripped.split('.') {
        if segment.is_empty() {
            continue;
        }
        pointer.push_str(&segment.replace('~', "~0").replace('/', "~1"));
        pointer.push('/');
    }
    Some(pointer.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::cel_builtins::{HostEvalContext, SpicedbHostBackend};
    use crate::wasm::store_state::WasmStoreState;

    struct AllowBackend;

    impl SpicedbHostBackend for AllowBackend {
        fn check(
            &self,
            _subject: &str,
            _permission: &str,
            _resource: &str,
        ) -> Result<bool, crate::cel_builtins::HostFailure> {
            Ok(true)
        }
    }

    #[test]
    fn host_spicedb_check_callthrough() {
        let host = HostEvalContext::for_tests().with_spicedb(Arc::new(AllowBackend));
        let mut state = WasmStoreState::new(
            host,
            super::super::config::PoolConfig::for_tests(),
            Arc::from("demo"),
            1,
        );
        let allowed = state
            .with_host(|ctx| ctx.spicedb_check("user:alice", "view", "tool:demo"))
            .expect("host")
            .expect("spicedb");
        assert!(allowed);
    }
}
