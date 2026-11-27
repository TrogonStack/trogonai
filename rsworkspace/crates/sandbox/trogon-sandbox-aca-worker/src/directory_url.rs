use url::Url;

/// `url` with a trailing slash on its path, so relative paths join beneath it.
pub(crate) fn directory_url(mut url: Url) -> Url {
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    url
}
