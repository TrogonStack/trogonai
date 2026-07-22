use bytes::Bytes;

/// Builds a truncated preview for prompt projection and artifact refs.
pub fn build_preview(content: &Bytes, max_bytes: usize) -> (String, bool) {
    if content.len() <= max_bytes {
        return (
            String::from_utf8_lossy(content).into_owned(),
            false,
        );
    }

    let preview_bytes = &content[..max_bytes];
    (
        String::from_utf8_lossy(preview_bytes).into_owned(),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_content_is_not_truncated() {
        let content = Bytes::from_static(b"short");
        let (preview, truncated) = build_preview(&content, 512);
        assert_eq!(preview, "short");
        assert!(!truncated);
    }

    #[test]
    fn long_content_is_truncated() {
        let content = Bytes::from("x".repeat(600));
        let (preview, truncated) = build_preview(&content, 512);
        assert_eq!(preview.len(), 512);
        assert!(truncated);
    }
}
