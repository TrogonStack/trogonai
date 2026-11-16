use acp_nats::{AcpPrefix, nats::AcpStream};
use async_nats::jetstream::context::{CreateStreamError, GetStreamError};

#[derive(Debug, thiserror::Error)]
pub enum ProvisionCommandError {
    #[error("could not inspect the {stream} stream for ACP prefix {}", .prefix.as_str())]
    Inspect {
        prefix: AcpPrefix,
        stream: AcpStream,
        #[source]
        source: GetStreamError,
    },
    #[error("the {stream} stream for ACP prefix {} is sealed", .prefix.as_str())]
    Sealed { prefix: AcpPrefix, stream: AcpStream },
    #[error("the {stream} stream for ACP prefix {} is a mirror", .prefix.as_str())]
    Mirrored { prefix: AcpPrefix, stream: AcpStream },
    #[error(transparent)]
    Reconcile(#[from] acp_nats::jetstream::provision::ProvisionError<CreateStreamError>),
}
