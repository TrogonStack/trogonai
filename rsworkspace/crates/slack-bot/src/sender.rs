use async_nats::jetstream::consumer::{Consumer, pull};
use futures::StreamExt;
use reqwest::Client as HttpClient;
use slack_morphism::prelude::*;
use slack_types::events::{
    SlackOutboundMessage, SlackReactionAction, SlackStreamAppendMessage, SlackStreamStopMessage,
};
use std::sync::Arc;
use std::time::Duration;

use crate::format;

/// Slack rate limit for `chat.postMessage`: ~1 request/second per channel.
/// We use a conservative global delay to stay safely below the limit.
const POST_MESSAGE_DELAY: Duration = Duration::from_millis(1_100);

pub async fn run_outbound_loop(
    consumer: Consumer<pull::Config>,
    slack_client: Arc<SlackHyperClient>,
    bot_token: String,
    http_client: Arc<HttpClient>,
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

                let chunks = format::chunk_text(&converted_text, format::SLACK_TEXT_LIMIT);
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
    slack_client: Arc<SlackHyperClient>,
    bot_token: String,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create stream_append message stream");
            return;
        }
    };
    let token = SlackApiToken::new(bot_token.into());

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
                let session = slack_client.open_session(&token);
                let channel: SlackChannelId = append.channel.into();
                let ts: SlackTs = append.ts.into();
                let converted = format::markdown_to_mrkdwn(&append.text);
                let content = SlackMessageContent::new().with_text(converted);
                let request = SlackApiChatUpdateRequest::new(channel, content, ts);
                let update_result = {
                    let r = session.chat_update(&request).await;
                    if let Err(ref e) = r
                        && format!("{e}").contains("ratelimited")
                    {
                        tracing::warn!(
                            "Slack rate limit on chat.update (append), retrying after 5 s"
                        );
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        slack_client
                            .open_session(&token)
                            .chat_update(&request)
                            .await
                    } else {
                        r
                    }
                };
                if let Err(e) = update_result {
                    tracing::error!(error = %e, "Failed to update streaming message (append)");
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
    slack_client: Arc<SlackHyperClient>,
    bot_token: String,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create stream_stop message stream");
            return;
        }
    };
    let token = SlackApiToken::new(bot_token.into());

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
                let session = slack_client.open_session(&token);
                let channel: SlackChannelId = stop.channel.into();
                let ts: SlackTs = stop.ts.into();

                let blocks: Option<Vec<SlackBlock>> = stop.blocks.as_ref().and_then(|v| {
                    let bytes = serde_json::to_vec(v).ok()?;
                    serde_json::from_slice::<Vec<SlackBlock>>(&bytes)
                        .map_err(|e| {
                            tracing::warn!(error = %e, "Failed to parse stop blocks JSON");
                        })
                        .ok()
                });

                let converted_final = format::markdown_to_mrkdwn(&stop.final_text);
                let mut content = SlackMessageContent::new().with_text(converted_final);
                if let Some(blks) = blocks {
                    content = content.with_blocks(blks);
                }

                let request = SlackApiChatUpdateRequest::new(channel, content, ts);
                let update_result = {
                    let r = session.chat_update(&request).await;
                    if let Err(ref e) = r
                        && format!("{e}").contains("ratelimited")
                    {
                        tracing::warn!(
                            "Slack rate limit on chat.update (stop), retrying after 5 s"
                        );
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        slack_client
                            .open_session(&token)
                            .chat_update(&request)
                            .await
                    } else {
                        r
                    }
                };
                if let Err(e) = update_result {
                    tracing::error!(error = %e, "Failed to update streaming message (stop)");
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

/// Upload a file to Slack using the two-step API introduced in 2023:
/// 1. `files.getUploadURLExternal` — obtain a pre-signed upload URL + file ID
/// 2. PUT the raw bytes to that URL
/// 3. `files.completeUploadExternal` — associate the file with a channel
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
) -> Result<(), String> {
    // 1. Download the file from the media URL.
    let download = http
        .get(media_url)
        .send()
        .await
        .map_err(|e| format!("download request: {e}"))?;

    let filename = media_url
        .rsplit('/')
        .next()
        .and_then(|s| s.split('?').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("file")
        .to_string();

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

    // 2. Ask Slack for a pre-signed upload URL.
    let get_url: serde_json::Value = http
        .post("https://slack.com/api/files.getUploadURLExternal")
        .bearer_auth(token)
        .form(&[("filename", filename.as_str()), ("length", &length.to_string())])
        .send()
        .await
        .map_err(|e| format!("getUploadURLExternal request: {e}"))?
        .json()
        .await
        .map_err(|e| format!("getUploadURLExternal parse: {e}"))?;

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
    http.put(&upload_url)
        .header(reqwest::header::CONTENT_TYPE, &content_type)
        .body(bytes)
        .send()
        .await
        .map_err(|e| format!("upload PUT: {e}"))?;

    // 4. Complete the upload and associate it with the channel.
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

    let complete: serde_json::Value = http
        .post("https://slack.com/api/files.completeUploadExternal")
        .bearer_auth(token)
        .json(&serde_json::Value::Object(body))
        .send()
        .await
        .map_err(|e| format!("completeUploadExternal request: {e}"))?
        .json()
        .await
        .map_err(|e| format!("completeUploadExternal parse: {e}"))?;

    if !complete["ok"].as_bool().unwrap_or(false) {
        return Err(format!(
            "files.completeUploadExternal: {}",
            complete["error"].as_str().unwrap_or("unknown")
        ));
    }

    Ok(())
}
