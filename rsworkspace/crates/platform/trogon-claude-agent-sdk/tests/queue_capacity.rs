use trogon_claude_agent_sdk::{QueueCapacity, QueueCapacityError};

#[test]
fn queue_capacities_preserve_supported_values_and_explicit_defaults() -> Result<(), QueueCapacityError> {
    for count in [1, 32, QueueCapacity::MAX] {
        let value = QueueCapacity::new(count)?;
        assert_eq!(value.get(), count);
        assert_eq!(QueueCapacity::try_from(count)?, value);
    }
    assert_eq!(QueueCapacity::default().get(), 32);
    assert_eq!(QueueCapacity::PYTHON_MESSAGES.get(), 100);
    Ok(())
}

#[test]
fn unsupported_queue_capacities_fail_before_any_channel_is_created() {
    assert!(matches!(QueueCapacity::new(0), Err(QueueCapacityError::Zero)));
    assert!(matches!(QueueCapacity::try_from(0), Err(QueueCapacityError::Zero)));
    assert!(matches!(
        QueueCapacity::new(usize::MAX),
        Err(QueueCapacityError::TooLarge)
    ));
}
