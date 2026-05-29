//! In-process ADS integration: connect a generated client and walk one ACK cycle.

use std::time::Duration;

use futures::StreamExt;
use tokio::net::TcpListener;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::{Endpoint, Server};
use tonic::Request;

use trogon_gateway_xds::config::fixtures::sample_snapshot;
use trogon_gateway_xds::proto::{
    AggregatedDiscoveryServiceClient, AggregatedDiscoveryServiceServer, DiscoveryRequest,
};
use trogon_gateway_xds::state::{ConfigStore, NodeSnapshot};
use trogon_gateway_xds::type_urls;
use trogon_gateway_xds::{AdsServer, AdsServerOpts};

#[tokio::test]
async fn ads_client_ack_cycle() {
    let node_id = "integration-envoy";
    let snapshot = NodeSnapshot::from_config(node_id, sample_snapshot());
    let version = snapshot.config.version.clone();
    let (store, _rx) = ConfigStore::new(std::collections::HashMap::from([(
        node_id.to_string(),
        std::sync::Arc::new(snapshot),
    )]));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let ads = AdsServer::new(
        store,
        AdsServerOpts {
            default_node_id: node_id.into(),
        },
    );
    tokio::spawn(async move {
        Server::builder()
            .add_service(AggregatedDiscoveryServiceServer::new(ads))
            .serve_with_incoming(incoming)
            .await
            .expect("serve");
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let channel = Endpoint::from_shared(format!("http://{addr}"))
        .expect("endpoint")
        .connect()
        .await
        .expect("connect");
    let mut client = AggregatedDiscoveryServiceClient::new(channel);

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tx.send(DiscoveryRequest {
        type_url: type_urls::CLUSTER.into(),
        node: Some(trogon_gateway_xds::proto::envoy::config::core::v3::Node {
            id: node_id.into(),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await
    .expect("send initial request");

    let response = client
        .stream_aggregated_resources(Request::new(ReceiverStream::new(rx)))
        .await
        .expect("open stream")
        .into_inner()
        .next()
        .await
        .expect("first response")
        .expect("response ok");

    assert_eq!(response.version_info, version);
    assert_eq!(response.type_url, type_urls::CLUSTER);
    assert!(!response.resources.is_empty());

    tx.send(DiscoveryRequest {
        version_info: response.version_info.clone(),
        response_nonce: response.nonce.clone(),
        type_url: response.type_url.clone(),
        node: Some(trogon_gateway_xds::proto::envoy::config::core::v3::Node {
            id: node_id.into(),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await
    .expect("send ack");

    tokio::time::sleep(Duration::from_millis(50)).await;

    let duplicate = tokio::time::timeout(Duration::from_millis(200), async {
        tx.send(DiscoveryRequest {
            type_url: type_urls::CLUSTER.into(),
            node: Some(trogon_gateway_xds::proto::envoy::config::core::v3::Node {
                id: node_id.into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .ok();
    })
    .await;
    assert!(duplicate.is_ok());
}
