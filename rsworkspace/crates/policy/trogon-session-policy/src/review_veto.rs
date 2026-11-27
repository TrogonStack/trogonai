/// A review result deliberately has no permission-granting arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewVeto {
    NoVeto,
    Veto,
}
