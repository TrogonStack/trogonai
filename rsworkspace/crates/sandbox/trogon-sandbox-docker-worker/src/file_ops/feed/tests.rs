use super::*;
use crate::support_tests::TestResult;

fn hint(path: &str, kind: FileWatchEventKind, target: FileWatchTarget) -> Option<FeedLine> {
    Some(FeedLine::Hint(ChangeHint {
        path: path.into(),
        kind,
        target,
    }))
}

fn file_hint(path: &str, kind: FileWatchEventKind) -> ChangeHint {
    ChangeHint {
        path: path.into(),
        kind,
        target: FileWatchTarget::File,
    }
}

#[test]
fn parses_the_pid_and_each_event_shape() {
    assert_eq!(parse_line("pid 42"), Some(FeedLine::Pid(42)));
    assert_eq!(
        parse_line("CREATE|/w/a"),
        hint("/w/a", FileWatchEventKind::Created, FileWatchTarget::File)
    );
    assert_eq!(
        parse_line("CREATE,ISDIR|/w/d"),
        hint("/w/d", FileWatchEventKind::Created, FileWatchTarget::Directory)
    );
    assert_eq!(
        parse_line("MOVED_TO|/w/b"),
        hint("/w/b", FileWatchEventKind::Created, FileWatchTarget::File)
    );
    assert_eq!(
        parse_line("DELETE|/w/a"),
        hint("/w/a", FileWatchEventKind::Deleted, FileWatchTarget::File)
    );
    assert_eq!(
        parse_line("MOVED_FROM|/w/a"),
        hint("/w/a", FileWatchEventKind::Deleted, FileWatchTarget::File)
    );
    assert_eq!(
        parse_line("CLOSE_WRITE,CLOSE|/w/a"),
        hint("/w/a", FileWatchEventKind::Modified, FileWatchTarget::File)
    );
    assert_eq!(
        parse_line("ATTRIB|/w//a"),
        hint("/w/a", FileWatchEventKind::Modified, FileWatchTarget::File)
    );
}

#[test]
fn a_name_with_a_bar_keeps_it_and_noise_is_dropped() {
    assert_eq!(
        parse_line("MODIFY|/w/a|b"),
        hint("/w/a|b", FileWatchEventKind::Modified, FileWatchTarget::File)
    );
    assert_eq!(parse_line("Watches established."), None);
    assert_eq!(parse_line("Q_OVERFLOW|"), None);
    assert_eq!(parse_line("MODIFY|relative"), None);
}

#[tokio::test(start_paused = true)]
async fn a_parked_wait_wakes_on_a_hint_and_hands_it_over_once() -> TestResult {
    let feed = Arc::new(Feed::new("l1"));
    let waiter = {
        let feed = Arc::clone(&feed);
        tokio::spawn(async move { feed.wait(Duration::from_secs(30)).await })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    feed.accept(FeedLine::Hint(file_hint("/w/a", FileWatchEventKind::Created)));
    tokio::time::timeout(Duration::from_secs(5), waiter).await??;
    assert_eq!(feed.drain().len(), 1);
    assert!(feed.drain().is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_wait_returns_at_once_when_hints_are_pending_or_the_feed_ended() -> TestResult {
    let feed = Feed::new("l1");
    feed.accept(FeedLine::Hint(file_hint("/w/a", FileWatchEventKind::Modified)));
    tokio::time::timeout(Duration::from_secs(5), feed.wait(Duration::from_secs(30))).await?;
    feed.drain();
    feed.end();
    assert!(!feed.is_live());
    tokio::time::timeout(Duration::from_secs(5), feed.wait(Duration::from_secs(30))).await?;
    Ok(())
}

#[test]
fn forgetting_a_lease_drops_only_its_feeds() {
    let feeds = WatchFeeds::default();
    feeds.feeds().insert("w1".into(), Arc::new(Feed::new("l1")));
    feeds.feeds().insert("w2".into(), Arc::new(Feed::new("l2")));
    feeds.forget("l1");
    assert!(feeds.get("w1").is_none());
    assert!(feeds.get("w2").is_some());
}
