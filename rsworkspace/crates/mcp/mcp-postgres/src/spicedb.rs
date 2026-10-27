use crate::{Error, ObjectId};
use async_trait::async_trait;
use authzed::v1::{
    CheckBulkPermissionsRequest, CheckBulkPermissionsRequestItem, Consistency, ObjectReference, SubjectReference,
    check_bulk_permissions_pair::Response, check_permission_response::Permissionship, consistency::Requirement,
    permissions_service_client::PermissionsServiceClient,
};
use tonic::{
    metadata::{Ascii, MetadataValue},
    transport::Channel,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Resource {
    pub kind: &'static str,
    pub id: ObjectId,
}
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub resource: Resource,
    pub subject_kind: &'static str,
    pub subject: ObjectId,
}
#[async_trait]
pub trait PermissionCheck: Send + Sync {
    async fn check(&self, resource: &Resource, subject_kind: &'static str, subject: &ObjectId) -> Result<(), Error> {
        self.check_all(&[PermissionRequest {
            resource: resource.clone(),
            subject_kind,
            subject: subject.clone(),
        }])
        .await
    }
    /// Evaluate the complete admission set at one coherent authorization revision.
    async fn check_all(&self, requests: &[PermissionRequest]) -> Result<(), Error>;
}
#[derive(Clone)]
pub struct SpiceDb {
    client: PermissionsServiceClient<Channel>,
    token: MetadataValue<Ascii>,
}
impl SpiceDb {
    pub async fn connect(endpoint: String, token: String) -> Result<Self, Error> {
        let token = MetadataValue::try_from(format!("Bearer {token}")).map_err(|_| Error::Configuration)?;
        let endpoint = crate::SpiceDbEndpoint::new(endpoint)?;
        let mut endpoint = Channel::from_shared(endpoint.as_str().to_owned()).map_err(|_| Error::Configuration)?;
        if endpoint.uri().scheme_str() == Some("https") {
            endpoint = endpoint.tls_config(tonic::transport::ClientTlsConfig::new().with_native_roots())?;
        }
        let client = PermissionsServiceClient::new(endpoint.connect().await?);
        Ok(Self { client, token })
    }
}
#[async_trait]
impl PermissionCheck for SpiceDb {
    async fn check_all(&self, requests: &[PermissionRequest]) -> Result<(), Error> {
        if requests.is_empty() || requests.len() > 2048 {
            return Err(Error::Denied);
        }
        let mut items = requests
            .iter()
            .map(|request| CheckBulkPermissionsRequestItem {
                resource: Some(ObjectReference {
                    object_type: request.resource.kind.into(),
                    object_id: request.resource.id.as_str().into(),
                }),
                permission: "query".into(),
                subject: Some(SubjectReference {
                    object: Some(ObjectReference {
                        object_type: request.subject_kind.into(),
                        object_id: request.subject.as_str().into(),
                    }),
                    optional_relation: String::new(),
                }),
                context: None,
            })
            .collect::<Vec<_>>();
        let mut request = tonic::Request::new(CheckBulkPermissionsRequest {
            consistency: Some(Consistency {
                requirement: Some(Requirement::FullyConsistent(true)),
            }),
            items: items.clone(),
            with_tracing: false,
        });
        request.metadata_mut().insert("authorization", self.token.clone());
        request.set_timeout(std::time::Duration::from_secs(2));
        let response = self.client.clone().check_bulk_permissions(request).await?.into_inner();
        if response.checked_at.as_ref().is_none_or(|token| token.token.is_empty())
            || response.pairs.len() != items.len()
        {
            return Err(Error::Denied);
        }
        for pair in response.pairs {
            let echoed = pair.request.ok_or(Error::Denied)?;
            let index = items.iter().position(|item| *item == echoed).ok_or(Error::Denied)?;
            items.swap_remove(index);
            match pair.response {
                Some(Response::Item(item))
                    if item.permissionship == Permissionship::HasPermission as i32
                        && item.partial_caveat_info.is_none() => {}
                _ => return Err(Error::Denied),
            }
        }
        if items.is_empty() { Ok(()) } else { Err(Error::Denied) }
    }
}
