//! Message handlers for Telegram updates

use teloxide::prelude::*;
use teloxide::types::{Message, CallbackQuery};
use tracing::{debug, error, info, warn};

use crate::bridge::TelegramBridge;
use crate::health::AppState;

/// Handle text messages
pub async fn handle_text_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let text = msg.text().unwrap_or_default();
    let update_id = msg.id.0 as i64;

    debug!("Received text message: {}", text);

    // Increment metrics
    health.increment_messages_received().await;

    // Check for commands
    if text.starts_with('/') {
        return handle_command(msg, bridge, health, update_id).await;
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
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle photo messages
pub async fn handle_photo_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received photo message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for photo message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_photo_message(&msg, update_id).await {
        error!("Failed to publish photo message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle video messages
pub async fn handle_video_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received video message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for video message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_video_message(&msg, update_id).await {
        error!("Failed to publish video message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle audio messages
pub async fn handle_audio_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received audio message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for audio message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_audio_message(&msg, update_id).await {
        error!("Failed to publish audio message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle document messages
pub async fn handle_document_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received document message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for document message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_document_message(&msg, update_id).await {
        error!("Failed to publish document message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle voice messages
pub async fn handle_voice_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received voice message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for voice message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_voice_message(&msg, update_id).await {
        error!("Failed to publish voice message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle callback queries (button clicks)
pub async fn handle_callback_query(_bot: Bot, query: CallbackQuery, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = query.id.parse::<i64>().unwrap_or(0);

    debug!("Received callback query: {:?}", query.data);
    health.increment_messages_received().await;

    // Check access
    let user_id = query.from.id.0 as i64;
    if !bridge.check_dm_access(user_id) && !bridge.access_config.is_admin(user_id) {
        warn!("Access denied for callback query from user {}", user_id);
        return Ok(());
    }

    if let Err(e) = bridge.publish_callback_query(&query, update_id).await {
        error!("Failed to publish callback query: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle commands
async fn handle_command(msg: Message, bridge: TelegramBridge, health: AppState, update_id: i64) -> ResponseResult<()> {
    let text = msg.text().unwrap_or_default();
    let parts: Vec<&str> = text.split_whitespace().collect();

    if parts.is_empty() {
        return Ok(());
    }

    let command = parts[0].trim_start_matches('/').to_lowercase();
    let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();

    info!("Received command: {} with {} args", command, args.len());
    health.increment_commands().await;

    // Check access
    if !check_access(&msg, &bridge) {
        warn!("Access denied for command: {}", command);
        return Ok(());
    }

    // Publish command to NATS
    if let Err(e) = bridge.publish_command(&msg, &command, args, update_id).await {
        error!("Failed to publish command: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle inline queries (@bot query)
pub async fn handle_inline_query(_bot: Bot, query: teloxide::types::InlineQuery, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    use telegram_types::events::{InlineQueryEvent, Location};

    let update_id = 0; // Inline queries don't have update_id

    debug!("Received inline query: {}", query.query);
    health.increment_messages_received().await;

    // Check access control
    let user_id = query.from.id.0 as i64;
    if !bridge.access_config.is_admin(user_id) && !bridge.check_dm_access(user_id) {
        warn!("Access denied for inline query from user {}", user_id);
        return Ok(());
    }

    // Create session ID for inline queries (user-specific)
    let session_id = format!("tg-inline-{}", user_id);

    // Convert location if present
    let location = query.location.map(|loc| Location {
        longitude: loc.longitude,
        latitude: loc.latitude,
    });

    // Create inline query event
    let event = InlineQueryEvent {
        metadata: telegram_types::events::EventMetadata::new(session_id, update_id),
        inline_query_id: query.id.clone(),
        from: telegram_types::chat::User {
            id: user_id,
            is_bot: query.from.is_bot,
            first_name: query.from.first_name.clone(),
            last_name: query.from.last_name.clone(),
            username: query.from.username.clone(),
            language_code: query.from.language_code.clone(),
        },
        query: query.query.clone(),
        offset: query.offset.clone(),
        chat_type: query.chat_type.map(|ct| format!("{:?}", ct)),
        location,
    };

    // Publish to NATS
    if let Err(e) = bridge.publish_inline_query(&event).await {
        error!("Failed to publish inline query: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle chosen inline results
pub async fn handle_chosen_inline_result(_bot: Bot, result: teloxide::types::ChosenInlineResult, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    use telegram_types::events::ChosenInlineResultEvent;

    let update_id = 0;

    debug!("Inline result chosen: {}", result.result_id);
    health.increment_messages_received().await;

    let user_id = result.from.id.0 as i64;
    let session_id = format!("tg-inline-{}", user_id);

    let event = ChosenInlineResultEvent {
        metadata: telegram_types::events::EventMetadata::new(session_id, update_id),
        result_id: result.result_id.clone(),
        from: telegram_types::chat::User {
            id: user_id,
            is_bot: result.from.is_bot,
            first_name: result.from.first_name.clone(),
            last_name: result.from.last_name.clone(),
            username: result.from.username.clone(),
            language_code: result.from.language_code.clone(),
        },
        query: result.query.clone(),
        inline_message_id: result.inline_message_id.clone(),
    };

    // Publish to NATS
    if let Err(e) = bridge.publish_chosen_inline_result(&event).await {
        error!("Failed to publish chosen inline result: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle chat member updates (when other users join/leave/get banned)
pub async fn handle_chat_member_updated(
    _bot: Bot,
    update: teloxide::types::ChatMemberUpdated,
    bridge: TelegramBridge,
    health: AppState
) -> ResponseResult<()> {
    let update_id = 0; // Chat member updates don't have a standard update_id

    debug!("Chat member updated: {:?} -> {:?}",
        update.old_chat_member.kind,
        update.new_chat_member.kind
    );
    health.increment_messages_received().await;

    // Publish to NATS
    if let Err(e) = bridge.publish_chat_member_updated(&update, update_id).await {
        error!("Failed to publish chat member updated: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle my chat member updates (when bot's status changes in a chat)
pub async fn handle_my_chat_member_updated(
    _bot: Bot,
    update: teloxide::types::ChatMemberUpdated,
    bridge: TelegramBridge,
    health: AppState
) -> ResponseResult<()> {
    let update_id = 0;

    info!("Bot chat member status changed: {:?} -> {:?} in chat {}",
        update.old_chat_member.kind,
        update.new_chat_member.kind,
        update.chat.id
    );
    health.increment_messages_received().await;

    // Publish to NATS
    if let Err(e) = bridge.publish_my_chat_member_updated(&update, update_id).await {
        error!("Failed to publish my chat member updated: {}", e);
        health.increment_errors().await;
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
