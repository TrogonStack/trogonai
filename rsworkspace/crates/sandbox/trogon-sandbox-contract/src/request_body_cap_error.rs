/// A number is not a [`RequestBodyCap`](crate::RequestBodyCap).
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum RequestBodyCapError {
    #[error("the request body cap must be at least 1 byte")]
    Zero,
}
