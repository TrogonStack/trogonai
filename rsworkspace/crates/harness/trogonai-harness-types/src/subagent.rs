//! What somebody asked the subagent manager for, and what it asks back.
//!
//! [`Command`] is the other direction: the state machine changes what it holds
//! and then names the one effect somebody else has to perform, because loading
//! more history or opening a terminal is not something a state machine can do.

/// One thing somebody asked the subagent manager to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Move the selection up.
    Up,
    /// Move the selection down.
    Down,
    /// Move the cursor one character back.
    Left,
    /// Move the cursor one character on.
    Right,
    /// A screen's worth up.
    PageUp,
    /// A screen's worth down.
    PageDown,
    /// Act on what is selected.
    Enter,
    /// Hand the keyboard to the next pane.
    FocusNext,
    /// Show what the children have been doing.
    Activity,
    /// Show what the children have asked for.
    Notifications,
    /// Show the children that have finished.
    Archived,
    /// Go to the next page of the list.
    NextPage,
    /// Go back to the previous page of the list.
    PreviousPage,
    /// Back out of whatever is open.
    Escape,
    /// Turn the thing under the selection on, or off again.
    Toggle,
    /// The interrupt.
    CtrlC,
    /// To the start of the line being written.
    Home,
    /// To the end of the line being written.
    End,
    /// A word back.
    WordLeft,
    /// A word on.
    WordRight,
    /// Take out the character before the cursor.
    DeleteBackward,
    /// Take out the character after the cursor.
    DeleteNext,
    /// Take out the word before the cursor.
    DeleteWordLeft,
    /// Take out the word after the cursor.
    DeleteWordRight,
    /// Take out back to the start of the line.
    DeleteToLineStart,
    /// Take out on to the end of the line.
    DeleteToLineEnd,
    /// Take out the whole line.
    ClearLine,
    /// Start another line rather than sending.
    InsertNewline,
}

/// The one effect a subagent state transition asks somebody else to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    /// Nothing, and nothing needs redrawing.
    Nothing,
    /// Something changed on screen.
    Redraw,
    /// A different page of the list is showing.
    PageChanged,
    /// Leave.
    ExitApp,
    /// Close the manager and go back to what was underneath.
    CloseManager,
    /// Mark what the children have asked for as seen.
    Acknowledge,
    /// A different child is selected.
    ChildChanged,
    /// Fetch the part of the selected child's history before what is showing.
    LoadOlderHistory,
    /// Fetch whatever the selected child has said since.
    RefreshNewestHistory,
    /// Send what has been written to the selected child.
    SubmitChildMessage,
    /// Fetch the things that could be attached to the message.
    LoadAttachCandidates,
    /// Fetch more of them.
    LoadMoreAttachCandidates,
    /// Carry out the change asked of the manager itself.
    SubmitManagerMutation,
    /// Answer the approval the selected child is waiting on.
    ResolveChildApproval,
    /// Open a terminal on the selected child.
    OpenTerminal,
}
