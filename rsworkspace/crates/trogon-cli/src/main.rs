mod print;
mod repl;
mod session;

use clap::Parser;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "trogon", about = "Trogon AI CLI")]
struct Args {
    /// NATS server URL (overrides TROGON_NATS_URL)
    #[arg(long, env = "TROGON_NATS_URL", default_value = "nats://localhost:4222")]
    nats_url: String,

    /// ACP prefix (overrides ACP_PREFIX)
    #[arg(long, env = "ACP_PREFIX", default_value = "acp")]
    prefix: String,

    /// Non-interactive mode: send this prompt and print result to stdout
    #[arg(short, long)]
    print: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cwd = std::env::current_dir()?;

    let (nats, _child) = connect_or_start_nats(&args.nats_url).await?;

    if let Some(prompt) = &args.print {
        let result = print::run(nats, &args.prefix, cwd, prompt).await;
        if let Err(e) = result {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    } else {
        repl::run(nats, &args.prefix, cwd).await?;
    }

    Ok(())
}

/// Connect to NATS. If the first attempt fails and `nats-server` is in PATH,
/// start it as a child process and retry for up to 3 seconds.
async fn connect_or_start_nats(url: &str) -> anyhow::Result<(async_nats::Client, Option<Child>)> {
    if let Ok(client) = async_nats::connect(url).await {
        return Ok((client, None));
    }

    // Try to launch nats-server
    let child = match Command::new("nats-server").args(["-p", "4222"]).spawn() {
        Ok(c) => c,
        Err(_) => {
            return Err(anyhow::anyhow!(
                "Could not connect to NATS at {url} and nats-server is not in PATH.\n\
                 Install it: https://docs.nats.io/running-a-nats-service/introduction/installation"
            ));
        }
    };

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!("nats-server started but not accepting connections after 3s"));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        if let Ok(client) = async_nats::connect(url).await {
            return Ok((client, Some(child)));
        }
    }
}

fn _cwd_unused(_: PathBuf) {}
