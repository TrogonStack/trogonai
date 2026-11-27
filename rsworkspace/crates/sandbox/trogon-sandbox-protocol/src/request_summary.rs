use trogon_sandbox_contract::{Host, HttpMethod};

/// A request as the per-request rules see it: where it goes and what it says.
///
/// `path` is the request target without its query.
#[derive(Debug, Clone, Copy)]
pub struct RequestSummary<'a> {
    pub host: &'a Host,
    pub port: u16,
    pub method: &'a HttpMethod,
    pub path: &'a str,
    pub headers: &'a [(String, String)],
}
