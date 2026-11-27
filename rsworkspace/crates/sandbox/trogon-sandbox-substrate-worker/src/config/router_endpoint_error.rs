/// Why a router endpoint string could not be parsed.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum RouterEndpointError {
    #[error("router endpoint {value:?} must be host:port")]
    MissingPort { value: String },
    #[error("router endpoint {value:?} has an empty host")]
    EmptyHost { value: String },
    #[error("router endpoint {value:?} has an invalid port: {source}")]
    InvalidPort {
        value: String,
        #[source]
        source: std::num::ParseIntError,
    },
}
