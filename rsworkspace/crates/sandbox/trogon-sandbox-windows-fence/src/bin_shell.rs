use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

/// The inner shell a command runs under, inside the sandbox: the executable
/// and the flags placed between it and the command string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinShell {
    pub exe: String,
    pub args: Vec<String>,
}

fn is_win32_absolute(p: &str) -> bool {
    let b = p.as_bytes();
    if matches!(b.first(), Some(b'\\' | b'/')) {
        return true;
    }
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/')
}

fn win32_basename(p: &str) -> &str {
    let after_drive = match p.as_bytes() {
        [d, b':', ..] if d.is_ascii_alphabetic() => &p[2..],
        _ => p,
    };
    after_drive.rsplit(['\\', '/']).next().unwrap_or(after_drive)
}

fn win32_join(root: &str, parts: &[&str]) -> String {
    let mut out = root.trim_end_matches(['\\', '/']).to_string();
    for part in parts {
        out.push('\\');
        out.push_str(part);
    }
    out
}

fn invalid(message: String) -> WindowsSandboxError {
    WindowsSandboxError::new(Code::BinShellInvalid, message)
}

impl BinShell {
    fn cmd_default(system_root: &str) -> Self {
        Self {
            exe: win32_join(system_root, &["System32", "cmd.exe"]),
            args: ["/d", "/s", "/c"].map(String::from).to_vec(),
        }
    }

    /// The only normalizer from the `bin_shell` setting to a spawnable pair.
    /// A bare token (`cmd`, `pwsh`, `powershell`) resolves to the default
    /// install; an absolute path to `bash.exe`, `sh.exe`, `pwsh.exe`,
    /// `powershell.exe` or `cmd.exe` keeps the caller's path with the
    /// matching flag shape. Anything else is an error, never a silent
    /// fallback to `cmd.exe`. Path rules are Windows' on every host.
    pub fn parse(raw: Option<&str>, system_root: &str) -> Result<Self, WindowsSandboxError> {
        let Some(raw) = raw else {
            return Ok(Self::cmd_default(system_root));
        };
        let raw_base = win32_basename(raw);
        let is_abs = is_win32_absolute(raw);
        if !is_abs && raw != raw_base {
            return Err(invalid(format!(
                "bin_shell string must be a bare token or an absolute path (got {raw:?})"
            )));
        }
        let pwsh_flags = || ["-NoProfile", "-Command"].map(String::from).to_vec();
        match raw_base.to_lowercase().as_str() {
            "bash" | "bash.exe" | "sh" | "sh.exe" => {
                if !is_abs {
                    return Err(invalid(format!(
                        "bin_shell bash path must be absolute (got {raw:?}); pass the resolved \
                         Git Bash install path"
                    )));
                }
                Ok(Self {
                    exe: raw.to_string(),
                    args: vec!["-c".to_string()],
                })
            }
            "pwsh" | "pwsh.exe" => Ok(Self {
                exe: if is_abs {
                    raw.to_string()
                } else {
                    "pwsh.exe".to_string()
                },
                args: pwsh_flags(),
            }),
            "powershell" | "powershell.exe" => Ok(Self {
                exe: if is_abs {
                    raw.to_string()
                } else {
                    win32_join(
                        system_root,
                        &["System32", "WindowsPowerShell", "v1.0", "powershell.exe"],
                    )
                },
                args: pwsh_flags(),
            }),
            "cmd" | "cmd.exe" => {
                let default = Self::cmd_default(system_root);
                Ok(if is_abs {
                    Self {
                        exe: raw.to_string(),
                        args: default.args,
                    }
                } else {
                    default
                })
            }
            _ => Err(invalid(format!(
                "unrecognised bin_shell {raw:?}: expected 'cmd' | 'powershell' | 'pwsh' or an \
                 absolute path to bash.exe/sh.exe/pwsh.exe/powershell.exe"
            ))),
        }
    }

    /// An explicit `{exe, args}` pair; `exe` must be absolute.
    pub fn explicit(exe: impl Into<String>, args: Vec<String>) -> Result<Self, WindowsSandboxError> {
        let exe = exe.into();
        if !is_win32_absolute(&exe) {
            return Err(invalid(format!("bin_shell.exe must be an absolute path (got {exe:?})")));
        }
        Ok(Self { exe, args })
    }
}
