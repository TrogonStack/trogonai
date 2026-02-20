use crate::user_settings::UserSettings;
use serde_json::Value;

/// Build the Block Kit view for the App Home tab.
pub fn build_app_home_view(model: &str, history_count: usize, system_prompt: Option<&str>) -> Value {
    let prompt_status = if system_prompt.is_some() { "Custom" } else { "Default" };
    serde_json::json!({
        "type": "home",
        "blocks": [
            {
                "type": "header",
                "text": { "type": "plain_text", "text": ":robot_face: AI Assistant", "emoji": true }
            },
            { "type": "divider" },
            {
                "type": "section",
                "fields": [
                    { "type": "mrkdwn", "text": format!("*Model*\n`{}`", model) },
                    { "type": "mrkdwn", "text": format!("*System Prompt*\n{}", prompt_status) },
                    { "type": "mrkdwn", "text": format!("*History*\n{} messages", history_count) }
                ]
            },
            { "type": "divider" },
            {
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": "_Customize your model and system prompt, or clear your conversation history below._"
                }
            },
            {
                "type": "actions",
                "elements": [
                    {
                        "type": "button",
                        "text": { "type": "plain_text", "text": ":gear: Settings", "emoji": true },
                        "action_id": "open_settings"
                    },
                    {
                        "type": "button",
                        "text": { "type": "plain_text", "text": ":wastebasket: Clear History", "emoji": true },
                        "action_id": "clear_history",
                        "style": "danger",
                        "confirm": {
                            "title": { "type": "plain_text", "text": "Clear History?" },
                            "text": {
                                "type": "mrkdwn",
                                "text": "This will permanently delete all your conversation history with this bot."
                            },
                            "confirm": { "type": "plain_text", "text": "Yes, clear it" },
                            "deny": { "type": "plain_text", "text": "Cancel" }
                        }
                    }
                ]
            }
        ]
    })
}

/// Build the settings modal (`callback_id: "user_settings"`).
pub fn build_settings_modal(settings: &UserSettings, default_model: &str) -> Value {
    let current_model = settings.model.as_deref().unwrap_or(default_model);
    let system_prompt_value = settings.system_prompt.as_deref().unwrap_or("");

    let model_options = [
        ("Claude Sonnet 4.6 (default)", "claude-sonnet-4-6"),
        ("Claude Opus 4.6", "claude-opus-4-6"),
        ("Claude Haiku 4.5", "claude-haiku-4-5-20251001"),
    ];

    let options: Vec<Value> = model_options
        .iter()
        .map(|(label, value)| {
            serde_json::json!({
                "text": { "type": "plain_text", "text": label },
                "value": value
            })
        })
        .collect();

    let initial_option = model_options
        .iter()
        .find(|(_, v)| *v == current_model)
        .map(|(label, value)| {
            serde_json::json!({
                "text": { "type": "plain_text", "text": label },
                "value": value
            })
        });

    let mut model_element = serde_json::json!({
        "type": "static_select",
        "action_id": "model_select",
        "placeholder": { "type": "plain_text", "text": "Select a model" },
        "options": options
    });
    if let Some(opt) = initial_option {
        model_element["initial_option"] = opt;
    }

    serde_json::json!({
        "type": "modal",
        "callback_id": "user_settings",
        "title": { "type": "plain_text", "text": "AI Settings" },
        "submit": { "type": "plain_text", "text": "Save" },
        "close": { "type": "plain_text", "text": "Cancel" },
        "blocks": [
            {
                "type": "input",
                "block_id": "model_block",
                "label": { "type": "plain_text", "text": "Model" },
                "element": model_element,
                "optional": true
            },
            {
                "type": "input",
                "block_id": "system_prompt_block",
                "label": { "type": "plain_text", "text": "System Prompt" },
                "hint": {
                    "type": "plain_text",
                    "text": "Overrides the global system prompt for your conversations. Leave blank to use the default."
                },
                "element": {
                    "type": "plain_text_input",
                    "action_id": "system_prompt_input",
                    "multiline": true,
                    "placeholder": {
                        "type": "plain_text",
                        "text": "You are a helpful assistant..."
                    },
                    "initial_value": system_prompt_value
                },
                "optional": true
            }
        ]
    })
}
