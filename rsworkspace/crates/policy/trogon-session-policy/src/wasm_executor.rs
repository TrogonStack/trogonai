use crate::WasmTool;

/// Verify the component digest and instantiate a fresh guest with exactly these grants.
/// The worker must reject unsupported grants and enforce resource, file and address confinement.
#[async_trait::async_trait]
pub trait WasmExecutor: Send {
    type Output;
    type Error: std::error::Error + Send + Sync + 'static;
    async fn wasm(&mut self, tool: &WasmTool, content: &str) -> Result<Self::Output, Self::Error>;
}
