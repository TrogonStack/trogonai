use serde::Deserialize;

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) enum DetachHookEvent {
    SessionEnd,
}
