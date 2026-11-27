/// HTTP status code carried by a refusal response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusCode(u16);

impl StatusCode {
    pub const BAD_REQUEST: Self = Self(400);
    pub const FORBIDDEN: Self = Self(403);
    pub const METHOD_NOT_ALLOWED: Self = Self(405);
    pub const PAYLOAD_TOO_LARGE: Self = Self(413);
    pub const MISDIRECTED_REQUEST: Self = Self(421);
    pub const HEADER_FIELDS_TOO_LARGE: Self = Self(431);
    pub const BAD_GATEWAY: Self = Self(502);

    pub fn as_u16(self) -> u16 {
        self.0
    }
}
