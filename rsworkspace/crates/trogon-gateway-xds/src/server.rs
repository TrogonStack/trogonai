//! SotW Aggregated Discovery Service (ADS) for Envoy xDS v3.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use prost::Message;
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tonic::{Request, Response, Status, Streaming};

use crate::mapping::MappedResources;
use crate::proto::envoy::service::discovery::v3::{
    DeltaDiscoveryRequest, DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse,
};
use crate::proto::AggregatedDiscoveryService;
use crate::state::{ConfigStore, NodeSnapshot};
use crate::type_urls;

#[derive(Clone)]
pub struct AdsServerOpts {
    pub default_node_id: String,
}

impl Default for AdsServerOpts {
    fn default() -> Self {
        Self {
            default_node_id: "trogon-gateway".into(),
        }
    }
}

pub struct AdsServer {
    store: ConfigStore,
    opts: AdsServerOpts,
    nonce: Arc<AtomicU64>,
}

impl AdsServer {
    pub fn new(store: ConfigStore, opts: AdsServerOpts) -> Self {
        Self {
            store,
            opts,
            nonce: Arc::new(AtomicU64::new(1)),
        }
    }

    fn next_nonce(&self) -> String {
        self.nonce.fetch_add(1, Ordering::Relaxed).to_string()
    }

    fn resolve_node<'a>(
        &'a self,
        request: &'a DiscoveryRequest,
    ) -> Result<Arc<NodeSnapshot>, Status> {
        let node_id = request
            .node
            .as_ref()
            .and_then(|node| {
                if node.id.is_empty() {
                    None
                } else {
                    Some(node.id.as_str())
                }
            })
            .unwrap_or(self.opts.default_node_id.as_str());

        self.store
            .get(node_id)
            .ok_or_else(|| Status::not_found(format!("no snapshot for node `{node_id}`")))
    }
}

struct StreamState {
    acked_versions: HashMap<String, String>,
    sent_versions: HashMap<String, String>,
    last_nonce: HashMap<String, String>,
}

impl StreamState {
    fn new() -> Self {
        Self {
            acked_versions: HashMap::new(),
            sent_versions: HashMap::new(),
            last_nonce: HashMap::new(),
        }
    }

    fn register_ack(&mut self, request: &DiscoveryRequest) {
        let Some(type_url) = non_empty_type_url(request) else {
            return;
        };
        if request.has_error_detail() {
            self.sent_versions.remove(&type_url);
            return;
        }
        if let Some(sent_version) = self.sent_versions.get(&type_url) {
            if &request.version_info == sent_version {
                self.acked_versions
                    .insert(type_url.clone(), request.version_info.clone());
            }
        }
    }

    fn should_push(&self, type_url: &str, version: &str) -> bool {
        self.acked_versions.get(type_url) != Some(&version.to_string())
            && self.sent_versions.get(type_url) != Some(&version.to_string())
    }

    fn mark_sent(&mut self, type_url: &str, version: &str, nonce: &str) {
        self.sent_versions
            .insert(type_url.to_string(), version.to_string());
        self.last_nonce
            .insert(type_url.to_string(), nonce.to_string());
    }
}

#[async_trait]
impl AggregatedDiscoveryService for AdsServer {
    type StreamAggregatedResourcesStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<DiscoveryResponse, Status>> + Send>>;

    async fn stream_aggregated_resources(
        &self,
        request: Request<Streaming<DiscoveryRequest>>,
    ) -> Result<Response<Self::StreamAggregatedResourcesStream>, Status> {
        let mut inbound = request.into_inner();
        let (tx, rx) = mpsc::channel(16);
        let server = self.clone();
        tokio::spawn(async move {
            let mut state = StreamState::new();
            while let Some(message) = inbound.next().await {
                let request = match message {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = tx.send(Err(error)).await;
                        break;
                    }
                };
                state.register_ack(&request);
                match server.handle_discovery_request(&request, &mut state) {
                    Ok(responses) => {
                        for response in responses {
                            if let Err(error) = tx.send(Ok(response)).await {
                                tracing::debug!(%error, "ADS stream consumer dropped");
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error)).await;
                        break;
                    }
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    type DeltaAggregatedResourcesStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<DeltaDiscoveryResponse, Status>> + Send>>;

    async fn delta_aggregated_resources(
        &self,
        _request: Request<Streaming<DeltaDiscoveryRequest>>,
    ) -> Result<Response<Self::DeltaAggregatedResourcesStream>, Status> {
        Err(Status::unimplemented("delta ADS is out of scope for this pass"))
    }
}

impl AdsServer {
    fn handle_discovery_request(
        &self,
        request: &DiscoveryRequest,
        state: &mut StreamState,
    ) -> Result<Vec<DiscoveryResponse>, Status> {
        let snapshot = self.resolve_node(request)?;
        let version = snapshot.config.version.clone();
        let type_urls = subscribed_type_urls(request);

        let mut responses = Vec::new();
        for type_url in type_urls {
            if !state.should_push(&type_url, &version) {
                continue;
            }
            let resources = resources_for_type(&type_url, &snapshot.mapped)?;
            if resources.is_empty() {
                continue;
            }
            let nonce = self.next_nonce();
            state.mark_sent(&type_url, &version, &nonce);
            responses.push(DiscoveryResponse {
                version_info: version.clone(),
                type_url: type_url.clone(),
                resources,
                nonce,
                ..Default::default()
            });
        }
        Ok(responses)
    }
}

impl Clone for AdsServer {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            opts: self.opts.clone(),
            nonce: Arc::clone(&self.nonce),
        }
    }
}

fn non_empty_type_url(request: &DiscoveryRequest) -> Option<String> {
    if request.type_url.is_empty() {
        None
    } else {
        Some(request.type_url.clone())
    }
}

fn subscribed_type_urls(request: &DiscoveryRequest) -> Vec<String> {
    if request.type_url.is_empty() {
        type_urls::ALL.iter().map(|value| (*value).to_string()).collect()
    } else {
        vec![request.type_url.clone()]
    }
}

fn resources_for_type(
    type_url: &str,
    mapped: &MappedResources,
) -> Result<Vec<prost_types::Any>, Status> {
    let mut resources = Vec::new();
    match type_url {
        type_urls::LISTENER => {
            for listener in &mapped.listeners {
                resources.push(encode_resource(type_url, listener)?);
            }
        }
        type_urls::ROUTE_CONFIGURATION => {
            for route in &mapped.routes {
                resources.push(encode_resource(type_url, route)?);
            }
        }
        type_urls::CLUSTER => {
            for cluster in &mapped.clusters {
                resources.push(encode_resource(type_url, cluster)?);
            }
        }
        type_urls::CLUSTER_LOAD_ASSIGNMENT => {
            for assignment in &mapped.assignments {
                resources.push(encode_resource(type_url, assignment)?);
            }
        }
        type_urls::RBAC => {
            for (_name, rbac) in &mapped.rbac_filters {
                resources.push(encode_resource(type_url, rbac)?);
            }
        }
        other => {
            return Err(Status::invalid_argument(format!(
                "unsupported type_url `{other}`"
            )));
        }
    }
    Ok(resources)
}

fn encode_resource<T: Message>(type_url: &str, message: &T) -> Result<prost_types::Any, Status> {
    Ok(prost_types::Any {
        type_url: type_url.to_string(),
        value: message.encode_to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_store_with_fixture;

    #[test]
    fn ack_prevents_duplicate_push_for_same_version() {
        let mut state = StreamState::new();
        state.mark_sent(type_urls::CLUSTER, "v1", "1");
        assert!(!state.should_push(type_urls::CLUSTER, "v1"));
        state.register_ack(&DiscoveryRequest {
            version_info: "v1".into(),
            type_url: type_urls::CLUSTER.into(),
            ..Default::default()
        });
        assert!(!state.should_push(type_urls::CLUSTER, "v1"));
        assert!(state.should_push(type_urls::CLUSTER, "v2"));
    }

    #[test]
    fn nack_clears_sent_version() {
        let mut state = StreamState::new();
        state.mark_sent(type_urls::LISTENER, "v1", "1");
        state.register_ack(&DiscoveryRequest {
            version_info: "v1".into(),
            type_url: type_urls::LISTENER.into(),
            error_detail: Some(crate::proto::google::rpc::Status {
                message: "nack".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert!(state.should_push(type_urls::LISTENER, "v1"));
    }

    #[test]
    fn handle_request_emits_cluster_resources() {
        let store = test_store_with_fixture("envoy-1");
        let server = AdsServer::new(store, AdsServerOpts::default());
        let mut state = StreamState::new();
        let responses = server
            .handle_discovery_request(
                &DiscoveryRequest {
                    type_url: type_urls::CLUSTER.into(),
                    node: Some(crate::proto::envoy::config::core::v3::Node {
                        id: "envoy-1".into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                &mut state,
            )
            .expect("cluster response");
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].version_info, "fixture-v1");
        assert_eq!(responses[0].resources.len(), 1);
    }
}

trait DiscoveryRequestExt {
    fn has_error_detail(&self) -> bool;
}

impl DiscoveryRequestExt for DiscoveryRequest {
    fn has_error_detail(&self) -> bool {
        self.error_detail.is_some()
    }
}
