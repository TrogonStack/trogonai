use std::fmt;

use trogon_peer_nats::{PeerRpcClient, PeerRpcClientError};
use trogonai_proto::peers::v1alpha1 as v1;

use crate::backend::{
    AcknowledgePeerBatch, DetachPeer, EnrollPeer, HeartbeatPeer, PreparePeerBatch, ReadPeerMessageStatus,
    ReadPeerRoster, SendPeerMessage,
};

pub struct RpcPeerBackend {
    client: PeerRpcClient,
}
impl RpcPeerBackend {
    pub fn new(client: PeerRpcClient) -> Self {
        Self { client }
    }
}
impl fmt::Debug for RpcPeerBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("RpcPeerBackend").finish_non_exhaustive()
    }
}

impl EnrollPeer for RpcPeerBackend {
    type Error = PeerRpcClientError;
    async fn enroll(&self, request: v1::EnrollRequest) -> Result<v1::EnrollResponse, Self::Error> {
        self.client.enroll(&request).await
    }
}

impl HeartbeatPeer for RpcPeerBackend {
    type Error = PeerRpcClientError;
    async fn heartbeat(&self, request: v1::HeartbeatRequest) -> Result<v1::HeartbeatResponse, Self::Error> {
        self.client.heartbeat(&request).await
    }
}

impl DetachPeer for RpcPeerBackend {
    type Error = PeerRpcClientError;
    async fn detach(&self, request: v1::DetachRequest) -> Result<v1::DetachResponse, Self::Error> {
        self.client.detach(&request).await
    }
}

impl ReadPeerRoster for RpcPeerBackend {
    type Error = PeerRpcClientError;
    async fn roster(&self, request: v1::RosterRequest) -> Result<v1::RosterResponse, Self::Error> {
        self.client.roster(&request).await
    }
}

impl SendPeerMessage for RpcPeerBackend {
    type Error = PeerRpcClientError;
    async fn send(&self, request: v1::SendRequest) -> Result<v1::SendResponse, Self::Error> {
        self.client.send(&request).await
    }
}

impl ReadPeerMessageStatus for RpcPeerBackend {
    type Error = PeerRpcClientError;
    async fn status(&self, request: v1::StatusRequest) -> Result<v1::StatusResponse, Self::Error> {
        self.client.status(&request).await
    }
}

impl PreparePeerBatch for RpcPeerBackend {
    type Error = PeerRpcClientError;
    async fn prepare_batch(&self, request: v1::PrepareBatchRequest) -> Result<v1::PrepareBatchResponse, Self::Error> {
        self.client.prepare_batch(&request).await
    }
}

impl AcknowledgePeerBatch for RpcPeerBackend {
    type Error = PeerRpcClientError;
    async fn acknowledge_batch(
        &self,
        request: v1::AcknowledgeBatchRequest,
    ) -> Result<v1::AcknowledgeBatchResponse, Self::Error> {
        self.client.acknowledge_batch(&request).await
    }
}
