#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationMode {
    Bearer,
    HmacSha256,
}
