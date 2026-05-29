use clap::ValueEnum;

use crate::output::{emit_json, emit_toml};
use crate::settings::CtlSettings;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ConfigFormat {
    Json,
    Toml,
}

pub fn run(
    settings: &CtlSettings,
    format: ConfigFormat,
    pretty: bool,
) -> Result<(), String> {
    let view = settings.to_show_view();
    match format {
        ConfigFormat::Json => emit_json(&view, pretty).map_err(|error| error.to_string()),
        ConfigFormat::Toml => emit_toml(&view).map_err(|error| error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_view_serializes_json_fields() {
        let settings = CtlSettings::from_env_and_config(None).expect("settings");
        let view = settings.to_show_view();
        let json = serde_json::to_value(&view).expect("json");
        assert_eq!(json["mcp_prefix"], "mcp");
        assert_eq!(json["audit_subject_wildcard"], "mcp.audit.>");
    }
}
