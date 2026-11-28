//! Lines the runtime says on its own behalf.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// What a notice means, which is what decides how it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeTone {
    /// Something happened that the person should know about.
    Information,
    /// Something the person asked for worked.
    Success,
    /// Something is off but the turn continues.
    Warning,
    /// Something failed.
    Error,
    /// Something stopped because it was asked to stop.
    Cancelled,
    /// Nothing in particular, so the line reads like an ordinary response.
    Neutral,
}

/// Status glyph leading every semantic notice, keyed by tone.
///
/// Tool activity owns "●"; notices deliberately use distinct git-style status
/// glyphs so the two channels never read as the same raw marker. The neutral
/// marker "*" avoids the middot, which the footer already uses as a separator.
#[must_use]
pub fn notice_glyph(tone: NoticeTone) -> &'static str {
    match tone {
        NoticeTone::Information => "i",
        NoticeTone::Success => "✓",
        NoticeTone::Warning => "!",
        NoticeTone::Error => "✗",
        NoticeTone::Cancelled => "⊘",
        NoticeTone::Neutral => "*",
    }
}

/// Where a notice is worth the space it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeVisibility {
    /// Shown however much of the transcript is on screen.
    #[default]
    CompactAndFull,
    /// Shown only when there is room for the whole transcript.
    FullOnly,
}

/// One thing the runtime has to say, before anyone has decided how it looks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticNotice {
    /// What the notice is about, such as the tool or subsystem it came from.
    pub topic: String,
    /// What it means.
    pub tone: NoticeTone,
    /// What it says.
    pub body: String,
    /// Where it is worth showing.
    pub visibility: NoticeVisibility,
}

impl SemanticNotice {
    /// Says something, everywhere the transcript is shown.
    #[must_use]
    pub fn new(topic: impl Into<String>, tone: NoticeTone, body: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            tone,
            body: body.into(),
            visibility: NoticeVisibility::default(),
        }
    }

    /// Says it only where there is room for the whole transcript.
    #[must_use]
    pub fn only_in_full(mut self) -> Self {
        self.visibility = NoticeVisibility::FullOnly;
        self
    }
}

/// The marker older sessions put on a line to mark it as a context notice.
const CONTEXT_MARKER: &str = "[context]";

/// Turns a context notice written as marked-up text into a notice body.
///
/// Sessions written before notices had a tone carry the marker inline on every
/// line. Stripping it per line rather than once at the front is what keeps a
/// multi-line notice from ending up half marked; a line that is nothing but the
/// marker becomes an empty line, because it was a spacer.
///
/// Borrows when there is no marker to remove, which is every notice written
/// since.
#[must_use]
pub fn context_notice_body(text: &str) -> Cow<'_, str> {
    if !text.contains(CONTEXT_MARKER) {
        return Cow::Borrowed(text);
    }

    let prefix = format!("{CONTEXT_MARKER} ");
    let mut body = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            body.push('\n');
        }
        if line == CONTEXT_MARKER {
            continue;
        }
        body.push_str(line.strip_prefix(&prefix).unwrap_or(line));
    }
    Cow::Owned(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_context_body_drops_the_legacy_marker_from_every_line() {
        assert_eq!(
            context_notice_body("[context] first\n[context] second\nalready semantic\n[context]\n"),
            "first\nsecond\nalready semantic\n\n",
        );
    }

    #[test]
    fn a_body_with_nothing_to_strip_is_not_copied() {
        let text = "already semantic\nacross two lines";
        assert!(matches!(context_notice_body(text), Cow::Borrowed(_)));
        assert_eq!(context_notice_body(text), text);
    }

    #[test]
    fn a_marker_that_is_not_a_prefix_is_left_where_it_is() {
        assert_eq!(
            context_notice_body("see [context] for the rest"),
            "see [context] for the rest",
        );
        assert_eq!(context_notice_body("[context]extra"), "[context]extra");
    }

    #[test]
    fn each_tone_gets_a_distinct_notice_glyph() {
        assert_eq!(notice_glyph(NoticeTone::Information), "i");
        assert_eq!(notice_glyph(NoticeTone::Success), "✓");
        assert_eq!(notice_glyph(NoticeTone::Warning), "!");
        assert_eq!(notice_glyph(NoticeTone::Error), "✗");
        assert_eq!(notice_glyph(NoticeTone::Cancelled), "⊘");
        assert_eq!(notice_glyph(NoticeTone::Neutral), "*");
    }

    #[test]
    fn a_notice_is_shown_everywhere_unless_it_says_otherwise() {
        let notice = SemanticNotice::new("compaction", NoticeTone::Information, "kept 12 turns");
        assert_eq!(notice.visibility, NoticeVisibility::CompactAndFull);
        assert_eq!(notice.topic, "compaction");
        assert_eq!(notice.body, "kept 12 turns");

        assert_eq!(notice.only_in_full().visibility, NoticeVisibility::FullOnly,);
    }

    #[test]
    fn a_notice_survives_a_round_trip_through_json() {
        let notice = SemanticNotice::new("permissions", NoticeTone::Cancelled, "denied").only_in_full();
        let encoded = serde_json::to_string(&notice).expect("a notice encodes");
        assert!(encoded.contains("\"cancelled\""));
        assert!(encoded.contains("\"full_only\""));
        assert_eq!(
            serde_json::from_str::<SemanticNotice>(&encoded).expect("a notice decodes"),
            notice,
        );
    }
}
