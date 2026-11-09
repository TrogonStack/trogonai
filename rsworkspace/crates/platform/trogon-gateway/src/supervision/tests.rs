use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::*;

fn backoff() -> RestartBackoff {
    RestartBackoff::new(Duration::from_secs(1), Duration::from_secs(4))
}

#[tokio::test(start_paused = true)]
async fn a_panicking_task_is_restarted_with_doubling_backoff_until_it_finishes() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(Mutex::new(Vec::new()));
    let origin = Instant::now();

    let output = supervise("test", backoff(), || {
        let attempts = attempts.clone();
        let starts = starts.clone();
        async move {
            starts.lock().unwrap().push(origin.elapsed());
            if attempts.fetch_add(1, Ordering::SeqCst) < 3 {
                panic!("boom");
            }
            "finished"
        }
    })
    .await;

    assert_eq!(output, "finished");
    assert_eq!(
        *starts.lock().unwrap(),
        vec![
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_secs(3),
            Duration::from_secs(7),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_task_that_returns_an_error_is_not_restarted() {
    let attempts = Arc::new(AtomicUsize::new(0));

    let output: Result<(), &str> = supervise("test", backoff(), || {
        let attempts = attempts.clone();
        async move {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err("permanent")
        }
    })
    .await;

    assert_eq!(output, Err("permanent"));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn backoff_resets_after_a_long_run() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(Mutex::new(Vec::new()));
    let origin = Instant::now();

    supervise("test", backoff(), || {
        let attempts = attempts.clone();
        let starts = starts.clone();
        async move {
            starts.lock().unwrap().push(origin.elapsed());
            match attempts.fetch_add(1, Ordering::SeqCst) {
                0 | 1 => panic!("early"),
                2 => {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    panic!("after a long run");
                }
                _ => {}
            }
        }
    })
    .await;

    let starts = starts.lock().unwrap();
    assert_eq!(starts[1] - starts[0], Duration::from_secs(1));
    assert_eq!(starts[2] - starts[1], Duration::from_secs(2));
    assert_eq!(starts[3] - starts[2], Duration::from_secs(10) + Duration::from_secs(1));
}

#[test]
fn panic_messages_are_read_from_str_and_string_payloads() {
    assert_eq!(panic_message(&"literal"), "literal");
    assert_eq!(panic_message(&String::from("owned")), "owned");
    assert_eq!(panic_message(&42_u32), "non-string panic payload");
}
