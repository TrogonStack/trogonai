use acp_nats::{AcpPrefix, Config, NatsAuth, NatsConfig};
use clap::Parser;
use std::time::Duration;
use trogon_cli::{
    connect_or_start_nats, session::TrogonSession, CrossRunnerSwitcher, NatsSessionFactory,
    OutputFormat, RealFs,
};

#[derive(Parser)]
#[command(name = "trogon", about = "Trogon AI CLI")]
struct Args {
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
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
        let session = TrogonSession::new(nats, &args.prefix, cwd).await?;
        let result = trogon_cli::print::run(session, &prompt, format).await;
        if let Err(e) = result {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    } else {
        let acp_prefix = AcpPrefix::new(&args.prefix)
            .map_err(|e| anyhow::anyhow!("invalid ACP prefix: {e}"))?;
        let nats_config = NatsConfig::new(vec![args.nats_url.clone()], NatsAuth::None);
        let acp_config = Config::new(acp_prefix, nats_config);
        let js = async_nats::jetstream::new(nats.clone());
        let reg_store = trogon_registry::provision(&js).await
            .map_err(|e| anyhow::anyhow!("registry provisioning failed: {e}"))?;
        let registry = trogon_registry::Registry::new(reg_store);
        let switcher = CrossRunnerSwitcher::new(nats.clone(), acp_config, registry);
        let factory = NatsSessionFactory::new(nats);
        trogon_cli::repl::run(factory, &args.prefix, cwd, RealFs, switcher).await?;
    }

    Ok(())
}

