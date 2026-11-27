use std::collections::HashMap;
use std::sync::Arc;

use trogon_sandbox_contract::CommandOutput;

use crate::builtin_handlers::{ServeHandler, SleepHandler, SyncHandler};
use crate::mutex_ext::MutexExt;
use crate::{CommandCtx, CommandHandler, out, resolve};

/// The scripted commands a worker knows. Anything else exits 127.
#[derive(Clone)]
pub struct CommandTable {
    handlers: HashMap<String, Arc<dyn CommandHandler>>,
}

impl Default for CommandTable {
    fn default() -> Self {
        let mut table = Self::empty();
        table.register_sync("echo", |ctx| out(0, &format!("{}\n", ctx.args.join(" ")), ""));
        table.register_sync("true", |_| out(0, "", ""));
        table.register_sync("false", |_| out(1, "", ""));
        table.register_sync("pwd", |ctx| out(0, &format!("{}\n", ctx.cwd), ""));
        table.register_sync("printenv", printenv);
        table.register_sync("cat", cat);
        table.register("sleep", SleepHandler);
        table.register("serve", ServeHandler);
        table
    }
}

fn printenv(ctx: &CommandCtx) -> CommandOutput {
    if ctx.args.is_empty() {
        let all: String = ctx.env.iter().map(|(key, value)| format!("{key}={value}\n")).collect();
        return out(0, &all, "");
    }
    let mut stdout = String::new();
    let mut missing = false;
    for name in &ctx.args {
        match ctx.env.get(name) {
            Some(value) => stdout.push_str(&format!("{value}\n")),
            None => missing = true,
        }
    }
    out(i32::from(missing), &stdout, "")
}

fn cat(ctx: &CommandCtx) -> CommandOutput {
    let fs = ctx.fs.lock_clean();
    let mut stdout = Vec::new();
    for arg in &ctx.args {
        match fs.read(&resolve(&ctx.cwd, arg)) {
            Some(data) => stdout.extend_from_slice(data),
            None => {
                return CommandOutput {
                    exit_code: 1,
                    stdout,
                    stderr: format!("cat: {arg}: No such file or directory\n").into_bytes(),
                    ..Default::default()
                };
            }
        }
    }
    CommandOutput {
        stdout,
        ..Default::default()
    }
}

impl CommandTable {
    /// A table with no commands.
    pub fn empty() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Adds or replaces the command `name`.
    pub fn register(&mut self, name: &str, handler: impl CommandHandler + 'static) {
        self.handlers.insert(name.to_string(), Arc::new(handler));
    }

    /// Adds or replaces the command `name` with a plain function.
    pub fn register_sync(&mut self, name: &str, f: impl Fn(&CommandCtx) -> CommandOutput + Send + Sync + 'static) {
        self.register(name, SyncHandler(f));
    }

    /// Runs `command` (program then args). Unknown programs exit 127. Unless
    /// a handler named `sh` is registered, `sh -c <script>` runs a one-line
    /// script: `exit <code>`, or a single command split on whitespace.
    pub async fn run(&self, command: &[String], ctx: CommandCtx) -> CommandOutput {
        match command {
            [sh, flag, script, ..] if sh == "sh" && flag == "-c" && !self.handlers.contains_key("sh") => {
                let words: Vec<String> = script.split_whitespace().map(str::to_string).collect();
                match words.as_slice() {
                    [exit] if exit == "exit" => out(0, "", ""),
                    [exit, code] if exit == "exit" => match code.parse::<i32>() {
                        Ok(code) => out(code & 0xff, "", ""),
                        Err(_) => out(2, "", &format!("sh: exit: {code}: numeric argument required\n")),
                    },
                    _ => self.dispatch(&words, ctx).await,
                }
            }
            _ => self.dispatch(command, ctx).await,
        }
    }

    async fn dispatch(&self, command: &[String], mut ctx: CommandCtx) -> CommandOutput {
        let Some((program, args)) = command.split_first() else {
            return out(127, "", "empty command\n");
        };
        match self.handlers.get(program) {
            Some(handler) => {
                ctx.args = args.to_vec();
                handler.run(ctx).await
            }
            None => out(127, "", &format!("{program}: command not found\n")),
        }
    }
}
