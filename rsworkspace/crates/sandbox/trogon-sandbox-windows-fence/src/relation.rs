/// How the path stands to the exclusion: it is the excluded one, lies beneath
/// an excluded directory (protected only), or an excluded path lies beneath
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    Equal,
    Inside,
    Above,
}
