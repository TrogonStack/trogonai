use serde::Deserialize;

use super::OpenBaoCallError;

pub(crate) async fn send_json<T: for<'de> Deserialize<'de>>(
    request: reqwest::RequestBuilder,
) -> Result<T, OpenBaoCallError> {
    let body = send_for_body(request).await?;
    serde_json::from_str(&body).map_err(|error| OpenBaoCallError::Backend(error.to_string()))
}

/// Sends a call whose successful answer carries a token.
///
/// A decoding error can quote the value it choked on, so it is replaced with a
/// fixed message rather than risk the token reaching an error or a log.
pub(crate) async fn send_token_json<T: for<'de> Deserialize<'de>>(
    request: reqwest::RequestBuilder,
) -> Result<T, OpenBaoCallError> {
    let body = send_for_body(request).await?;
    serde_json::from_str(&body)
        .map_err(|_| OpenBaoCallError::Backend("OpenBao answered with a malformed token response".to_string()))
}

pub(crate) async fn send_empty(request: reqwest::RequestBuilder) -> Result<(), OpenBaoCallError> {
    let response = request
        .send()
        .await
        .map_err(|error| OpenBaoCallError::Backend(error.to_string()))?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response
        .text()
        .await
        .map_err(|error| OpenBaoCallError::Backend(error.to_string()))?;
    Err(OpenBaoCallError::Status { status, body })
}

async fn send_for_body(request: reqwest::RequestBuilder) -> Result<String, OpenBaoCallError> {
    let response = request
        .send()
        .await
        .map_err(|error| OpenBaoCallError::Backend(error.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| OpenBaoCallError::Backend(error.to_string()))?;

    if !status.is_success() {
        return Err(OpenBaoCallError::Status { status, body });
    }
    Ok(body)
}
