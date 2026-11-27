/// What `run_command` means on a worker.
///
/// The suite runs only the checks whose semantics the worker declares, so a worker that cannot meet a check says so
/// here instead of failing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecModel {
    /// `command` is an argv run as a process in a POSIX userland, and `write_files` and `read_file` reach the
    /// filesystem it sees.
    Shell,
    /// The lease holds a persistent JS runtime. `command` is `["eval", <source>]`, evaluated in the foreground
    /// against globals that outlive the call; a thrown error exits 1. Detached commands, `cwd`, `env`, any other
    /// argv and the file verbs are refused with a typed error.
    JsEval,
    /// The worker serves no `run_command` and no file verbs; only the lease lifecycle checks apply.
    None,
}
