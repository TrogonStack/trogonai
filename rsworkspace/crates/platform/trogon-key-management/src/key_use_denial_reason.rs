#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyUseDenialReason {
    ProviderDenied,
    BackendPaused,
    KeyState,
    BackendNotActive,
}
