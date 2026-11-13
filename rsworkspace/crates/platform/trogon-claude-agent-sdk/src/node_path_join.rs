//! Node's `path.join` for both platforms, which `extractFromBunfs` uses to build every path it touches, answered
//! by the shared `node_path` port.

use crate::node_path::{JsString, NodeError, NodePath, NodePathDialect, NodeProcess, js, rust};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PathStyle {
    Posix,
    Win32,
}

impl PathStyle {
    /// The style Node's `path` module has on the platform this crate was built for.
    pub(crate) const fn host() -> Self {
        if cfg!(windows) { Self::Win32 } else { Self::Posix }
    }
}

/// `path.join` reads neither the working directory nor the environment, so it runs against an empty process.
struct NoProcess;

impl NodeProcess for NoProcess {
    fn cwd(&self) -> Result<JsString, NodeError> {
        Ok(JsString::new())
    }

    fn env(&self, _key: &[u16]) -> Option<JsString> {
        None
    }
}

/// `path.posix.join` or `path.win32.join`: empty parts are skipped and nothing left gives `.`.
pub(crate) fn join(style: PathStyle, parts: &[&str]) -> String {
    let path = NodePath {
        dialect: NodePathDialect::Current,
        windows_host: style == PathStyle::Win32,
        process: &NoProcess,
    };
    let parts: Vec<JsString> = parts.iter().map(|part| js(part)).collect();
    let parts: Vec<&[u16]> = parts.iter().map(Vec::as_slice).collect();
    rust(&match style {
        PathStyle::Posix => path.posix_join(&parts),
        PathStyle::Win32 => path.win32_join(&parts),
    })
}

#[cfg(test)]
#[path = "node_path_join_tests.rs"]
mod tests;
