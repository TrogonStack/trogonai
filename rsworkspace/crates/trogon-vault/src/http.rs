//! Minimal HTTP client abstraction for vault backends.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

/// Opaque HTTP transport error — wraps the string message so `Error::source()` chain is preserved.
#[derive(Debug)]
pub struct HttpError(pub String);

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HttpError {}

impl From<String> for HttpError {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// A minimal HTTP response carrying status code and text body.
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Abstraction over HTTP operations used by vault backends.
///
/// Implemented for [`reqwest::Client`] in production. Tests can supply
/// a `MockHttpClient` without performing real network I/O.
pub trait HttpClient: Clone + Send + Sync + 'static {
    fn get(
        &self,
        url: String,
        headers: Vec<(String, String)>,
        query: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>>;

    fn post_json(
        &self,
        url: String,
        body: Value,
        headers: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>>;

    fn put_json(
        &self,
        url: String,
        body: Value,
        headers: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>>;

    fn patch_json(
        &self,
        url: String,
        body: Value,
        headers: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>>;

    /// DELETE request with an optional JSON body.
    fn delete(
        &self,
        url: String,
        body: Option<Value>,
        headers: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>>;
}

#[cfg(any(feature = "hashicorp-vault", feature = "infisical"))]
impl HttpClient for reqwest::Client {
    fn get(
        &self,
        url: String,
        headers: Vec<(String, String)>,
        query: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
        let query_refs: Vec<(&str, &str)> =
            query.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let mut builder = reqwest::Client::get(self, &url).query(&query_refs);
        for (k, v) in &headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        Box::pin(async move {
            let resp = builder.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Ok(HttpResponse { status, body })
        })
    }

    fn post_json(
        &self,
        url: String,
        body: Value,
        headers: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
        let mut builder = reqwest::Client::post(self, &url).json(&body);
        for (k, v) in &headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        Box::pin(async move {
            let resp = builder.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Ok(HttpResponse { status, body })
        })
    }

    fn put_json(
        &self,
        url: String,
        body: Value,
        headers: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
        let mut builder = reqwest::Client::put(self, &url).json(&body);
        for (k, v) in &headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        Box::pin(async move {
            let resp = builder.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Ok(HttpResponse { status, body })
        })
    }

    fn patch_json(
        &self,
        url: String,
        body: Value,
        headers: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
        let mut builder = reqwest::Client::patch(self, &url).json(&body);
        for (k, v) in &headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        Box::pin(async move {
            let resp = builder.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Ok(HttpResponse { status, body })
        })
    }

    fn delete(
        &self,
        url: String,
        body: Option<Value>,
        headers: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
        let mut builder = reqwest::Client::delete(self, &url);
        if let Some(b) = body {
            builder = builder.json(&b);
        }
        for (k, v) in &headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        Box::pin(async move {
            let resp = builder.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Ok(HttpResponse { status, body })
        })
    }
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Mock [`HttpClient`] that returns pre-programmed `(status, body)` pairs in FIFO order.
    ///
    /// Panics if called when the queue is empty — useful for asserting
    /// that no HTTP call is made.
    pub struct MockHttpClient {
        responses: Mutex<VecDeque<(u16, String)>>,
    }

    impl MockHttpClient {
        pub fn new() -> Self {
            Self { responses: Mutex::new(VecDeque::new()) }
        }

        /// Queue a response to be returned on the next call.
        pub fn enqueue(&self, status: u16, body: impl Into<String>) -> &Self {
            self.responses.lock().unwrap().push_back((status, body.into()));
            self
        }
    }

    impl Clone for MockHttpClient {
        fn clone(&self) -> Self {
            let queue = self.responses.lock().unwrap().clone();
            Self { responses: Mutex::new(queue) }
        }
    }

    impl HttpClient for MockHttpClient {
        fn get(&self, _url: String, _headers: Vec<(String, String)>, _query: Vec<(String, String)>) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
            let (status, body) = self.responses.lock().unwrap().pop_front()
                .expect("MockHttpClient: no response queued for get");
            Box::pin(async move { Ok(HttpResponse { status, body }) })
        }

        fn post_json(&self, _url: String, _body: Value, _headers: Vec<(String, String)>) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
            let (status, body) = self.responses.lock().unwrap().pop_front()
                .expect("MockHttpClient: no response queued for post_json");
            Box::pin(async move { Ok(HttpResponse { status, body }) })
        }

        fn put_json(&self, _url: String, _body: Value, _headers: Vec<(String, String)>) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
            let (status, body) = self.responses.lock().unwrap().pop_front()
                .expect("MockHttpClient: no response queued for put_json");
            Box::pin(async move { Ok(HttpResponse { status, body }) })
        }

        fn patch_json(&self, _url: String, _body: Value, _headers: Vec<(String, String)>) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
            let (status, body) = self.responses.lock().unwrap().pop_front()
                .expect("MockHttpClient: no response queued for patch_json");
            Box::pin(async move { Ok(HttpResponse { status, body }) })
        }

        fn delete(&self, _url: String, _body: Option<Value>, _headers: Vec<(String, String)>) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
            let (status, body) = self.responses.lock().unwrap().pop_front()
                .expect("MockHttpClient: no response queued for delete");
            Box::pin(async move { Ok(HttpResponse { status, body }) })
        }
    }
}
