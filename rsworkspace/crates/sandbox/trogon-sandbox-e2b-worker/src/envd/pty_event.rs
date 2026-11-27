/// What a PTY `Start` stream reports.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PtyEvent {
    Started(u32),
    Data(Vec<u8>),
    Ended(i32),
}
