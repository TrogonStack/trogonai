use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};

pub async fn start() -> (ContainerAsync<Nats>, async_nats::Client) {
    // Scheduler command writes require atomic publish, absent from the shared fixture's NATS 2.10.
    let container = Nats::default()
        .with_cmd(&NatsServerCmd::default().with_jetstream())
        .with_tag("2.14.2-alpine")
        .start()
        .await
        .expect("start scheduler NATS container");
    let host = container.get_host().await.expect("NATS host");
    let port = container.get_host_port_ipv4(4222).await.expect("NATS port");
    let client = async_nats::connect(format!("nats://{host}:{port}"))
        .await
        .expect("connect scheduler NATS");
    (container, client)
}
