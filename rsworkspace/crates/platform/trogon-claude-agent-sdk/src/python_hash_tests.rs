use super::*;
use serde_json::Value;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt as _;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// `hash()` of strings and the orders of sets, recorded from CPython 3.12 under several `PYTHONHASHSEED` values, and
/// what Python made of each `PYTHONHASHSEED` text at startup.
const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_set_order.json");

fn list<'v>(value: &'v Value, name: &str) -> Result<&'v Vec<Value>, String> {
    value
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{name} is not a list"))
}

fn seed(record: &Value) -> Result<u32, Box<dyn std::error::Error>> {
    let seed = record
        .get("seed")
        .and_then(Value::as_u64)
        .ok_or("a seed is not a number")?;
    Ok(u32::try_from(seed)?)
}

#[test]
fn str_hash_is_the_hash_cpython_gives_under_every_recorded_seed() -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    let strings = list(&fixture, "strings")?;
    let seeds = list(&fixture, "seeds")?;
    assert_eq!(seeds.len(), 11);
    for record in seeds {
        let secret = HashSecret::from_seed(seed(record)?);
        let hashes = list(record, "hashes")?;
        assert_eq!(hashes.len(), strings.len());
        for (text, expected) in strings.iter().zip(hashes) {
            let text = text.as_str().ok_or("a string is not text")?;
            assert_eq!(
                Some(secret.str_hash(text)),
                expected.as_i64(),
                "seed {record:?} text {text:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn a_seed_is_read_like_python_reads_pythonhashseed_at_startup() -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    let startup = list(&fixture, "startup")?;
    assert_eq!(startup.len(), 24);
    for record in startup {
        let text = record.get("text").and_then(Value::as_str).ok_or("a text is not text")?;
        let parsed = Seed::parse(Some(text));
        match record.get("seed").and_then(Value::as_str) {
            Some("random") => assert_eq!(parsed, Seed::Random, "{text:?}"),
            Some("invalid") => assert_eq!(parsed, Seed::Invalid, "{text:?}"),
            Some("fixed") => {
                let Seed::Fixed(seed) = parsed else {
                    return Err(format!("{text:?} gave {parsed:?}, Python a fixed seed").into());
                };
                let hash = HashSecret::from_seed(seed).str_hash("abc");
                assert_eq!(Some(hash), record.get("hash").and_then(Value::as_i64), "{text:?}");
            }
            other => return Err(format!("{text:?} has the outcome {other:?}").into()),
        }
    }
    assert_eq!(Seed::parse(None), Seed::Random);
    Ok(())
}

#[test]
fn the_environment_value_is_a_seed_or_invalid_when_it_is_not_utf_8() {
    assert_eq!(env_seed(None), Seed::Random);
    assert_eq!(env_seed(Some(std::ffi::OsStr::new("42"))), Seed::Fixed(42));
    assert_eq!(env_seed(Some(std::ffi::OsStr::new("x"))), Seed::Invalid);
    #[cfg(unix)]
    assert_eq!(env_seed(Some(std::ffi::OsStr::from_bytes(b"4\xff"))), Seed::Invalid);
}

#[test]
fn a_fixed_seed_gives_its_secret_and_any_other_a_fresh_random_one() {
    assert_eq!(HashSecret::of(Seed::Fixed(42)), HashSecret::from_seed(42));
    assert_eq!(HashSecret::of(Seed::Fixed(0)), HashSecret::default());
    assert_ne!(HashSecret::of(Seed::Random), HashSecret::of(Seed::Random));
    assert_ne!(HashSecret::of(Seed::Invalid), HashSecret::of(Seed::Invalid));
    assert_ne!(HashSecret::random(), HashSecret::from_seed(42));
}

#[test]
fn the_process_secret_is_read_once_from_pythonhashseed() {
    let secret = HashSecret::process();
    assert_eq!(HashSecret::process(), secret);
    if let Seed::Fixed(seed) = env_seed(SystemEnvironment.var_os(OsStr::new("PYTHONHASHSEED")).as_deref()) {
        assert_eq!(secret, HashSecret::from_seed(seed));
    }
}

#[test]
fn a_hash_of_minus_one_is_minus_two() {
    assert_eq!(fold(u64::MAX), -2);
    assert_eq!(fold(u64::MAX - 1), -2);
    assert_eq!(fold(5), 5);
    assert_eq!(fold(1 << 63), i64::MIN);
}
