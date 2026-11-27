use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ApplicationAcknowledgment {
    ApplicationAcknowledged,
}
