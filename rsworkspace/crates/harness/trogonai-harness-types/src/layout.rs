//! Where a terminal frame's rows sit.

use core::num::NonZeroU16;

use thiserror::Error;

/// A terminal too short to hold a frame.
///
/// Mirrors `error.TerminalTooSmall`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[error("TerminalTooSmall")]
pub struct TerminalTooSmall;

/// How big a terminal is, as a terminal that exists reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowSize {
    rows: NonZeroU16,
    cols: NonZeroU16,
}

impl WindowSize {
    /// A terminal of this size, or `None` for a terminal reporting no rows or
    /// no columns.
    #[must_use]
    pub const fn new(rows: u16, cols: u16) -> Option<Self> {
        match (NonZeroU16::new(rows), NonZeroU16::new(cols)) {
            (Some(rows), Some(cols)) => Some(Self { rows, cols }),
            _ => None,
        }
    }

    /// How many rows the terminal has.
    #[must_use]
    pub const fn rows(self) -> u16 {
        self.rows.get()
    }

    /// How many columns the terminal has.
    #[must_use]
    pub const fn cols(self) -> u16 {
        self.cols.get()
    }

    /// The frame this terminal holds, with `footer_rows` kept for the footer.
    ///
    /// # Errors
    ///
    /// Returns [`TerminalTooSmall`] for a terminal too short to hold the
    /// frame; see [`Layout::from_size`].
    pub const fn layout(self, footer_rows: u16) -> Result<Layout, TerminalTooSmall> {
        Layout::from_size(self.rows(), self.cols(), footer_rows)
    }
}

/// Which row of a terminal each part of a frame is drawn on.
///
/// Rows count from one, the way a terminal addresses them, so `hint_row` is
/// `rows` and not `rows - 1`. Construct one with [`Layout::from_size`]: the
/// derived rows only agree with each other for a terminal tall enough to hold
/// them, and that is the check `from_size` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Layout {
    rows: u16,
    cols: u16,
    content_bottom: u16,
    divider_top_row: u16,
    input_row: u16,
    divider_bottom_row: u16,
    hint_row: u16,
}

impl Layout {
    /// The rows a frame reserves for its own dividers, input line and hint.
    ///
    /// Named here because [`Layout::from_size`] has to test it (see the doc comment there).
    pub const FRAME_ROWS: u16 = 3;

    /// The frame a terminal of this size holds, with `footer_rows` kept for
    /// the footer.
    ///
    ///
    /// So the refusal is the taller of the two bounds.
    pub const fn from_size(rows: u16, cols: u16, footer_rows: u16) -> Result<Self, TerminalTooSmall> {
        if rows <= footer_rows || rows < Self::FRAME_ROWS {
            return Err(TerminalTooSmall);
        }
        Ok(Self {
            rows,
            cols,
            content_bottom: rows - footer_rows,
            divider_top_row: rows - 3,
            input_row: rows - 2,
            divider_bottom_row: rows - 1,
            hint_row: rows,
        })
    }

    /// Builds a layout from already-solved row geometry.
    ///
    /// Renderers use this after independently solving footer placement. The
    /// caller is responsible for keeping every row within `rows`.
    #[must_use]
    pub const fn from_geometry(
        rows: u16,
        cols: u16,
        content_bottom: u16,
        divider_top_row: u16,
        input_row: u16,
        divider_bottom_row: u16,
        hint_row: u16,
    ) -> Self {
        Self {
            rows,
            cols,
            content_bottom,
            divider_top_row,
            input_row,
            divider_bottom_row,
            hint_row,
        }
    }

    /// How many rows the terminal has.
    #[must_use]
    pub const fn rows(self) -> u16 {
        self.rows
    }

    /// How many columns the terminal has.
    #[must_use]
    pub const fn cols(self) -> u16 {
        self.cols
    }

    /// The last row the transcript may draw on.
    #[must_use]
    pub const fn content_bottom(self) -> u16 {
        self.content_bottom
    }

    /// The divider above the input line.
    #[must_use]
    pub const fn divider_top_row(self) -> u16 {
        self.divider_top_row
    }

    /// The row the composer draws on.
    #[must_use]
    pub const fn input_row(self) -> u16 {
        self.input_row
    }

    /// The divider below the input line.
    #[must_use]
    pub const fn divider_bottom_row(self) -> u16 {
        self.divider_bottom_row
    }

    /// The last row of the terminal, where the hint draws.
    #[must_use]
    pub const fn hint_row(self) -> u16 {
        self.hint_row
    }
}

/// What a frame cost to draw.
///
/// They start at zero and only ever climb, so a saturating add is the whole contract: a frame
/// counter that wrapped would read as a frame nobody drew.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Metrics {
    ansi_bytes: usize,
    full_redraws: usize,
    debounced_resizes: usize,
}

impl Metrics {
    /// Counts `bytes` more of escape sequences written.
    pub const fn add_ansi_bytes(&mut self, bytes: usize) {
        self.ansi_bytes = self.ansi_bytes.saturating_add(bytes);
    }

    /// Counts one more repaint of the whole screen.
    pub const fn add_full_redraw(&mut self) {
        self.full_redraws = self.full_redraws.saturating_add(1);
    }

    /// Counts one more resize geometry application admitted by the debounce lifecycle.
    pub const fn add_debounced_resize(&mut self) {
        self.debounced_resizes = self.debounced_resizes.saturating_add(1);
    }

    /// How many bytes of escape sequences have been written.
    #[must_use]
    pub const fn ansi_bytes(self) -> usize {
        self.ansi_bytes
    }

    /// How many times the whole screen has been repainted.
    #[must_use]
    pub const fn full_redraws(self) -> usize {
        self.full_redraws
    }

    /// How many resize geometry applications the debounce lifecycle admitted.
    #[must_use]
    pub const fn debounced_resizes(self) -> usize {
        self.debounced_resizes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_measures_its_footer_up_from_the_bottom_row() {
        let layout = Layout::from_size(24, 80, 4).expect("24 rows hold a 4-row footer");
        assert_eq!(layout.rows(), 24);
        assert_eq!(layout.cols(), 80);
        assert_eq!(layout.content_bottom(), 20);
        assert_eq!(layout.divider_top_row(), 21);
        assert_eq!(layout.input_row(), 22);
        assert_eq!(layout.divider_bottom_row(), 23);
        assert_eq!(layout.hint_row(), 24);
    }

    #[test]
    fn a_terminal_with_no_room_above_its_footer_holds_no_frame() {
        assert_eq!(Layout::from_size(4, 80, 4), Err(TerminalTooSmall));
        assert_eq!(Layout::from_size(3, 80, 4), Err(TerminalTooSmall));
    }

    #[test]
    fn a_terminal_too_short_for_the_frame_itself_is_refused_even_with_no_footer() {
        assert_eq!(Layout::from_size(1, 80, 0), Err(TerminalTooSmall));
        assert_eq!(Layout::from_size(2, 80, 0), Err(TerminalTooSmall));
        assert_eq!(Layout::from_size(2, 80, 1), Err(TerminalTooSmall));
    }

    #[test]
    fn the_shortest_terminal_that_holds_a_frame_stacks_it_from_row_zero() {
        let layout = Layout::from_size(3, 80, 0).expect("3 rows are the frame exactly");
        assert_eq!(layout.divider_top_row(), 0);
        assert_eq!(layout.input_row(), 1);
        assert_eq!(layout.divider_bottom_row(), 2);
        assert_eq!(layout.hint_row(), 3);
        assert_eq!(layout.content_bottom(), 3);
    }

    #[test]
    fn a_frame_that_keeps_no_footer_leaves_every_row_to_the_transcript() {
        let layout = Layout::from_size(24, 80, 0).expect("24 rows hold no footer");
        assert_eq!(layout.content_bottom(), 24);
        assert_eq!(layout.hint_row(), 24);
    }

    #[test]
    fn counters_start_at_nothing_and_only_climb() {
        let mut metrics = Metrics::default();
        assert_eq!(metrics.ansi_bytes(), 0);
        assert_eq!(metrics.full_redraws(), 0);
        assert_eq!(metrics.debounced_resizes(), 0);
        metrics.add_ansi_bytes(12);
        metrics.add_ansi_bytes(30);
        metrics.add_full_redraw();
        metrics.add_debounced_resize();
        assert_eq!(metrics.ansi_bytes(), 42);
        assert_eq!(metrics.full_redraws(), 1);
        assert_eq!(metrics.debounced_resizes(), 1);
    }

    #[test]
    fn a_counter_at_its_ceiling_stays_there_rather_than_wrapping() {
        let mut metrics = Metrics::default();
        metrics.add_ansi_bytes(usize::MAX);
        metrics.add_ansi_bytes(1);
        metrics.add_debounced_resize();
        metrics.add_debounced_resize();
        assert_eq!(metrics.ansi_bytes(), usize::MAX);
        assert_eq!(metrics.debounced_resizes(), 2);
    }

    #[test]
    fn a_terminal_reporting_no_rows_or_no_columns_is_not_a_size() {
        assert!(WindowSize::new(0, 80).is_none());
        assert!(WindowSize::new(24, 0).is_none());
        assert!(WindowSize::new(0, 0).is_none());
    }

    #[test]
    fn a_size_lays_out_its_own_frame() {
        let size = WindowSize::new(24, 80).expect("24x80 is a size");
        assert_eq!(size.rows(), 24);
        assert_eq!(size.cols(), 80);
        assert_eq!(size.layout(4), Layout::from_size(24, 80, 4));
        assert_eq!(
            WindowSize::new(2, 80).expect("2x80 is a size").layout(0),
            Err(TerminalTooSmall)
        );
    }
}
