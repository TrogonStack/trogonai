/// The request a sentinel was found in, for bindings scoped to a path or a method.
///
/// `path` is the request target without its query.
#[derive(Debug, Clone, Copy)]
pub struct RequestSite<'a> {
    pub method: &'a str,
    pub path: &'a str,
}
