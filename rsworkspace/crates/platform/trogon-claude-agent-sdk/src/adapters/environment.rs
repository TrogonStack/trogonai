use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
};

pub trait EnvironmentVariable {
    fn var_os(&self, key: &OsStr) -> Option<OsString>;

    /// Every variable the process would inherit, exactly as [`std::env::vars_os`] yields them.
    ///
    /// The default is empty so existing adapters keep compiling; an adapter that stands in for the process
    /// environment should override it so the case-insensitive `CLAUDE_CODE_SDK_READS_SESSION_STATE` check sees
    /// every inherited name, as upstream does.
    fn vars_os(&self) -> Vec<(OsString, OsString)> {
        Vec::new()
    }

    /// The home directory of the effective user from the passwd database, which is what Node's `os.homedir()` falls
    /// back to when `HOME` is unset (libuv `uv_os_get_passwd` reads the effective user id).
    ///
    /// The default asks the operating system; an adapter that stands in for the process environment overrides it so
    /// the answer stays deterministic.
    fn effective_user_home_directory(&self) -> Option<OsString> {
        super::passwd::effective_user_home()
    }

    /// The home directory of the real user from the passwd database, which is what Python's `Path.home()` falls back
    /// to when `HOME` is unset (`pwd.getpwuid(os.getuid())` reads the real user id).
    fn real_user_home_directory(&self) -> Option<OsString> {
        super::passwd::real_user_home()
    }
}

/// Returns the directory for temporary files exactly as [`std::env::temp_dir`] does.
pub trait TemporaryDirectory {
    fn temp_dir(&self) -> PathBuf;
}

pub trait CurrentDirectory {
    type Error;

    fn current_dir(&self) -> Result<PathBuf, Self::Error>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemEnvironment;

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        std_env_access,
        reason = "The native environment adapter is the explicit process-environment boundary."
    )
)]
impl EnvironmentVariable for SystemEnvironment {
    fn var_os(&self, key: &OsStr) -> Option<OsString> {
        std::env::var_os(key)
    }

    fn vars_os(&self) -> Vec<(OsString, OsString)> {
        std::env::vars_os().collect()
    }
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        std_env_access,
        reason = "The native directory adapter is the explicit process-directory boundary."
    )
)]
impl CurrentDirectory for SystemEnvironment {
    type Error = std::io::Error;

    fn current_dir(&self) -> Result<PathBuf, Self::Error> {
        std::env::current_dir()
    }
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        std_env_access,
        reason = "The native temporary-directory adapter is the explicit process-environment boundary."
    )
)]
impl TemporaryDirectory for SystemEnvironment {
    fn temp_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }
}
