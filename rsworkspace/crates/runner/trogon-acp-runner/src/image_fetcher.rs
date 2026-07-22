//! Reqwest-backed [`ImageFetcher`] adapter (§ Imagen por URL: "fetchear la URL con
//! limites de timeout, tamano, redirects y content type"). HTTP lives here in the runner,
//! not in the domain `trogonai-artifacts` crate (ADR-0002/ADR-0003).

use std::time::Duration;

use bytes::Bytes;
use trogonai_artifacts::{FetchLimits, FetchedImage, ImageFetchError, ImageFetcher};

/// Fetches remote images with reqwest, enforcing the timeout / size / redirect limits.
#[derive(Clone)]
pub struct ReqwestImageFetcher {
    client: reqwest::Client,
}

impl ReqwestImageFetcher {
    /// Build a fetcher whose client honors the timeout and redirect limits. The size
    /// limit is enforced per fetch (it can vary), while the body is streamed so an
    /// oversized download is aborted mid-stream rather than fully buffered.
    pub fn new(limits: &FetchLimits) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(limits.timeout_secs))
            .redirect(reqwest::redirect::Policy::limited(limits.max_redirects as usize))
            .build()?;
        Ok(Self { client })
    }
}

impl ImageFetcher for ReqwestImageFetcher {
    async fn fetch(&self, url: &str, limits: &FetchLimits) -> Result<FetchedImage, ImageFetchError> {
        let mut response = self.client.get(url).send().await.map_err(map_reqwest_error)?;
        if !response.status().is_success() {
            return Err(ImageFetchError::BadStatus(response.status().as_u16()));
        }

        // Reject early on a declared Content-Length over the limit (§ "limites de tamano").
        if let Some(len) = response.content_length()
            && len as usize > limits.max_size_bytes
        {
            return Err(ImageFetchError::TooLarge);
        }

        // § "content type": keep the transport-declared MIME (without charset params).
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|raw| raw.split(';').next().unwrap_or(raw).trim().to_string());

        // Stream the body so the size limit is enforced during download, not after
        // buffering an unbounded response.
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(map_reqwest_error)? {
            if bytes.len() + chunk.len() > limits.max_size_bytes {
                return Err(ImageFetchError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }

        Ok(FetchedImage {
            content_type,
            bytes: Bytes::from(bytes),
        })
    }
}

fn map_reqwest_error(err: reqwest::Error) -> ImageFetchError {
    if err.is_timeout() {
        ImageFetchError::Timeout
    } else if err.is_status() {
        ImageFetchError::BadStatus(err.status().map(|status| status.as_u16()).unwrap_or(0))
    } else {
        ImageFetchError::Network(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Serve a single raw HTTP response on an ephemeral port; returns its URL.
    async fn serve_once(response: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let _ = socket.write_all(&response).await;
                let _ = socket.flush().await;
            }
        });
        format!("http://{addr}/img.png")
    }

    fn http_response(status_line: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "{status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    #[tokio::test]
    async fn fetches_image_bytes_and_content_type() {
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
        let url = serve_once(http_response("HTTP/1.1 200 OK", "image/png", &png)).await;
        let fetcher = ReqwestImageFetcher::new(&FetchLimits::default()).unwrap();

        let image = fetcher.fetch(&url, &FetchLimits::default()).await.unwrap();

        assert_eq!(image.bytes.as_ref(), png.as_slice());
        assert_eq!(image.content_type.as_deref(), Some("image/png"));
    }

    #[tokio::test]
    async fn rejects_image_over_the_size_limit() {
        let big = vec![0u8; 100];
        let url = serve_once(http_response("HTTP/1.1 200 OK", "image/png", &big)).await;
        let fetcher = ReqwestImageFetcher::new(&FetchLimits::default()).unwrap();
        let limits = FetchLimits {
            max_size_bytes: 10,
            ..FetchLimits::default()
        };

        let err = fetcher.fetch(&url, &limits).await.unwrap_err();
        assert!(matches!(err, ImageFetchError::TooLarge));
    }

    #[tokio::test]
    async fn maps_not_found_to_bad_status() {
        let url = serve_once(http_response("HTTP/1.1 404 Not Found", "text/plain", b"nope")).await;
        let fetcher = ReqwestImageFetcher::new(&FetchLimits::default()).unwrap();

        let err = fetcher.fetch(&url, &FetchLimits::default()).await.unwrap_err();
        assert!(matches!(err, ImageFetchError::BadStatus(404)));
    }
}
