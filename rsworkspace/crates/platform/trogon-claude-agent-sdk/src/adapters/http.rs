use std::future::Future;

/// Sends one prepared request exactly as [`reqwest::Client::execute`] does, so a decorator can record the request
/// or inject a transport error and otherwise delegate to the native client.
pub trait HttpExecute {
    fn execute(&self, request: reqwest::Request) -> impl Future<Output = reqwest::Result<reqwest::Response>> + Send;
}

impl HttpExecute for reqwest::Client {
    fn execute(&self, request: reqwest::Request) -> impl Future<Output = reqwest::Result<reqwest::Response>> + Send {
        reqwest::Client::execute(self, request)
    }
}
