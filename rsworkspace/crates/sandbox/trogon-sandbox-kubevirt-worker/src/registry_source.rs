use serde_json::{Map, Value};
use trogon_sandbox_contract::OciImageRef;

/// The DataVolume source that imports the registry image `image`.
pub(crate) fn registry_source(image: &OciImageRef) -> Value {
    let mut registry = Map::new();
    registry.insert(
        "url".to_owned(),
        Value::String(format!("docker://{}", image.allowlist_string())),
    );
    let mut source = Map::new();
    source.insert("registry".to_owned(), Value::Object(registry));
    Value::Object(source)
}
