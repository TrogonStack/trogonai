use async_nats::jetstream::consumer::{Consumer, pull};
use futures::StreamExt;
use reqwest::Client as HttpClient;
use slack_morphism::prelude::*;
use slack_types::events::{
    SlackDeleteFile, SlackDeleteMessage, SlackEphemeralMessage, SlackOutboundMessage,
    SlackProactiveMessage, SlackReactionAction, SlackSetStatusRequest,
    SlackSetSuggestedPromptsRequest, SlackStreamAppendMessage, SlackStreamStopMessage,
    SlackUpdateMessage, SlackViewOpenRequest, SlackViewPublishRequest,
};
use std::sync::Arc;
use std::time::Duration;

use crate::format;
use crate::rate_limit::RateLimiter;

/// Slack rate limit for `chat.postMessage`: ~1 request/second per channel.
/// We use a conservative global delay to stay safely below the limit.
const POST_MESSAGE_DELAY: Duration = Duration::from_millis(1_100);

const SLACK_API_BASE: &str = "https://slack.com";

pub async fn run_outbound_loop(
    consumer: Consumer<pull::Config>,
    slack_client: Arc<SlackHyperClient>,
    bot_token: String,
    http_client: Arc<HttpClient>,
    rate_limiter: Arc<RateLimiter>,
    text_chunk_limit: usize,
    chunk_mode_newline: bool,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create outbound message stream");
            return;
        }
    };
    let raw_token = bot_token.clone();
    let token = SlackApiToken::new(bot_token.into());

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on outbound consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackOutboundMessage>(&msg.payload) {
            Ok(outbound) => {
                let session = slack_client.open_session(&token);
                let channel: SlackChannelId = outbound.channel.into();
                let thread_ts: Option<SlackTs> = outbound.thread_ts.map(|ts| ts.into());

                let converted_text = format::markdown_to_mrkdwn(&outbound.text);

                let blocks: Option<Vec<SlackBlock>> = outbound.blocks.as_ref().and_then(|v| {
                    let bytes = serde_json::to_vec(v).ok()?;
                    serde_json::from_slice::<Vec<SlackBlock>>(&bytes)
                        .map_err(|e| {
                            tracing::warn!(error = %e, "Failed to parse blocks JSON, falling back to text");
                        })
                        .ok()
                });

                if let Some(ref media_url) = outbound.media_url {
                    match upload_file_to_slack(
                        &http_client,
                        &raw_token,
                        &channel,
                        thread_ts.as_ref(),
                        &converted_text,
                        media_url,
                        SLACK_API_BASE,
                        &rate_limiter,
                    )
                    .await
                    {
                        Ok(()) => {
                            let _ = msg.ack().await;
                            continue;
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                url = %media_url,
                                "Media upload failed, falling back to text-only post"
                            );
                        }
                    }
                }

                let chunks = format::chunk_text(&converted_text, text_chunk_limit, chunk_mode_newline);
                if chunks.is_empty() {
                    let _ = msg.ack().await;
                    continue;
                }

                let first_text = chunks[0].clone();
                let mut content = SlackMessageContent::new().with_text(first_text);
                if chunks.len() == 1
                    && let Some(ref blks) = blocks
                {
                    content = content.with_blocks(blks.clone());
                }

                let mut request = SlackApiChatPostMessageRequest::new(channel.clone(), content);
                if let Some(ref ts) = thread_ts {
                    request = request.with_thread_ts(ts.clone());
                }
                if let Some(ref username) = outbound.username {
                    request = request.with_username(username.clone());
                }
                if let Some(ref icon_url) = outbound.icon_url {
                    request = request.with_icon_url(icon_url.clone());
                }
                if let Some(ref icon_emoji) = outbound.icon_emoji {
                    request = request.with_icon_emoji(icon_emoji.clone());
                }

                let post_result = {
                    let r = session.chat_post_message(&request).await;
                    if let Err(ref e) = r
                        && format!("{e}").contains("ratelimited")
                    {
                        tracing::warn!("Slack rate limit on postMessage, retrying after 5 s");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        slack_client
                            .open_session(&token)
                            .chat_post_message(&request)
                            .await
                    } else {
                        r
                    }
                };
                let first_ts = match post_result {
                    Ok(resp) => {
                        tokio::time::sleep(POST_MESSAGE_DELAY).await;
                        Some(resp.ts)
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to send message to Slack");
                        None
                    }
                };

                if chunks.len() > 1 {
                    let reply_ts = first_ts.or(thread_ts.clone());
                    for (idx, chunk) in chunks[1..].iter().enumerate() {
                        let is_last = idx == chunks.len() - 2;
                        let mut chunk_content = SlackMessageContent::new().with_text(chunk.clone());
                        if is_last && let Some(ref blks) = blocks {
                            chunk_content = chunk_content.with_blocks(blks.clone());
                        }
                        let mut chunk_req =
                            SlackApiChatPostMessageRequest::new(channel.clone(), chunk_content);
                        if let Some(ref ts) = reply_ts {
                            chunk_req = chunk_req.with_thread_ts(ts.clone());
                        }
                        if let Some(ref username) = outbound.username {
                            chunk_req = chunk_req.with_username(username.clone());
                        }
                        if let Some(ref icon_url) = outbound.icon_url {
                            chunk_req = chunk_req.with_icon_url(icon_url.clone());
                        }
                        if let Some(ref icon_emoji) = outbound.icon_emoji {
                            chunk_req = chunk_req.with_icon_emoji(icon_emoji.clone());
                        }
                        if let Err(e) = session.chat_post_message(&chunk_req).await {
                            tracing::error!(error = %e, "Failed to send message chunk to Slack");
                        }
                        tokio::time::sleep(POST_MESSAGE_DELAY).await;
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize outbound NATS message");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK JetStream outbound message");
        }
    }
}

pub async fn run_stream_append_loop(
    consumer: Consumer<pull::Config>,
    bot_token: String,
    http_client: Arc<HttpClient>,
    rate_limiter: Arc<RateLimiter>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create stream_append message stream");
            return;
        }
    };
    let url = format!("{SLACK_API_BASE}/api/chat.appendStream");

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on stream_append consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackStreamAppendMessage>(&msg.payload) {
            Ok(append) => {
                let body = serde_json::json!({
                    "channel": append.channel,
                    "message_ts": append.ts,
                    "content": append.text,
                });
                rate_limiter.acquire().await;
                match http_client
                    .post(&url)
                    .bearer_auth(&bot_token)
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(body) if !body["ok"].as_bool().unwrap_or(false) => {
                            tracing::error!(
                                api_error = body["error"].as_str().unwrap_or("unknown"),
                                "chat.appendStream failed"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to parse chat.appendStream response");
                        }
                    },
                    Err(e) => tracing::error!(error = %e, "HTTP error calling chat.appendStream"),
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize stream append NATS message");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK JetStream stream_append message");
        }
    }
}

pub async fn run_stream_stop_loop(
    consumer: Consumer<pull::Config>,
    bot_token: String,
    http_client: Arc<HttpClient>,
    rate_limiter: Arc<RateLimiter>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create stream_stop message stream");
            return;
        }
    };
    let url = format!("{SLACK_API_BASE}/api/chat.stopStream");

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on stream_stop consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackStreamStopMessage>(&msg.payload) {
            Ok(stop) => {
                let body = serde_json::json!({
                    "channel": stop.channel,
                    "message_ts": stop.ts,
                });
                rate_limiter.acquire().await;
                match http_client
                    .post(&url)
                    .bearer_auth(&bot_token)
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(body) if !body["ok"].as_bool().unwrap_or(false) => {
                            tracing::error!(
                                api_error = body["error"].as_str().unwrap_or("unknown"),
                                "chat.stopStream failed"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to parse chat.stopStream response");
                        }
                    },
                    Err(e) => tracing::error!(error = %e, "HTTP error calling chat.stopStream"),
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize stream stop NATS message");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK JetStream stream_stop message");
        }
    }
}


pub async fn run_reaction_action_loop(
    consumer: Consumer<pull::Config>,
    slack_client: Arc<SlackHyperClient>,
    bot_token: String,
    rate_limiter: Arc<RateLimiter>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create reaction_action message stream");
            return;
        }
    };
    let token = SlackApiToken::new(bot_token.into());

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on reaction_action consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackReactionAction>(&msg.payload) {
            Ok(action) => {
                let session = slack_client.open_session(&token);
                let channel = SlackChannelId(action.channel.clone());
                let name = SlackReactionName(action.reaction.clone());
                let timestamp = SlackTs(action.ts.clone());

                rate_limiter.acquire().await;
                let outcome = if action.add {
                    session
                        .reactions_add(&SlackApiReactionsAddRequest {
                            channel,
                            name,
                            timestamp,
                        })
                        .await
                        .map(|_| ())
                } else {
                    session
                        .reactions_remove(&SlackApiReactionsRemoveRequest {
                            channel: Some(channel),
                            name,
                            timestamp: Some(timestamp),
                            file: None,
                            full: None,
                        })
                        .await
                        .map(|_| ())
                };

                if let Err(e) = outcome {
                    tracing::warn!(
                        error = %e,
                        reaction = %action.reaction,
                        add = action.add,
                        "Failed to update reaction on Slack message"
                    );
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize SlackReactionAction");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK reaction_action message");
        }
    }
}

// ── File upload ───────────────────────────────────────────────────────────────

/// Derive a filename from the last path segment of a URL, stripping query params.
fn extract_upload_filename(url: &str) -> String {
    url.rsplit('/')
        .next()
        .and_then(|s| s.split('?').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("file")
        .to_string()
}

/// Upload a file to Slack using the two-step API introduced in 2023:
/// 1. `files.getUploadURLExternal` — obtain a pre-signed upload URL + file ID
/// 2. PUT the raw bytes to that URL
/// 3. `files.completeUploadExternal` — associate the file with a channel
///
/// `api_base` should be `"https://slack.com"` in production; it is exposed for
/// testing against a local mock server.
///
/// On success the file appears as a native Slack attachment; the caller should
/// skip the plain-text fallback post.  On failure the error is returned so the
/// caller can fall back to text-only.
async fn upload_file_to_slack(
    http: &HttpClient,
    token: &str,
    channel: &SlackChannelId,
    thread_ts: Option<&SlackTs>,
    text: &str,
    media_url: &str,
    api_base: &str,
    rate_limiter: &RateLimiter,
) -> Result<(), String> {
    // 1. Download the file from the media URL.
    rate_limiter.acquire().await;
    let download = http
        .get(media_url)
        .send()
        .await
        .map_err(|e| format!("download request: {e}"))?;

    let filename = extract_upload_filename(media_url);

    let content_type = download
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let bytes = download
        .bytes()
        .await
        .map_err(|e| format!("download body: {e}"))?;
    let length = bytes.len();
    let length_str = length.to_string();

    // 2. Ask Slack for a pre-signed upload URL (retry once on rate limit).
    let get_url_url = format!("{api_base}/api/files.getUploadURLExternal");
    let make_get_url_req = || {
        http.post(&get_url_url)
            .bearer_auth(token)
            .form(&[("filename", filename.as_str()), ("length", length_str.as_str())])
    };
    let get_url: serde_json::Value = {
        rate_limiter.acquire().await;
        let r: serde_json::Value = make_get_url_req()
            .send()
            .await
            .map_err(|e| format!("getUploadURLExternal request: {e}"))?
            .json()
            .await
            .map_err(|e| format!("getUploadURLExternal parse: {e}"))?;
        if !r["ok"].as_bool().unwrap_or(false) && r["error"].as_str() == Some("ratelimited") {
            tracing::warn!("Slack rate limit on getUploadURLExternal, retrying after 5 s");
            tokio::time::sleep(Duration::from_secs(5)).await;
            rate_limiter.acquire().await;
            make_get_url_req()
                .send()
                .await
                .map_err(|e| format!("getUploadURLExternal retry request: {e}"))?
                .json()
                .await
                .map_err(|e| format!("getUploadURLExternal retry parse: {e}"))?
        } else {
            r
        }
    };

    if !get_url["ok"].as_bool().unwrap_or(false) {
        return Err(format!(
            "files.getUploadURLExternal: {}",
            get_url["error"].as_str().unwrap_or("unknown")
        ));
    }

    let upload_url = get_url["upload_url"]
        .as_str()
        .ok_or_else(|| "missing upload_url in Slack response".to_string())?
        .to_string();
    let file_id = get_url["file_id"]
        .as_str()
        .ok_or_else(|| "missing file_id in Slack response".to_string())?
        .to_string();

    // 3. PUT the raw bytes to the pre-signed URL.
    rate_limiter.acquire().await;
    http.put(&upload_url)
        .header(reqwest::header::CONTENT_TYPE, &content_type)
        .body(bytes)
        .send()
        .await
        .map_err(|e| format!("upload PUT: {e}"))?;

    // 4. Complete the upload and associate it with the channel (retry once on rate limit).
    let mut body = serde_json::Map::new();
    body.insert("files".into(), serde_json::json!([{"id": file_id}]));
    body.insert(
        "channel_id".into(),
        serde_json::Value::String(channel.0.clone()),
    );
    if !text.is_empty() {
        body.insert(
            "initial_comment".into(),
            serde_json::Value::String(text.to_string()),
        );
    }
    if let Some(ts) = thread_ts {
        body.insert(
            "thread_ts".into(),
            serde_json::Value::String(ts.0.clone()),
        );
    }

    let complete_url = format!("{api_base}/api/files.completeUploadExternal");
    let complete: serde_json::Value = {
        rate_limiter.acquire().await;
        let r: serde_json::Value = http
            .post(&complete_url)
            .bearer_auth(token)
            .json(&serde_json::Value::Object(body.clone()))
            .send()
            .await
            .map_err(|e| format!("completeUploadExternal request: {e}"))?
            .json()
            .await
            .map_err(|e| format!("completeUploadExternal parse: {e}"))?;
        if !r["ok"].as_bool().unwrap_or(false) && r["error"].as_str() == Some("ratelimited") {
            tracing::warn!("Slack rate limit on completeUploadExternal, retrying after 5 s");
            tokio::time::sleep(Duration::from_secs(5)).await;
            rate_limiter.acquire().await;
            http.post(&complete_url)
                .bearer_auth(token)
                .json(&serde_json::Value::Object(body))
                .send()
                .await
                .map_err(|e| format!("completeUploadExternal retry request: {e}"))?
                .json()
                .await
                .map_err(|e| format!("completeUploadExternal retry parse: {e}"))?
        } else {
            r
        }
    };

    if !complete["ok"].as_bool().unwrap_or(false) {
        return Err(format!(
            "files.completeUploadExternal: {}",
            complete["error"].as_str().unwrap_or("unknown")
        ));
    }

    Ok(())
}

// ── View modal loops ──────────────────────────────────────────────────────────

pub async fn run_view_open_loop(
    consumer: Consumer<pull::Config>,
    bot_token: String,
    http_client: Arc<HttpClient>,
    rate_limiter: Arc<RateLimiter>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create view_open message stream");
            return;
        }
    };
    let url = format!("{SLACK_API_BASE}/api/views.open");

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on view_open consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackViewOpenRequest>(&msg.payload) {
            Ok(req) => {
                rate_limiter.acquire().await;
                match http_client
                    .post(&url)
                    .bearer_auth(&bot_token)
                    .json(&req)
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(body) if !body["ok"].as_bool().unwrap_or(false) => {
                            tracing::error!(
                                api_error = body["error"].as_str().unwrap_or("unknown"),
                                "views.open failed"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to parse views.open response");
                        }
                    },
                    Err(e) => tracing::error!(error = %e, "HTTP error calling views.open"),
                }
            }
            Err(e) => tracing::error!(error = %e, "Failed to deserialize SlackViewOpenRequest"),
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK view_open message");
        }
    }
}

pub async fn run_view_publish_loop(
    consumer: Consumer<pull::Config>,
    bot_token: String,
    http_client: Arc<HttpClient>,
    rate_limiter: Arc<RateLimiter>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create view_publish message stream");
            return;
        }
    };
    let url = format!("{SLACK_API_BASE}/api/views.publish");

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on view_publish consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackViewPublishRequest>(&msg.payload) {
            Ok(req) => {
                rate_limiter.acquire().await;
                match http_client
                    .post(&url)
                    .bearer_auth(&bot_token)
                    .json(&req)
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(body) if !body["ok"].as_bool().unwrap_or(false) => {
                            tracing::error!(
                                api_error = body["error"].as_str().unwrap_or("unknown"),
                                "views.publish failed"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to parse views.publish response");
                        }
                    },
                    Err(e) => tracing::error!(error = %e, "HTTP error calling views.publish"),
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize SlackViewPublishRequest");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK view_publish message");
        }
    }
}

pub async fn run_set_status_loop(
    consumer: Consumer<pull::Config>,
    bot_token: String,
    http_client: Arc<HttpClient>,
    rate_limiter: Arc<RateLimiter>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create set_status message stream");
            return;
        }
    };
    let url = format!("{SLACK_API_BASE}/api/assistant.threads.setStatus");

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on set_status consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackSetStatusRequest>(&msg.payload) {
            Ok(req) => {
                let mut body = serde_json::Map::new();
                body.insert(
                    "channel_id".into(),
                    serde_json::Value::String(req.channel_id.clone()),
                );
                body.insert(
                    "thread_ts".into(),
                    serde_json::Value::String(req.thread_ts.clone()),
                );
                if let Some(ref status) = req.status {
                    body.insert(
                        "status".into(),
                        serde_json::Value::String(status.clone()),
                    );
                } else {
                    body.insert(
                        "status".into(),
                        serde_json::Value::String(String::new()),
                    );
                }

                rate_limiter.acquire().await;
                match http_client
                    .post(&url)
                    .bearer_auth(&bot_token)
                    .json(&serde_json::Value::Object(body))
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(body) if !body["ok"].as_bool().unwrap_or(false) => {
                            tracing::error!(
                                api_error = body["error"].as_str().unwrap_or("unknown"),
                                "assistant.threads.setStatus failed"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(
                                error = %e,
                                "Failed to parse assistant.threads.setStatus response"
                            );
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "HTTP error calling assistant.threads.setStatus"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize SlackSetStatusRequest");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK set_status message");
        }
    }
}

pub async fn run_delete_loop(
    consumer: Consumer<pull::Config>,
    bot_token: String,
    http_client: Arc<HttpClient>,
    rate_limiter: Arc<RateLimiter>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create delete message stream");
            return;
        }
    };
    let url = format!("{SLACK_API_BASE}/api/chat.delete");

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on delete consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackDeleteMessage>(&msg.payload) {
            Ok(req) => {
                let mut body = serde_json::Map::new();
                body.insert("channel".into(), serde_json::Value::String(req.channel.clone()));
                body.insert("ts".into(), serde_json::Value::String(req.ts.clone()));

                rate_limiter.acquire().await;
                match http_client
                    .post(&url)
                    .bearer_auth(&bot_token)
                    .json(&serde_json::Value::Object(body))
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(body) if !body["ok"].as_bool().unwrap_or(false) => {
                            tracing::error!(
                                api_error = body["error"].as_str().unwrap_or("unknown"),
                                "chat.delete failed"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to parse chat.delete response");
                        }
                    },
                    Err(e) => {
                        tracing::error!(error = %e, "HTTP error calling chat.delete");
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize SlackDeleteMessage");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK delete message");
        }
    }
}

pub async fn run_update_loop(
    consumer: Consumer<pull::Config>,
    bot_token: String,
    http_client: Arc<HttpClient>,
    rate_limiter: Arc<RateLimiter>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create update message stream");
            return;
        }
    };
    let url = format!("{SLACK_API_BASE}/api/chat.update");

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on update consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackUpdateMessage>(&msg.payload) {
            Ok(req) => {
                let converted_text = format::markdown_to_mrkdwn(&req.text);
                let mut body = serde_json::Map::new();
                body.insert("channel".into(), serde_json::Value::String(req.channel.clone()));
                body.insert("ts".into(), serde_json::Value::String(req.ts.clone()));
                body.insert("text".into(), serde_json::Value::String(converted_text));
                if let Some(blocks) = req.blocks {
                    body.insert("blocks".into(), blocks);
                }

                rate_limiter.acquire().await;
                match http_client
                    .post(&url)
                    .bearer_auth(&bot_token)
                    .json(&serde_json::Value::Object(body))
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(body) if !body["ok"].as_bool().unwrap_or(false) => {
                            tracing::error!(
                                api_error = body["error"].as_str().unwrap_or("unknown"),
                                "chat.update failed"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to parse chat.update response");
                        }
                    },
                    Err(e) => {
                        tracing::error!(error = %e, "HTTP error calling chat.update");
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize SlackUpdateMessage");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK update message");
        }
    }
}


pub async fn run_suggested_prompts_loop(
    consumer: Consumer<pull::Config>,
    bot_token: String,
    http_client: Arc<HttpClient>,
    rate_limiter: Arc<RateLimiter>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create suggested_prompts message stream");
            return;
        }
    };
    let url = format!("{SLACK_API_BASE}/api/assistant.threads.setSuggestedPrompts");

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on suggested_prompts consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackSetSuggestedPromptsRequest>(&msg.payload) {
            Ok(req) => {
                let prompts: Vec<serde_json::Value> = req
                    .prompts
                    .iter()
                    .map(|p| serde_json::json!({"title": p.title, "message": p.message}))
                    .collect();
                let mut body = serde_json::Map::new();
                body.insert(
                    "channel_id".into(),
                    serde_json::Value::String(req.channel_id.clone()),
                );
                body.insert(
                    "thread_ts".into(),
                    serde_json::Value::String(req.thread_ts.clone()),
                );
                if let Some(ref title) = req.title {
                    body.insert("title".into(), serde_json::Value::String(title.clone()));
                }
                body.insert("prompts".into(), serde_json::Value::Array(prompts));

                let make_req = || {
                    http_client
                        .post(&url)
                        .bearer_auth(&bot_token)
                        .json(&serde_json::Value::Object(body.clone()))
                };
                rate_limiter.acquire().await;
                let resp_val: Option<serde_json::Value> = match make_req().send().await {
                    Ok(resp) => resp.json::<serde_json::Value>().await.ok(),
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "HTTP error calling assistant.threads.setSuggestedPrompts"
                        );
                        None
                    }
                };
                let resp_val = if let Some(ref v) = resp_val
                    && v["error"].as_str() == Some("ratelimited")
                {
                    tracing::warn!(
                        "Slack rate limit on setSuggestedPrompts, retrying after 5 s"
                    );
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    rate_limiter.acquire().await;
                    match make_req().send().await {
                        Ok(resp) => resp.json::<serde_json::Value>().await.ok(),
                        Err(e) => {
                            tracing::error!(
                                error = %e,
                                "HTTP error calling assistant.threads.setSuggestedPrompts (retry)"
                            );
                            None
                        }
                    }
                } else {
                    resp_val
                };
                match resp_val {
                    Some(body) if !body["ok"].as_bool().unwrap_or(false) => {
                        tracing::error!(
                            api_error = body["error"].as_str().unwrap_or("unknown"),
                            "assistant.threads.setSuggestedPrompts failed"
                        );
                    }
                    Some(_) => {}
                    None => {}
                }
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "Failed to deserialize SlackSetSuggestedPromptsRequest"
                );
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK suggested_prompts message");
        }
    }
}

pub async fn run_proactive_loop(
    consumer: Consumer<pull::Config>,
    slack_client: Arc<SlackHyperClient>,
    bot_token: String,
    http_client: Arc<HttpClient>,
    rate_limiter: Arc<RateLimiter>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create proactive message stream");
            return;
        }
    };
    let token = SlackApiToken::new(bot_token.clone().into());

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on proactive consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackProactiveMessage>(&msg.payload) {
            Ok(proactive) => {
                // Resolve channel: if only user_id provided, open a DM first.
                let channel_id = if let Some(ref uid) = proactive.user_id {
                    if proactive.channel.is_none() {
                        let open_url =
                            format!("{SLACK_API_BASE}/api/conversations.open");
                        rate_limiter.acquire().await;
                        match http_client
                            .post(&open_url)
                            .bearer_auth(&bot_token)
                            .json(&serde_json::json!({"users": uid}))
                            .send()
                            .await
                        {
                            Ok(resp) => match resp.json::<serde_json::Value>().await {
                                Ok(body) if body["ok"].as_bool().unwrap_or(false) => {
                                    body["channel"]["id"]
                                        .as_str()
                                        .map(String::from)
                                }
                                Ok(body) => {
                                    tracing::error!(
                                        api_error = body["error"].as_str().unwrap_or("unknown"),
                                        "conversations.open failed"
                                    );
                                    None
                                }
                                Err(e) => {
                                    tracing::error!(
                                        error = %e,
                                        "Failed to parse conversations.open response"
                                    );
                                    None
                                }
                            },
                            Err(e) => {
                                tracing::error!(
                                    error = %e,
                                    "HTTP error calling conversations.open"
                                );
                                None
                            }
                        }
                    } else {
                        proactive.channel.clone()
                    }
                } else {
                    proactive.channel.clone()
                };

                let channel_id = match channel_id {
                    Some(c) => c,
                    None => {
                        tracing::error!("No channel resolved for proactive message, skipping");
                        let _ = msg.ack().await;
                        continue;
                    }
                };

                let session = slack_client.open_session(&token);
                let channel: SlackChannelId = channel_id.into();
                let thread_ts: Option<SlackTs> = proactive.thread_ts.map(|ts| ts.into());
                let converted_text = format::markdown_to_mrkdwn(&proactive.text);

                let blocks: Option<Vec<SlackBlock>> =
                    proactive.blocks.as_ref().and_then(|s| {
                        serde_json::from_str::<Vec<SlackBlock>>(s)
                            .map_err(|e| {
                                tracing::warn!(
                                    error = %e,
                                    "Failed to parse proactive blocks JSON"
                                );
                            })
                            .ok()
                    });

                let mut content =
                    SlackMessageContent::new().with_text(converted_text.clone());
                if let Some(ref blks) = blocks {
                    content = content.with_blocks(blks.clone());
                }

                let mut request =
                    SlackApiChatPostMessageRequest::new(channel.clone(), content);
                if let Some(ref ts) = thread_ts {
                    request = request.with_thread_ts(ts.clone());
                }
                if let Some(ref username) = proactive.username {
                    request = request.with_username(username.clone());
                }
                if let Some(ref icon_url) = proactive.icon_url {
                    request = request.with_icon_url(icon_url.clone());
                }
                if let Some(ref icon_emoji) = proactive.icon_emoji {
                    request = request.with_icon_emoji(icon_emoji.clone());
                }

                rate_limiter.acquire().await;
                let post_result = {
                    let r = session.chat_post_message(&request).await;
                    if let Err(ref e) = r
                        && format!("{e}").contains("ratelimited")
                    {
                        tracing::warn!(
                            "Slack rate limit on proactive postMessage, retrying after 5 s"
                        );
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        slack_client
                            .open_session(&token)
                            .chat_post_message(&request)
                            .await
                    } else {
                        r
                    }
                };

                if let Err(e) = post_result {
                    tracing::error!(error = %e, "Failed to send proactive message to Slack");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize SlackProactiveMessage");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK proactive message");
        }
    }
}

pub async fn run_ephemeral_loop(
    consumer: Consumer<pull::Config>,
    bot_token: String,
    http_client: Arc<HttpClient>,
    rate_limiter: Arc<RateLimiter>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create ephemeral message stream");
            return;
        }
    };
    let url = format!("{SLACK_API_BASE}/api/chat.postEphemeral");

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on ephemeral consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackEphemeralMessage>(&msg.payload) {
            Ok(req) => {
                let mut body = serde_json::Map::new();
                body.insert(
                    "channel".into(),
                    serde_json::Value::String(req.channel.clone()),
                );
                body.insert(
                    "user".into(),
                    serde_json::Value::String(req.user.clone()),
                );
                body.insert(
                    "text".into(),
                    serde_json::Value::String(req.text.clone()),
                );
                if let Some(ref ts) = req.thread_ts {
                    body.insert(
                        "thread_ts".into(),
                        serde_json::Value::String(ts.clone()),
                    );
                }
                if let Some(ref blocks_str) = req.blocks {
                    match serde_json::from_str::<serde_json::Value>(blocks_str) {
                        Ok(blocks_val) => {
                            body.insert("blocks".into(), blocks_val);
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "Failed to parse ephemeral blocks JSON, sending text only"
                            );
                        }
                    }
                }

                let make_req = || {
                    http_client
                        .post(&url)
                        .bearer_auth(&bot_token)
                        .json(&serde_json::Value::Object(body.clone()))
                };
                rate_limiter.acquire().await;
                let resp_val: Option<serde_json::Value> = match make_req().send().await {
                    Ok(resp) => resp.json::<serde_json::Value>().await.ok(),
                    Err(e) => {
                        tracing::error!(error = %e, "HTTP error calling chat.postEphemeral");
                        None
                    }
                };
                let resp_val = if let Some(ref v) = resp_val
                    && v["error"].as_str() == Some("ratelimited")
                {
                    tracing::warn!("Slack rate limit on postEphemeral, retrying after 5 s");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    rate_limiter.acquire().await;
                    match make_req().send().await {
                        Ok(resp) => resp.json::<serde_json::Value>().await.ok(),
                        Err(e) => {
                            tracing::error!(
                                error = %e,
                                "HTTP error calling chat.postEphemeral (retry)"
                            );
                            None
                        }
                    }
                } else {
                    resp_val
                };
                match resp_val {
                    Some(body) if !body["ok"].as_bool().unwrap_or(false) => {
                        tracing::error!(
                            api_error = body["error"].as_str().unwrap_or("unknown"),
                            "chat.postEphemeral failed"
                        );
                    }
                    Some(_) => {}
                    None => {}
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize SlackEphemeralMessage");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK ephemeral message");
        }
    }
}

pub async fn run_delete_file_loop(
    consumer: Consumer<pull::Config>,
    bot_token: String,
    http_client: Arc<HttpClient>,
    rate_limiter: Arc<RateLimiter>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create delete_file message stream");
            return;
        }
    };
    let url = format!("{SLACK_API_BASE}/api/files.delete");

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on delete_file consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackDeleteFile>(&msg.payload) {
            Ok(req) => {
                let body = serde_json::json!({"file": req.file_id});

                let make_req = || {
                    http_client
                        .post(&url)
                        .bearer_auth(&bot_token)
                        .json(&body)
                };
                rate_limiter.acquire().await;
                let resp_val: Option<serde_json::Value> = match make_req().send().await {
                    Ok(resp) => resp.json::<serde_json::Value>().await.ok(),
                    Err(e) => {
                        tracing::error!(error = %e, "HTTP error calling files.delete");
                        None
                    }
                };
                let resp_val = if let Some(ref v) = resp_val
                    && v["error"].as_str() == Some("ratelimited")
                {
                    tracing::warn!("Slack rate limit on files.delete, retrying after 5 s");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    rate_limiter.acquire().await;
                    match make_req().send().await {
                        Ok(resp) => resp.json::<serde_json::Value>().await.ok(),
                        Err(e) => {
                            tracing::error!(
                                error = %e,
                                "HTTP error calling files.delete (retry)"
                            );
                            None
                        }
                    }
                } else {
                    resp_val
                };
                match resp_val {
                    Some(body) if !body["ok"].as_bool().unwrap_or(false) => {
                        tracing::error!(
                            api_error = body["error"].as_str().unwrap_or("unknown"),
                            "files.delete failed"
                        );
                    }
                    Some(_) => {}
                    None => {}
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize SlackDeleteFile");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK delete_file message");
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn channel(id: &str) -> SlackChannelId {
        SlackChannelId(id.to_string())
    }

    // ── extract_upload_filename ───────────────────────────────────────────────

    #[test]
    fn filename_basic_path() {
        assert_eq!(extract_upload_filename("https://example.com/foo/bar.png"), "bar.png");
    }

    #[test]
    fn filename_strips_query() {
        assert_eq!(
            extract_upload_filename("https://cdn.example.com/image.jpg?X-Amz-Expires=300"),
            "image.jpg"
        );
    }

    #[test]
    fn filename_trailing_slash_falls_back() {
        assert_eq!(extract_upload_filename("https://example.com/path/"), "file");
    }

    #[test]
    fn filename_no_path_falls_back() {
        assert_eq!(extract_upload_filename("https://example.com"), "example.com");
    }

    // ── upload_file_to_slack (wiremock) ───────────────────────────────────────

    #[tokio::test]
    async fn upload_success() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/media/photo.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"PNGDATA".as_ref())
                    .insert_header("content-type", "image/png"),
            )
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "upload_url": format!("{}/upload-slot", server.uri()),
                "file_id": "FTEST"
            })))
            .mount(&server)
            .await;

        Mock::given(method("PUT"))
            .and(path("/upload-slot"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.completeUploadExternal"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})),
            )
            .mount(&server)
            .await;

        let result = upload_file_to_slack(
            &HttpClient::new(),
            "xoxb-test",
            &channel("C123"),
            None,
            "look at this",
            &format!("{}/media/photo.png", server.uri()),
            &server.uri(),
            &RateLimiter::new(100.0),
        )
        .await;

        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn upload_with_thread_ts_and_empty_text() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/f.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello".as_ref()))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "upload_url": format!("{}/slot", server.uri()),
                "file_id": "F2"
            })))
            .mount(&server)
            .await;

        Mock::given(method("PUT"))
            .and(path("/slot"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.completeUploadExternal"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})),
            )
            .mount(&server)
            .await;

        let ts = SlackTs("1234567890.000100".to_string());
        let result = upload_file_to_slack(
            &HttpClient::new(),
            "xoxb-test",
            &channel("C456"),
            Some(&ts),
            "",
            &format!("{}/f.txt", server.uri()),
            &server.uri(),
            &RateLimiter::new(100.0),
        )
        .await;

        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn upload_geturl_error_propagated() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/file.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"data".as_ref()))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"ok": false, "error": "not_allowed"}),
            ))
            .mount(&server)
            .await;

        let result = upload_file_to_slack(
            &HttpClient::new(),
            "xoxb-test",
            &channel("C123"),
            None,
            "",
            &format!("{}/file.bin", server.uri()),
            &server.uri(),
            &RateLimiter::new(100.0),
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not_allowed"), "expected not_allowed in error");
    }

    #[tokio::test]
    async fn upload_complete_error_propagated() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/f.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"x".as_ref()))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "upload_url": format!("{}/slot", server.uri()),
                "file_id": "F3"
            })))
            .mount(&server)
            .await;

        Mock::given(method("PUT"))
            .and(path("/slot"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.completeUploadExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"ok": false, "error": "invalid_auth"}),
            ))
            .mount(&server)
            .await;

        let result = upload_file_to_slack(
            &HttpClient::new(),
            "xoxb-test",
            &channel("C123"),
            None,
            "",
            &format!("{}/f.bin", server.uri()),
            &server.uri(),
            &RateLimiter::new(100.0),
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid_auth"), "expected invalid_auth in error");
    }

    #[test]
    fn delete_message_roundtrip() {
        let msg = SlackDeleteMessage {
            channel: "C123".into(),
            ts: "1234567890.001".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackDeleteMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C123");
        assert_eq!(decoded.ts, "1234567890.001");
    }

    #[test]
    fn update_message_roundtrip() {
        let msg = SlackUpdateMessage {
            channel: "C456".into(),
            ts: "1234567890.002".into(),
            text: "updated text".into(),
            blocks: Some(serde_json::json!([{"type": "section"}])),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackUpdateMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C456");
        assert_eq!(decoded.ts, "1234567890.002");
        assert_eq!(decoded.text, "updated text");
        assert!(decoded.blocks.is_some());
    }
}

pub async fn run_upload_loop(
    consumer: Consumer<pull::Config>,
    bot_token: String,
    http_client: Arc<HttpClient>,
    rate_limiter: Arc<RateLimiter>,
    nats_client: async_nats::Client,
) {
    use slack_types::events::SlackUploadRequest;

    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to subscribe to upload consumer");
            return;
        }
    };

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on upload consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackUploadRequest>(&msg.payload) {
            Ok(req) => {
                let content_bytes = req.content.as_bytes();
                let length = content_bytes.len();

                // Step 1: Get upload URL
                rate_limiter.acquire().await;
                let get_url_resp = http_client
                    .post("https://slack.com/api/files.getUploadURLExternal")
                    .bearer_auth(&bot_token)
                    .form(&[
                        ("filename", req.filename.as_str()),
                        ("length", &length.to_string()),
                    ])
                    .send()
                    .await;

                let (upload_url, file_id) = match get_url_resp {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(v) if v["ok"].as_bool().unwrap_or(false) => {
                            let url = v["upload_url"].as_str().unwrap_or("").to_string();
                            let id = v["file_id"].as_str().unwrap_or("").to_string();
                            if url.is_empty() || id.is_empty() {
                                tracing::error!("files.getUploadURLExternal missing fields");
                                let _ = msg.ack().await;
                                continue;
                            }
                            (url, id)
                        }
                        Ok(v) => {
                            tracing::error!(
                                error = v["error"].as_str().unwrap_or("unknown"),
                                "files.getUploadURLExternal failed"
                            );
                            let _ = msg.ack().await;
                            continue;
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to parse getUploadURLExternal");
                            let _ = msg.ack().await;
                            continue;
                        }
                    },
                    Err(e) => {
                        tracing::error!(error = %e, "HTTP error on getUploadURLExternal");
                        let _ = msg.ack().await;
                        continue;
                    }
                };

                // Step 2: Upload content
                let upload_resp = http_client
                    .post(&upload_url)
                    .body(req.content.clone())
                    .send()
                    .await;

                if let Err(e) = upload_resp {
                    tracing::error!(error = %e, "Failed to upload file content");
                    let _ = msg.ack().await;
                    continue;
                }

                // Step 3: Complete upload
                let mut complete_body = serde_json::json!({
                    "files": [{"id": file_id, "title": req.title.as_deref().unwrap_or(&req.filename)}],
                    "channel_id": req.channel,
                });
                if let Some(ref thread_ts) = req.thread_ts {
                    complete_body["thread_ts"] = serde_json::Value::String(thread_ts.clone());
                }

                rate_limiter.acquire().await;
                match http_client
                    .post("https://slack.com/api/files.completeUploadExternal")
                    .bearer_auth(&bot_token)
                    .json(&complete_body)
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(v) if v["ok"].as_bool().unwrap_or(false) => {
                            tracing::info!(file_id = %file_id, "File uploaded successfully");
                            // Notify agent of the file_id for potential future deletion.
                            let notification = serde_json::json!({
                                "channel": req.channel,
                                "thread_ts": req.thread_ts,
                                "file_id": file_id,
                            }).to_string();
                            let _ = nats_client.publish("slack.file.uploaded", notification.into()).await;
                        }
                        Ok(v) => {
                            tracing::error!(
                                error = v["error"].as_str().unwrap_or("unknown"),
                                "files.completeUploadExternal failed"
                            );
                        }
                        Err(e) => tracing::error!(error = %e, "Failed to parse completeUploadExternal"),
                    },
                    Err(e) => tracing::error!(error = %e, "HTTP error on completeUploadExternal"),
                }
            }
            Err(e) => tracing::error!(error = %e, "Failed to deserialize SlackUploadRequest"),
        }

        let _ = msg.ack().await;
    }
}
