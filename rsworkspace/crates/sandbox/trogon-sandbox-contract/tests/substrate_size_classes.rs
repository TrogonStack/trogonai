use std::str::FromStr;

use serde_json::json;
use trogon_sandbox_contract::{SubstrateSizeClass, SubstrateSizeClassError, SubstrateSizeClasses};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_size_class_holds_a_label_and_memory() -> TestResult {
    let class = SubstrateSizeClass::new("small", 2048)?;
    assert_eq!(class.label(), "small");
    assert_eq!(class.memory_mib(), 2048);
    let wire = json!({ "label": "small", "memory_mib": 2048 });
    assert_eq!(serde_json::to_value(&class)?, wire);
    assert_eq!(serde_json::from_value::<SubstrateSizeClass>(wire)?, class);
    Ok(())
}

#[test]
fn a_size_class_rejects_an_empty_label_and_zero_memory() -> TestResult {
    let error = SubstrateSizeClass::new("", 1).err().ok_or("expected error")?;
    assert_eq!(error, SubstrateSizeClassError::EmptyLabel);
    assert_eq!(error.to_string(), "size class label must not be empty");

    let error = SubstrateSizeClass::new("tiny", 0).err().ok_or("expected error")?;
    assert_eq!(
        error,
        SubstrateSizeClassError::ZeroMemory {
            label: "tiny".to_owned()
        }
    );
    assert_eq!(error.to_string(), "size class `tiny` must declare non-zero memory");

    let error = serde_json::from_value::<SubstrateSizeClass>(json!({ "label": "", "memory_mib": 1 }))
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "size class label must not be empty");
    let error = serde_json::from_value::<SubstrateSizeClass>(json!({ "label": "tiny", "memory_mib": 0 }))
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "size class `tiny` must declare non-zero memory");
    Ok(())
}

#[test]
fn a_size_class_rejects_a_label_the_text_form_cannot_carry() -> TestResult {
    for label in ["small,backup", "small=big", " small", "small "] {
        let error = SubstrateSizeClass::new(label, 2048).err().ok_or("expected error")?;
        assert_eq!(
            error,
            SubstrateSizeClassError::TextIncompatibleLabel {
                label: label.to_owned()
            }
        );
        assert_eq!(
            error.to_string(),
            format!("size class label `{label}` must not contain `,` or `=` or surrounding whitespace")
        );

        let error = serde_json::from_value::<SubstrateSizeClass>(json!({ "label": label, "memory_mib": 2048 }))
            .err()
            .ok_or("expected error")?;
        assert_eq!(
            error.to_string(),
            format!("size class label `{label}` must not contain `,` or `=` or surrounding whitespace")
        );
    }
    Ok(())
}

#[test]
fn a_ladder_parses_sorts_and_renders() -> TestResult {
    let classes = SubstrateSizeClasses::from_str(" large=32768, small = 2048 ,,medium=8192")?;
    assert_eq!(classes.to_string(), "small=2048,medium=8192,large=32768");
    assert_eq!(classes.as_slice().len(), 3);
    assert_eq!(classes.as_slice()[0].label(), "small");
    assert_eq!(SubstrateSizeClasses::from_str(&classes.to_string())?, classes);

    let wire = json!([
        { "label": "small", "memory_mib": 2048 },
        { "label": "medium", "memory_mib": 8192 },
        { "label": "large", "memory_mib": 32768 },
    ]);
    assert_eq!(serde_json::to_value(&classes)?, wire);
    assert_eq!(serde_json::from_value::<SubstrateSizeClasses>(wire)?, classes);
    Ok(())
}

#[test]
fn every_ladder_round_trips_through_its_text_form() -> TestResult {
    let built = SubstrateSizeClasses::new(vec![
        SubstrateSizeClass::new("gpu-large", 65536)?,
        SubstrateSizeClass::new("small.v2", 2048)?,
    ])?;
    assert_eq!(built.to_string(), "small.v2=2048,gpu-large=65536");
    assert_eq!(SubstrateSizeClasses::from_str(&built.to_string())?, built);

    let decoded = serde_json::from_value::<SubstrateSizeClasses>(json!([
        { "label": "pool a", "memory_mib": 4096 },
        { "label": "pool:b", "memory_mib": 1024 },
    ]))?;
    assert_eq!(decoded.to_string(), "pool:b=1024,pool a=4096");
    assert_eq!(SubstrateSizeClasses::from_str(&decoded.to_string())?, decoded);
    Ok(())
}

#[test]
fn a_ladder_rejects_what_cannot_be_ordered() -> TestResult {
    let error = SubstrateSizeClasses::from_str("").err().ok_or("expected error")?;
    assert_eq!(error.to_string(), "at least one size class is required");
    let error = SubstrateSizeClasses::new(Vec::new()).err().ok_or("expected error")?;
    assert_eq!(error, SubstrateSizeClassError::Empty);

    let error = SubstrateSizeClasses::from_str("small=2048,tiny=2048")
        .err()
        .ok_or("expected error")?;
    assert_eq!(error, SubstrateSizeClassError::DuplicateMemory { memory_mib: 2048 });
    assert_eq!(error.to_string(), "two size classes declare 2048 MiB");

    let error = SubstrateSizeClasses::from_str("small").err().ok_or("expected error")?;
    assert_eq!(
        error,
        SubstrateSizeClassError::MalformedEntry {
            entry: "small".to_owned()
        }
    );
    assert_eq!(
        error.to_string(),
        "size class `small` is not in `label=memory_mib` form"
    );

    let error = SubstrateSizeClasses::from_str("small=big")
        .err()
        .ok_or("expected error")?;
    assert_eq!(
        error,
        SubstrateSizeClassError::MalformedMemory {
            entry: "small=big".to_owned(),
            source: "big".parse::<u32>().err().ok_or("expected parse error")?,
        }
    );
    assert_eq!(
        error.to_string(),
        "size class `small=big` has a non-numeric memory value"
    );
    assert!(std::error::Error::source(&error).is_some());

    let error = SubstrateSizeClasses::from_str("small=0")
        .err()
        .ok_or("expected error")?;
    assert_eq!(
        error,
        SubstrateSizeClassError::ZeroMemory {
            label: "small".to_owned()
        }
    );
    assert_eq!(error.to_string(), "size class `small` must declare non-zero memory");
    let error = SubstrateSizeClasses::from_str("=2048").err().ok_or("expected error")?;
    assert_eq!(error, SubstrateSizeClassError::EmptyLabel);

    let error = serde_json::from_value::<SubstrateSizeClasses>(json!([]))
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "at least one size class is required");
    let error = serde_json::from_value::<SubstrateSizeClasses>(json!([
        { "label": "a", "memory_mib": 1 },
        { "label": "b", "memory_mib": 1 },
    ]))
    .err()
    .ok_or("expected error")?;
    assert_eq!(error.to_string(), "two size classes declare 1 MiB");
    Ok(())
}
