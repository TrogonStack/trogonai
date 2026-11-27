pub(super) const CA_LIFETIME_DAYS: i64 = 825;

pub(super) const LEAF_LIFETIME_DAYS: i64 = 99;

pub(super) const BACKDATE_DAYS: i64 = 1;

pub(super) const UNREACHABLE_SUBTREE: &str = "invalid";

pub const LEAF_CACHE_LIMIT: usize = 256;

/// The origin-form path the proxy answers with the empty revocation list.
pub const CRL_PATH: &str = "/ca.crl";

pub(super) const DEFAULT_GENERATED_CA_CN: &str = "sandbox ephemeral CA";

pub(super) const DEFAULT_GENERATED_CA_VALIDITY_DAYS: i64 = 825;

pub(super) const RSA_ENCRYPTION_OID: &str = "1.2.840.113549.1.1.1";

pub(super) const EC_PUBLIC_KEY_OID: &str = "1.2.840.10045.2.1";

pub(super) const RSA_KEY_BITS: usize = 2048;

pub(super) const OID_SHA256_WITH_RSA: &[u8] = &[0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];

pub(super) const OID_ECDSA_WITH_SHA256: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];

pub(super) const OID_AUTHORITY_KEY_IDENTIFIER: &[u8] = &[0x06, 0x03, 0x55, 0x1d, 0x23];

pub(super) const OID_CRL_NUMBER: &[u8] = &[0x06, 0x03, 0x55, 0x1d, 0x14];

pub(super) const DER_SEQUENCE: u8 = 0x30;

pub(super) const DER_INTEGER: u8 = 0x02;

pub(super) const DER_OCTET_STRING: u8 = 0x04;

pub(super) const DER_BIT_STRING: u8 = 0x03;

pub(super) const DER_NULL: &[u8] = &[0x05, 0x00];

pub(super) const DER_UTC_TIME: u8 = 0x17;

pub(super) const DER_GENERALIZED_TIME: u8 = 0x18;

pub(super) const DER_EXTENSIONS: u8 = 0xa0;

pub(super) const DER_KEY_IDENTIFIER: u8 = 0x80;

pub(super) const UTC_TIME_LAST_YEAR: i32 = 2049;
