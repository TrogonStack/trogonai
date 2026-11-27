use tonic::{Request, Status};

use super::BearerTokenFile;

#[derive(Clone)]
pub(super) struct BearerAuth {
    pub(super) token_file: Option<BearerTokenFile>,
}

impl tonic::service::Interceptor for BearerAuth {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        let Some(token_file) = &self.token_file else {
            return Ok(request);
        };
        let token = token_file
            .read()
            .map_err(|err| Status::unauthenticated(format!("reading bearer token: {err}")))?;
        let value = format!("Bearer {token}")
            .parse()
            .map_err(|_| Status::unauthenticated("bearer token is not valid header metadata"))?;
        request.metadata_mut().insert("authorization", value);
        Ok(request)
    }
}
