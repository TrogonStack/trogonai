//! What a Windows payload is started with: its command line, its
//! environment and the encoding `CreateProcessW` wants for both.
//!
//! Nothing here calls Win32, so the rules are checked on every host.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::ensure;

/// The environment of a payload: the allowlist above, a `PATH` of the system
/// directories only, the workspace as the home directory and the lease's
/// scratch directory as the temporary and application data directory. The
/// lease's own variables replace any of these, whatever the case of the name.
///
/// `CreateProcessW` redirects `LOCALAPPDATA` into the AppContainer's own
/// folder and fails with `ERROR_ENVVAR_NOT_FOUND` when the block has none.
pub fn sandbox_env(
    lease_env: &[(String, String)],
    inherited: impl Fn(&str) -> Option<String>,
    workspace: &Path,
    scratch: &Path,
) -> Vec<(String, String)> {
    let mut env = EnvMap::default();
    for name in INHERITED {
        if let Some(value) = inherited(name) {
            env.set(name, value);
        }
    }
    if let Some(root) = inherited("SystemRoot") {
        env.set("PATH", format!("{root}\\System32;{root};{root}\\System32\\Wbem"));
    }
    let workspace = workspace.to_string_lossy().into_owned();
    let scratch = scratch.to_string_lossy().into_owned();
    env.set("HOME", workspace.clone());
    env.set("USERPROFILE", workspace);
    for name in ["TEMP", "TMP", "TMPDIR", "LOCALAPPDATA", "APPDATA"] {
        env.set(name, scratch.clone());
    }
    for (name, value) in lease_env {
        env.set(name, value.clone());
    }
    env.into_entries()
}

#[derive(Default)]
struct EnvMap(BTreeMap<String, (String, String)>);

impl EnvMap {
    fn set(&mut self, name: &str, value: String) {
        self.0.insert(name.to_uppercase(), (name.to_string(), value));
    }

    fn into_entries(self) -> Vec<(String, String)> {
        self.0.into_values().collect()
    }
}

/// The `lpEnvironment` block for `CREATE_UNICODE_ENVIRONMENT`: `NAME=value`
/// entries sorted by upper-cased name, each ended by a NUL, and a final NUL.
pub fn environment_block(entries: &[(String, String)]) -> anyhow::Result<Vec<u16>> {
    let mut sorted: Vec<&(String, String)> = entries.iter().collect();
    sorted.sort_by_key(|(name, _)| name.to_uppercase());
    let mut block = Vec::new();
    for (name, value) in sorted {
        ensure!(
            !name.is_empty() && !name.contains(['=', '\0']),
            "{name:?} is not a usable environment variable name"
        );
        ensure!(!value.contains('\0'), "the value of {name} contains a NUL character");
        block.extend(name.encode_utf16());
        block.push(u16::from(b'='));
        block.extend(value.encode_utf16());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

/// One argument quoted so `CommandLineToArgvW` in the child recovers it.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => {
                backslashes += 1;
                out.push('\\');
            }
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                backslashes = 0;
                out.push(c);
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes));
    out.push('"');
    out
}

fn is_cmd(exe: &Path) -> bool {
    let text = exe.to_string_lossy();
    let name = text.rsplit(['\\', '/']).next().unwrap_or_default();
    let name = name.trim_end_matches(['.', ' ']);
    name.eq_ignore_ascii_case("cmd.exe") || name.eq_ignore_ascii_case("cmd")
}

/// The command line of `exe` with `args`. `cmd.exe` does not read its command
/// line like other programs: what follows `/c`, `/k` or `/r` is one command
/// string it parses itself, so it is passed whole inside a single pair of
/// quotes for `/s` to strip, and not quoted per argument.
pub fn command_line(exe: &Path, args: &[String]) -> String {
    let split = if is_cmd(exe) {
        args.iter()
            .position(|arg| matches!(arg.to_ascii_lowercase().as_str(), "/c" | "/k" | "/r"))
    } else {
        None
    };
    let mut line = quote_arg(&exe.to_string_lossy());
    match split {
        Some(at) => {
            for arg in &args[..=at] {
                line.push(' ');
                line.push_str(&quote_arg(arg));
            }
            line.push_str(" \"");
            line.push_str(&args[at + 1..].join(" "));
            line.push('"');
        }
        None => {
            for arg in args {
                line.push(' ');
                line.push_str(&quote_arg(arg));
            }
        }
    }
    line
}

mod constants;
#[cfg(test)]
mod tests;
use constants::INHERITED;
