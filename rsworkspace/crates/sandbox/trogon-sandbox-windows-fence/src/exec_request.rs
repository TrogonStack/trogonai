use crate::bin_shell::BinShell;
use crate::constants::WINDOWS_LOOPBACK_NO_PROXY;
use crate::windows_env::merge_windows_env;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

/// CreateProcessW caps the command line at 32 767 characters; the estimate
/// does not model quoting overhead, so the budget leaves headroom.
const ARGV_BUDGET: usize = 30_000;

/// Everything the argv of one `fence-helper exec` launch is built from.
#[derive(Debug, Clone)]
pub struct ExecRequest {
    pub command: String,
    pub bin_shell: BinShell,
    /// Run `bin_shell.exe` with `bin_shell.args` as they are, with no command
    /// string after them.
    direct: bool,
    /// Print nothing of its own (`--quiet`); on unless set otherwise.
    pub quiet: bool,
    /// The process whose session this command belongs to.
    pub held_by: Option<u32>,
    pub write_roots: Vec<String>,
    pub allow_loopback: bool,
    pub deny_read: Vec<String>,
    pub deny_read_except: Vec<String>,
    pub deny_write: Vec<String>,
    /// The broker's `PATH` and `PATHEXT`: the first layer of the overlay.
    pub base_env: Vec<(String, String)>,
    /// Whether a proxy is set, which turns on `NODE_USE_ENV_PROXY`.
    pub proxy_in_use: bool,
    /// `mode: mask` sentinels, overlaid before the proxy set so a caller who
    /// masks a proxy name cannot break the sandbox's own proxy plumbing.
    pub mask_env: Vec<(String, String)>,
    /// The generated proxy set.
    pub proxy_env: Vec<(String, String)>,
    /// The `GIT_CONFIG_*` set; last, so its count wins.
    pub git_env: Vec<(String, String)>,
}

impl ExecRequest {
    pub fn new(command: impl Into<String>, bin_shell: BinShell) -> Self {
        Self {
            command: command.into(),
            bin_shell,
            direct: false,
            quiet: true,
            held_by: None,
            write_roots: Vec::new(),
            allow_loopback: false,
            deny_read: Vec::new(),
            deny_read_except: Vec::new(),
            deny_write: Vec::new(),
            base_env: Vec::new(),
            proxy_in_use: false,
            mask_env: Vec::new(),
            proxy_env: Vec::new(),
            git_env: Vec::new(),
        }
    }

    /// A launch of `exe` with `args` as its argv, with no shell in between.
    pub fn program(exe: impl Into<String>, args: Vec<String>) -> Result<Self, WindowsSandboxError> {
        let mut request = Self::new(String::new(), BinShell::explicit(exe, args)?);
        request.direct = true;
        Ok(request)
    }

    /// The proxy variables the child gets: `TMPDIR` is a POSIX path that
    /// breaks msys2 tools, and `NO_PROXY` would make clients bypass the proxy
    /// and hit the fence (which blocks every direct connect from the account,
    /// loopback outside the permit range included). With loopback open the
    /// loopback hosts alone skip the proxy.
    fn proxy_overlay(&self) -> Vec<(String, String)> {
        let mut generated: Vec<(String, String)> = self
            .proxy_env
            .iter()
            .filter(|(k, _)| !k.eq_ignore_ascii_case("TMPDIR") && !k.eq_ignore_ascii_case("NO_PROXY"))
            .cloned()
            .collect();
        if self.allow_loopback && self.proxy_in_use {
            generated.push(("no_proxy".to_string(), WINDOWS_LOOPBACK_NO_PROXY.to_string()));
        }
        generated
    }

    /// The overlay `--env` passes: the runner starts with the sandbox
    /// account's own profile environment and adds exactly this.
    pub fn env_overlay(&self) -> Vec<(String, String)> {
        let mut first = self.base_env.clone();
        if self.proxy_in_use {
            first.push(("NODE_USE_ENV_PROXY".to_string(), "1".to_string()));
        }
        let generated = self.proxy_overlay();
        merge_windows_env([
            first.as_slice(),
            self.mask_env.as_slice(),
            generated.as_slice(),
            self.git_env.as_slice(),
        ])
    }

    /// The argv after the `fence-helper` executable and its dispatch argument.
    /// Spawn it without a shell: that keeps untrusted bytes off the host's
    /// shell, the inner shell runs inside the sandbox.
    pub fn args(&self) -> Result<Vec<String>, WindowsSandboxError> {
        let mut argv: Vec<String> = vec!["exec".to_string()];
        if self.quiet {
            argv.push("--quiet".to_string());
        }
        if let Some(pid) = self.held_by {
            argv.extend(["--held-by".to_string(), pid.to_string()]);
        }
        argv.push("--write-restricted-token".to_string());
        for r in &self.write_roots {
            argv.extend(["--write-root".to_string(), r.clone()]);
        }
        if self.allow_loopback {
            argv.push("--allow-loopback".to_string());
        }
        for d in &self.deny_read {
            argv.extend(["--deny-read".to_string(), d.clone()]);
        }
        for a in &self.deny_read_except {
            argv.extend(["--deny-read-except".to_string(), a.clone()]);
        }
        for d in &self.deny_write {
            argv.extend(["--deny-write".to_string(), d.clone()]);
        }
        for (k, v) in self.env_overlay() {
            argv.extend(["--env".to_string(), format!("{k}={v}")]);
        }
        argv.push("--".to_string());
        argv.push(self.bin_shell.exe.clone());
        argv.extend(self.bin_shell.args.iter().cloned());
        if !self.direct {
            argv.push(self.command.clone());
        }

        let estimate: usize = argv.iter().map(|a| a.chars().count() + 3).sum();
        if estimate > ARGV_BUDGET {
            return Err(WindowsSandboxError::new(
                Code::ArgvTooLong,
                format!(
                    "Windows sandbox argv is ~{estimate} chars (CreateProcessW limit is 32 767). \
                     Besides the command itself it carries {} per-command read denies, {} \
                     session grants beneath them, {} write denies and {} write roots; the \
                     command was not run with fewer.",
                    self.deny_read.len(),
                    self.deny_read_except.len(),
                    self.deny_write.len(),
                    self.write_roots.len()
                ),
            ));
        }
        Ok(argv)
    }
}
