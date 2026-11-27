use axum::http::HeaderValue;

use super::preview_location;

fn rewrite(location: &str, base_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let target = reqwest::Url::parse("http://upstream.invalid/base/page")?;
    let rewritten = preview_location(
        &HeaderValue::from_str(location)?,
        &target,
        base_path,
        "/v1/preview/l/1/t",
    );
    Ok(rewritten.to_str()?.to_string())
}

#[test]
fn a_path_under_the_base_loses_the_base() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(rewrite("/base/next", "/base")?, "/v1/preview/l/1/t/next");
    Ok(())
}

#[test]
fn a_path_outside_the_base_is_kept_whole() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(rewrite("/other", "/base")?, "/v1/preview/l/1/t/other");
    assert_eq!(rewrite("/basement", "/base")?, "/v1/preview/l/1/t/basement");
    Ok(())
}
