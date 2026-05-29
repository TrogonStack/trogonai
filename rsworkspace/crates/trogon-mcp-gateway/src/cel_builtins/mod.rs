//! Host-side CEL builtins for MCP gateway policy evaluation.

mod audit;
mod cache;
mod context;
mod errors;
mod jsonpath;
mod rate;
mod spicedb;
mod time;
mod value;

pub use context::{with_host_eval, HostEvalContext, PolicyCache, PolicyRateLimiter};
pub use errors::{CelBuiltinsError, HostFailure};
pub use spicedb::{PermissionCheckerSpicedbBackend, SpicedbHostBackend};

use cel_interpreter::extractors::{Arguments, This};
use cel_interpreter::{Context, ExecutionError, FunctionContext, Value};
use std::sync::Arc;

fn namespace_marker(name: &'static str) -> Value {
    Value::String(Arc::new(format!("__trogon.cel_builtins.{name}")))
}

fn namespace_id(value: &Value) -> Option<&'static str> {
    match value {
        Value::String(marker) => match marker.as_str() {
            "__trogon.cel_builtins.spicedb" => Some("spicedb"),
            "__trogon.cel_builtins.cache" => Some("cache"),
            "__trogon.cel_builtins.jsonpath" => Some("jsonpath"),
            "__trogon.cel_builtins.audit" => Some("audit"),
            "__trogon.cel_builtins.time" => Some("time"),
            "__trogon.cel_builtins.rate" => Some("rate"),
            _ => None,
        },
        _ => None,
    }
}

fn builtin_error(ftx: &FunctionContext, err: CelBuiltinsError) -> ExecutionError {
    ftx.error(err.to_string())
}

fn cel_check(
    ftx: &FunctionContext,
    This(this): This<Value>,
    args: Arguments,
) -> Result<Value, ExecutionError> {
    if namespace_id(&this) != Some("spicedb") {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongType {
                name: spicedb::BUILTIN_NAME,
                position: 0,
                expected: "spicedb namespace receiver",
            },
        ));
    }
    if args.0.len() != 3 {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongArity {
                name: spicedb::BUILTIN_NAME,
                expected: 3,
                got: args.0.len(),
            },
        ));
    }
    spicedb::check(args.0[0].clone(), args.0[1].clone(), args.0[2].clone())
        .map_err(|err| builtin_error(ftx, err))
}

fn cel_get(
    ftx: &FunctionContext,
    This(this): This<Value>,
    args: Arguments,
) -> Result<Value, ExecutionError> {
    match namespace_id(&this) {
        Some("cache") => {
            if args.0.len() != 1 {
                return Err(builtin_error(
                    ftx,
                    CelBuiltinsError::WrongArity {
                        name: cache::GET_NAME,
                        expected: 1,
                        got: args.0.len(),
                    },
                ));
            }
            cache::get(args.0[0].clone()).map_err(|err| builtin_error(ftx, err))
        }
        Some("jsonpath") => {
            if args.0.len() != 2 {
                return Err(builtin_error(
                    ftx,
                    CelBuiltinsError::WrongArity {
                        name: jsonpath::GET_NAME,
                        expected: 2,
                        got: args.0.len(),
                    },
                ));
            }
            jsonpath::get(args.0[0].clone(), args.0[1].clone())
                .map_err(|err| builtin_error(ftx, err))
        }
        _ => Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongType {
                name: "get",
                position: 0,
                expected: "cache or jsonpath namespace receiver",
            },
        )),
    }
}

fn cel_set(
    ftx: &FunctionContext,
    This(this): This<Value>,
    args: Arguments,
) -> Result<Value, ExecutionError> {
    match namespace_id(&this) {
        Some("cache") => {
            if args.0.len() != 3 {
                return Err(builtin_error(
                    ftx,
                    CelBuiltinsError::WrongArity {
                        name: cache::SET_NAME,
                        expected: 3,
                        got: args.0.len(),
                    },
                ));
            }
            cache::set(args.0[0].clone(), args.0[1].clone(), args.0[2].clone())
                .map_err(|err| builtin_error(ftx, err))
        }
        Some("jsonpath") => {
            if args.0.len() != 3 {
                return Err(builtin_error(
                    ftx,
                    CelBuiltinsError::WrongArity {
                        name: jsonpath::SET_NAME,
                        expected: 3,
                        got: args.0.len(),
                    },
                ));
            }
            jsonpath::set(args.0[0].clone(), args.0[1].clone(), args.0[2].clone())
                .map_err(|err| builtin_error(ftx, err))
        }
        _ => Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongType {
                name: "set",
                position: 0,
                expected: "cache or jsonpath namespace receiver",
            },
        )),
    }
}

fn cel_delete(
    ftx: &FunctionContext,
    This(this): This<Value>,
    args: Arguments,
) -> Result<Value, ExecutionError> {
    if namespace_id(&this) != Some("jsonpath") {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongType {
                name: jsonpath::DELETE_NAME,
                position: 0,
                expected: "jsonpath namespace receiver",
            },
        ));
    }
    if args.0.len() != 2 {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongArity {
                name: jsonpath::DELETE_NAME,
                expected: 2,
                got: args.0.len(),
            },
        ));
    }
    jsonpath::delete(args.0[0].clone(), args.0[1].clone()).map_err(|err| builtin_error(ftx, err))
}

fn cel_query(
    ftx: &FunctionContext,
    This(this): This<Value>,
    args: Arguments,
) -> Result<Value, ExecutionError> {
    if namespace_id(&this) != Some("jsonpath") {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongType {
                name: jsonpath::QUERY_NAME,
                position: 0,
                expected: "jsonpath namespace receiver",
            },
        ));
    }
    if args.0.len() != 2 {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongArity {
                name: jsonpath::QUERY_NAME,
                expected: 2,
                got: args.0.len(),
            },
        ));
    }
    jsonpath::query(args.0[0].clone(), args.0[1].clone()).map_err(|err| builtin_error(ftx, err))
}

fn cel_extract(
    ftx: &FunctionContext,
    This(this): This<Value>,
    args: Arguments,
) -> Result<Value, ExecutionError> {
    if namespace_id(&this) != Some("jsonpath") {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongType {
                name: jsonpath::EXTRACT_NAME,
                position: 0,
                expected: "jsonpath namespace receiver",
            },
        ));
    }
    if args.0.len() != 2 {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongArity {
                name: jsonpath::EXTRACT_NAME,
                expected: 2,
                got: args.0.len(),
            },
        ));
    }
    jsonpath::extract(args.0[0].clone(), args.0[1].clone()).map_err(|err| builtin_error(ftx, err))
}

fn cel_has(
    ftx: &FunctionContext,
    This(this): This<Value>,
    args: Arguments,
) -> Result<Value, ExecutionError> {
    if namespace_id(&this) != Some("jsonpath") {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongType {
                name: jsonpath::HAS_NAME,
                position: 0,
                expected: "jsonpath namespace receiver",
            },
        ));
    }
    if args.0.len() != 2 {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongArity {
                name: jsonpath::HAS_NAME,
                expected: 2,
                got: args.0.len(),
            },
        ));
    }
    jsonpath::has(args.0[0].clone(), args.0[1].clone()).map_err(|err| builtin_error(ftx, err))
}

fn cel_emit(
    ftx: &FunctionContext,
    This(this): This<Value>,
    args: Arguments,
) -> Result<Value, ExecutionError> {
    if namespace_id(&this) != Some("audit") {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongType {
                name: audit::EMIT_NAME,
                position: 0,
                expected: "audit namespace receiver",
            },
        ));
    }
    if args.0.len() != 1 {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongArity {
                name: audit::EMIT_NAME,
                expected: 1,
                got: args.0.len(),
            },
        ));
    }
    audit::emit(args.0[0].clone()).map_err(|err| builtin_error(ftx, err))
}

fn cel_now(
    ftx: &FunctionContext,
    This(this): This<Value>,
    args: Arguments,
) -> Result<Value, ExecutionError> {
    if namespace_id(&this) != Some("time") {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongType {
                name: time::NOW_NAME,
                position: 0,
                expected: "time namespace receiver",
            },
        ));
    }
    if !args.0.is_empty() {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongArity {
                name: time::NOW_NAME,
                expected: 0,
                got: args.0.len(),
            },
        ));
    }
    time::now().map_err(|err| builtin_error(ftx, err))
}

fn cel_acquire(
    ftx: &FunctionContext,
    This(this): This<Value>,
    args: Arguments,
) -> Result<Value, ExecutionError> {
    if namespace_id(&this) != Some("rate") {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongType {
                name: rate::ACQUIRE_NAME,
                position: 0,
                expected: "rate namespace receiver",
            },
        ));
    }
    if args.0.len() != 4 {
        return Err(builtin_error(
            ftx,
            CelBuiltinsError::WrongArity {
                name: rate::ACQUIRE_NAME,
                expected: 4,
                got: args.0.len(),
            },
        ));
    }
    rate::acquire(
        args.0[0].clone(),
        args.0[1].clone(),
        args.0[2].clone(),
        args.0[3].clone(),
    )
    .map_err(|err| builtin_error(ftx, err))
}

/// Wires every host builtin into the CEL context paired with a compiled program at execution time.
pub fn register_all(ctx: &mut Context<'_>) -> Result<(), CelBuiltinsError> {
    ctx.add_variable_from_value("spicedb", namespace_marker("spicedb"));
    ctx.add_variable_from_value("cache", namespace_marker("cache"));
    ctx.add_variable_from_value("jsonpath", namespace_marker("jsonpath"));
    ctx.add_variable_from_value("audit", namespace_marker("audit"));
    ctx.add_variable_from_value("time", namespace_marker("time"));
    ctx.add_variable_from_value("rate", namespace_marker("rate"));

    ctx.add_function("check", cel_check);
    ctx.add_function("get", cel_get);
    ctx.add_function("set", cel_set);
    ctx.add_function("delete", cel_delete);
    ctx.add_function("query", cel_query);
    ctx.add_function("extract", cel_extract);
    ctx.add_function("has", cel_has);
    ctx.add_function("emit", cel_emit);
    ctx.add_function("now", cel_now);
    ctx.add_function("acquire", cel_acquire);

    Ok(())
}

/// Alias for [`register_all`].
pub fn register(ctx: &mut Context<'_>) -> Result<(), CelBuiltinsError> {
    register_all(ctx)
}

#[cfg(test)]
mod tests {
    use super::{register_all, with_host_eval, HostEvalContext};
    use cel_interpreter::{Context, Program, Value};

    fn ctx_with_builtins() -> Context<'static> {
        let mut ctx = Context::default();
        register_all(&mut ctx).unwrap();
        ctx
    }

    fn execute_with_host(source: &str) -> Result<Value, String> {
        let ctx = ctx_with_builtins();
        let host = HostEvalContext::for_tests();
        let program = Program::compile(source).map_err(|e| e.to_string())?;
        with_host_eval(&host, || program.execute(&ctx).map_err(|e| e.to_string()))
    }

    #[test]
    fn register_wires_spicedb_check() {
        let err = execute_with_host(r#"spicedb.check("user:alice", "invoke", "tool:github|create_issue")"#)
            .unwrap_err();
        assert!(err.contains("spicedb.check"));
    }

    #[test]
    fn register_wires_cache_get_miss_null() {
        let value = execute_with_host(r#"cache.get("key")"#).unwrap();
        assert_eq!(value, Value::Null);
    }

    #[test]
    fn register_wires_jsonpath_get() {
        let value = execute_with_host(r#"jsonpath.get({"a": 1}, "$.a")"#).unwrap();
        assert_eq!(value, Value::Int(1));
    }

    #[test]
    fn register_wires_audit_emit() {
        let value = execute_with_host(r#"audit.emit({"k": "v"})"#).unwrap();
        assert_eq!(value, Value::Bool(true));
    }

    #[test]
    fn register_wires_time_now() {
        let host = HostEvalContext::for_tests().with_clock_ms(std::sync::Arc::new(|| 123));
        let ctx = ctx_with_builtins();
        let program = Program::compile("time.now()").unwrap();
        let value = with_host_eval(&host, || program.execute(&ctx)).unwrap();
        assert_eq!(value, Value::Int(123));
    }

    #[test]
    fn register_wires_rate_acquire_local() {
        let value = execute_with_host(
            r#"rate.acquire("local", "k", 1, duration("60s")) && rate.acquire("local", "k", 1, duration("60s"))"#,
        )
        .unwrap();
        assert_eq!(value, Value::Bool(false));
    }
}
