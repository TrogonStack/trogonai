use axum::http::Method;

#[derive(Clone, Copy)]
pub(super) enum RequestMethod {
    Get,
    Post,
    Head,
    Put,
    Delete,
    Connect,
    Options,
    Trace,
    Patch,
    Other,
}

impl From<&Method> for RequestMethod {
    fn from(method: &Method) -> Self {
        match *method {
            Method::GET => Self::Get,
            Method::POST => Self::Post,
            Method::HEAD => Self::Head,
            Method::PUT => Self::Put,
            Method::DELETE => Self::Delete,
            Method::CONNECT => Self::Connect,
            Method::OPTIONS => Self::Options,
            Method::TRACE => Self::Trace,
            Method::PATCH => Self::Patch,
            _ => Self::Other,
        }
    }
}

impl RequestMethod {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Head => "HEAD",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Connect => "CONNECT",
            Self::Options => "OPTIONS",
            Self::Trace => "TRACE",
            Self::Patch => "PATCH",
            Self::Other => "_OTHER",
        }
    }
}
