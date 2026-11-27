/// What looking at a directory found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirLook {
    Absent,
    Link,
    NotDirectory,
    Failed(String),
    Names(Vec<String>),
}
