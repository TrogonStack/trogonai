use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::signal::unix::{SignalKind, signal};
use trogon_sandbox_policy_exec::{
    ChildStdio, CommandSpec, DependencyCheck, HostPaths, PolicyExec, PolicyExecConfig, SandboxSettings, Signal,
    StdioMode,
};
use trogon_std::env::{ReadEnv, SystemEnv};

use crate::args::{Args, CommandLine, ControlFd};
use crate::control::{ControlChannel, ControlEvent, next_event, report};
use crate::debug::Debug;
use crate::shell;
use crate::source::SettingsSource;

struct Interrupts {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    arrived: [(Signal, Arc<AtomicBool>); 2],
}

impl Interrupts {
    fn install() -> std::io::Result<Self> {
        let arrived = [Signal::SIGINT, Signal::SIGTERM].map(|which| {
            let flag = Arc::new(AtomicBool::new(false));
            let raised = Arc::clone(&flag);
            let registered =
                unsafe { signal_hook_registry::register(which as i32, move || raised.store(true, Ordering::SeqCst)) };
            registered.map(|_| (which, flag))
        });
        let [interrupt, terminate] = arrived;
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            arrived: [interrupt?, terminate?],
        })
    }

    fn arrived(&self) -> Option<Signal> {
        self.arrived
            .iter()
            .find(|(_, flag)| flag.load(Ordering::SeqCst))
            .map(|(which, _)| *which)
    }

    async fn next(&mut self) -> Signal {
        tokio::select! {
            _ = self.interrupt.recv() => Signal::SIGINT,
            _ = self.terminate.recv() => Signal::SIGTERM,
        }
    }
}

struct Prepared {
    runtime: PolicyExec,
    spec: CommandSpec,
    workspace: PathBuf,
    control: Option<ControlChannel>,
    host: Option<HostPaths>,
}

fn shell_spec(text: String, env: &impl ReadEnv) -> CommandSpec {
    let bash = shell::find_in_path("bash", env.var_os("PATH").as_deref()).unwrap_or_else(|| PathBuf::from("bash"));
    CommandSpec::new(bash).arg("-c").arg(text.clone()).command_line(text)
}

fn fail(message: impl std::fmt::Display) -> i32 {
    report(format_args!("Error: {message}"));
    EXIT_FAILURE
}

pub(crate) async fn run(args: Args) -> i32 {
    let env = SystemEnv;
    let debug = Debug::from(&args);
    let mut interrupts = match Interrupts::install() {
        Ok(interrupts) => interrupts,
        Err(err) => return fail(format!("cannot listen for signals: {err}")),
    };
    let prepared = tokio::select! {
        biased;
        signal = interrupts.next() => return interrupted(signal),
        prepared = prepare(&args, &debug, &env) => prepared,
    };
    match prepared {
        Ok(prepared) => execute(prepared, interrupts, &debug).await,
        Err(code) => code,
    }
}

fn interrupted(signal: Signal) -> i32 {
    fail(format_args!("interrupted by {signal}"))
}

async fn prepare(args: &Args, debug: &Debug, env: &impl ReadEnv) -> Result<Prepared, i32> {
    let control_fd = args.control_fd;
    let source = SettingsSource::choose(args.settings.clone(), env.var_os("HOME").map(PathBuf::from));
    let workspace = std::env::current_dir().map_err(|err| fail(format!("cannot read the current directory: {err}")))?;
    let host = (!args.guest_view).then(|| host_paths(&workspace, env));
    let settings = source.load(host.as_ref()).map_err(fail)?;
    let control = match control_fd {
        Some(fd) => Some(ControlChannel::open(fd).map_err(|why| {
            report(format_args!(
                "Error: --control-fd {fd} is not usable: {why}. Refusing to run the command without the control channel it asks for."
            ));
            EXIT_FAILURE
        })?),
        None => None,
    };

    debug.log("Initializing sandbox...");
    let config = config_of(&workspace, &settings).map_err(fail)?;

    for warning in settings.warnings() {
        debug.log(format_args!("Warning: {warning}"));
    }
    let check = DependencyCheck::of(&config);
    for warning in &check.warnings {
        debug.log(format_args!("Warning: {warning}"));
    }
    if !check.is_ok() {
        return Err(fail(check.errors.join("; ")));
    }
    let runtime = PolicyExec::builder(config)
        .tool_proxy_env_from(&settings)
        .build()
        .map_err(fail)?;

    let Some(command) = CommandLine::from_args(args) else {
        return Err(fail(
            "No command specified. Use -c <command> or provide command arguments.",
        ));
    };
    debug.log(command.debug_line());
    let spec = shell_spec(command.shell_text(), env).stdio(ChildStdio {
        stdin: StdioMode::Inherit,
        stdout: StdioMode::Inherit,
        stderr: StdioMode::Inherit,
    });
    Ok(Prepared {
        runtime,
        spec,
        workspace,
        control,
        host,
    })
}

fn host_paths(workspace: &Path, env: &impl ReadEnv) -> HostPaths {
    let paths = HostPaths::new(workspace.to_path_buf());
    match env.var_os("HOME") {
        Some(home) => paths.with_home(home),
        None => paths,
    }
}

fn config_of(workspace: &Path, settings: &SandboxSettings) -> Result<PolicyExecConfig, String> {
    settings.to_standard_config(workspace).map_err(|err| err.to_string())
}

async fn execute(prepared: Prepared, mut interrupts: Interrupts, debug: &Debug) -> i32 {
    let Prepared {
        runtime,
        spec,
        workspace,
        control,
        host,
    } = prepared;
    let control_fd = control.as_ref().map(ControlChannel::fd);
    let mut updates = control.map(|channel| {
        debug.log(format_args!("Listening for config updates on fd {}", channel.fd()));
        channel.listen()
    });
    let mut delivered_any = false;
    let mut channel_failed = false;
    let mut kill_at: Option<tokio::time::Instant> = None;

    if let Some(signal) = interrupts.arrived() {
        runtime.reset().await;
        return interrupted(signal);
    }

    let command = runtime.run(spec);
    tokio::pin!(command);
    let result = loop {
        tokio::select! {
            result = &mut command => break result,
            signal = interrupts.next() => {
                if runtime.signal_commands(signal) == 0 {
                    runtime.reset().await;
                    return interrupted(signal);
                }
            }
            event = next_event(&mut updates) => match event {
                ControlEvent::Line(line) => {
                    delivered_any = true;
                    apply_update(&runtime, &workspace, host.as_ref(), &line, control_fd, debug).await;
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
                        runtime.signal_commands(Signal::SIGTERM);
                        kill_at = Some(tokio::time::Instant::now() + KILL_GRACE);
                    }
                }
            },
            () = until(kill_at) => {
                kill_at = None;
                runtime.reset().await;
            }
        }
    };
    runtime.reset().await;

    let output = match result {
        Ok(output) => output,
        Err(err) => {
            report(format_args!("Failed to execute command: {err}"));
            return EXIT_FAILURE;
        }
    };
    if channel_failed {
        return EXIT_FAILURE;
    }
    exit_code(&output.status)
}

async fn until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

async fn apply_update(
    runtime: &PolicyExec,
    workspace: &Path,
    host: Option<&HostPaths>,
    line: &str,
    fd: Option<ControlFd>,
    debug: &Debug,
) {
    if line.trim().is_empty() {
        return;
    }
    let fd = fd.map(|fd| fd.to_string()).unwrap_or_default();
    let rejected = |detail: &str| {
        report(format_args!(
            "Invalid config on control fd {fd}: ignored, previous config still in force"
        ));
        debug.log(format_args!("Rejected update on control fd {fd}: {detail}"));
    };
    let settings = match SandboxSettings::from_json_lenient(line, host) {
        Ok(settings) => settings,
        Err(err) => return rejected(&format!("at {:?}", err.path())),
    };
    let config = match config_of(workspace, &settings) {
        Ok(config) => config,
        Err(err) => return rejected(&err),
    };
    if let Err(err) = runtime.update_config(config).await {
        return rejected(&err.to_string());
    }
    debug.log(format_args!(
        "Config updated from control fd {fd}: allowed domains [{}]",
        settings.allowed_domains().join(", ")
    ));
}

fn exit_code(status: &std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    match status.signal().map(Signal::try_from) {
        Some(Ok(Signal::SIGINT | Signal::SIGTERM)) => 0,
        Some(Ok(signal)) => {
            report(format_args!("Process killed by signal: {signal}"));
            EXIT_FAILURE
        }
        Some(Err(_)) | None => {
            report(format_args!("Process killed by an unknown signal"));
            EXIT_FAILURE
        }
    }
}

mod constants;
#[cfg(test)]
mod tests;
use constants::{EXIT_FAILURE, KILL_GRACE};
