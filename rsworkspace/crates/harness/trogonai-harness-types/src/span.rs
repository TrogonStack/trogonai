//! Half-open byte ranges over composer text, and what edits do to them.
//!
//! Every range here is half-open: `start` is inside it and `end` is the first
//! byte after it. That is what makes an insertion at `end` land outside the
//! entity rather than at its last byte.

/// A half-open range of bytes in the raw composer text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    start: usize,
    end: usize,
}

impl Span {
    /// A span from `start` up to but not including `end`.
    ///
    /// `None` when `end` precedes `start`. Nothing downstream wants a backwards range, so it
    /// does not exist.
    #[must_use]
    pub const fn new(start: usize, end: usize) -> Option<Self> {
        if end < start {
            return None;
        }
        Some(Self { start, end })
    }

    /// The empty span at the start of the text.
    #[must_use]
    pub const fn empty() -> Self {
        Self { start: 0, end: 0 }
    }

    /// First byte of the range.
    #[must_use]
    pub const fn start(self) -> usize {
        self.start
    }

    /// First byte after the range.
    #[must_use]
    pub const fn end(self) -> usize {
        self.end
    }

    /// How many bytes the range covers.
    #[must_use]
    pub const fn len(self) -> usize {
        self.end - self.start
    }

    /// Whether the range covers nothing.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Whether this marks actual text in an input of `input_len` bytes.
    ///
    /// An empty span marks no text, and a span reaching past the end marks text that is not
    /// there.
    #[must_use]
    pub const fn marks_text_in(self, input_len: usize) -> bool {
        !self.is_empty() && self.end <= input_len
    }

    /// Whether `offset` is strictly inside the range.
    ///
    /// Both edges are excluded, because the question this answers is where an
    /// edit lands: an edit at `start` or at `end` is beside the entity, and only
    /// one strictly between them is in it.
    #[must_use]
    pub const fn contains(self, offset: usize) -> bool {
        self.start < offset && offset < self.end
    }

    /// Whether the two ranges share at least one byte.
    #[must_use]
    pub const fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// Whether `inner` lies entirely within this range.
    #[must_use]
    pub const fn contains_span(self, inner: Self) -> bool {
        self.start <= inner.start && inner.end <= self.end
    }

    /// Where this range ends up after `byte_len` bytes are inserted at `at`.
    ///
    /// Insertion at `start` pushes the entity along; insertion at `end` leaves
    /// it alone. `None` means the insertion landed inside the entity, which
    /// destroys it rather than moving it, or that the text grew past what an
    /// offset can address.
    #[must_use]
    pub const fn after_insert(self, at: usize, byte_len: usize) -> Option<Self> {
        if byte_len == 0 || at >= self.end {
            return Some(self);
        }
        if at > self.start {
            return None;
        }
        match (self.start.checked_add(byte_len), self.end.checked_add(byte_len)) {
            (Some(start), Some(end)) => Some(Self { start, end }),
            _ => None,
        }
    }

    /// Where this range ends up after `deleted` is removed from the text.
    ///
    /// `None` means the deletion touched the entity, which destroys it. A
    /// deletion that covers nothing changes nothing.
    #[must_use]
    pub const fn after_delete(self, deleted: Self) -> Option<Self> {
        if deleted.is_empty() || deleted.start >= self.end {
            return Some(self);
        }
        if deleted.end <= self.start {
            let removed = deleted.len();
            return Some(Self {
                start: self.start - removed,
                end: self.end - removed,
            });
        }
        None
    }
}

/// Which image an attachment placeholder stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageId(usize);

impl ImageId {
    /// The id as the number written into the placeholder text.
    #[must_use]
    pub const fn new(id: usize) -> Self {
        Self(id)
    }

    /// The underlying number.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// An image placeholder in the composer text, and where it sits.
///
/// It is not a kind of span, it is a thing that has one, and the rename is what stops
/// `token.span.span`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageToken {
    /// The bytes the placeholder occupies.
    pub span: Span,
    /// The image it stands for.
    pub id: ImageId,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, end: usize) -> Span {
        Span::new(start, end).expect("a forward range")
    }

    #[test]
    fn entity_spans_shift_only_for_edits_outside_their_contents() {
        let entity = span(4, 10);

        assert_eq!(entity.after_insert(4, 2), Some(span(6, 12)));
        assert_eq!(entity.after_insert(10, 2), Some(entity));
        assert_eq!(entity.after_insert(7, 2), None);

        assert_eq!(entity.after_delete(span(1, 3)), Some(span(2, 8)));
        assert_eq!(entity.after_delete(span(10, 12)), Some(entity));
        assert_eq!(entity.after_delete(span(3, 5)), None);
        assert_eq!(entity.after_delete(span(5, 7)), None);
    }

    #[test]
    fn entity_span_relationship_helpers_use_half_open_ranges() {
        let entity = span(4, 10);

        assert!(entity.marks_text_in(10));
        assert!(!entity.marks_text_in(9));
        assert!(!entity.contains(4));
        assert!(entity.contains(5));
        assert!(!entity.contains(10));
        assert!(entity.overlaps(span(9, 12)));
        assert!(!entity.overlaps(span(10, 12)));
        assert!(entity.contains_span(span(5, 9)));
    }

    #[test]
    fn an_edit_that_moves_nothing_leaves_the_entity_exactly_where_it_was() {
        let entity = span(4, 10);

        assert_eq!(
            entity.after_insert(7, 0),
            Some(entity),
            "an insertion of nothing lands inside without disturbing anything"
        );
        assert_eq!(entity.after_delete(span(7, 7)), Some(entity));
        assert_eq!(entity.after_delete(Span::empty()), Some(entity));
    }

    #[test]
    fn an_entity_pushed_past_the_end_of_addressable_text_is_lost_rather_than_wrapped() {
        let entity = span(4, 10);
        assert_eq!(entity.after_insert(0, usize::MAX), None);
    }

    #[test]
    fn a_range_that_ends_before_it_starts_is_not_a_range() {
        assert_eq!(Span::new(10, 4), None);

        let touching = Span::new(4, 4).expect("a range may cover nothing");
        assert!(touching.is_empty());
        assert!(
            !touching.marks_text_in(usize::MAX),
            "it sits in the text without marking any of it"
        );
        assert_eq!(Span::empty(), Span::default());
    }

    #[test]
    fn a_placeholder_knows_which_image_it_stands_for_without_being_a_number() {
        let token = ImageToken {
            span: span(0, "[Image #1]".len()),
            id: ImageId::new(1),
        };
        assert_eq!(token.id.get(), 1);
        assert!(token.span.marks_text_in("[Image #1]".len()));
    }
}
