//! Bundled Claude Code executable lookup, ported from the pinned upstream SDKs.
//!
//! Python resolves `<package>/_bundled/claude[.exe]` before any other discovery (`_find_bundled_cli`).
//! TypeScript resolves the native binary from one of the `@anthropic-ai/claude-agent-sdk-<platform>-<arch>`
//! optional dependency packages through Node module resolution (`vJ`). This crate ships no binary and has no
//! Node runtime, so both lookups take the roots from the caller and nothing here runs during `connect`.

use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

use crate::{
    adapters::{CanonicalizePath, ReadFile, ReadMetadata},
    constants::{
        BUNDLED_CLI_VERSION_FILE, BUNDLED_DIRECTORY, DERIVED_CLIENT_APP_MINIMUM_CLAUDE_CODE_VERSION, PACKAGE_SCOPE_NAME,
    },
};

fn is_file<F: ReadMetadata>(filesystem: &F, path: &Path) -> bool {
    filesystem.metadata(path).is_ok_and(|metadata| metadata.is_file())
}

/// Python `_find_bundled_cli`: `<package_root>/_bundled/claude` (`claude.exe` on Windows), accepted only when it
/// is a file. `package_root` is the directory that holds the `_bundled` directory.
pub fn find_bundled_cli<F: ReadMetadata>(package_root: &Path, windows: bool, filesystem: &F) -> Option<PathBuf> {
    let candidate = package_root
        .join(BUNDLED_DIRECTORY)
        .join(if windows { "claude.exe" } else { "claude" });
    is_file(filesystem, &candidate).then_some(candidate)
}

/// The leading `major.minor.patch` of a version string, like Python's `re.match(r"(\d{1,9})\.(\d{1,9})\.(\d{1,9})")`:
/// anchored at the start, nothing is required after the patch number and a patch run longer than nine digits is
/// cut at nine. Only ASCII digits count, where `\d` also accepts other Unicode decimal digits.
fn leading_version(version: &str) -> Option<[u32; 3]> {
    fn number(text: &str, exact: bool) -> Option<(u32, &str)> {
        let digits = text.bytes().take_while(u8::is_ascii_digit).count();
        let take = if exact { digits } else { digits.min(9) };
        if take == 0 || take > 9 {
            return None;
        }
        text[..take].parse().ok().map(|value| (value, &text[take..]))
    }
    let (major, rest) = number(version, true)?;
    let (minor, rest) = number(rest.strip_prefix('.')?, true)?;
    let (patch, _) = number(rest.strip_prefix('.')?, false)?;
    Some([major, minor, patch])
}

/// Python `_meets_derived_client_app_minimum`: the version starts with `major.minor.patch` and that triple is not
/// below `999.0.0`, compared as a list. Nothing qualifies until upstream lowers the minimum.
pub fn meets_derived_client_app_minimum(version: &str) -> bool {
    leading_version(version).is_some_and(|parsed| parsed >= DERIVED_CLIENT_APP_MINIMUM_CLAUDE_CODE_VERSION)
}

/// Python `_derive_client_app` for the bundled executable: whether `CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP` is
/// derived. It is when the pinned CLI version of the SDK package (`pinned_cli_version`, Python's
/// `__cli_version__`) meets the minimum, `cli_path` is the bundled executable that [`find_bundled_cli`] finds
/// under `package_root`, and the `claude.version` file beside it records a version that meets the minimum. A
/// missing or unreadable record, or one that is not UTF-8, means no. Any other executable counts as too old.
///
/// TypeScript has neither the gate nor the trigger, so `connect` does not call this; the caller that derives
/// the label asks here when it uses the bundled executable.
pub fn bundled_cli_derives_client_app<F: ReadMetadata, H: ReadFile>(
    cli_path: &Path,
    package_root: &Path,
    windows: bool,
    pinned_cli_version: &str,
    filesystem: &F,
    host: &H,
) -> bool {
    if !meets_derived_client_app_minimum(pinned_cli_version) {
        return false;
    }
    if find_bundled_cli(package_root, windows, filesystem).as_deref() != Some(cli_path) {
        return false;
    }
    host.read_file(&cli_path.with_file_name(BUNDLED_CLI_VERSION_FILE))
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .is_some_and(|recorded| meets_derived_client_app_minimum(&recorded))
}

/// The platform a native package is selected for, spelled like Node's `process.platform` and `process.arch`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativePackageTarget {
    platform: String,
    arch: String,
    prefer_musl: bool,
}

impl NativePackageTarget {
    /// An explicit target, like the `platform`, `arch` and `preferMusl` options of the upstream `vJ`.
    pub fn new(platform: impl Into<String>, arch: impl Into<String>, prefer_musl: bool) -> Self {
        Self {
            platform: platform.into(),
            arch: arch.into(),
            prefer_musl,
        }
    }

    /// The platform this crate was compiled for. Upstream decides musl at run time from the Node process report
    /// (`glibcVersionRuntime` absent on Linux); a Rust process has no such report, so the compile target's libc
    /// stands in for it.
    pub fn host() -> Self {
        let platform = match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            "x86" => "ia32",
            "powerpc64" => "ppc64",
            "loongarch64" => "loong64",
            other => other,
        };
        Self::new(platform, arch, cfg!(all(target_os = "linux", target_env = "musl")))
    }

    pub fn platform(&self) -> &str {
        &self.platform
    }

    pub fn arch(&self) -> &str {
        &self.arch
    }

    /// Specifiers to resolve, in upstream order. Only Linux has a libc choice, Android has its own package and
    /// only Windows adds `.exe`.
    pub fn specifiers(&self) -> Vec<String> {
        let (platform, arch) = (&self.platform, &self.arch);
        let packages = match platform.as_str() {
            "android" => vec![format!("{PACKAGE_SCOPE_NAME}-linux-{arch}-android")],
            "linux" if self.prefer_musl => vec![
                format!("{PACKAGE_SCOPE_NAME}-linux-{arch}-musl"),
                format!("{PACKAGE_SCOPE_NAME}-linux-{arch}"),
            ],
            "linux" => vec![
                format!("{PACKAGE_SCOPE_NAME}-linux-{arch}"),
                format!("{PACKAGE_SCOPE_NAME}-linux-{arch}-musl"),
            ],
            _ => vec![format!("{PACKAGE_SCOPE_NAME}-{platform}-{arch}")],
        };
        let extension = if platform == "win32" { ".exe" } else { "" };
        packages
            .into_iter()
            .map(|package| format!("{package}/claude{extension}"))
            .collect()
    }
}

/// Resolves a module specifier to a file path the way Node's `require.resolve` does for the calling module.
/// Implement it to resolve from another location, a bundler or a virtual file system.
pub trait ResolveNodeModule {
    type Error;

    fn resolve(&self, specifier: &str) -> Result<PathBuf, Self::Error>;
}

/// The native executable is missing: no platform package resolved to an existing file.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error(
    "Native CLI binary for {platform}-{arch} not found. Reinstall @anthropic-ai/claude-agent-sdk without --omit=optional, or set options.pathToClaudeCodeExecutable."
)]
pub struct NativeCliNotFoundError {
    platform: String,
    arch: String,
}

/// Upstream `vJ`: the first specifier that resolves to a path that exists. A failed resolution moves on to the
/// next specifier.
pub fn find_native_cli<R: ResolveNodeModule, F: ReadMetadata>(
    target: &NativePackageTarget,
    resolver: &R,
    filesystem: &F,
) -> Result<PathBuf, NativeCliNotFoundError> {
    target
        .specifiers()
        .iter()
        .filter_map(|specifier| resolver.resolve(specifier).ok())
        .find(|path| filesystem.metadata(path).is_ok())
        .ok_or_else(|| NativeCliNotFoundError {
            platform: target.platform.clone(),
            arch: target.arch.clone(),
        })
}

/// The default [`ResolveNodeModule`]: Node's lookup directories for a module that lives in `start_directory`.
///
/// Candidates are every ancestor's `node_modules` (skipping an ancestor that is itself named `node_modules`),
/// then the `NODE_PATH` entries, then `$HOME/.node_modules`, `$HOME/.node_libraries` and `<prefix>/lib/node`.
/// A specifier resolves when `<directory>/<specifier>` is a file; the result is its canonical path, as Node
/// returns the real path.
#[derive(Clone, Debug)]
pub struct NodeModuleLookup<F> {
    directories: Vec<PathBuf>,
    filesystem: F,
}

impl<F> NodeModuleLookup<F> {
    /// `node_path` is the raw `NODE_PATH` value, `home` the home directory and `prefix` the Node installation
    /// prefix (the parent of the directory that holds the `node` executable). Absent values add no directory.
    pub fn new(
        start_directory: &Path,
        node_path: Option<&OsString>,
        home: Option<&Path>,
        prefix: Option<&Path>,
        filesystem: F,
    ) -> Self {
        let mut directories: Vec<PathBuf> = start_directory
            .ancestors()
            .filter(|ancestor| ancestor.file_name().is_none_or(|name| name != "node_modules"))
            .map(|ancestor| ancestor.join("node_modules"))
            .collect();
        if let Some(node_path) = node_path {
            directories.extend(std::env::split_paths(node_path).filter(|entry| !entry.as_os_str().is_empty()));
        }
        if let Some(home) = home {
            directories.push(home.join(".node_modules"));
            directories.push(home.join(".node_libraries"));
        }
        if let Some(prefix) = prefix {
            directories.push(prefix.join("lib").join("node"));
        }
        Self {
            directories,
            filesystem,
        }
    }

    /// The directories searched, in order.
    pub fn directories(&self) -> &[PathBuf] {
        &self.directories
    }
}

impl<F: ReadMetadata + CanonicalizePath<Error = io::Error>> ResolveNodeModule for NodeModuleLookup<F> {
    type Error = NodeModuleNotFoundError;

    fn resolve(&self, specifier: &str) -> Result<PathBuf, Self::Error> {
        self.directories
            .iter()
            .map(|directory| directory.join(specifier))
            .filter(|candidate| is_file(&self.filesystem, candidate))
            .find_map(|candidate| self.filesystem.canonicalize(&candidate).ok())
            .ok_or_else(|| NodeModuleNotFoundError {
                specifier: specifier.to_owned(),
            })
    }
}

/// No lookup directory holds the requested module file.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Cannot find module '{specifier}'")]
pub struct NodeModuleNotFoundError {
    specifier: String,
}
