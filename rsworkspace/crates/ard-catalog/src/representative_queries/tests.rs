use super::*;

#[test]
fn accepts_two_to_five_queries() {
    let queries = RepresentativeQueries::new(vec!["one".to_owned(), "two".to_owned()]).unwrap();
    assert_eq!(queries.as_slice().len(), 2);

    let queries =
        RepresentativeQueries::new((1..=5).map(|index| format!("query-{index}")).collect::<Vec<_>>()).unwrap();
    assert_eq!(queries.as_slice().len(), 5);
}

#[test]
fn rejects_too_few() {
    assert_eq!(
        RepresentativeQueries::new(vec!["only-one".to_owned()]),
        Err(RepresentativeQueriesError::TooFew { count: 1 })
    );
}

#[test]
fn rejects_too_many() {
    assert_eq!(
        RepresentativeQueries::new((1..=6).map(|index| format!("query-{index}")).collect::<Vec<_>>(),),
        Err(RepresentativeQueriesError::TooMany { count: 6 })
    );
}

#[test]
fn rejects_empty_entry() {
    assert_eq!(
        RepresentativeQueries::new(vec!["valid".to_owned(), "   ".to_owned()]),
        Err(RepresentativeQueriesError::EmptyEntry)
    );
}
