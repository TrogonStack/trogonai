#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestKeyState {
    Active,
    DecryptOnly,
    Disabled,
    PendingDestruction,
    Destroyed,
}
