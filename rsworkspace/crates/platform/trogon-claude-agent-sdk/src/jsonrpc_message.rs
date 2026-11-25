//! `mcp.types.jsonrpc_message_adapter.validate_python(message, by_name=False)`: the pydantic union
//! `JSONRPCRequest | JSONRPCNotification | JSONRPCResponse | JSONRPCError` the Python `mcp` package reads every
//! message with, and the `ValidationError` text it raises for a message none of the four accepts.

use crate::{
    JsonObject, constants,
    pydantic_int_string::{IntStringFailure, read_int},
    pydantic_validation_error::{ErrorInput, LineError, render},
};
use serde_json::Value;

/// The model a message validated as.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JsonRpcKind {
    Request,
    Notification,
    /// A `JSONRPCResponse` or a `JSONRPCError`.
    Response,
}

/// The model pydantic picks, or the text of the `ValidationError` it raises. More than one model can accept a
/// message; an error response wins over a request, which wins over a response, which wins over a notification.
pub(crate) fn parse(message: &JsonObject) -> Result<JsonRpcKind, String> {
    let request = request_errors(message);
    let notification = notification_errors(message);
    let response = response_errors(message);
    let error = error_errors(message);
    if error.is_empty() {
        return Ok(JsonRpcKind::Response);
    }
    if request.is_empty() {
        return Ok(JsonRpcKind::Request);
    }
    if response.is_empty() {
        return Ok(JsonRpcKind::Response);
    }
    if notification.is_empty() {
        return Ok(JsonRpcKind::Notification);
    }
    let errors = [request, notification, response, error].concat();
    Err(render(
        constants::PYDANTIC_JSONRPC_MESSAGE_TITLE,
        &errors,
        &Value::Object(message.clone()),
    ))
}

fn request_errors(message: &JsonObject) -> Vec<LineError> {
    const MODEL: &str = "JSONRPCRequest";
    let mut errors = Vec::new();
    literal(MODEL, message, &mut errors);
    request_id(MODEL, message, false, &mut errors);
    method(MODEL, message, &mut errors);
    params(MODEL, message, &mut errors);
    errors
}

fn notification_errors(message: &JsonObject) -> Vec<LineError> {
    const MODEL: &str = "JSONRPCNotification";
    let mut errors = Vec::new();
    literal(MODEL, message, &mut errors);
    method(MODEL, message, &mut errors);
    params(MODEL, message, &mut errors);
    errors
}

fn response_errors(message: &JsonObject) -> Vec<LineError> {
    const MODEL: &str = "JSONRPCResponse";
    let mut errors = Vec::new();
    literal(MODEL, message, &mut errors);
    request_id(MODEL, message, false, &mut errors);
    match message.get("result") {
        None => errors.push(missing(vec![MODEL, "result"])),
        Some(Value::Object(_)) => {}
        Some(other) => errors.push(not_a_dict(vec![MODEL, "result"], other)),
    }
    errors
}

fn error_errors(message: &JsonObject) -> Vec<LineError> {
    const MODEL: &str = "JSONRPCError";
    let mut errors = Vec::new();
    literal(MODEL, message, &mut errors);
    request_id(MODEL, message, true, &mut errors);
    match message.get("error") {
        None => errors.push(missing(vec![MODEL, "error"])),
        Some(Value::Object(data)) => error_data(MODEL, data, &mut errors),
        Some(other) => errors.push(LineError {
            location: vec![MODEL, "error"],
            kind: "model_type",
            message: "Input should be a valid dictionary or instance of ErrorData",
            input: ErrorInput::Value(other.clone()),
        }),
    }
    errors
}

fn missing(location: Vec<&'static str>) -> LineError {
    LineError {
        location,
        kind: "missing",
        message: "Field required",
        input: ErrorInput::Whole,
    }
}

fn not_a_dict(location: Vec<&'static str>, value: &Value) -> LineError {
    LineError {
        location,
        kind: "dict_type",
        message: "Input should be a valid dictionary",
        input: ErrorInput::Value(value.clone()),
    }
}

fn not_a_string(location: Vec<&'static str>, value: &Value) -> LineError {
    LineError {
        location,
        kind: "string_type",
        message: "Input should be a valid string",
        input: ErrorInput::Value(value.clone()),
    }
}

/// `jsonrpc: Literal["2.0"]`.
fn literal(model: &'static str, message: &JsonObject, errors: &mut Vec<LineError>) {
    match message.get("jsonrpc") {
        None => errors.push(missing(vec![model, "jsonrpc"])),
        Some(Value::String(version)) if version == "2.0" => {}
        Some(other) => errors.push(LineError {
            location: vec![model, "jsonrpc"],
            kind: "literal_error",
            message: "Input should be '2.0'",
            input: ErrorInput::Value(other.clone()),
        }),
    }
}

/// `id: Annotated[int, Field(strict=True)] | str`, or that or `None` for `JSONRPCError`.
fn request_id(model: &'static str, message: &JsonObject, nullable: bool, errors: &mut Vec<LineError>) {
    let Some(id) = message.get("id") else {
        errors.push(missing(vec![model, "id"]));
        return;
    };
    let accepted = match id {
        Value::String(_) => true,
        Value::Number(number) => number.is_i64() || number.is_u64(),
        Value::Null => nullable,
        _ => false,
    };
    if !accepted {
        errors.push(LineError {
            location: vec![model, "id", "int"],
            kind: "int_type",
            message: "Input should be a valid integer",
            input: ErrorInput::Value(id.clone()),
        });
        errors.push(not_a_string(vec![model, "id", "str"], id));
    }
}

/// `method: str`.
fn method(model: &'static str, message: &JsonObject, errors: &mut Vec<LineError>) {
    match message.get("method") {
        None => errors.push(missing(vec![model, "method"])),
        Some(Value::String(_)) => {}
        Some(other) => errors.push(not_a_string(vec![model, "method"], other)),
    }
}

/// `params: dict[str, Any] | None = None`.
fn params(model: &'static str, message: &JsonObject, errors: &mut Vec<LineError>) {
    match message.get("params") {
        None | Some(Value::Null | Value::Object(_)) => {}
        Some(other) => errors.push(not_a_dict(vec![model, "params"], other)),
    }
}

/// `ErrorData`: an `int` code (lax, so numeric strings, whole floats and booleans are read), a `str` message and
/// any `data`.
fn error_data(model: &'static str, data: &JsonObject, errors: &mut Vec<LineError>) {
    match data.get("code") {
        None => errors.push(LineError {
            location: vec![model, "error", "code"],
            kind: "missing",
            message: "Field required",
            input: ErrorInput::Value(Value::Object(data.clone())),
        }),
        Some(code) => {
            if let Some((kind, text)) = lax_int_failure(code) {
                errors.push(LineError {
                    location: vec![model, "error", "code"],
                    kind,
                    message: text,
                    input: ErrorInput::Value(code.clone()),
                });
            }
        }
    }
    match data.get("message") {
        None => errors.push(LineError {
            location: vec![model, "error", "message"],
            kind: "missing",
            message: "Field required",
            input: ErrorInput::Value(Value::Object(data.clone())),
        }),
        Some(Value::String(_)) => {}
        Some(other) => errors.push(not_a_string(vec![model, "error", "message"], other)),
    }
}

/// The error pydantic's lax `int` raises for the value, if any.
fn lax_int_failure(value: &Value) -> Option<(&'static str, &'static str)> {
    const PARSING: (&str, &str) = (
        "int_parsing",
        "Input should be a valid integer, unable to parse string as an integer",
    );
    const SIZE: (&str, &str) = (
        "int_parsing_size",
        "Unable to parse input string as an integer, exceeded maximum size",
    );
    const FRACTION: (&str, &str) = (
        "int_from_float",
        "Input should be a valid integer, got a number with a fractional part",
    );
    match value {
        Value::Bool(_) => None,
        Value::Number(number) => match number.as_f64().filter(|_| number.is_f64()) {
            None => None,
            Some(float) if float.fract() != 0.0 => Some(FRACTION),
            Some(float) if float.abs() >= 9_223_372_036_854_775_808.0 => Some(SIZE),
            Some(_) => None,
        },
        Value::String(text) => match read_int(text) {
            Ok(()) => None,
            Err(IntStringFailure::Parsing) => Some(PARSING),
            Err(IntStringFailure::Size) => Some(SIZE),
        },
        _ => Some(("int_type", "Input should be a valid integer")),
    }
}

#[cfg(test)]
#[path = "jsonrpc_message_tests.rs"]
mod tests;
