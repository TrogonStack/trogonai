use trogon_sandbox_contract::FileOpResponse;

/// A reply that refuses a request, boxed so a `Result` that carries it stays small.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Refusal(Box<FileOpResponse>);

impl Refusal {
    /// The refusing reply.
    #[cfg(test)]
    pub(crate) fn response(&self) -> &FileOpResponse {
        &self.0
    }
}

impl From<FileOpResponse> for Refusal {
    fn from(response: FileOpResponse) -> Self {
        Self(Box::new(response))
    }
}

impl From<Refusal> for FileOpResponse {
    fn from(refusal: Refusal) -> Self {
        *refusal.0
    }
}
