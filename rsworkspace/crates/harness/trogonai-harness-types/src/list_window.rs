//! Which rows of a long list a short picker shows.
//!
//! Every window is half-open, so `items[window.start()..window.end()]` is the
//! slice to draw.

use core::ops::Range;

/// Which way a keystroke moves a selection.
///
/// Every caller passes one row up or one row down, which is what the two cases are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuStep {
    /// Toward the first row, wrapping to the last.
    Previous,
    /// Toward the last row, wrapping to the first.
    Next,
}

impl MenuStep {
    /// The row this step lands on, wrapping at either end.
    ///
    /// `count` is the number of rows; a list with no rows has no row to land on.
    #[must_use]
    pub const fn wrapping_from(self, current: usize, count: usize) -> Option<usize> {
        if count == 0 {
            return None;
        }
        let current = current % count;
        Some(match self {
            Self::Next => (current + 1) % count,
            Self::Previous => (current + count - 1) % count,
        })
    }
}

/// The rows of a list that are on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Window {
    start: usize,
    end: usize,
}

impl Window {
    /// How many rows a picker shows when nothing narrows it.
    pub const DEFAULT_MAX_PICKER_ROWS: u16 = 6;

    /// The window for a list that does not need one.
    ///
    /// A list with no rows, a picker with no room, or a list that fits whole:
    /// in all three there is nothing to choose between, and every function below
    /// answers them the same way before it does any arithmetic.
    fn unscrollable(count: usize, max_rows: u16) -> Option<Self> {
        if count == 0 || max_rows == 0 {
            return Some(Self { start: 0, end: 0 });
        }
        if count <= max_rows as usize {
            return Some(Self { start: 0, end: count });
        }
        None
    }

    /// A window holding the selected row as near the middle as it can.
    ///
    /// The middle is unreachable near either end of the list, where the window
    /// stops rather than showing blank rows past the edge.
    #[must_use]
    pub fn centered(count: usize, selected: usize, max_rows: u16) -> Self {
        if let Some(window) = Self::unscrollable(count, max_rows) {
            return window;
        }
        let rows = max_rows as usize;
        let start = selected.saturating_sub(rows / 2).min(count - rows);
        Self {
            start,
            end: start + rows,
        }
    }

    /// A window whose last row is the selected one.
    ///
    /// What a list scrolled by walking down it looks like: the selection sits at
    /// the bottom and everything above it is what came before.
    #[must_use]
    pub fn at_selection_edge(count: usize, selected: usize, max_rows: u16) -> Self {
        if let Some(window) = Self::unscrollable(count, max_rows) {
            return window;
        }
        let rows = max_rows as usize;
        let start = selected.saturating_sub(rows - 1);
        Self {
            start,
            end: start.saturating_add(rows).min(count),
        }
    }

    /// A window beginning at a remembered row.
    ///
    /// A start past what the list can show is pulled back to the last full
    /// window, so a remembered position survives the list getting shorter.
    #[must_use]
    pub fn from_start(count: usize, start: usize, max_rows: u16) -> Self {
        if let Some(window) = Self::unscrollable(count, max_rows) {
            return window;
        }
        let rows = max_rows as usize;
        let start = start.min(count - rows);
        Self {
            start,
            end: start + rows,
        }
    }

    /// The window a remembered one becomes once the selection has moved.
    ///
    /// It moves only as far as it has to. A selection still on screen leaves the
    /// window alone, which is what lets the highlight walk back up through the
    /// visible rows before the list scrolls the other way.
    ///
    /// The start is only ever used to derive this window, and deriving it here is what stops a
    /// caller pairing a fresh start with a stale row count.
    #[must_use]
    pub fn scrolled_to(current_start: usize, count: usize, selected: usize, max_rows: u16) -> Self {
        if let Some(window) = Self::unscrollable(count, max_rows) {
            return window;
        }
        let rows = max_rows as usize;
        let selected = selected.min(count - 1);
        let mut start = current_start.min(count - rows);
        if selected < start {
            start = selected;
        } else if selected >= start + rows {
            start = selected - rows + 1;
        }
        Self {
            start,
            end: start + rows,
        }
    }

    /// Moves a picker selection by `delta`, wrapping at either end, and scrolls
    /// the remembered window start so the new selection stays visible.
    ///
    /// An empty list leaves both values alone.
    pub fn advance_selection(index: &mut usize, window_start: &mut usize, count: usize, delta: i32) {
        if count == 0 {
            return;
        }
        let current = i32::try_from(*index % count).unwrap_or(i32::MAX);
        let mut next = current.saturating_add(delta);
        let last = i32::try_from(count - 1).unwrap_or(i32::MAX);
        if next < 0 {
            next = last;
        }
        if next > last {
            next = 0;
        }
        *index = usize::try_from(next).unwrap_or(0);
        *window_start = Self::scrolled_to(*window_start, count, *index, Self::DEFAULT_MAX_PICKER_ROWS).start();
    }

    /// First row on screen.
    #[must_use]
    pub const fn start(self) -> usize {
        self.start
    }

    /// First row past the bottom of the screen.
    #[must_use]
    pub const fn end(self) -> usize {
        self.end
    }

    /// How many rows are on screen.
    #[must_use]
    pub const fn len(self) -> usize {
        self.end - self.start
    }

    /// Whether the picker shows nothing at all.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Whether the row at `index` is on screen.
    #[must_use]
    pub const fn shows(self, index: usize) -> bool {
        self.start <= index && index < self.end
    }

    /// The window as a range, for slicing the list it describes.
    #[must_use]
    pub const fn rows(self) -> Range<usize> {
        self.start..self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROWS: u16 = 6;

    #[test]
    fn edge_window_start_lets_selection_move_back_up_before_reverse_scrolling() {
        let mut window = Window::scrolled_to(0, 11, 5, ROWS);
        assert_eq!(window.start(), 0);
        assert_eq!(5 - window.start(), 5);

        window = Window::scrolled_to(window.start(), 11, 6, ROWS);
        assert_eq!(window.start(), 1);
        assert_eq!(6 - window.start(), 5);

        window = Window::scrolled_to(window.start(), 11, 5, ROWS);
        assert_eq!(
            window.start(),
            1,
            "the selection is still on screen, so the list has no reason to move"
        );
        assert_eq!(5 - window.start(), 4);
    }

    #[test]
    fn a_list_that_fits_is_shown_whole_however_it_is_windowed() {
        for window in [
            Window::centered(4, 3, ROWS),
            Window::at_selection_edge(4, 3, ROWS),
            Window::from_start(4, 2, ROWS),
            Window::scrolled_to(2, 4, 3, ROWS),
        ] {
            assert_eq!(window.rows(), 0..4);
        }
    }

    #[test]
    fn a_picker_with_no_room_and_a_list_with_no_rows_both_show_nothing() {
        assert!(Window::centered(40, 3, 0).is_empty());
        assert!(Window::at_selection_edge(0, 0, ROWS).is_empty());
        assert!(Window::from_start(0, 0, ROWS).is_empty());
        assert!(Window::scrolled_to(0, 0, 0, ROWS).is_empty());
    }

    #[test]
    fn a_centered_window_stops_at_the_ends_of_the_list_rather_than_past_them() {
        assert_eq!(Window::centered(11, 0, ROWS).rows(), 0..6);
        assert_eq!(Window::centered(11, 5, ROWS).rows(), 2..8);
        assert_eq!(
            Window::centered(11, 10, ROWS).rows(),
            5..11,
            "the last row is on screen and no blank row is below it"
        );
    }

    #[test]
    fn an_edge_window_puts_the_selection_on_its_last_row() {
        assert_eq!(Window::at_selection_edge(11, 0, ROWS).rows(), 0..6);
        assert_eq!(Window::at_selection_edge(11, 5, ROWS).rows(), 0..6);
        assert_eq!(Window::at_selection_edge(11, 6, ROWS).rows(), 1..7);
        assert_eq!(Window::at_selection_edge(11, 10, ROWS).rows(), 5..11);
    }

    #[test]
    fn a_remembered_start_survives_the_list_getting_shorter() {
        assert_eq!(Window::from_start(11, 9, ROWS).rows(), 5..11);
        assert_eq!(Window::scrolled_to(9, 11, 10, ROWS).rows(), 5..11);
    }

    #[test]
    fn a_window_never_shows_more_rows_than_the_picker_has_or_the_list_holds() {
        for count in 0..25usize {
            for selected in 0..count.max(1) {
                for window in [
                    Window::centered(count, selected, ROWS),
                    Window::at_selection_edge(count, selected, ROWS),
                    Window::from_start(count, selected, ROWS),
                    Window::scrolled_to(selected, count, selected, ROWS),
                ] {
                    assert!(window.len() <= ROWS as usize);
                    assert!(window.end() <= count);
                    assert!(
                        count == 0 || window.shows(selected),
                        "a window that hides the selected row is a picker that cannot be used"
                    );
                }
            }
        }
    }

    #[test]
    fn advancing_a_selection_wraps_and_keeps_the_window_honest() {
        let mut index = 0usize;
        let mut window_start = 0usize;
        Window::advance_selection(&mut index, &mut window_start, 0, 1);
        assert_eq!((index, window_start), (0, 0));

        Window::advance_selection(&mut index, &mut window_start, 11, 1);
        assert_eq!(index, 1);
        assert_eq!(window_start, 0);

        index = 10;
        Window::advance_selection(&mut index, &mut window_start, 11, 1);
        assert_eq!(index, 0);
        assert_eq!(window_start, 0);

        Window::advance_selection(&mut index, &mut window_start, 11, -1);
        assert_eq!(index, 10);
        assert_eq!(
            window_start,
            Window::scrolled_to(0, 11, 10, Window::DEFAULT_MAX_PICKER_ROWS).start()
        );
    }
}
