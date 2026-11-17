use super::*;
use std::thread::ThreadId;

/// A stack no system can map: larger than any user address space.
const UNGRANTABLE: usize = usize::MAX / 4 + 1;

/// Where the answer came from: the thread that ran `work`, or the inline fallback.
#[derive(Debug, PartialEq)]
enum Ran {
    Thread(ThreadId),
    Inline,
    Panicked,
}

fn run(sizes: &[usize]) -> Ran {
    on_large_stack(
        sizes,
        || Ran::Thread(std::thread::current().id()),
        || Ran::Inline,
        Ran::Panicked,
    )
}

#[test]
fn work_runs_on_a_thread_of_its_own() {
    let caller = std::thread::current().id();
    assert!(matches!(run(&[1 << 20]), Ran::Thread(id) if id != caller));
}

#[test]
fn a_stack_the_system_refuses_falls_through_to_the_next_size() {
    let caller = std::thread::current().id();
    assert!(matches!(run(&[UNGRANTABLE, 1 << 20]), Ran::Thread(id) if id != caller));
}

#[test]
fn where_no_stack_is_granted_the_work_answers_inline() {
    assert_eq!(run(&[]), Ran::Inline);
    assert_eq!(run(&[UNGRANTABLE]), Ran::Inline);
}

#[test]
fn a_thread_that_panics_answers_with_the_panicked_value() {
    let answer = on_large_stack(
        &[1 << 20],
        || -> Ran { std::panic::resume_unwind(Box::new("work failed")) },
        || Ran::Inline,
        Ran::Panicked,
    );
    assert_eq!(answer, Ran::Panicked);
}
