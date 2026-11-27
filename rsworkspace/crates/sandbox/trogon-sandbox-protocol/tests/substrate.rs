use trogon_sandbox_contract::SubstrateSizeClasses;
use trogon_sandbox_protocol::{
    DEFAULT_SUBSTRATE_SIZE_CLASSES, default_substrate_size_classes, max_substrate_memory_mib,
    select_substrate_size_class,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn label(classes: &SubstrateSizeClasses, memory_mib: u32) -> Option<&str> {
    select_substrate_size_class(classes, memory_mib).map(|class| class.label())
}

#[test]
fn the_default_ladder_matches_the_documented_flag_default() -> TestResult {
    assert_eq!(
        default_substrate_size_classes()?.to_string(),
        DEFAULT_SUBSTRATE_SIZE_CLASSES
    );
    assert_eq!(
        default_substrate_size_classes()?,
        DEFAULT_SUBSTRATE_SIZE_CLASSES.parse()?
    );
    Ok(())
}

#[test]
fn select_picks_the_smallest_class_that_fits() -> TestResult {
    let classes = default_substrate_size_classes()?;
    assert_eq!(label(&classes, 1), Some("small"));
    assert_eq!(label(&classes, 2048), Some("small"));
    assert_eq!(label(&classes, 2049), Some("medium"));
    assert_eq!(label(&classes, 32768), Some("large"));
    assert_eq!(label(&classes, 32769), None);
    Ok(())
}

#[test]
fn the_ceiling_is_the_largest_class() -> TestResult {
    assert_eq!(max_substrate_memory_mib(&default_substrate_size_classes()?), 32768);
    let one: SubstrateSizeClasses = "only=512".parse()?;
    assert_eq!(max_substrate_memory_mib(&one), 512);
    Ok(())
}
