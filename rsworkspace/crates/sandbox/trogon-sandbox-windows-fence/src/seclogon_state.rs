/// Readiness of the Secondary Logon service every launch goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeclogonState {
    Ok,
    Disabled,
    Missing,
    CannotRead,
}

impl SeclogonState {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Self::Ok),
            "disabled" => Some(Self::Disabled),
            "missing" => Some(Self::Missing),
            "cannot-read" => Some(Self::CannotRead),
            _ => None,
        }
    }
}
