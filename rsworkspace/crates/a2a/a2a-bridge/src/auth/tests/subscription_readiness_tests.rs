use std::net::SocketAddr;

use futures_util::StreamExt as _;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::*;

const WAIT: Duration = Duration::from_secs(5);

pub(super) async fn wait_for_callout_subscription(
    client: &async_nats::Client,
    deadline: Duration,
) -> Result<(), tokio::time::error::Elapsed> {
    tokio::time::timeout(deadline, async {
        let inbox = client.new_inbox();
        let mut echo = client.subscribe(inbox.clone()).await.unwrap();
        client
            .publish(inbox.clone(), inbox.as_bytes().to_vec().into())
            .await
            .unwrap();
        let echoed = echo
            .next()
            .await
            .expect("broker closed before confirming the subscription");
        assert_eq!(echoed.subject.as_str(), inbox);
        assert_eq!(echoed.payload.as_ref(), inbox.as_bytes());
    })
    .await
}

struct HeldSubscription {
    address: SocketAddr,
    held: oneshot::Receiver<()>,
    release: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl HeldSubscription {
    async fn new(server: &str, subject: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = server.to_owned();
        let prefix = format!("SUB {subject} ").into_bytes();
        let (held_tx, held) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (downstream, _) = listener.accept().await.unwrap();
            let upstream = TcpStream::connect(server).await.unwrap();
            let (downstream_read, mut downstream_write) = downstream.into_split();
            let (mut upstream_read, mut upstream_write) = upstream.into_split();
            let mut downstream_read = BufReader::new(downstream_read);
            let forward = async {
                let mut line = Vec::new();
                loop {
                    line.clear();
                    let read = downstream_read.read_until(b'\n', &mut line).await?;
                    assert_ne!(read, 0, "client closed before sending the service subscription");
                    if line.starts_with(&prefix) {
                        held_tx.send(()).unwrap();
                        release_rx.await.unwrap();
                        upstream_write.write_all(&line).await?;
                        return tokio::io::copy(&mut downstream_read, &mut upstream_write).await;
                    }
                    upstream_write.write_all(&line).await?;
                }
            };
            let reverse = tokio::io::copy(&mut upstream_read, &mut downstream_write);
            tokio::try_join!(forward, reverse).unwrap();
        });
        Self {
            address,
            held,
            release: Some(release),
            task,
        }
    }

    async fn wait_until_held(&mut self) {
        tokio::time::timeout(WAIT, &mut self.held).await.unwrap().unwrap();
    }

    fn release(&mut self) {
        self.release.take().unwrap().send(()).unwrap();
    }
}

impl Drop for HeldSubscription {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn readiness_waits_for_the_broker_even_when_socket_flush_has_finished() {
    let server = CoreTestServer::start().await;
    let subject = AuthCalloutJsonMintClient::<AsyncNatsAuthMintWire>::default_mint_subject();
    let mut proxy = HeldSubscription::new(server.address(), subject).await;
    let callout_client = async_nats::connect(format!("nats://{}", proxy.address)).await.unwrap();
    let bridge_client = async_nats::connect(server.address()).await.unwrap();
    let (dispatcher, tenant) = harness_callout_dispatcher("controlled-subscription");
    let callout = harness_callout_xkey();
    let xkey = callout.public_key();
    let subscriber = BridgeMintSubscriber::subscribe(
        callout_client.clone(),
        BridgeMintResponder::new(dispatcher, Some(callout)),
        &InMemoryEnv::new(),
    )
    .await
    .unwrap();
    let service = tokio::spawn(subscriber.serve());
    proxy.wait_until_held().await;

    let mut ready = Box::pin(wait_for_callout_subscription(&callout_client, WAIT));
    assert!(futures_util::poll!(ready.as_mut()).is_pending());
    callout_client.flush().await.unwrap();
    let ready_before_delivery = futures_util::poll!(ready.as_mut()).is_ready();
    let absent = bridge_client
        .request(subject, b"{}".as_slice().into())
        .await
        .unwrap_err();
    assert_eq!(absent.kind(), async_nats::RequestErrorKind::NoResponders);

    proxy.release();
    if !ready_before_delivery {
        ready.await.unwrap();
    }
    let wire = Arc::new(AsyncNatsAuthMintWire::from_client(bridge_client, WAIT));
    let client = AuthCalloutJsonMintClient::with_tenant_account(wire, subject, Some(tenant)).sealing_to(xkey);
    let jwt = client
        .mint(&CallerHttpsAuth::new("Bearer fixture-token"))
        .await
        .expect("mint after the broker confirms the subscription");
    assert_eq!(jwt.as_str().split('.').count(), 3);
    callout_client.drain().await.unwrap();
    tokio::time::timeout(WAIT, service).await.unwrap().unwrap();

    assert!(
        !ready_before_delivery,
        "readiness completed while the broker had no subscription"
    );
}

#[tokio::test]
async fn readiness_times_out_when_the_broker_never_receives_the_subscription() {
    let server = CoreTestServer::start().await;
    let subject = "held.service";
    let mut proxy = HeldSubscription::new(server.address(), subject).await;
    let client = async_nats::connect(format!("nats://{}", proxy.address)).await.unwrap();
    let _subscription = client.subscribe(subject).await.unwrap();
    proxy.wait_until_held().await;

    let ready = wait_for_callout_subscription(&client, Duration::from_millis(50)).await;

    proxy.release();
    assert!(ready.is_err(), "a local flush must not claim server readiness");
}
