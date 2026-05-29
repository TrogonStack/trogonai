//! Per-instance store state shared by host and WASI import shims.

use std::sync::Arc;

use crate::cel_builtins::{HostEvalContext, with_host_eval};

use super::bindings::HostFailure;
use super::config::PoolConfig;

/// Store payload for a pooled component instance.
pub struct WasmStoreState {
    pub host: HostEvalContext,
    import_calls: u32,
    max_host_imports: u32,
    pub component_id: Arc<str>,
    pub instance_id: u64,
}

impl WasmStoreState {
    pub fn new(
        host: HostEvalContext,
        config: PoolConfig,
        component_id: Arc<str>,
        instance_id: u64,
    ) -> Self {
        Self {
            host,
            import_calls: 0,
            max_host_imports: config.max_host_imports,
            component_id,
            instance_id,
        }
    }

    pub fn with_host<R>(&mut self, f: impl FnOnce(&HostEvalContext) -> R) -> Result<R, HostFailure> {
        self.bump_import_calls()?;
        Ok(with_host_eval(&self.host, || f(&self.host)))
    }

    fn bump_import_calls(&mut self) -> Result<(), HostFailure> {
        self.import_calls = self.import_calls.saturating_add(1);
        if self.import_calls > self.max_host_imports {
            return Err(HostFailure {
                code: "wasm_import_limit".into(),
                message: format!("host import limit {} exceeded", self.max_host_imports),
            });
        }
        Ok(())
    }

    pub fn reset_import_calls(&mut self) {
        self.import_calls = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::cel_builtins::HostEvalContext;

    use super::*;
    use crate::wasm::config::PoolConfig;

    #[test]
    fn import_limit_enforced() {
        let config = PoolConfig {
            max_host_imports: 2,
            ..PoolConfig::for_tests()
        };
        let mut state = WasmStoreState::new(
            HostEvalContext::for_tests(),
            config,
            Arc::from("demo"),
            1,
        );
        state.with_host(|_| ()).expect("first");
        state.with_host(|_| ()).expect("second");
        let err = state.with_host(|_| ()).expect_err("third");
        assert_eq!(err.code, "wasm_import_limit");
    }
}
