mod cmd;
mod nats;
mod output;
mod settings;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use cmd::bundle::{self, BundleOutcome};
use cmd::config::{self, ConfigFormat};
use cmd::policy::{self, PolicyOutcome};

use settings::CtlSettings;

mod exit_codes {
    pub const SUCCESS: u8 = 0;
    pub const VALIDATION: u8 = 1;
    pub const TRANSPORT: u8 = 2;
    pub const USAGE: u8 = 3;
}

#[derive(Parser, Debug)]
#[command(
    name = "trogon-gateway-ctl",
    about = "Operator CLI for the MCP gateway",
    version
)]
struct Cli {
    #[arg(long, global = true, help = "Gateway/NATS settings TOML overlay")]
    config: Option<PathBuf>,

    #[arg(long, global = true, help = "Pretty-print JSON output")]
    pretty: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Config(ConfigCommand),
    Trace(TraceArgs),
    Bundle(BundleCommand),
    Policy(PolicyCommand),
    Audit(AuditCommand),
}

#[derive(Args, Debug)]
struct ConfigCommand {
    #[command(subcommand)]
    command: ConfigSubcommand,
}

#[derive(Subcommand, Debug)]
enum ConfigSubcommand {
    Show(ConfigShowArgs),
}

#[derive(Args, Debug)]
struct ConfigShowArgs {
    #[arg(long, value_enum, default_value_t = ConfigFormat::Json)]
    format: ConfigFormat,
}

#[derive(Args, Debug)]
struct TraceArgs {
    request_id: String,

    #[arg(long, default_value_t = 256)]
    limit: usize,
}

#[derive(Args, Debug)]
struct BundleCommand {
    #[command(subcommand)]
    command: BundleSubcommand,
}

#[derive(Subcommand, Debug)]
enum BundleSubcommand {
    Validate(BundleValidateArgs),
}

#[derive(Args, Debug)]
struct BundleValidateArgs {
    path: PathBuf,

    #[arg(long, help = "Trusted signer NKey public keys, one per line")]
    trusted_keys: PathBuf,
}

#[derive(Args, Debug)]
struct PolicyCommand {
    #[command(subcommand)]
    command: PolicySubcommand,
}

#[derive(Subcommand, Debug)]
enum PolicySubcommand {
    DryRun(PolicyDryRunArgs),
}

#[derive(Args, Debug)]
struct PolicyDryRunArgs {
    #[arg(long)]
    policy: PathBuf,

    #[arg(long)]
    input: PathBuf,
}

#[derive(Args, Debug)]
struct AuditCommand {
    #[command(subcommand)]
    command: AuditSubcommand,
}

#[derive(Subcommand, Debug)]
enum AuditSubcommand {
    Tail(AuditTailArgs),
}

#[derive(Args, Debug)]
struct AuditTailArgs {
    #[arg(long, default_value_t = 32)]
    max: usize,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            use clap::error::ErrorKind;
            if matches!(error.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                error.print().expect("print clap help");
                return ExitCode::from(exit_codes::SUCCESS);
            }
            error.print().expect("print clap error");
            return ExitCode::from(exit_codes::USAGE);
        }
    };

    let settings = match CtlSettings::from_env_and_config(cli.config.as_deref()) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(exit_codes::USAGE);
        }
    };

    match cli.command {
        Command::Config(config_cmd) => match config_cmd.command {
            ConfigSubcommand::Show(args) => match config::run(&settings, args.format, cli.pretty) {
                Ok(()) => ExitCode::from(exit_codes::SUCCESS),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(exit_codes::TRANSPORT)
                }
            },
        },
        Command::Trace(args) => match cmd::trace::run(&settings, &args.request_id, args.limit, cli.pretty).await {
            Ok(()) => ExitCode::from(exit_codes::SUCCESS),
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(exit_codes::TRANSPORT)
            }
        },
        Command::Bundle(bundle_cmd) => match bundle_cmd.command {
            BundleSubcommand::Validate(args) => match bundle::run(&args.path, &args.trusted_keys, cli.pretty) {
                Ok(()) => ExitCode::from(exit_codes::SUCCESS),
                Err(outcome) => {
                    let _ = outcome.emit_error_json(cli.pretty);
                    eprintln!("{}", outcome.message());
                    let code = match &outcome {
                        BundleOutcome::Validation(_) => exit_codes::VALIDATION,
                        BundleOutcome::Runtime(_) => exit_codes::TRANSPORT,
                    };
                    ExitCode::from(code)
                }
            },
        },
        Command::Policy(policy_cmd) => match policy_cmd.command {
            PolicySubcommand::DryRun(args) => match policy::run(&args.policy, &args.input, cli.pretty) {
                Ok(()) => ExitCode::from(exit_codes::SUCCESS),
                Err(outcome) => {
                    let _ = outcome.emit_error_json(cli.pretty);
                    eprintln!("{}", outcome.message());
                    let code = match &outcome {
                        PolicyOutcome::Evaluation(_) => exit_codes::VALIDATION,
                        PolicyOutcome::Runtime(_) => exit_codes::TRANSPORT,
                    };
                    ExitCode::from(code)
                }
            },
        },
        Command::Audit(audit_cmd) => match audit_cmd.command {
            AuditSubcommand::Tail(args) => match cmd::audit::run(&settings, args.max, cli.pretty).await {
                Ok(()) => ExitCode::from(exit_codes::SUCCESS),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(exit_codes::TRANSPORT)
                }
            },
        },
    }
}
