use std::time::Duration;

use trogon_nats::connect;
use trogon_std::env::SystemEnv;
use trogon_outcomes::{
    AnthropicEvaluationProvider, Evaluator, EvaluationService, OutcomesConfig,
    provision_rubrics_kv, provision_results_kv, provision_stream, serve,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = OutcomesConfig::from_env(&SystemEnv);
    let nats = connect(&config.nats, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");

    let js = async_nats::jetstream::new(nats);

    provision_stream(&js).await.expect("Failed to provision SESSION_EVALUATIONS stream");
    let rubric_kv = provision_rubrics_kv(&js).await.expect("Failed to provision rubrics bucket");
    let result_kv = provision_results_kv(&js).await.expect("Failed to provision results bucket");

    let provider = AnthropicEvaluationProvider::new(config.llm);
    let evaluator = Evaluator::new(provider, rubric_kv.clone(), result_kv.clone());
    let service = EvaluationService::new(js, config.consumer_name, evaluator);

    tokio::join!(
        async move {
            serve(config.port, rubric_kv, result_kv)
                .await
                .expect("management API failed")
        },
        async move { service.run().await.ok() },
    );
}
