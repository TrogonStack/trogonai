use trogon_sandbox_contract::SandboxImage;

use crate::HlukSpecError;

/// A published hluk runtime image a lease runs its commands in, named by the lease image's repository.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum HlukRuntime {
    /// BusyBox's hush shell: every command runs as a shell command line.
    Shell,
    /// CPython 3.12: commands are `python3 -c CODE`.
    Python,
    /// Node.js 22: commands are `node -e CODE`.
    Node,
}

impl HlukRuntime {
    /// The runtime the lease image names: its repository's last path segment, `bash`, `python` or `node`.
    pub fn from_image(image: &SandboxImage) -> Result<Self, HlukSpecError> {
        let SandboxImage::Registry { reference, .. } = image else {
            return Err(HlukSpecError::ImageKind);
        };
        let name = reference.repository.rsplit('/').next().unwrap_or_default();
        [Self::Shell, Self::Python, Self::Node]
            .into_iter()
            .find(|runtime| runtime.image_name() == name)
            .ok_or_else(|| HlukSpecError::UnknownRuntime(name.to_owned()))
    }

    /// The name `hluk run --runtime` takes.
    pub fn image_name(self) -> &'static str {
        match self {
            Self::Shell => "bash",
            Self::Python => "python",
            Self::Node => "node",
        }
    }

    /// Code that does nothing, run once to pull the image and save its warm snapshot.
    pub fn noop(self) -> &'static str {
        match self {
            Self::Shell => "true",
            Self::Python => "pass",
            Self::Node => "0",
        }
    }

    /// The code `hluk run --exec` hands this runtime's driver to run `argv` in `cwd`, or why it cannot.
    pub fn workload(self, argv: &[String], cwd: &str) -> Result<String, String> {
        // A JSON string is also a valid Python and JavaScript string literal.
        let literal = serde_json::Value::from(cwd).to_string();
        match (self, argv) {
            (_, []) => Err("the command is empty".to_owned()),
            (Self::Shell, argv) => {
                let line: Vec<String> = argv.iter().map(|arg| sh_quote(arg)).collect();
                // hluk reports exit 1 for any failing command the shell runs last, but the exact code when the
                // shell exits explicitly, and `$?` holds the real status. `exec` would also work for programs, but
                // the image's applets (`pwd`, `printenv`) are builtins with no file to exec.
                Ok(format!("cd {} || exit 1\n{}\nexit $?", sh_quote(cwd), line.join(" ")))
            }
            (Self::Python, [bin, flag, code]) if matches!(bin.as_str(), "python" | "python3") && flag == "-c" => {
                Ok(format!("__import__('os').chdir({literal})\n{code}"))
            }
            (Self::Node, [bin, flag, code]) if bin == "node" && matches!(flag.as_str(), "-e" | "--eval") => {
                Ok(format!("process.chdir({literal});\n{code}"))
            }
            (Self::Python, _) => Err("the hluk python runtime runs only `python3 -c CODE`".to_owned()),
            (Self::Node, _) => Err("the hluk node runtime runs only `node -e CODE`".to_owned()),
        }
    }
}

/// `value` as one single-quoted shell word.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn a_shell_command_is_quoted_word_by_word_between_a_cd_and_an_explicit_exit() {
        let code = HlukRuntime::Shell.workload(&argv(&["sh", "-c", "echo 'hi'"]), "/workspace");
        assert_eq!(
            code,
            Ok("cd '/workspace' || exit 1\n'sh' '-c' 'echo '\\''hi'\\'''\nexit $?".to_owned())
        );
    }

    #[test]
    fn python_and_node_take_only_inline_code() {
        assert!(
            HlukRuntime::Python
                .workload(&argv(&["python3", "-c", "print(1)"]), "/w")
                .is_ok()
        );
        assert!(HlukRuntime::Python.workload(&argv(&["ls"]), "/w").is_err());
        assert!(HlukRuntime::Node.workload(&argv(&["node", "-e", "1"]), "/w").is_ok());
        assert!(HlukRuntime::Shell.workload(&[], "/w").is_err());
    }
}
