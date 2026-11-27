use crate::K8sOwnershipError;

/// Failures of the Kubernetes API operations.
#[derive(Debug, thiserror::Error)]
pub enum K8sOpsError {
    #[error("{0} is unsupported")]
    Unsupported(&'static str),
    #[error(transparent)]
    Kube(Box<kube::Error>),
    #[error(transparent)]
    Ownership(#[from] K8sOwnershipError),
    #[error("job already exists: {0}")]
    JobAlreadyExists(String),
    #[error("no pod found for job {job_name}")]
    NoPodForJob { job_name: String },
    #[error("encoding the label selector: {0}")]
    LabelSelector(#[source] serde_urlencoded::ser::Error),
    #[error("building the list request: {0}")]
    ListRequest(#[source] http::Error),
    #[error("listing {path} from the apiserver: {source}")]
    ListSend {
        path: String,
        #[source]
        source: Box<kube::Error>,
    },
    #[error("reading the list response body: {0}")]
    ListBody(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("listing {path} failed with {status}: {body}")]
    ListStatus {
        path: String,
        status: http::StatusCode,
        body: String,
    },
    #[error("decoding the list response: {0}")]
    ListDecode(#[source] serde_json::Error),
    #[error("exec attached process missing {stream}")]
    ExecStreamMissing { stream: &'static str },
    #[error("failed to {action} exec {stream}: {source}")]
    ExecIo {
        action: &'static str,
        stream: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("exec process failed: {0}")]
    ExecJoin(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("looking up the user of image {image}: {source}")]
    ImageUser {
        image: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("{0}")]
    Other(String),
}

impl From<kube::Error> for K8sOpsError {
    fn from(error: kube::Error) -> Self {
        Self::Kube(Box::new(error))
    }
}
