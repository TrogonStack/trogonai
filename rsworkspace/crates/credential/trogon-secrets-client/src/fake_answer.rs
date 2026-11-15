use std::time::{Duration, SystemTime};

use buffa::MessageField;
use buffa_types::google::protobuf::{Duration as WireDuration, Timestamp};
use grpc_nats_micro::ServiceFault;
use nkeys::XKey;
use trogon_std::SecretString;
use trogonai_proto::secrets::exchange::v1alpha1::{
    ResolveFailure, ResolveFailureCategory as Category, ResolveForReleaseRequest, ResolveForReleaseResponse,
    SealedAccess,
};

/// What a scripted outcome answers once a request arrives.
#[derive(Clone, Debug)]
pub(crate) enum FakeAnswer {
    Release {
        value: SecretString,
        valid_until: SystemTime,
        bundle_generation: u64,
    },
    Failure {
        category: Category,
        retry_after: Option<Duration>,
    },
    InvalidArgument(String),
}

impl FakeAnswer {
    /// The reply to `request`, sealed with `service` when it releases.
    ///
    /// A requester xkey that is not a curve public key is refused with
    /// `KEY_DENIED`, as the secrets service refuses it.
    pub(crate) fn reply(
        &self,
        request: &ResolveForReleaseRequest,
        service: &XKey,
    ) -> Result<ResolveForReleaseResponse, ServiceFault> {
        match self {
            Self::Release {
                value,
                valid_until,
                bundle_generation,
            } => Ok(released(request, service, value, *valid_until, *bundle_generation)),
            Self::Failure { category, retry_after } => Ok(failure(*category, *retry_after)),
            Self::InvalidArgument(message) => Err(ServiceFault::invalid_argument(message.clone())),
        }
    }
}

fn released(
    request: &ResolveForReleaseRequest,
    service: &XKey,
    value: &SecretString,
    valid_until: SystemTime,
    bundle_generation: u64,
) -> ResolveForReleaseResponse {
    let Ok(requester) = XKey::from_public_key(&request.requester_xkey.public_key) else {
        return failure(Category::RESOLVE_FAILURE_CATEGORY_KEY_DENIED, None);
    };
    ResolveForReleaseResponse {
        result: SealedAccess {
            xkey_sealed_value: service.seal(value.as_str().as_bytes(), &requester).unwrap_or_default(),
            bundle_generation,
            valid_until: MessageField::some(Timestamp::from(valid_until)),
        }
        .into(),
    }
}

fn failure(category: Category, retry_after: Option<Duration>) -> ResolveForReleaseResponse {
    ResolveForReleaseResponse {
        result: ResolveFailure {
            category: category.into(),
            retry_after: retry_after.map(WireDuration::from).into(),
        }
        .into(),
    }
}
