use std::collections::BTreeMap;

/// Wraps `command` in a `sh -c` invocation that applies `cwd`/`env` when set,
/// since kube's exec API runs a raw argv with no shell semantics of its own.
///
/// When `run_as_user` is set, the whole cd/export/command script is nested
/// inside a `runuser -u '<user>' -- sh -c '<script>'` wrapper, since kube's
/// exec API has no native uid/username parameter of its own (unlike Docker's
/// exec `user` field). `runuser` is preferred over `su` because it needs
/// neither a login shell nor a password prompt.
pub(crate) fn shell_wrap(
    command: &[String],
    cwd: Option<&str>,
    env: &BTreeMap<String, String>,
    run_as_user: Option<&str>,
) -> Vec<String> {
    let mut script = String::new();
    if let Some(cwd) = cwd {
        // `|| exit 1` rather than `&&`: the exports that follow would
        // otherwise swallow the failed cd and the command would still run.
        script.push_str(&format!("cd '{}' || exit 1; ", cwd.replace('\'', "'\\''")));
    }
    for (key, value) in env {
        script.push_str(&format!("export {}='{}'; ", key, value.replace('\'', "'\\''")));
    }
    let quoted: Vec<String> = command
        .iter()
        .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
        .collect();
    script.push_str(&quoted.join(" "));

    if let Some(user) = run_as_user {
        return runuser(user, &script);
    }

    vec!["sh".to_string(), "-c".to_string(), script]
}

/// Runs `script` through `sh -c` as `user`, since kube's exec API has no user of its own.
pub(crate) fn runuser(user: &str, script: &str) -> Vec<String> {
    let wrapped = format!(
        "runuser -u '{}' -- sh -c '{}'",
        user.replace('\'', "'\\''"),
        script.replace('\'', "'\\''")
    );
    vec!["sh".to_string(), "-c".to_string(), wrapped]
}

#[cfg(test)]
mod tests;
