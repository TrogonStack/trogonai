//! Message handlers for Telegram updates

use teloxide::prelude::*;
use teloxide::types::{Message, CallbackQuery};
use tracing::{debug, error, info, warn};

use crate::bridge::TelegramBridge;

/// Handle text messages
pub async fn handle_text_message(_bot: Bot, msg: Message, bridge: TelegramBridge) -> ResponseResult<()> {
    let text = msg.text().unwrap_or_default();
    let update_id = msg.id.0 as i64;

    debug!("Received text message: {}", text);

    // Check for commands
    if text.starts_with('/') {
        return handle_command(msg, bridge, update_id).await;
    }

    // Check access control
    if !check_access(&msg, &bridge) {
        warn!("Access denied for user {} in chat {}",
            msg.from.as_ref().map(|u| u.id.0).unwrap_or(0),
            msg.chat.id.0);
        return Ok(());
    }

    // Publish to NATS
    if let Err(e) = bridge.publish_text_message(&msg, update_id).await {
        error!("Failed to publish text message: {}", e);
    }

    Ok(())
}

/// Handle photo messages
pub async fn handle_photo_message(_bot: Bot, msg: Message, bridge: TelegramBridge) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received photo message");

    if !check_access(&msg, &bridge) {
        warn!("Access denied for photo message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_photo_message(&msg, update_id).await {
        error!("Failed to publish photo message: {}", e);
    }

    Ok(())
}

/// Handle video messages
pub async fn handle_video_message(_bot: Bot, msg: Message, bridge: TelegramBridge) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received video message");

    if !check_access(&msg, &bridge) {
        warn!("Access denied for video message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_video_message(&msg, update_id).await {
        error!("Failed to publish video message: {}", e);
    }

    Ok(())
}

/// Handle audio messages
pub async fn handle_audio_message(_bot: Bot, msg: Message, bridge: TelegramBridge) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received audio message");

    if !check_access(&msg, &bridge) {
        warn!("Access denied for audio message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_audio_message(&msg, update_id).await {
        error!("Failed to publish audio message: {}", e);
    }

    Ok(())
}

/// Handle document messages
pub async fn handle_document_message(_bot: Bot, msg: Message, bridge: TelegramBridge) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received document message");

    if !check_access(&msg, &bridge) {
        warn!("Access denied for document message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_document_message(&msg, update_id).await {
        error!("Failed to publish document message: {}", e);
    }

    Ok(())
}

/// Handle voice messages
pub async fn handle_voice_message(_bot: Bot, msg: Message, bridge: TelegramBridge) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received voice message");

    if !check_access(&msg, &bridge) {
        warn!("Access denied for voice message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_voice_message(&msg, update_id).await {
        error!("Failed to publish voice message: {}", e);
    }

    Ok(())
}

/// Handle callback queries (button clicks)
pub async fn handle_callback_query(_bot: Bot, query: CallbackQuery, bridge: TelegramBridge) -> ResponseResult<()> {
    let update_id = query.id.parse::<i64>().unwrap_or(0);

    debug!("Received callback query: {:?}", query.data);

    // Check access
    let user_id = query.from.id.0 as i64;
    if !bridge.check_dm_access(user_id) && !bridge.access_config.is_admin(user_id) {
        warn!("Access denied for callback query from user {}", user_id);
        return Ok(());
    }

    if let Err(e) = bridge.publish_callback_query(&query, update_id).await {
        error!("Failed to publish callback query: {}", e);
    }

    Ok(())
}

/// Handle commands
async fn handle_command(msg: Message, bridge: TelegramBridge, update_id: i64) -> ResponseResult<()> {
    let text = msg.text().unwrap_or_default();
    let parts: Vec<&str> = text.split_whitespace().collect();

    if parts.is_empty() {
        return Ok(());
    }

    let command = parts[0].trim_start_matches('/').to_lowercase();
    let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();

    info!("Received command: {} with {} args", command, args.len());

    // Check access
    if !check_access(&msg, &bridge) {
        warn!("Access denied for command: {}", command);
        return Ok(());
    }

    // Publish command to NATS
    if let Err(e) = bridge.publish_command(&msg, &command, args, update_id).await {
        error!("Failed to publish command: {}", e);
    }

    Ok(())
}

/// Check if the message sender has access
fn check_access(msg: &Message, bridge: &TelegramBridge) -> bool {
    use teloxide::types::ChatKind;

    let user_id = msg.from.as_ref().map(|u| u.id.0 as i64).unwrap_or(0);

    // Admins always have access
    if bridge.access_config.is_admin(user_id) {
        return true;
    }

    match &msg.chat.kind {
        ChatKind::Private(_) => bridge.check_dm_access(user_id),
        ChatKind::Public(_) => {
            let chat_id = msg.chat.id.0;
            bridge.check_group_access(chat_id)
        }
    }
}
