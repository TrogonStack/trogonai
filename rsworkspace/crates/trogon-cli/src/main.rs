use acp_nats::{AcpPrefix, Config, NatsAuth, NatsConfig};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;
use trogon_cli::{
    connect_or_start_nats, repl::resolve_model_alias, session::TrogonSession, CrossRunnerSwitcher,
    NatsSessionFactory, OutputFormat, PrintOptions, RealFs, SessionEntry, SessionIndex, SessionInit,
};
use trogon_cli::Session as _;

#[derive(Subcommand)]
enum Command {
    /// Start the local dev stack (NATS, wasm runtime, runners)
    Dev,
    /// Run health checks on the Trogon stack
    Doctor,
}

#[derive(Parser)]
#[command(name = "trogon", about = "Trogon AI CLI")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// NATS server URL (overrides TROGON_NATS_URL)
    #[arg(long, env = "TROGON_NATS_URL", default_value = "nats://localhost:4222")]
    nats_url: String,

    /// ACP prefix (overrides ACP_PREFIX)
    #[arg(long, env = "ACP_PREFIX", default_value = "acp.claude")]
    prefix: String,

    /// Non-interactive mode: send PROMPT and print result to stdout.
    /// Omit the value to read the prompt from stdin instead:
    ///   trogon --print "explain this" < error.log
    ///   echo "what is 2+2?" | trogon --print
    #[arg(short = 'p', long, num_args = 0..=1, default_missing_value = "-")]
    print: Option<String>,

    /// Output format for non-interactive mode: "text" (default) or "json".
    /// json emits a single line: {"text":"...","stop_reason":"..."}
    #[arg(long, default_value = "text")]
    output_format: String,

    /// Emit NDJSON tool lines during --print: {"type":"tool","name":"...","output":"..."}
    #[arg(long)]
    print_tools: bool,

    /// Set bypassPermissions before the prompt (skips permission gates — use with care)
    #[arg(long)]
    dangerously_skip_permissions: bool,

    /// Start the interactive session in plan mode (writes are denied; plan first)
    #[arg(long, conflicts_with = "dangerously_skip_permissions")]
    plan: bool,

    /// Enable verbose (debug-level) logging for the CLI. Respects an explicit RUST_LOG.
    #[arg(short = 'v', long)]
    verbose: bool,

    /// Override the agent's built-in system prompt entirely (replaces the default identity)
    #[arg(long)]
    system_prompt: Option<String>,

    /// Append extra text to the agent's system prompt (kept alongside the default)
    #[arg(long)]
    append_system_prompt: Option<String>,

    /// Grant the agent an additional working directory (repeatable)
    #[arg(long, value_name = "PATH")]
    add_dir: Vec<PathBuf>,

    /// Give the interactive session a human-readable name (shown in /status, recorded in the index)
    #[arg(long, value_name = "NAME")]
    name: Option<String>,

    /// Model id for --print (resolved via aliases; uses --prefix runner)
    #[arg(long)]
    model: Option<String>,

    /// Run health checks on NATS, registry, runners, and compactor
    #[arg(long)]
    doctor: bool,

    /// Stream assistant text live instead of buffering until a tool boundary
    #[arg(long)]
    stream: bool,

    /// Resume the last session for this project (from ~/.local/share/trogon/sessions.json)
    #[arg(long, conflicts_with = "session_id")]
    continue_session: bool,

    /// Attach to a specific session id (uses --prefix runner)
    #[arg(long, conflicts_with = "continue_session")]
    session_id: Option<String>,
}

fn trogon_dev_script() -> anyhow::Result<PathBuf> {
    if let Ok(path) = std::env::var("TROGON_DEV_SCRIPT") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe()?;
    let release_dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("could not resolve trogon binary directory"))?;
    let script = release_dir.join("../../scripts/trogon-dev.sh");
    script
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("trogon dev script not found at {}", script.display()))
}

/// Initialize the CLI tracing subscriber.
///
/// Without `--verbose` the CLI stays quiet unless the user opts in via `RUST_LOG`
/// (default filter: `warn`). With `--verbose` the default filter is raised to
/// `debug`, while an explicit `RUST_LOG` always wins so power users keep full
/// control. Logs go to stderr so they never pollute `--print` stdout output.
fn init_tracing(verbose: bool) {
    use tracing_subscriber::{fmt, EnvFilter};

    let default_level = if verbose { "debug" } else { "warn" };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_level));

    // `try_init` so a double-init (e.g. in tests) is a no-op rather than a panic.
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Build the per-session init metadata from the CLI flags.
///
/// `--add-dir` paths are canonicalised to absolute form; entries that do not
/// resolve to an existing directory are warned about and skipped (matching the
/// CLI's warn-and-continue style elsewhere) so a typo never aborts startup.
fn build_session_init(
    system_prompt: Option<String>,
    append_system_prompt: Option<String>,
    add_dir: &[PathBuf],
) -> SessionInit {
    let mut additional_roots = Vec::new();
    for dir in add_dir {
        match dir.canonicalize() {
            Ok(p) if p.is_dir() => additional_roots.push(p.to_string_lossy().into_owned()),
            Ok(p) => eprintln!("warning: --add-dir {} is not a directory — skipping", p.display()),
            Err(e) => eprintln!("warning: --add-dir {} could not be resolved ({e}) — skipping", dir.display()),
        }
    }
    SessionInit {
        system_prompt_override: system_prompt,
        append_system_prompt,
        additional_roots,
    }
}

fn run_dev_stack() -> anyhow::Result<()> {
    let script = trogon_dev_script()?;
    let status = std::process::Command::new("bash").arg(script).status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    trogon_cli::env_local::load_env_local();
    let args = Args::parse();

    init_tracing(args.verbose);

    if matches!(args.command, Some(Command::Dev)) {
        return run_dev_stack();
    }

    if args.doctor || matches!(args.command, Some(Command::Doctor)) {
        return trogon_cli::doctor::run(&args.nats_url).await;
    }

    let cwd = std::env::current_dir()?;

    let session_init = build_session_init(
        args.system_prompt.clone(),
        args.append_system_prompt.clone(),
        &args.add_dir,
    );

    // MED-40: keep `nats_server` in a named binding (not `_child`) so Drop is
    // visible and we can call `drop()` explicitly before `process::exit`.
    let (nats, nats_server) = connect_or_start_nats(&args.nats_url, Duration::from_secs(3)).await?;

    if let Some(prompt_arg) = &args.print {
        let prompt = if prompt_arg == "-" {
            use std::io::Read as _;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf.trim().to_string()
        } else {
            prompt_arg.clone()
        };
        if prompt.is_empty() {
            eprintln!("error: prompt is empty — pass a string or pipe text to stdin");
            // MED-40: drop KillOnDrop guard so the autostarted NATS server is killed.
            drop(nats_server);
            std::process::exit(1);
        }
        let format = if args.output_format == "json" { OutputFormat::Json } else { OutputFormat::Text };
        let options = PrintOptions { print_tools: args.print_tools };

        // Resolve the target runner prefix: if a model is requested, look it up in
        // the registry so cross-runner models (e.g. `--model haiku` while prefix is
        // `acp.grok`) route to the correct runner instead of failing with "unknown model".
        let target_prefix = if let Some(model) = &args.model {
            let model_id = resolve_model_alias(model);
            let js = async_nats::jetstream::new(nats.clone());
            let reg_store = trogon_registry::ReprovisioningStore::new(js).await
                .map_err(|e| anyhow::anyhow!("registry provisioning failed: {e}"))?;
            let registry = trogon_registry::Registry::new(reg_store);
            match registry.find_by_model(&model_id).await {
                Ok(Some(cap)) => cap.metadata["acp_prefix"]
                    .as_str()
                    .unwrap_or(&args.prefix)
                    .to_string(),
                _ => args.prefix.clone(),
            }
        } else {
            args.prefix.clone()
        };

        let session =
            TrogonSession::new_with_init(nats, &target_prefix, cwd, vec![], &session_init).await?;
        if args.dangerously_skip_permissions
            && let Err(e) = session.set_mode("bypassPermissions").await
        {
            eprintln!("warning: could not set bypassPermissions: {e}");
        }
        if let Some(model) = &args.model {
            let model_id = resolve_model_alias(model);
            if let Err(e) = session.set_model(&model_id).await {
                eprintln!("error: could not set model: {e}");
                // MED-40: drop KillOnDrop guard before exit.
                drop(nats_server);
                std::process::exit(1);
            }
        }
        let code = trogon_cli::print::run(session, &prompt, format, options).await;
        // MED-40: explicitly drop the KillOnDrop guard so the auto-started NATS
        // server process is killed before process::exit bypasses normal Drop.
        drop(nats_server);
        std::process::exit(code as i32);
    } else {
        let acp_prefix = AcpPrefix::new(&args.prefix)
            .map_err(|e| anyhow::anyhow!("invalid ACP prefix: {e}"))?;
        let nats_config = NatsConfig::new(vec![args.nats_url.clone()], NatsAuth::None);
        let acp_config = Config::new(acp_prefix, nats_config);
        let js = async_nats::jetstream::new(nats.clone());
        // MED-36: use a store that re-provisions the (in-memory) AGENT_REGISTRY
        // bucket on failure, so /model and /status keep working after a NATS server
        // restart instead of erroring until the CLI is restarted.
        let reg_store = trogon_registry::ReprovisioningStore::new(js).await
            .map_err(|e| anyhow::anyhow!("registry provisioning failed: {e}"))?;
        let registry = trogon_registry::Registry::new(reg_store);
        let registry_for_repl = registry.clone();
        let switcher = CrossRunnerSwitcher::new(nats.clone(), acp_config.clone(), registry);
        let factory = NatsSessionFactory::new(nats.clone());

        let resume = if args.continue_session {
            let index = SessionIndex::load(&RealFs);
            let canon = match cwd.canonicalize() {
                Ok(c) => c,
                Err(e) => {
                    // LOW-21: warn when the working directory can no longer be resolved
                    // (e.g. deleted between sessions) so the user knows why lookup may fail.
                    eprintln!("warning: current directory could not be resolved ({e}), session lookup may fail");
                    cwd.clone()
                }
            };
            index.get_last(&canon).cloned()
        } else if let Some(id) = &args.session_id {
            Some(SessionEntry {
                prefix: args.prefix.clone(),
                session_id: id.clone(),
                model: String::new(),
                updated_at: String::new(),
                name: None,
            })
        } else {
            None
        };

        if args.continue_session && resume.is_none() {
            eprintln!("warning: no saved session for this project — starting fresh");
        }

        trogon_cli::runtime::run_interactive(
            factory,
            &args.prefix,
            cwd,
            RealFs,
            switcher,
            registry_for_repl,
            nats,
            acp_config,
            args.nats_url,
            args.stream,
            resume,
            args.dangerously_skip_permissions,
            args.plan,
            session_init,
            args.name,
        )
        .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::build_session_init;
    use std::path::PathBuf;

    #[test]
    fn prompts_flow_into_init_without_add_dir() {
        let init = build_session_init(Some("over".into()), Some("app".into()), &[]);
        assert_eq!(init.system_prompt_override.as_deref(), Some("over"));
        assert_eq!(init.append_system_prompt.as_deref(), Some("app"));
        assert!(init.additional_roots.is_empty());
    }

    #[test]
    fn existing_dir_is_canonicalised_into_roots() {
        // The current directory always exists and is a directory.
        let cwd = std::env::current_dir().unwrap();
        let init = build_session_init(None, None, &[cwd.clone()]);
        let expected = cwd.canonicalize().unwrap().to_string_lossy().into_owned();
        assert_eq!(init.additional_roots, vec![expected]);
    }

    #[test]
    fn missing_dir_is_skipped_not_fatal() {
        let init = build_session_init(None, None, &[PathBuf::from("/no/such/dir/xyzzy")]);
        assert!(init.additional_roots.is_empty());
    }
}

