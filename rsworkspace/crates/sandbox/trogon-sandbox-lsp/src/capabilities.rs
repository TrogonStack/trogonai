use serde_json::Value;

use crate::constants::DEFAULT_CLIENT_CAPABILITIES;
use crate::lsp_error::LspError;

/// The default client capabilities with `extra` merged over them, object by object.
pub(crate) fn client_capabilities(extra: Option<&Value>) -> Result<Value, LspError> {
    let base: Value = serde_json::from_str(DEFAULT_CLIENT_CAPABILITIES)?;
    Ok(merge_capabilities(base, extra))
}

fn merge_capabilities(base: Value, extra: Option<&Value>) -> Value {
    let Some(extra) = extra else { return base };
    match (base, extra) {
        (Value::Object(mut base), Value::Object(extra)) => {
            for (key, value) in extra {
                let merged = match base.remove(key) {
                    Some(existing @ Value::Object(_)) if value.is_object() => merge_capabilities(existing, Some(value)),
                    _ => value.clone(),
                };
                base.insert(key.clone(), merged);
            }
            Value::Object(base)
        }
        (_, extra) => extra.clone(),
    }
}
