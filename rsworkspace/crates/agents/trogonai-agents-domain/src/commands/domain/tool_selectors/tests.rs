use super::*;

#[test]
fn validates_ordered_disjoint_tool_selectors() {
    let selectors = ToolSelectors::new(
        BTreeSet::from(["bash".to_string()]),
        BTreeSet::from(["web_search".to_string()]),
    )
    .unwrap();
    assert!(selectors.required().contains("bash"));
    assert!(matches!(
        ToolSelectors::new(
            BTreeSet::from(["bash".to_string()]),
            BTreeSet::from(["bash".to_string()])
        ),
        Err(ToolSelectorsError::Overlap { .. })
    ));
}

#[test]
fn rejects_blank_required_and_optional_selectors() {
    assert!(matches!(
        ToolSelectors::new(BTreeSet::from([" bash".to_string()]), BTreeSet::new()),
        Err(ToolSelectorsError::InvalidRequired { .. })
    ));
    assert!(matches!(
        ToolSelectors::new(BTreeSet::new(), BTreeSet::from([" web_search".to_string()])),
        Err(ToolSelectorsError::InvalidOptional { .. })
    ));
}
