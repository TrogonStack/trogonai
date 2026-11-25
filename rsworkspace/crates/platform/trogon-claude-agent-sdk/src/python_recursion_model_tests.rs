use super::*;

#[test]
fn the_default_is_a_warm_server_called_from_module_level() {
    let model = RecursionModel::default();

    assert_eq!(model.first_frame(), 18);
    assert_eq!(model.handler_frame(), 17);
    assert_eq!(model.cache(), CacheState::Warm);
    assert!(!model.exceeded(1000));
    assert!(model.exceeded(1001));
    assert_eq!(model.room(990), 10);
    assert_eq!(model.room(1200), 0);
}

#[test]
fn options_set_every_field() -> Result<(), RecursionModelError> {
    let model = RecursionModel::new(RecursionModelOptions {
        caller_depth: 5,
        cache: CacheState::Cold,
        limit: 50,
    })?;

    assert_eq!(model.first_frame(), 6);
    assert_eq!(model.handler_frame(), 5);
    assert_eq!(model.cache(), CacheState::Cold);
    assert!(!model.exceeded(50));
    assert!(model.exceeded(51));
    Ok(())
}

#[test]
fn a_caller_past_the_limit_is_rejected() {
    let built = RecursionModel::new(RecursionModelOptions {
        caller_depth: 51,
        cache: CacheState::Warm,
        limit: 50,
    });

    assert_eq!(
        built,
        Err(RecursionModelError::CallerPastLimit {
            caller_depth: 51,
            limit: 50
        })
    );
    assert_eq!(
        built.err().map(|error| error.to_string()),
        Some("a caller at depth 51 is already past the recursion limit 50".to_owned())
    );
}
