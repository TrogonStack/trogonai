use serde::{Deserialize, Serialize};

use crate::jsonrpc::JsonRpcId;

#[derive(Debug, Serialize)]
pub struct JsonRpcRequest<P> {
    pub jsonrpc: &'static str,
    pub id: JsonRpcId,
    pub method: &'static str,
    pub params: P,
}

impl<P> JsonRpcRequest<P> {
    pub fn new(id: JsonRpcId, method: &'static str, params: P) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method,
            params,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcResponse<R> {
    Success(JsonRpcSuccess<R>),
    Error(JsonRpcErrorEnvelope),
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcSuccess<R> {
    pub result: R,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcErrorEnvelope {
    pub error: JsonRpcErrorBody,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcErrorBody {
    pub code: i32,
    pub message: String,
}

#[cfg(test)]
mod tests;
