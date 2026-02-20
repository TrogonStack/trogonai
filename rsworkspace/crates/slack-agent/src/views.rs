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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user_settings::UserSettings;

    #[test]
    fn app_home_view_type_is_home() {
        let view = build_app_home_view("claude-sonnet-4-6", 5, None);
        assert_eq!(view["type"], "home");
    }

    #[test]
    fn app_home_view_has_blocks() {
        let view = build_app_home_view("claude-sonnet-4-6", 5, None);
        let blocks = view["blocks"].as_array().unwrap();
        assert!(!blocks.is_empty());
    }

    #[test]
    fn app_home_view_shows_model() {
        let view = build_app_home_view("claude-sonnet-4-6", 5, None);
        let s = serde_json::to_string(&view).unwrap();
        assert!(s.contains("claude-sonnet-4-6"));
    }

    #[test]
    fn app_home_view_shows_history_count() {
        let view = build_app_home_view("claude-sonnet-4-6", 42, None);
        let s = serde_json::to_string(&view).unwrap();
        assert!(s.contains("42"));
    }

    #[test]
    fn app_home_view_default_prompt_status() {
        let view = build_app_home_view("claude-sonnet-4-6", 5, None);
        let s = serde_json::to_string(&view).unwrap();
        assert!(s.contains("Default"));
    }

    #[test]
    fn app_home_view_custom_prompt_status() {
        let view = build_app_home_view("claude-sonnet-4-6", 5, Some("You are helpful"));
        let s = serde_json::to_string(&view).unwrap();
        assert!(s.contains("Custom"));
    }

    #[test]
    fn app_home_view_has_open_settings_action() {
        let view = build_app_home_view("claude-sonnet-4-6", 5, None);
        let s = serde_json::to_string(&view).unwrap();
        assert!(s.contains("open_settings"));
    }

    #[test]
    fn app_home_view_has_clear_history_action() {
        let view = build_app_home_view("claude-sonnet-4-6", 5, None);
        let s = serde_json::to_string(&view).unwrap();
        assert!(s.contains("clear_history"));
    }

    #[test]
    fn settings_modal_type_is_modal() {
        let settings = UserSettings::default();
        let modal = build_settings_modal(&settings, "claude-sonnet-4-6");
        assert_eq!(modal["type"], "modal");
    }

    #[test]
    fn settings_modal_callback_id() {
        let settings = UserSettings::default();
        let modal = build_settings_modal(&settings, "claude-sonnet-4-6");
        assert_eq!(modal["callback_id"], "user_settings");
    }

    #[test]
    fn settings_modal_has_model_block() {
        let settings = UserSettings::default();
        let modal = build_settings_modal(&settings, "claude-sonnet-4-6");
        let blocks = modal["blocks"].as_array().unwrap();
        let has_model = blocks.iter().any(|b| b["block_id"] == "model_block");
        assert!(has_model);
    }

    #[test]
    fn settings_modal_has_system_prompt_block() {
        let settings = UserSettings::default();
        let modal = build_settings_modal(&settings, "claude-sonnet-4-6");
        let blocks = modal["blocks"].as_array().unwrap();
        let has_system_prompt = blocks.iter().any(|b| b["block_id"] == "system_prompt_block");
        assert!(has_system_prompt);
    }

    #[test]
    fn settings_modal_initial_option_set_when_model_matches() {
        let settings = UserSettings {
            model: Some("claude-opus-4-6".to_string()),
            system_prompt: None,
        };
        let modal = build_settings_modal(&settings, "claude-sonnet-4-6");
        let s = serde_json::to_string(&modal).unwrap();
        assert!(s.contains("claude-opus-4-6"));
    }

    #[test]
    fn settings_modal_no_initial_option_for_unknown_model() {
        let settings = UserSettings {
            model: Some("some-custom-model".to_string()),
            system_prompt: None,
        };
        let modal = build_settings_modal(&settings, "claude-sonnet-4-6");
        let s = serde_json::to_string(&modal).unwrap();
        assert!(!s.contains("initial_option"));
    }

    #[test]
    fn settings_modal_system_prompt_initial_value() {
        let settings = UserSettings {
            model: None,
            system_prompt: Some("Be concise".to_string()),
        };
        let modal = build_settings_modal(&settings, "claude-sonnet-4-6");
        let s = serde_json::to_string(&modal).unwrap();
        assert!(s.contains("Be concise"));
    }

    #[test]
    fn settings_modal_empty_system_prompt_when_none() {
        let settings = UserSettings::default();
        let modal = build_settings_modal(&settings, "claude-sonnet-4-6");
        let blocks = modal["blocks"].as_array().unwrap();
        let system_prompt_block = blocks
            .iter()
            .find(|b| b["block_id"] == "system_prompt_block")
            .unwrap();
        assert_eq!(system_prompt_block["element"]["initial_value"], "");
    }
}
