use axum::http::HeaderMap;

pub(crate) fn single_header<'a>(headers: &'a HeaderMap, name: &str, max_bytes: usize) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_some() || value.as_bytes().len() > max_bytes {
        return None;
    }
    value.to_str().ok()
}
