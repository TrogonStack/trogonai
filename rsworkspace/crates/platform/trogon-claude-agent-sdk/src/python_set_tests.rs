use super::*;
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// `list(set(keys))` of several key lists, and every `step`-th key of a set of `count` keys, recorded from CPython
/// 3.12 under several `PYTHONHASHSEED` values.
const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_set_order.json");

fn list<'v>(value: &'v Value, name: &str) -> Result<&'v Vec<Value>, String> {
    value
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{name} is not a list"))
}

fn texts(value: &Value) -> Result<Vec<&str>, String> {
    value
        .as_array()
        .ok_or("keys are not a list")?
        .iter()
        .map(|key| key.as_str().ok_or_else(|| "a key is not text".to_owned()))
        .collect()
}

fn secret(record: &Value) -> Result<HashSecret, Box<dyn std::error::Error>> {
    let seed = record
        .get("seed")
        .and_then(Value::as_u64)
        .ok_or("a seed is not a number")?;
    Ok(HashSecret::from_seed(u32::try_from(seed)?))
}

fn number(value: &Value, name: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let number = value.get(name).and_then(Value::as_u64).ok_or("not a number")?;
    Ok(usize::try_from(number)?)
}

#[test]
fn a_set_iterates_its_keys_in_the_order_cpython_does_under_every_recorded_seed() -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    let sets = list(&fixture, "sets")?;
    let seeds = list(&fixture, "seeds")?;
    assert_eq!(seeds.len(), 11);
    for record in seeds {
        let secret = secret(record)?;
        let orders = list(record, "sets")?;
        assert_eq!(orders.len(), sets.len());
        for (keys, expected) in sets.iter().zip(orders) {
            let keys = texts(keys)?;
            let actual: Vec<&str> = set_order(&keys, secret).into_iter().map(|at| keys[at]).collect();
            assert_eq!(actual, texts(expected)?, "seed {:?}", record.get("seed"));
        }
    }
    Ok(())
}

/// The large set grows past the 50000 keys above which a resize only doubles them: from 131072 slots at 78643 keys,
/// to 262144, then at 157286 keys to 524288.
#[test]
fn a_large_set_resizes_like_cpython_past_fifty_thousand_keys() -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    let large = fixture.get("large").ok_or("the fixture has no large set")?;
    let (count, step) = (number(large, "count")?, number(large, "step")?);
    assert!(count > 157_286);
    let names: Vec<String> = (0..count).map(|at| format!("n{at}")).collect();
    let keys: Vec<&str> = names.iter().map(String::as_str).collect();
    for record in list(&fixture, "seeds")? {
        let actual: Vec<Value> = set_order(&keys, secret(record)?)
            .into_iter()
            .step_by(step)
            .map(Value::from)
            .collect();
        assert_eq!(&actual, list(record, "large")?, "seed {:?}", record.get("seed"));
    }
    Ok(())
}

#[test]
fn a_set_keeps_the_first_of_equal_keys() {
    assert_eq!(set_order(&["a", "b", "a", "c", "b"], HashSecret::default()).len(), 3);
    let order = set_order(&["a", "a"], HashSecret::default());
    assert_eq!(order, vec![0]);
}

#[test]
fn a_display_of_fewer_than_three_items_adds_them_and_a_longer_one_merges_its_constant() {
    let secret = HashSecret::from_seed(42);
    for keys in [&["x", "y"][..], &["x"], &[]] {
        assert_eq!(display_order(keys, secret), set_order(keys, secret));
    }
    // Three or four keys merge into a table of the same size, which is a copy; five or more merge into a smaller one.
    for keys in [&["a", "b", "c"][..], &["a", "b", "c", "d"]] {
        assert_eq!(display_order(keys, secret), set_order(keys, secret));
    }
    let keys = ["a", "b", "c", "d", "e"];
    assert_eq!(added(&keys, secret).slots.len(), 32);
    let merged = added(&keys, secret).merged();
    assert_eq!((merged.slots.len(), merged.fill, merged.used), (16, 5, 5));
    let mut sorted = display_order(&keys, secret);
    sorted.sort_unstable();
    assert_eq!(sorted, vec![0, 1, 2, 3, 4]);
}

#[test]
fn merging_an_empty_table_or_resizing_a_clean_minimal_one_keeps_it() {
    let merged = Table::default().merged();
    assert_eq!((merged.slots.len(), merged.fill, merged.used), (MIN_SIZE, 0, 0));
    let mut table = Table::default();
    table.resize(0);
    assert_eq!(table.slots.len(), MIN_SIZE);
    table.resize(MIN_SIZE);
    assert_eq!(table.slots.len(), 2 * MIN_SIZE);
}
