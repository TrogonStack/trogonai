use acp_nats::{AcpPrefix, Config, NatsAuth, NatsConfig};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;
use trogon_cli::{
    connect_or_start_nats, repl::resolve_model_alias, session::TrogonSession, CrossRunnerSwitcher,
    NatsSessionFactory, OutputFormat, PrintOptions, RealFs, SessionEntry, SessionIndex,
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
    let args = Args::parse();

    if matches!(args.command, Some(Command::Dev)) {
        return run_dev_stack();
    }

    if args.doctor || matches!(args.command, Some(Command::Doctor)) {
        return trogon_cli::doctor::run(&args.nats_url).await;
    }

    let cwd = std::env::current_dir()?;

    let (nats, _child) = connect_or_start_nats(&args.nats_url, Duration::from_secs(3)).await?;

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
            std::process::exit(1);
        }
        let format = if args.output_format == "json" { OutputFormat::Json } else { OutputFormat::Text };
        let options = PrintOptions { print_tools: args.print_tools };
        let session = TrogonSession::new(nats, &args.prefix, cwd, vec![]).await?;
        if args.dangerously_skip_permissions {
            if let Err(e) = session.set_mode("bypassPermissions").await {
                eprintln!("warning: could not set bypassPermissions: {e}");
            }
        }
        if let Some(model) = &args.model {
            let model_id = resolve_model_alias(model);
            if let Err(e) = session.set_model(&model_id).await {
                eprintln!("error: could not set model: {e}");
                std::process::exit(1);
            }
        }
        let code = trogon_cli::print::run(session, &prompt, format, options).await;
        std::process::exit(code as i32);
    } else {
        let acp_prefix = AcpPrefix::new(&args.prefix)
            .map_err(|e| anyhow::anyhow!("invalid ACP prefix: {e}"))?;
        let nats_config = NatsConfig::new(vec![args.nats_url.clone()], NatsAuth::None);
        let acp_config = Config::new(acp_prefix, nats_config);
        let js = async_nats::jetstream::new(nats.clone());
        let reg_store = trogon_registry::provision(&js).await
            .map_err(|e| anyhow::anyhow!("registry provisioning failed: {e}"))?;
        let registry = trogon_registry::Registry::new(reg_store);
        let registry_for_repl = registry.clone();
        let switcher = CrossRunnerSwitcher::new(nats.clone(), acp_config.clone(), registry);
        let factory = NatsSessionFactory::new(nats.clone());

        let resume = if args.continue_session {
            let index = SessionIndex::load(&RealFs);
            let canon = cwd.canonicalize().unwrap_or(cwd.clone());
            index.get_last(&canon).cloned()
        } else if let Some(id) = &args.session_id {
            Some(SessionEntry {
                prefix: args.prefix.clone(),
                session_id: id.clone(),
                model: String::new(),
                updated_at: String::new(),
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
        )
        .await?;
    }

    Ok(())
}

