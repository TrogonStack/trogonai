//! CPython 3.12's `str.__hash__` and the hash secret `PYTHONHASHSEED` picks for it, which set the iteration order of
//! every `set` and `frozenset` of strings.
//!
//! `unicode_hash` (`Objects/unicodeobject.c`) hashes the code points at the width CPython stores the string at, one
//! byte each when none is above U+00FF, two when none is above U+FFFF and four otherwise, little endian on the
//! platforms this crate runs on. `_Py_HashBytes` (`Python/pyhash.c`) hashes no bytes to 0 and the rest with
//! `siphash13` keyed by the first 16 bytes of `_Py_HashSecret`, turning -1 into -2.

use crate::adapters::{EnvironmentVariable, SystemEnvironment};
use crate::constants::MAX_HASH_SEED;
use std::{
    ffi::OsStr,
    hash::{BuildHasher as _, RandomState},
    sync::LazyLock,
};

use crate::{python_code_point::PythonCodePoint, python_string::PythonString};

/// The SipHash key of `_Py_HashSecret.siphash`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct HashSecret {
    k0: u64,
    k1: u64,
}

/// What `config_init_hash_seed` (`Python/initconfig.c`) makes of `PYTHONHASHSEED`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Seed {
    /// Unset, empty or `random`: a secret from `os.urandom`.
    Random,
    /// A seed in `[0; 4294967295]`.
    Fixed(u32),
    /// Anything else, which stops Python from starting.
    Invalid,
}

impl Seed {
    /// Reads a `PYTHONHASHSEED` value: `strtoul` in base 10 after optional white space and sign, which must use the
    /// whole text and stay within `MAX_HASH_SEED`.
    pub(crate) fn parse(text: Option<&str>) -> Self {
        let Some(text) = text.filter(|text| !text.is_empty()) else {
            return Self::Random;
        };
        if text == "random" {
            return Self::Random;
        }
        let unsigned = text.trim_start_matches([' ', '\t', '\n', '\u{b}', '\u{c}', '\r']);
        let (negative, digits) = match unsigned.as_bytes().first() {
            Some(b'-') => (true, &unsigned[1..]),
            Some(b'+') => (false, &unsigned[1..]),
            _ => (false, unsigned),
        };
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Self::Invalid;
        }
        let digits = digits.trim_start_matches('0');
        let value = if digits.is_empty() {
            Some(0)
        } else {
            digits.parse::<u64>().ok()
        };
        match value {
            // `strtoul` negates a negative value modulo 2**64, so only zero stays in range.
            Some(0) => Self::Fixed(0),
            Some(_) if negative => Self::Invalid,
            Some(value) if value <= MAX_HASH_SEED => u32::try_from(value).map_or(Self::Invalid, Self::Fixed),
            _ => Self::Invalid,
        }
    }
}

impl HashSecret {
    /// `_Py_HashRandomization_Init` for a fixed seed: all zeros for 0, otherwise the bytes `lcg_urandom`
    /// (`Python/bootstrap_hash.c`) draws from it.
    pub(crate) fn from_seed(seed: u32) -> Self {
        if seed == 0 {
            return Self::default();
        }
        let mut state = seed;
        let mut bytes = [0_u8; 16];
        for byte in &mut bytes {
            state = state.wrapping_mul(214_013).wrapping_add(2_531_011);
            *byte = state.to_le_bytes()[2];
        }
        let [a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p] = bytes;
        Self {
            k0: u64::from_le_bytes([a, b, c, d, e, f, g, h]),
            k1: u64::from_le_bytes([i, j, k, l, m, n, o, p]),
        }
    }

    /// A secret no other process shares, as `os.urandom` gives one.
    pub(crate) fn random() -> Self {
        let state = RandomState::new();
        Self {
            k0: state.hash_one(0_u8),
            k1: state.hash_one(1_u8),
        }
    }

    /// The secret a Python process started with `seed` uses. A seed Python refuses to start with is taken as random.
    pub(crate) fn of(seed: Seed) -> Self {
        match seed {
            Seed::Fixed(seed) => Self::from_seed(seed),
            Seed::Random | Seed::Invalid => Self::random(),
        }
    }

    /// The secret of this process, from its `PYTHONHASHSEED` as Python would read it at startup.
    pub(crate) fn process() -> Self {
        *PROCESS_SECRET
    }

    /// `hash(text)`.
    pub(crate) fn str_hash(self, text: &str) -> i64 {
        self.code_point_hash(text.chars().map(u32::from))
    }

    /// `hash` of the `str` of the code points `codes`. A lone surrogate widens the string as any code point up to
    /// U+FFFF does.
    fn code_point_hash(self, codes: impl Iterator<Item = u32> + Clone) -> i64 {
        let widest = codes.clone().max().unwrap_or(0);
        let mut bytes = Vec::new();
        for code in codes {
            let le = code.to_le_bytes();
            match widest {
                0..=0xff => bytes.push(le[0]),
                0x100..=0xffff => bytes.extend_from_slice(&le[..2]),
                _ => bytes.extend_from_slice(&le),
            }
        }
        if bytes.is_empty() {
            return 0;
        }
        fold(siphash13(self.k0, self.k1, &bytes))
    }
}

/// A `str` that `hash` reads.
pub(crate) trait PythonHashed {
    /// `hash(self)` under `secret`.
    fn python_hash(&self, secret: HashSecret) -> i64;
}

impl PythonHashed for str {
    fn python_hash(&self, secret: HashSecret) -> i64 {
        secret.str_hash(self)
    }
}

impl PythonHashed for PythonString {
    fn python_hash(&self, secret: HashSecret) -> i64 {
        secret.code_point_hash(self.code_points().map(PythonCodePoint::as_u32))
    }
}

/// The hash as the signed `Py_hash_t` it is, where -1 means an error, so a hash of -1 is -2.
fn fold(hash: u64) -> i64 {
    match i64::from_ne_bytes(hash.to_ne_bytes()) {
        -1 => -2,
        hash => hash,
    }
}

/// What Python makes of `PYTHONHASHSEED` set to `value`, or unset. A value that is not UTF-8 holds a byte no digit
/// is, so Python refuses to start with it.
fn env_seed(value: Option<&OsStr>) -> Seed {
    match value.map(OsStr::to_str) {
        None => Seed::Random,
        Some(None) => Seed::Invalid,
        Some(Some(text)) => Seed::parse(Some(text)),
    }
}

static PROCESS_SECRET: LazyLock<HashSecret> = LazyLock::new(|| {
    HashSecret::of(env_seed(
        SystemEnvironment.var_os(OsStr::new("PYTHONHASHSEED")).as_deref(),
    ))
});

/// `HALF_ROUND` of `Python/pyhash.c`.
fn half_round(a: &mut u64, b: &mut u64, c: &mut u64, d: &mut u64, s: u32, t: u32) {
    *a = a.wrapping_add(*b);
    *c = c.wrapping_add(*d);
    *b = b.rotate_left(s) ^ *a;
    *d = d.rotate_left(t) ^ *c;
    *a = a.rotate_left(32);
}

/// `SINGLE_ROUND` of `Python/pyhash.c`.
fn single_round(v: &mut [u64; 4]) {
    let [v0, v1, v2, v3] = v;
    half_round(v0, v1, v2, v3, 13, 16);
    half_round(v2, v1, v0, v3, 17, 21);
}

/// `siphash13` of `Python/pyhash.c`.
fn siphash13(k0: u64, k1: u64, src: &[u8]) -> u64 {
    let mut v = [
        k0 ^ 0x736f_6d65_7073_6575,
        k1 ^ 0x646f_7261_6e64_6f6d,
        k0 ^ 0x6c79_6765_6e65_7261,
        k1 ^ 0x7465_6462_7974_6573,
    ];
    let mut blocks = src.chunks_exact(8);
    for block in &mut blocks {
        let mut word = [0_u8; 8];
        word.copy_from_slice(block);
        let mi = u64::from_le_bytes(word);
        v[3] ^= mi;
        single_round(&mut v);
        v[0] ^= mi;
    }
    let mut tail = [0_u8; 8];
    let rest = blocks.remainder();
    tail[..rest.len()].copy_from_slice(rest);
    let b = ((src.len() as u64) << 56) | u64::from_le_bytes(tail);
    v[3] ^= b;
    single_round(&mut v);
    v[0] ^= b;
    v[2] ^= 0xff;
    for _ in 0..3 {
        single_round(&mut v);
    }
    v[0] ^ v[1] ^ v[2] ^ v[3]
}

#[cfg(test)]
#[path = "python_hash_tests.rs"]
mod tests;
