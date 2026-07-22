use super::*;

#[test]
fn validates_ordered_disjoint_delegate_selectors() {
    let selectors = DelegateSelectors::new(
        BTreeSet::from(["family=reviewer".to_string()]),
        BTreeSet::from(["family=researcher".to_string()]),
    )
    .unwrap();
    assert!(selectors.required().contains("family=reviewer"));
    assert!(matches!(
        DelegateSelectors::new(
            BTreeSet::from(["family=reviewer".to_string()]),
            BTreeSet::from(["family=reviewer".to_string()])
        ),
        Err(DelegateSelectorsError::Overlap { .. })
    ));
}

#[test]
fn rejects_blank_required_and_optional_selectors() {
    assert!(matches!(
        DelegateSelectors::new(BTreeSet::from([" family=reviewer".to_string()]), BTreeSet::new()),
        Err(DelegateSelectorsError::InvalidRequired { .. })
    ));
    assert!(matches!(
        DelegateSelectors::new(BTreeSet::new(), BTreeSet::from([" family=researcher".to_string()])),
        Err(DelegateSelectorsError::InvalidOptional { .. })
    ));
}
