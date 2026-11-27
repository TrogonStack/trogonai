use clap::Parser;
use trogon_sandbox_credential_broker::BrokerArgs;
use trogon_sandbox_observability::{CREDENTIAL_BROKER, bootstrap};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = BrokerArgs::parse();
    let observability = &args.observability;
    let _guards = bootstrap("trogon-sandbox-credential-broker", CREDENTIAL_BROKER, observability)?;
    args.serve(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
