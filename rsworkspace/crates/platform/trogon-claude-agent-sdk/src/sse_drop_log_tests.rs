use super::*;

fn record(log: &mut SseDropLog, category: SseDropCategory) -> Option<String> {
    log.record(category, SseLogLevel::Debug, "dropped".to_owned())
        .map(|record| record.text)
}

#[test]
fn the_first_five_are_logged_then_every_fiftieth() {
    let mut log = SseDropLog::default();
    let logged: Vec<(u64, String)> = (1..=100)
        .filter_map(|count| record(&mut log, SseDropCategory::MissingType).map(|text| (count, text)))
        .collect();
    let counts: Vec<u64> = logged.iter().map(|(count, _)| *count).collect();
    assert_eq!(counts, [1, 2, 3, 4, 5, 50, 100]);
    assert_eq!(logged[5].1, "dropped (50 total \u{2014} sampling 1/50)");
}

#[test]
fn counts_keep_first_occurrence_order() {
    let mut log = SseDropLog::default();
    record(&mut log, SseDropCategory::ForgedReply);
    record(&mut log, SseDropCategory::MissingType);
    record(&mut log, SseDropCategory::ForgedReply);
    assert_eq!(
        log.counts(),
        [(SseDropCategory::ForgedReply, 2), (SseDropCategory::MissingType, 1)]
    );
}

#[test]
fn the_record_carries_its_level() {
    let mut log = SseDropLog::default();
    let line = log.record(SseDropCategory::ForgedReply, SseLogLevel::Warn, "x".to_owned());
    assert_eq!(
        line,
        Some(SseLogRecord {
            level: SseLogLevel::Warn,
            text: "x".to_owned()
        })
    );
}
