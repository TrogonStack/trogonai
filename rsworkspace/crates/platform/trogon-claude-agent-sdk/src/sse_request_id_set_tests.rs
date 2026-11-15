use super::*;

fn overflow(name: &str) -> SseLogRecord {
    SseLogRecord {
        level: SseLogLevel::Warn,
        text: format!("[BrowserSSETransport] {name} overflow \u{2014} evicting oldest unanswered request_id"),
    }
}

#[test]
fn evicts_the_oldest_past_the_cap_and_warns() {
    let shadow = SseRequestIdSet::new("other", 10);
    let mut set = SseRequestIdSet::new("mine", 2);
    assert_eq!(set.insert("a", &shadow), None);
    assert_eq!(set.insert("b", &shadow), None);
    assert_eq!(set.insert("a", &shadow), None);
    assert_eq!(set.insert("c", &shadow), Some(overflow("mine")));
    assert_eq!(set.iter().collect::<Vec<_>>(), ["b", "c"]);
    assert!(!set.contains("a"));
}

#[test]
fn an_id_known_to_the_shadow_set_is_evicted_quietly() {
    let mut shadow = SseRequestIdSet::new("other", 10);
    shadow.insert("a", &SseRequestIdSet::new("none", 1));
    let mut set = SseRequestIdSet::new("mine", 1);
    set.insert("a", &shadow);
    assert_eq!(set.insert("b", &shadow), None);
}

#[test]
fn removes_ids() {
    let shadow = SseRequestIdSet::new("other", 10);
    let mut set = SseRequestIdSet::new("mine", 3);
    set.insert("a", &shadow);
    set.remove("a");
    set.remove("missing");
    assert!(!set.contains("a"));
}
