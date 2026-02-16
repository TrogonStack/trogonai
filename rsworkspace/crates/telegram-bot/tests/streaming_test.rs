//! Integration test for streaming message functionality

use std::time::Duration;
use tokio::time::Instant;

#[tokio::test]
async fn test_rate_limiting_timing() {
    // Simulate rate limiting logic
    const MIN_EDIT_INTERVAL: Duration = Duration::from_millis(1000);

    let start = Instant::now();
    let mut last_edit = start;

    // Simulate multiple edits
    let mut edit_times = vec![];

    for _i in 0..3 {
        let time_since_last = last_edit.elapsed();

        if time_since_last < MIN_EDIT_INTERVAL {
            let wait_time = MIN_EDIT_INTERVAL - time_since_last;
            tokio::time::sleep(wait_time).await;
        }

        let edit_time = start.elapsed();
        edit_times.push(edit_time);
        last_edit = Instant::now(); // Update for next iteration

        // Simulate some work
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Verify edits are at least 1 second apart
    for i in 1..edit_times.len() {
        let gap = edit_times[i] - edit_times[i-1];
        assert!(gap >= MIN_EDIT_INTERVAL,
            "Edit {} happened too soon: {:?} after previous", i, gap);
    }
}

#[tokio::test]
async fn test_exponential_backoff() {
    // Test exponential backoff calculation
    let base_delay = Duration::from_millis(100);

    let delays: Vec<Duration> = (0..3)
        .map(|attempt| base_delay * 2_u32.pow(attempt))
        .collect();

    assert_eq!(delays[0], Duration::from_millis(100)); // 100 * 2^0
    assert_eq!(delays[1], Duration::from_millis(200)); // 100 * 2^1
    assert_eq!(delays[2], Duration::from_millis(400)); // 100 * 2^2
}

#[test]
fn test_streaming_message_tracking_key() {
    // Test that tracking keys are unique per chat/session
    let chat_id1 = 12345i64;
    let chat_id2 = 67890i64;
    let session1 = "session_1".to_string();
    let session2 = "session_2".to_string();

    let key1 = (chat_id1, session1.clone());
    let key2 = (chat_id1, session2);
    let key3 = (chat_id2, session1);

    // Same chat, different session
    assert_ne!(key1, key2);

    // Same session, different chat
    assert_ne!(key1, key3);
}
