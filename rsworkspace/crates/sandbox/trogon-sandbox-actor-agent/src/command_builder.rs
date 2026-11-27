use std::process::Stdio;

use tokio::process::Command;
use trogon_sandbox_contract::{CommandExecution, RunCommandRequest, StdinMode};
use trogon_sandbox_protocol::resolve_exec_cwd;

use crate::LeaseBinding;
use crate::launch::Launch;
use crate::target_user::TargetUser;

/// The stdin an execution asks for; an attached command never has one.
pub(crate) fn stdin_mode(request: &RunCommandRequest) -> StdinMode {
    match &request.execution {
        CommandExecution::Attached => StdinMode::Closed,
        CommandExecution::Detached { stdin, .. } => *stdin,
    }
}

pub(crate) fn build_command(
    launch: Launch<'_>,
    request: &RunCommandRequest,
    binding: &LeaseBinding,
    user: Option<&TargetUser>,
) -> Command {
    let mut command = Command::new(launch.program);
    command.args(launch.args);
    command.stdin(match stdin_mode(request) {
        StdinMode::Closed => Stdio::null(),
        StdinMode::Interactive => Stdio::piped(),
    });
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(false);
    // A shell that forks its children (dash, the Linux /bin/sh) would otherwise outlive a kill aimed only at it, and its
    // children would keep the output pipes open.
    command.process_group(0);

    command.envs(&binding.env);
    // Between the two, so the identity variables the drop implies are visible but a caller that sets `HOME` explicitly
    // still wins.
    user.inspect(|user| user.apply(&mut command));
    command.envs(request.env.iter());

    // A command run as a sandbox user lands in that user's home unless told otherwise, which is the one directory it is
    // guaranteed to be able to write. A relative cwd joins that same default; with no default it resolves against the
    // agent's own directory.
    let default = user
        .map(|user| user.home().to_string())
        .or_else(|| binding.working_dir.clone());
    let cwd = match default {
        Some(default) => Some(resolve_exec_cwd(&default, request.cwd.as_deref())),
        None => request.cwd.clone(),
    };
    cwd.inspect(|cwd| {
        command.current_dir(cwd);
    });
    command
}
