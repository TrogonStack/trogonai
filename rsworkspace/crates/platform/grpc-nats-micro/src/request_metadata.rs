#[derive(Clone, Copy)]
pub struct RequestMetadata<'a> {
    pub subject: &'a str,
    pub reply: Option<&'a str>,
}
