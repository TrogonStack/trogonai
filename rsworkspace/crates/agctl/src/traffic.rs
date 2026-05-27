use std::process::ExitCode;

use clap::{Args, Subcommand};
use trogon_traffic_view::indexer::postgres::PostgresIndexer;
use trogon_traffic_view::indexer::TrafficIndex;
use trogon_traffic_view::{parse_since, render_table, TrafficQueryFilter};

#[derive(Args, Debug)]
pub struct TrafficCommand {
    #[command(subcommand)]
    pub command: TrafficSubcommand,

    #[arg(long, env = "TROGON_PG_URL", hide_env_values = true)]
    pub database_url: String,
}

#[derive(Subcommand, Debug)]
pub enum TrafficSubcommand {
    /// Query indexed traffic events in a time window.
    Query(QueryArgs),
    /// Tail recent traffic events for a tenant.
    Tail(TailArgs),
}

#[derive(Args, Debug)]
pub struct QueryArgs {
    #[arg(long, default_value = "1h")]
    pub since: String,

    #[arg(long)]
    pub tenant: String,

    #[arg(long)]
    pub agent_id: Option<String>,

    #[arg(long, default_value_t = 100)]
    pub limit: u32,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct TailArgs {
    #[arg(long)]
    pub tenant: String,

    #[arg(long, default_value = "5m")]
    pub since: String,

    #[arg(long, default_value_t = 50)]
    pub limit: u32,

    #[arg(long)]
    pub json: bool,
}

pub async fn run(traffic: TrafficCommand) -> ExitCode {
    let indexer = match PostgresIndexer::connect(&traffic.database_url).await {
        Ok(indexer) => indexer,
        Err(error) => {
            eprintln!("connect postgres: {error}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = indexer.migrate().await {
        eprintln!("migrate postgres: {error}");
        return ExitCode::FAILURE;
    }

    match traffic.command {
        TrafficSubcommand::Query(args) => run_query(&indexer, args).await,
        TrafficSubcommand::Tail(args) => run_tail(&indexer, args).await,
    }
}

async fn run_query(indexer: &PostgresIndexer, args: QueryArgs) -> ExitCode {
    let since = match parse_since(&args.since) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("invalid --since: {error}");
            return ExitCode::FAILURE;
        }
    };

    let filter = TrafficQueryFilter {
        tenant: Some(args.tenant),
        agent_id: args.agent_id,
        since: Some(since),
        until: None,
        limit: Some(args.limit),
    };

    match indexer.query(filter).await {
        Ok(events) => {
            if args.json {
                match serde_json::to_string_pretty(&events) {
                    Ok(json) => println!("{json}"),
                    Err(error) => {
                        eprintln!("encode json: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                println!("{}", render_table(&events));
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("query failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_tail(indexer: &PostgresIndexer, args: TailArgs) -> ExitCode {
    let since = match parse_since(&args.since) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("invalid --since: {error}");
            return ExitCode::FAILURE;
        }
    };

    match indexer.tail(&args.tenant, Some(since), args.limit).await {
        Ok(events) => {
            if args.json {
                match serde_json::to_string_pretty(&events) {
                    Ok(json) => println!("{json}"),
                    Err(error) => {
                        eprintln!("encode json: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                println!("{}", render_table(&events));
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("tail failed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser, Debug)]
    struct Cli {
        #[command(subcommand)]
        command: TestCommand,
    }

    #[derive(Subcommand, Debug)]
    enum TestCommand {
        Traffic(TrafficCommand),
    }

    #[test]
    fn query_args_parse() {
        let cli = Cli::try_parse_from([
            "agctl",
            "traffic",
            "--database-url",
            "postgres://localhost/traffic",
            "query",
            "--since",
            "1h",
            "--tenant",
            "acme",
            "--agent-id",
            "acme/agent1",
        ])
        .expect("parse");
        let TestCommand::Traffic(traffic) = cli.command;
        let TrafficSubcommand::Query(query) = traffic.command else {
            panic!("expected query");
        };
        assert_eq!(query.tenant, "acme");
        assert_eq!(query.agent_id.as_deref(), Some("acme/agent1"));
    }

    #[test]
    fn tail_args_parse() {
        let cli = Cli::try_parse_from([
            "agctl",
            "traffic",
            "--database-url",
            "postgres://localhost/traffic",
            "tail",
            "--tenant",
            "acme",
        ])
        .expect("parse");
        let TestCommand::Traffic(traffic) = cli.command;
        let TrafficSubcommand::Tail(tail) = traffic.command else {
            panic!("expected tail");
        };
        assert_eq!(tail.tenant, "acme");
    }
}
