mod support;

use support::{Reply, Server, TestResult};

#[tokio::test]
async fn the_fake_answers_a_request() -> TestResult {
    let server = Server::start(|_| Reply::json(200, serde_json::json!({}))).await?;
    server.api()?;
    server.exec()?;
    server.shutdown().await;
    Ok(())
}
