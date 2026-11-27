// @generated
/// Generated client implementations.
pub mod control_client {
    #![allow(
        unused_variables,
        dead_code,
        missing_docs,
        clippy::wildcard_imports,
        clippy::let_unit_value,
    )]
    use tonic::codegen::*;
    use tonic::codegen::http::Uri;
    #[derive(Debug, Clone)]
    pub struct ControlClient<T> {
        inner: tonic::client::Grpc<T>,
    }
    impl ControlClient<tonic::transport::Channel> {
        /// Attempt to create a new client by connecting to a given endpoint.
        pub async fn connect<D>(dst: D) -> Result<Self, tonic::transport::Error>
        where
            D: TryInto<tonic::transport::Endpoint>,
            D::Error: Into<StdError>,
        {
            let conn = tonic::transport::Endpoint::new(dst)?.connect().await?;
            Ok(Self::new(conn))
        }
    }
    impl<T> ControlClient<T>
    where
        T: tonic::client::GrpcService<tonic::body::Body>,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + std::marker::Send + 'static,
        <T::ResponseBody as Body>::Error: Into<StdError> + std::marker::Send,
    {
        pub fn new(inner: T) -> Self {
            let inner = tonic::client::Grpc::new(inner);
            Self { inner }
        }
        pub fn with_origin(inner: T, origin: Uri) -> Self {
            let inner = tonic::client::Grpc::with_origin(inner, origin);
            Self { inner }
        }
        pub fn with_interceptor<F>(
            inner: T,
            interceptor: F,
        ) -> ControlClient<InterceptedService<T, F>>
        where
            F: tonic::service::Interceptor,
            T::ResponseBody: Default,
            T: tonic::codegen::Service<
                http::Request<tonic::body::Body>,
                Response = http::Response<
                    <T as tonic::client::GrpcService<tonic::body::Body>>::ResponseBody,
                >,
            >,
            <T as tonic::codegen::Service<
                http::Request<tonic::body::Body>,
            >>::Error: Into<StdError> + std::marker::Send + std::marker::Sync,
        {
            ControlClient::new(InterceptedService::new(inner, interceptor))
        }
        /// Compress requests with the given encoding.
        ///
        /// This requires the server to support it otherwise it might respond with an
        /// error.
        #[must_use]
        pub fn send_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.send_compressed(encoding);
            self
        }
        /// Enable decompressing responses.
        #[must_use]
        pub fn accept_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.accept_compressed(encoding);
            self
        }
        /// Limits the maximum size of a decoded message.
        ///
        /// Default: `4MB`
        #[must_use]
        pub fn max_decoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_decoding_message_size(limit);
            self
        }
        /// Limits the maximum size of an encoded message.
        ///
        /// Default: `usize::MAX`
        #[must_use]
        pub fn max_encoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_encoding_message_size(limit);
            self
        }
        pub async fn get_actor(
            &mut self,
            request: impl tonic::IntoRequest<super::GetActorRequest>,
        ) -> std::result::Result<tonic::Response<super::Actor>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/GetActor",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "GetActor"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn create_actor(
            &mut self,
            request: impl tonic::IntoRequest<super::CreateActorRequest>,
        ) -> std::result::Result<tonic::Response<super::Actor>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/CreateActor",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "CreateActor"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn update_actor(
            &mut self,
            request: impl tonic::IntoRequest<super::UpdateActorRequest>,
        ) -> std::result::Result<tonic::Response<super::Actor>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/UpdateActor",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "UpdateActor"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn suspend_actor(
            &mut self,
            request: impl tonic::IntoRequest<super::SuspendActorRequest>,
        ) -> std::result::Result<
            tonic::Response<super::SuspendActorResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/SuspendActor",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "SuspendActor"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn resume_actor(
            &mut self,
            request: impl tonic::IntoRequest<super::ResumeActorRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ResumeActorResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/ResumeActor",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "ResumeActor"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn revert_actor(
            &mut self,
            request: impl tonic::IntoRequest<super::RevertActorRequest>,
        ) -> std::result::Result<
            tonic::Response<super::RevertActorResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/RevertActor",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "RevertActor"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn delete_actor(
            &mut self,
            request: impl tonic::IntoRequest<super::DeleteActorRequest>,
        ) -> std::result::Result<tonic::Response<super::Actor>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/DeleteActor",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "DeleteActor"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_actor_egress_policy(
            &mut self,
            request: impl tonic::IntoRequest<super::GetActorEgressPolicyRequest>,
        ) -> std::result::Result<tonic::Response<super::EgressPolicy>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/GetActorEgressPolicy",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "substrate.ateapi.v1.Control",
                        "GetActorEgressPolicy",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn create_actor_egress_policy(
            &mut self,
            request: impl tonic::IntoRequest<super::CreateActorEgressPolicyRequest>,
        ) -> std::result::Result<tonic::Response<super::EgressPolicy>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/CreateActorEgressPolicy",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "substrate.ateapi.v1.Control",
                        "CreateActorEgressPolicy",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn update_actor_egress_policy(
            &mut self,
            request: impl tonic::IntoRequest<super::UpdateActorEgressPolicyRequest>,
        ) -> std::result::Result<tonic::Response<super::EgressPolicy>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/UpdateActorEgressPolicy",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "substrate.ateapi.v1.Control",
                        "UpdateActorEgressPolicy",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn delete_actor_egress_policy(
            &mut self,
            request: impl tonic::IntoRequest<super::DeleteActorEgressPolicyRequest>,
        ) -> std::result::Result<tonic::Response<super::EgressPolicy>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/DeleteActorEgressPolicy",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "substrate.ateapi.v1.Control",
                        "DeleteActorEgressPolicy",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn mint_actor_jwt(
            &mut self,
            request: impl tonic::IntoRequest<super::MintActorJwtRequest>,
        ) -> std::result::Result<
            tonic::Response<super::MintActorJwtResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/MintActorJwt",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "MintActorJwt"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn create_tag(
            &mut self,
            request: impl tonic::IntoRequest<super::CreateTagRequest>,
        ) -> std::result::Result<tonic::Response<super::Tag>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/CreateTag",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "CreateTag"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_tag(
            &mut self,
            request: impl tonic::IntoRequest<super::GetTagRequest>,
        ) -> std::result::Result<tonic::Response<super::Tag>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/GetTag",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "GetTag"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn list_tags(
            &mut self,
            request: impl tonic::IntoRequest<super::ListTagsRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ListTagsResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/ListTags",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "ListTags"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn update_tag(
            &mut self,
            request: impl tonic::IntoRequest<super::UpdateTagRequest>,
        ) -> std::result::Result<tonic::Response<super::Tag>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/UpdateTag",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "UpdateTag"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn delete_tag(
            &mut self,
            request: impl tonic::IntoRequest<super::DeleteTagRequest>,
        ) -> std::result::Result<tonic::Response<super::Tag>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/DeleteTag",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "DeleteTag"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn list_workers(
            &mut self,
            request: impl tonic::IntoRequest<super::ListWorkersRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ListWorkersResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/ListWorkers",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "ListWorkers"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_worker(
            &mut self,
            request: impl tonic::IntoRequest<super::GetWorkerRequest>,
        ) -> std::result::Result<tonic::Response<super::Worker>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/GetWorker",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "GetWorker"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn create_worker(
            &mut self,
            request: impl tonic::IntoRequest<super::CreateWorkerRequest>,
        ) -> std::result::Result<tonic::Response<super::Worker>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/CreateWorker",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "CreateWorker"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn update_worker(
            &mut self,
            request: impl tonic::IntoRequest<super::UpdateWorkerRequest>,
        ) -> std::result::Result<tonic::Response<super::Worker>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/UpdateWorker",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "UpdateWorker"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn delete_worker(
            &mut self,
            request: impl tonic::IntoRequest<super::DeleteWorkerRequest>,
        ) -> std::result::Result<tonic::Response<super::Worker>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/DeleteWorker",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "DeleteWorker"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn drain_worker(
            &mut self,
            request: impl tonic::IntoRequest<super::DrainWorkerRequest>,
        ) -> std::result::Result<
            tonic::Response<super::DrainWorkerResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/DrainWorker",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "DrainWorker"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn mark_worker_available(
            &mut self,
            request: impl tonic::IntoRequest<super::MarkWorkerAvailableRequest>,
        ) -> std::result::Result<
            tonic::Response<super::MarkWorkerAvailableResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/MarkWorkerAvailable",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("substrate.ateapi.v1.Control", "MarkWorkerAvailable"),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn mark_worker_unavailable(
            &mut self,
            request: impl tonic::IntoRequest<super::MarkWorkerUnavailableRequest>,
        ) -> std::result::Result<
            tonic::Response<super::MarkWorkerUnavailableResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/MarkWorkerUnavailable",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "substrate.ateapi.v1.Control",
                        "MarkWorkerUnavailable",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn list_worker_actor_assignments(
            &mut self,
            request: impl tonic::IntoRequest<super::ListWorkerActorAssignmentsRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ListWorkerActorAssignmentsResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/ListWorkerActorAssignments",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "substrate.ateapi.v1.Control",
                        "ListWorkerActorAssignments",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn list_actors(
            &mut self,
            request: impl tonic::IntoRequest<super::ListActorsRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ListActorsResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/ListActors",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "ListActors"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn create_atespace(
            &mut self,
            request: impl tonic::IntoRequest<super::CreateAtespaceRequest>,
        ) -> std::result::Result<tonic::Response<super::Atespace>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/CreateAtespace",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("substrate.ateapi.v1.Control", "CreateAtespace"),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_atespace(
            &mut self,
            request: impl tonic::IntoRequest<super::GetAtespaceRequest>,
        ) -> std::result::Result<tonic::Response<super::Atespace>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/GetAtespace",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "GetAtespace"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn list_atespaces(
            &mut self,
            request: impl tonic::IntoRequest<super::ListAtespacesRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ListAtespacesResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/ListAtespaces",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("substrate.ateapi.v1.Control", "ListAtespaces"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn delete_atespace(
            &mut self,
            request: impl tonic::IntoRequest<super::DeleteAtespaceRequest>,
        ) -> std::result::Result<tonic::Response<super::Atespace>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/DeleteAtespace",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("substrate.ateapi.v1.Control", "DeleteAtespace"),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn create_actor_template(
            &mut self,
            request: impl tonic::IntoRequest<super::CreateActorTemplateRequest>,
        ) -> std::result::Result<tonic::Response<super::ActorTemplate>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/CreateActorTemplate",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("substrate.ateapi.v1.Control", "CreateActorTemplate"),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_actor_template(
            &mut self,
            request: impl tonic::IntoRequest<super::GetActorTemplateRequest>,
        ) -> std::result::Result<tonic::Response<super::ActorTemplate>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/GetActorTemplate",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("substrate.ateapi.v1.Control", "GetActorTemplate"),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn list_actor_templates(
            &mut self,
            request: impl tonic::IntoRequest<super::ListActorTemplatesRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ListActorTemplatesResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/ListActorTemplates",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("substrate.ateapi.v1.Control", "ListActorTemplates"),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn delete_actor_template(
            &mut self,
            request: impl tonic::IntoRequest<super::DeleteActorTemplateRequest>,
        ) -> std::result::Result<tonic::Response<super::ActorTemplate>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/DeleteActorTemplate",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("substrate.ateapi.v1.Control", "DeleteActorTemplate"),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_global_access_policy(
            &mut self,
            request: impl tonic::IntoRequest<super::GetGlobalAccessPolicyRequest>,
        ) -> std::result::Result<tonic::Response<super::AccessPolicy>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/GetGlobalAccessPolicy",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "substrate.ateapi.v1.Control",
                        "GetGlobalAccessPolicy",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn create_global_access_policy(
            &mut self,
            request: impl tonic::IntoRequest<super::CreateGlobalAccessPolicyRequest>,
        ) -> std::result::Result<tonic::Response<super::AccessPolicy>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/CreateGlobalAccessPolicy",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "substrate.ateapi.v1.Control",
                        "CreateGlobalAccessPolicy",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn update_global_access_policy(
            &mut self,
            request: impl tonic::IntoRequest<super::UpdateGlobalAccessPolicyRequest>,
        ) -> std::result::Result<tonic::Response<super::AccessPolicy>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/UpdateGlobalAccessPolicy",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "substrate.ateapi.v1.Control",
                        "UpdateGlobalAccessPolicy",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn get_atespace_access_policy(
            &mut self,
            request: impl tonic::IntoRequest<super::GetAtespaceAccessPolicyRequest>,
        ) -> std::result::Result<tonic::Response<super::AccessPolicy>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/GetAtespaceAccessPolicy",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "substrate.ateapi.v1.Control",
                        "GetAtespaceAccessPolicy",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn create_atespace_access_policy(
            &mut self,
            request: impl tonic::IntoRequest<super::CreateAtespaceAccessPolicyRequest>,
        ) -> std::result::Result<tonic::Response<super::AccessPolicy>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/CreateAtespaceAccessPolicy",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "substrate.ateapi.v1.Control",
                        "CreateAtespaceAccessPolicy",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn update_atespace_access_policy(
            &mut self,
            request: impl tonic::IntoRequest<super::UpdateAtespaceAccessPolicyRequest>,
        ) -> std::result::Result<tonic::Response<super::AccessPolicy>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/UpdateAtespaceAccessPolicy",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "substrate.ateapi.v1.Control",
                        "UpdateAtespaceAccessPolicy",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        pub async fn delete_atespace_access_policy(
            &mut self,
            request: impl tonic::IntoRequest<super::DeleteAtespaceAccessPolicyRequest>,
        ) -> std::result::Result<tonic::Response<super::AccessPolicy>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/substrate.ateapi.v1.Control/DeleteAtespaceAccessPolicy",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "substrate.ateapi.v1.Control",
                        "DeleteAtespaceAccessPolicy",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
    }
}
