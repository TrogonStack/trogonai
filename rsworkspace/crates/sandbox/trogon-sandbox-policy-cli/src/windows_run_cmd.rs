//! `trogon-sandbox-policy [-s settings] [-c command | words...]` on Windows: the command
//! runs under the network fence that `windows-install` set up.

use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use tokio::sync::mpsc;
use trogon_sandbox_policy_exec::{
    CommandOverrides, HostPaths, RuntimeError, SandboxSettings, WindowsRun, WindowsRunHandle, run_on_windows,
};
use trogon_std::env::{ReadEnv, SystemEnv};

use crate::args::{Args, CommandLine, ControlFd};
use crate::control::{ControlChannel, ControlEvent, next_event, report};
use crate::debug::Debug;
use crate::source::SettingsSource;

pub(crate) fn run(args: Args) -> i32 {
    let env = SystemEnv;
    if args.guest_view {
        report(format_args!("Error: --guest-view is not available on Windows"));
        return 1;
    }
    let debug = Debug::from(&args);
    let command = match CommandLine::from_args(&args) {
        Some(line) => line.shell_text(),
        None => {
            report(format_args!(
                "Error: No command specified. Use -c <command> or provide command arguments."
            ));
            return 1;
        }
    };
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            report(format_args!("Error: the working directory is not readable: {error}"));
            return 1;
        }
    };
    let home = env.var_os("USERPROFILE").map(PathBuf::from);
    let host = match &home {
        Some(home) => HostPaths::new(cwd.clone()).with_home(home),
        None => HostPaths::new(cwd.clone()),
    };
    let source = SettingsSource::choose(args.settings, home);
    let settings = match source.load(Some(&host)) {
        Ok(settings) => settings,
        Err(refusal) => {
            report(refusal);
            return 1;
        }
    };
    let control = match args.control_fd {
        Some(fd) => match ControlChannel::open(fd) {
            Ok(channel) => Some(channel),
            Err(why) => {
                report(format_args!(
                    "Error: --control-fd {fd} is not usable: {why}. Refusing to run the command without the control channel it asks for."
                ));
                return 1;
            }
        },
        None => None,
    };
    let fence_helper = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            report(format_args!("Error: this executable cannot be located: {error}"));
            return 1;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            report(format_args!("Error: {error}"));
            return 1;
        }
    };
    let run = WindowsRun {
        fence_helper,
        command,
        cwd: cwd.clone(),
        allow_profile_exclusions: Vec::new(),
        overrides: CommandOverrides::default(),
        handle: None,
        env: Vec::new(),
        command_id: None,
    };
    let supervised = Supervised {
        settings: &settings,
        host: &host,
        workspace: &cwd,
        debug: &debug,
    };
    match runtime.block_on(supervised.run(run, control)) {
        Ok(code) => code,
        Err(error) => {
            report(format_args!("Error: {error}"));
            1
        }
    }
}

struct Supervised<'a> {
    settings: &'a SandboxSettings,
    host: &'a HostPaths,
    workspace: &'a Path,
    debug: &'a Debug,
}

impl Supervised<'_> {
    /// Runs the command while the control channel, when there is one, keeps
    /// replacing its network policy. An update that arrives before the proxy
    /// is up waits for it, so the command is polled alongside.
    async fn run(&self, mut run: WindowsRun, control: Option<ControlChannel>) -> Result<i32, RuntimeError> {
        let handle = WindowsRunHandle::new();
        run.handle = Some(handle.clone());
        let control_fd = control.as_ref().map(ControlChannel::fd);
        let mut updates: Option<mpsc::Receiver<ControlEvent>> = control.map(|channel| {
            self.debug
                .log(format_args!("Listening for config updates on fd {}", channel.fd()));
            channel.listen()
        });
        let command = run_on_windows(self.settings, run);
        tokio::pin!(command);
        let mut queued: VecDeque<String> = VecDeque::new();
        let mut applying: Option<Pin<Box<dyn Future<Output = ()> + '_>>> = None;
        let mut delivered_any = false;
        let mut channel_failed = false;
        let result = loop {
            if applying.is_none()
                && let Some(line) = queued.pop_front()
            {
                applying = Some(Box::pin(self.apply_update(&handle, line, control_fd)));
            }
            tokio::select! {
                result = &mut command => break result,
                () = async {
                    match applying.as_mut() {
                        Some(update) => update.await,
                        None => std::future::pending().await,
                    }
                } => applying = None,
                event = next_event(&mut updates) => match event {
                    ControlEvent::Line(line) => {
                        delivered_any = true;
                        queued.push_back(line);
                    }
                    ControlEvent::Closed => updates = None,
                    ControlEvent::Failed(err) => {
                        updates = None;
                        let fd = control_fd.map(|fd| fd.to_string()).unwrap_or_default();
                        if delivered_any {
                            report(format_args!("Error reading control fd {fd}: {err}. The control channel is closed; no further updates will be applied."));
                        } else {
                            report(format_args!("Error: control fd {fd} failed before delivering an update: {err}. Refusing to run the command without the control channel it asks for."));
                            channel_failed = true;
                            handle.terminate();
                        }
                    }
                },
            }
        };
        let code = result?;
        Ok(if channel_failed { 1 } else { code })
    }

    async fn apply_update(&self, handle: &WindowsRunHandle, line: String, fd: Option<ControlFd>) {
        if line.trim().is_empty() {
            return;
        }
        let fd = fd.map(|fd| fd.to_string()).unwrap_or_default();
        let rejected = |detail: &str| {
            report(format_args!(
                "Invalid config on control fd {fd}: ignored, previous config still in force"
            ));
            self.debug
                .log(format_args!("Rejected update on control fd {fd}: {detail}"));
        };
        let settings = match SandboxSettings::from_json_lenient(&line, Some(self.host)) {
            Ok(settings) => settings,
            Err(err) => return rejected(&format!("at {:?}", err.path())),
        };
        let config = match settings.to_standard_config(self.workspace) {
            Ok(config) => config,
            Err(err) => return rejected(&err.to_string()),
        };
        if let Err(err) = handle.update_config(config).await {
            return rejected(&err.to_string());
        }
        self.debug.log(format_args!(
            "Config updated from control fd {fd}: allowed domains [{}]",
            settings.allowed_domains().join(", ")
        ));
    }
}
